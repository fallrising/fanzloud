use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use crate::{LoginBrokerError, LoginOperationId};

const LEDGER_FILE_NAME: &str = "login-ledger.json";
const LEDGER_SCHEMA_VERSION: u8 = 1;
const MAX_LEDGER_BYTES: u64 = 64 * 1024;
const MAX_HISTORY_ENTRIES: usize = 32;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ProcessIdentity {
    pub pid: u32,
    pub start_time_ticks: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum LedgerPhase {
    Intent,
    Started,
    DeviceInstructions,
    LoggedIn,
    LoggedOut,
    Failed,
    OutcomeUnknown,
}

impl LedgerPhase {
    pub(crate) fn is_uncertain(self) -> bool {
        matches!(
            self,
            Self::Intent | Self::Started | Self::DeviceInstructions | Self::OutcomeUnknown
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct LoginLedger {
    schema_version: u8,
    operation_id: LoginOperationId,
    process: Option<ProcessIdentity>,
    history: Vec<LedgerPhase>,
}

impl LoginLedger {
    pub(crate) fn new(operation_id: LoginOperationId) -> Self {
        Self {
            schema_version: LEDGER_SCHEMA_VERSION,
            operation_id,
            process: None,
            history: vec![LedgerPhase::Intent],
        }
    }

    pub(crate) fn operation_id(&self) -> LoginOperationId {
        self.operation_id
    }

    pub(crate) fn process(&self) -> Option<ProcessIdentity> {
        self.process
    }

    pub(crate) fn latest(&self) -> LedgerPhase {
        self.history
            .last()
            .copied()
            .unwrap_or(LedgerPhase::OutcomeUnknown)
    }

    pub(crate) fn append(&mut self, phase: LedgerPhase) -> Result<(), LoginBrokerError> {
        if self.history.len() >= MAX_HISTORY_ENTRIES || !valid_transition(self.latest(), phase) {
            return Err(LoginBrokerError::LedgerInvalid);
        }
        self.history.push(phase);
        Ok(())
    }

    pub(crate) fn record_started(
        &mut self,
        process: ProcessIdentity,
    ) -> Result<(), LoginBrokerError> {
        if self.process.is_some() {
            return Err(LoginBrokerError::LedgerInvalid);
        }
        self.process = Some(process);
        self.append(LedgerPhase::Started)
    }

    fn validate(&self) -> Result<(), LoginBrokerError> {
        if self.schema_version != LEDGER_SCHEMA_VERSION
            || self.operation_id.as_uuid().is_nil()
            || self.history.is_empty()
            || self.history.len() > MAX_HISTORY_ENTRIES
            || self.history[0] != LedgerPhase::Intent
        {
            return Err(LoginBrokerError::LedgerInvalid);
        }

        for transition in self.history.windows(2) {
            if !valid_transition(transition[0], transition[1]) {
                return Err(LoginBrokerError::LedgerInvalid);
            }
        }

        let started = self
            .history
            .iter()
            .any(|phase| matches!(phase, LedgerPhase::Started));
        if started != self.process.is_some() {
            return Err(LoginBrokerError::LedgerInvalid);
        }

        Ok(())
    }
}

fn valid_transition(from: LedgerPhase, to: LedgerPhase) -> bool {
    match from {
        LedgerPhase::Intent => {
            matches!(
                to,
                LedgerPhase::Started | LedgerPhase::Failed | LedgerPhase::OutcomeUnknown
            )
        }
        LedgerPhase::Started => {
            matches!(
                to,
                LedgerPhase::DeviceInstructions
                    | LedgerPhase::LoggedIn
                    | LedgerPhase::LoggedOut
                    | LedgerPhase::Failed
                    | LedgerPhase::OutcomeUnknown
            )
        }
        LedgerPhase::DeviceInstructions | LedgerPhase::OutcomeUnknown => {
            matches!(
                to,
                LedgerPhase::LoggedIn
                    | LedgerPhase::LoggedOut
                    | LedgerPhase::Failed
                    | LedgerPhase::OutcomeUnknown
            )
        }
        LedgerPhase::LoggedIn | LedgerPhase::LoggedOut | LedgerPhase::Failed => false,
    }
}

pub(crate) fn load_ledger(
    state_dir: &Path,
    runner_uid: u32,
) -> Result<Option<LoginLedger>, LoginBrokerError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state_dir, runner_uid);
        Err(LoginBrokerError::CredentialScope(
            crate::CredentialScopeError::UnsupportedPlatform,
        ))
    }

    #[cfg(target_os = "linux")]
    {
        let path = state_dir.join(LEDGER_FILE_NAME);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                return Err(LoginBrokerError::LedgerInvalid);
            }
            Err(source) => return Err(LoginBrokerError::LedgerUnavailable { source }),
        };

        validate_ledger_file(&file, runner_uid)?;
        let metadata = file
            .metadata()
            .map_err(|source| LoginBrokerError::LedgerUnavailable { source })?;
        if metadata.len() > MAX_LEDGER_BYTES {
            return Err(LoginBrokerError::LedgerInvalid);
        }

        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_LEDGER_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| LoginBrokerError::LedgerUnavailable { source })?;
        if bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err(LoginBrokerError::LedgerInvalid);
        }

        let ledger: LoginLedger =
            serde_json::from_slice(&bytes).map_err(|_| LoginBrokerError::LedgerInvalid)?;
        ledger.validate()?;
        Ok(Some(ledger))
    }
}

pub(crate) fn persist_ledger(
    state_dir: &Path,
    runner_uid: u32,
    ledger: &LoginLedger,
) -> Result<(), LoginBrokerError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state_dir, runner_uid, ledger);
        Err(LoginBrokerError::CredentialScope(
            crate::CredentialScopeError::UnsupportedPlatform,
        ))
    }

    #[cfg(target_os = "linux")]
    {
        ledger.validate()?;
        let bytes = serde_json::to_vec(ledger).map_err(|_| LoginBrokerError::LedgerInvalid)?;
        if bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err(LoginBrokerError::LedgerInvalid);
        }

        let final_path = state_dir.join(LEDGER_FILE_NAME);
        let temporary_path = state_dir.join(format!(".login-ledger.{}.tmp", ledger.operation_id()));
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let mut file = options
            .open(&temporary_path)
            .map_err(|source| LoginBrokerError::LedgerUnavailable { source })?;

        // SAFETY: `file` owns a valid descriptor and `fchmod` retains no pointer.
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
            return Err(LoginBrokerError::LedgerUnavailable {
                source: std::io::Error::last_os_error(),
            });
        }
        validate_ledger_file(&file, runner_uid)?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| LoginBrokerError::LedgerUnavailable { source })?;
        std::fs::rename(&temporary_path, &final_path)
            .map_err(|source| LoginBrokerError::LedgerUnavailable { source })?;
        File::open(state_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| LoginBrokerError::LedgerUnavailable { source })
    }
}

#[cfg(target_os = "linux")]
fn validate_ledger_file(file: &File, runner_uid: u32) -> Result<(), LoginBrokerError> {
    let metadata = file
        .metadata()
        .map_err(|source| LoginBrokerError::LedgerUnavailable { source })?;
    if !metadata.is_file()
        || metadata.uid() != runner_uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(LoginBrokerError::LedgerInvalid);
    }
    Ok(())
}

pub(crate) fn read_process_identity(pid: u32) -> Result<ProcessIdentity, LoginBrokerError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Err(LoginBrokerError::CredentialScope(
            crate::CredentialScopeError::UnsupportedPlatform,
        ))
    }

    #[cfg(target_os = "linux")]
    {
        read_process_stat(pid).map(|(identity, _state)| identity)
    }
}

pub(crate) fn process_is_alive(identity: ProcessIdentity) -> Result<bool, LoginBrokerError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = identity;
        Err(LoginBrokerError::CredentialScope(
            crate::CredentialScopeError::UnsupportedPlatform,
        ))
    }

    #[cfg(target_os = "linux")]
    {
        match read_process_stat(identity.pid) {
            Ok((actual, state)) => Ok(actual == identity && !matches!(state, 'Z' | 'X')),
            Err(LoginBrokerError::Process { source })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(target_os = "linux")]
fn read_process_stat(pid: u32) -> Result<(ProcessIdentity, char), LoginBrokerError> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|source| LoginBrokerError::Process { source })?;
    let close = stat.rfind(')').ok_or(LoginBrokerError::LedgerInvalid)?;
    let mut fields = stat[close + 1..].split_whitespace();
    let state = fields
        .next()
        .and_then(|value| value.chars().next())
        .ok_or(LoginBrokerError::LedgerInvalid)?;
    let start_time_ticks = fields
        .nth(18)
        .ok_or(LoginBrokerError::LedgerInvalid)?
        .parse()
        .map_err(|_| LoginBrokerError::LedgerInvalid)?;
    Ok((
        ProcessIdentity {
            pid,
            start_time_ticks,
        },
        state,
    ))
}
