use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use serde::{Deserialize, Serialize};

use crate::cloud_runner_types::{
    CloudRunnerError, CloudRunnerErrorCategory, CloudSubmitOperationId,
};
use crate::ledger::ProcessIdentity;
use crate::{CloudBranch, CloudEnvironmentId, CloudTaskId};

pub(crate) const CLOUD_LEDGER_FILE_NAME: &str = "cloud-submit-ledger.json";
const CLOUD_LEDGER_SCHEMA_VERSION: u8 = 1;
const MAX_CLOUD_LEDGER_BYTES: u64 = 64 * 1024;
const MAX_CLOUD_HISTORY_ENTRIES: usize = 32;
const MAX_RECONCILIATION_CANDIDATES: usize = 100;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CloudLedgerPhase {
    Intent,
    Authorized,
    Started,
    TaskRecorded,
    FailedBeforeSpawn,
    OutcomeUnknown,
    ReconciliationObserved,
    TaskAdopted,
    ExplicitlyAbandoned,
}

impl CloudLedgerPhase {
    pub(crate) const fn is_unknown(self) -> bool {
        matches!(self, Self::OutcomeUnknown | Self::ReconciliationObserved)
    }

    pub(crate) const fn permits_new_operation(self) -> bool {
        matches!(
            self,
            Self::TaskRecorded
                | Self::FailedBeforeSpawn
                | Self::TaskAdopted
                | Self::ExplicitlyAbandoned
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct CloudSubmitLedger {
    schema_version: u8,
    operation_id: CloudSubmitOperationId,
    environment: String,
    branch: String,
    process: Option<ProcessIdentity>,
    task_id: Option<String>,
    candidate_task_ids: Vec<String>,
    reconciliation_complete: Option<bool>,
    history: Vec<CloudLedgerPhase>,
}

impl CloudSubmitLedger {
    pub(crate) fn new(
        operation_id: CloudSubmitOperationId,
        environment: &CloudEnvironmentId,
        branch: &CloudBranch,
    ) -> Self {
        Self {
            schema_version: CLOUD_LEDGER_SCHEMA_VERSION,
            operation_id,
            environment: environment.as_str().to_owned(),
            branch: branch.as_str().to_owned(),
            process: None,
            task_id: None,
            candidate_task_ids: Vec::new(),
            reconciliation_complete: None,
            history: vec![CloudLedgerPhase::Intent],
        }
    }

    pub(crate) const fn operation_id(&self) -> CloudSubmitOperationId {
        self.operation_id
    }

    pub(crate) fn matches_config(
        &self,
        environment: &CloudEnvironmentId,
        branch: &CloudBranch,
    ) -> bool {
        self.environment == environment.as_str() && self.branch == branch.as_str()
    }

    pub(crate) fn latest(&self) -> CloudLedgerPhase {
        match self.history.last() {
            Some(phase) => *phase,
            None => CloudLedgerPhase::OutcomeUnknown,
        }
    }

    pub(crate) fn recorded_task(&self) -> Result<Option<CloudTaskId>, CloudRunnerError> {
        self.task_id
            .as_deref()
            .map(CloudTaskId::try_new)
            .transpose()
            .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerInvalid))
    }

    pub(crate) fn append(&mut self, phase: CloudLedgerPhase) -> Result<(), CloudRunnerError> {
        if self.history.len() >= MAX_CLOUD_HISTORY_ENTRIES
            || !valid_transition(self.latest(), phase)
        {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }
        self.history.push(phase);
        Ok(())
    }

    pub(crate) fn record_started(
        &mut self,
        process: ProcessIdentity,
    ) -> Result<(), CloudRunnerError> {
        if self.process.is_some() {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }
        self.process = Some(process);
        self.append(CloudLedgerPhase::Started)
    }

    pub(crate) fn record_task(&mut self, task_id: &CloudTaskId) -> Result<(), CloudRunnerError> {
        if self.task_id.is_some() {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }
        self.task_id = Some(task_id.as_str().to_owned());
        self.append(CloudLedgerPhase::TaskRecorded)
    }

    pub(crate) fn record_reconciliation(
        &mut self,
        task_ids: &[CloudTaskId],
        complete: bool,
    ) -> Result<(), CloudRunnerError> {
        if task_ids.len() > MAX_RECONCILIATION_CANDIDATES {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }
        let mut seen = HashSet::with_capacity(task_ids.len());
        for task_id in task_ids {
            if !seen.insert(task_id.as_str()) {
                return Err(CloudRunnerError::new(
                    CloudRunnerErrorCategory::LedgerInvalid,
                ));
            }
        }
        self.candidate_task_ids = task_ids
            .iter()
            .map(|task_id| task_id.as_str().to_owned())
            .collect();
        self.reconciliation_complete = Some(complete);
        self.append(CloudLedgerPhase::ReconciliationObserved)
    }

    pub(crate) fn record_adopted_task(
        &mut self,
        task_id: &CloudTaskId,
    ) -> Result<(), CloudRunnerError> {
        if self.latest() != CloudLedgerPhase::ReconciliationObserved {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::ResolutionUnavailable,
            ));
        }
        if self.task_id.is_some() {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }
        if !self
            .candidate_task_ids
            .iter()
            .any(|candidate| candidate == task_id.as_str())
        {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::CandidateNotRecorded,
            ));
        }
        self.append(CloudLedgerPhase::TaskAdopted)?;
        self.task_id = Some(task_id.as_str().to_owned());
        Ok(())
    }

    pub(crate) fn record_explicitly_abandoned(&mut self) -> Result<(), CloudRunnerError> {
        if self.latest() != CloudLedgerPhase::ReconciliationObserved || self.task_id.is_some() {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::ResolutionUnavailable,
            ));
        }
        self.append(CloudLedgerPhase::ExplicitlyAbandoned)
    }

    fn validate(&self) -> Result<(), CloudRunnerError> {
        if self.schema_version != CLOUD_LEDGER_SCHEMA_VERSION
            || self.operation_id.as_uuid().is_nil()
            || self.history.is_empty()
            || self.history.len() > MAX_CLOUD_HISTORY_ENTRIES
            || self.history[0] != CloudLedgerPhase::Intent
            || CloudEnvironmentId::try_new(&self.environment).is_err()
            || CloudBranch::try_new(&self.branch).is_err()
        {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }

        if self
            .history
            .windows(2)
            .any(|transition| !valid_transition(transition[0], transition[1]))
        {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }

        let started = self.history.contains(&CloudLedgerPhase::Started);
        if started != self.process.is_some() {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }

        let task_recorded = self.history.contains(&CloudLedgerPhase::TaskRecorded)
            || self.history.contains(&CloudLedgerPhase::TaskAdopted);
        if task_recorded != self.task_id.is_some() || self.recorded_task().is_err() {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }

        let reconciliation_observed = self
            .history
            .contains(&CloudLedgerPhase::ReconciliationObserved);
        if reconciliation_observed != self.reconciliation_complete.is_some()
            || self.candidate_task_ids.len() > MAX_RECONCILIATION_CANDIDATES
        {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }
        let mut candidates = HashSet::with_capacity(self.candidate_task_ids.len());
        for task_id in &self.candidate_task_ids {
            let task_id = CloudTaskId::try_new(task_id)
                .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerInvalid))?;
            if !candidates.insert(task_id) {
                return Err(CloudRunnerError::new(
                    CloudRunnerErrorCategory::LedgerInvalid,
                ));
            }
        }
        if self.history.contains(&CloudLedgerPhase::TaskAdopted)
            && self
                .task_id
                .as_deref()
                .is_none_or(|task_id| !self.candidate_task_ids.iter().any(|item| item == task_id))
        {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }

        Ok(())
    }
}

fn valid_transition(from: CloudLedgerPhase, to: CloudLedgerPhase) -> bool {
    match from {
        CloudLedgerPhase::Intent => matches!(
            to,
            CloudLedgerPhase::Authorized | CloudLedgerPhase::FailedBeforeSpawn
        ),
        CloudLedgerPhase::Authorized => {
            matches!(
                to,
                CloudLedgerPhase::Started
                    | CloudLedgerPhase::FailedBeforeSpawn
                    | CloudLedgerPhase::OutcomeUnknown
            )
        }
        CloudLedgerPhase::Started => {
            matches!(
                to,
                CloudLedgerPhase::TaskRecorded | CloudLedgerPhase::OutcomeUnknown
            )
        }
        CloudLedgerPhase::OutcomeUnknown => to == CloudLedgerPhase::ReconciliationObserved,
        CloudLedgerPhase::ReconciliationObserved => {
            matches!(
                to,
                CloudLedgerPhase::ReconciliationObserved
                    | CloudLedgerPhase::TaskAdopted
                    | CloudLedgerPhase::ExplicitlyAbandoned
            )
        }
        CloudLedgerPhase::TaskRecorded
        | CloudLedgerPhase::FailedBeforeSpawn
        | CloudLedgerPhase::TaskAdopted
        | CloudLedgerPhase::ExplicitlyAbandoned => false,
    }
}

pub(crate) fn load_cloud_ledger(
    state_dir: &Path,
    runner_uid: u32,
) -> Result<Option<CloudSubmitLedger>, CloudRunnerError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state_dir, runner_uid);
        Err(CloudRunnerError::new(CloudRunnerErrorCategory::Scope))
    }

    #[cfg(target_os = "linux")]
    {
        let path = state_dir.join(CLOUD_LEDGER_FILE_NAME);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                return Err(CloudRunnerError::new(
                    CloudRunnerErrorCategory::LedgerInvalid,
                ));
            }
            Err(_) => {
                return Err(CloudRunnerError::new(
                    CloudRunnerErrorCategory::LedgerUnavailable,
                ));
            }
        };

        validate_cloud_ledger_file(&file, runner_uid)?;
        let metadata = file
            .metadata()
            .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerUnavailable))?;
        if metadata.len() > MAX_CLOUD_LEDGER_BYTES {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }

        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_CLOUD_LEDGER_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerUnavailable))?;
        if bytes.len() as u64 > MAX_CLOUD_LEDGER_BYTES {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }

        let ledger: CloudSubmitLedger = serde_json::from_slice(&bytes)
            .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerInvalid))?;
        ledger.validate()?;
        Ok(Some(ledger))
    }
}

pub(crate) fn persist_cloud_ledger(
    state_dir: &Path,
    runner_uid: u32,
    ledger: &CloudSubmitLedger,
) -> Result<(), CloudRunnerError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state_dir, runner_uid, ledger);
        Err(CloudRunnerError::new(CloudRunnerErrorCategory::Scope))
    }

    #[cfg(target_os = "linux")]
    {
        ledger.validate()?;
        let bytes = serde_json::to_vec(ledger)
            .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerInvalid))?;
        if bytes.len() as u64 > MAX_CLOUD_LEDGER_BYTES {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ));
        }

        let final_path = state_dir.join(CLOUD_LEDGER_FILE_NAME);
        let temporary_path = state_dir.join(format!(
            ".cloud-submit-ledger.{}.tmp",
            ledger.operation_id()
        ));
        let result =
            persist_replacement(state_dir, runner_uid, &temporary_path, &final_path, &bytes);
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary_path);
        }
        result
    }
}

#[cfg(target_os = "linux")]
fn persist_replacement(
    state_dir: &Path,
    runner_uid: u32,
    temporary_path: &Path,
    final_path: &Path,
    bytes: &[u8],
) -> Result<(), CloudRunnerError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(temporary_path)
        .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerUnavailable))?;

    // SAFETY: `file` owns a valid descriptor and `fchmod` retains no pointer.
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
        return Err(CloudRunnerError::new(
            CloudRunnerErrorCategory::LedgerUnavailable,
        ));
    }
    validate_cloud_ledger_file(&file, runner_uid)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerUnavailable))?;
    std::fs::rename(temporary_path, final_path)
        .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerUnavailable))?;
    File::open(state_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerUnavailable))
}

#[cfg(target_os = "linux")]
fn validate_cloud_ledger_file(file: &File, runner_uid: u32) -> Result<(), CloudRunnerError> {
    let metadata = file
        .metadata()
        .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::LedgerUnavailable))?;
    if !metadata.is_file()
        || metadata.uid() != runner_uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(CloudRunnerError::new(
            CloudRunnerErrorCategory::LedgerInvalid,
        ));
    }
    Ok(())
}
