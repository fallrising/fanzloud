use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cloud_lifecycle_types::{
    CloudLifecycle, CloudLifecycleError, CloudLifecycleErrorCategory,
};
use crate::{CloudSubmitOperationId, CloudTaskId, CloudTaskStatus};

pub(crate) const CLOUD_LIFECYCLE_FILE_NAME: &str = "cloud-lifecycle.json";
const CLOUD_LIFECYCLE_SCHEMA_VERSION: u8 = 1;
const MAX_LIFECYCLE_BYTES: u64 = 64 * 1024;
const MAX_LIFECYCLE_HISTORY: usize = 32;
const MAX_RECONCILIATION_CANDIDATES: usize = 100;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CloudLifecyclePhase {
    Submitting,
    FailedBeforeSubmit,
    OutcomeUnknown,
    Pending,
    Ready,
    Applied,
    ProviderError,
    CanceledLocally { provider_may_continue: bool },
    AbandonedUnknown,
}

impl CloudLifecyclePhase {
    pub(crate) const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::FailedBeforeSubmit
                | Self::Ready
                | Self::Applied
                | Self::ProviderError
                | Self::CanceledLocally { .. }
                | Self::AbandonedUnknown
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct CloudLifecycleLedger {
    schema_version: u8,
    operation_id: CloudSubmitOperationId,
    phase: CloudLifecyclePhase,
    task_id: Option<String>,
    reconciliation_generation: u64,
    candidate_task_ids: Vec<String>,
    reconciliation_complete: Option<bool>,
    history: Vec<CloudLifecyclePhase>,
}

impl CloudLifecycleLedger {
    pub(crate) fn submitting(operation_id: CloudSubmitOperationId) -> Self {
        Self {
            schema_version: CLOUD_LIFECYCLE_SCHEMA_VERSION,
            operation_id,
            phase: CloudLifecyclePhase::Submitting,
            task_id: None,
            reconciliation_generation: 0,
            candidate_task_ids: Vec::new(),
            reconciliation_complete: None,
            history: vec![CloudLifecyclePhase::Submitting],
        }
    }

    pub(crate) const fn operation_id(&self) -> CloudSubmitOperationId {
        self.operation_id
    }

    pub(crate) const fn phase(&self) -> CloudLifecyclePhase {
        self.phase
    }

    pub(crate) fn lifecycle(&self) -> Result<CloudLifecycle, CloudLifecycleError> {
        let operation_id = self.operation_id;
        let task_id = self.task_id()?;
        match self.phase {
            CloudLifecyclePhase::Submitting => Ok(CloudLifecycle::Submitting { operation_id }),
            CloudLifecyclePhase::FailedBeforeSubmit => {
                Ok(CloudLifecycle::FailedBeforeSubmit { operation_id })
            }
            CloudLifecyclePhase::OutcomeUnknown => {
                Ok(CloudLifecycle::OutcomeUnknown { operation_id })
            }
            CloudLifecyclePhase::Pending => Ok(CloudLifecycle::Pending {
                operation_id,
                task_id: task_id.ok_or_else(invalid_ledger)?,
            }),
            CloudLifecyclePhase::Ready => Ok(CloudLifecycle::Ready {
                operation_id,
                task_id: task_id.ok_or_else(invalid_ledger)?,
            }),
            CloudLifecyclePhase::Applied => Ok(CloudLifecycle::Applied {
                operation_id,
                task_id: task_id.ok_or_else(invalid_ledger)?,
            }),
            CloudLifecyclePhase::ProviderError => Ok(CloudLifecycle::ProviderError {
                operation_id,
                task_id: task_id.ok_or_else(invalid_ledger)?,
            }),
            CloudLifecyclePhase::CanceledLocally {
                provider_may_continue,
            } => Ok(CloudLifecycle::CanceledLocally {
                operation_id,
                task_id,
                provider_may_continue,
            }),
            CloudLifecyclePhase::AbandonedUnknown => {
                Ok(CloudLifecycle::AbandonedUnknown { operation_id })
            }
        }
    }

    pub(crate) fn record_failed_before_submit(&mut self) -> Result<(), CloudLifecycleError> {
        self.append(CloudLifecyclePhase::FailedBeforeSubmit)
    }

    pub(crate) fn record_outcome_unknown(&mut self) -> Result<(), CloudLifecycleError> {
        self.append(CloudLifecyclePhase::OutcomeUnknown)
    }

    pub(crate) fn record_status(
        &mut self,
        task_id: &CloudTaskId,
        status: CloudTaskStatus,
    ) -> Result<(), CloudLifecycleError> {
        if self
            .task_id
            .as_deref()
            .is_some_and(|current| current != task_id.as_str())
        {
            return Err(invalid_ledger());
        }
        let phase = phase_for_status(status);
        if self.phase == CloudLifecyclePhase::Pending
            && phase == CloudLifecyclePhase::Pending
            && self.task_id.as_deref() == Some(task_id.as_str())
        {
            return Ok(());
        }
        self.append(phase)?;
        self.task_id = Some(task_id.as_str().to_owned());
        Ok(())
    }

    pub(crate) fn record_canceled_without_task(&mut self) -> Result<(), CloudLifecycleError> {
        if self.task_id.is_some() {
            return Err(invalid_ledger());
        }
        self.append(CloudLifecyclePhase::CanceledLocally {
            provider_may_continue: false,
        })
    }

    pub(crate) fn record_canceled_with_task(&mut self) -> Result<(), CloudLifecycleError> {
        if self.task_id.is_none() {
            return Err(invalid_ledger());
        }
        self.append(CloudLifecyclePhase::CanceledLocally {
            provider_may_continue: true,
        })
    }

    pub(crate) fn record_abandoned_unknown(&mut self) -> Result<(), CloudLifecycleError> {
        self.append(CloudLifecyclePhase::AbandonedUnknown)
    }

    pub(crate) fn record_reconciliation(
        &mut self,
        task_ids: &[CloudTaskId],
        complete: bool,
    ) -> Result<(), CloudLifecycleError> {
        if self.phase != CloudLifecyclePhase::OutcomeUnknown
            || task_ids.len() > MAX_RECONCILIATION_CANDIDATES
        {
            return Err(invalid_ledger());
        }
        let mut seen = HashSet::with_capacity(task_ids.len());
        if task_ids
            .iter()
            .any(|task_id| !seen.insert(task_id.as_str()))
        {
            return Err(invalid_ledger());
        }
        self.reconciliation_generation = self
            .reconciliation_generation
            .checked_add(1)
            .ok_or_else(invalid_ledger)?;
        self.candidate_task_ids = task_ids
            .iter()
            .map(|task_id| task_id.as_str().to_owned())
            .collect();
        self.reconciliation_complete = Some(complete);
        Ok(())
    }

    pub(crate) const fn reconciliation_generation(&self) -> u64 {
        self.reconciliation_generation
    }

    pub(crate) const fn reconciliation_complete(&self) -> Option<bool> {
        self.reconciliation_complete
    }

    pub(crate) fn has_candidate(&self, task_id: &CloudTaskId) -> bool {
        self.candidate_task_ids
            .iter()
            .any(|candidate| candidate == task_id.as_str())
    }

    fn task_id(&self) -> Result<Option<CloudTaskId>, CloudLifecycleError> {
        self.task_id
            .as_deref()
            .map(CloudTaskId::try_new)
            .transpose()
            .map_err(|_| invalid_ledger())
    }

    fn append(&mut self, phase: CloudLifecyclePhase) -> Result<(), CloudLifecycleError> {
        if self.history.len() >= MAX_LIFECYCLE_HISTORY || !valid_transition(self.phase, phase) {
            return Err(invalid_ledger());
        }
        self.history.push(phase);
        self.phase = phase;
        Ok(())
    }

    fn validate(&self) -> Result<(), CloudLifecycleError> {
        if self.schema_version != CLOUD_LIFECYCLE_SCHEMA_VERSION
            || self.operation_id.as_uuid().is_nil()
            || self.history.is_empty()
            || self.history.len() > MAX_LIFECYCLE_HISTORY
            || self.history[0] != CloudLifecyclePhase::Submitting
            || self.history.last() != Some(&self.phase)
            || self
                .history
                .windows(2)
                .any(|transition| !valid_transition(transition[0], transition[1]))
        {
            return Err(invalid_ledger());
        }

        let task_id = self.task_id()?;
        let requires_task = matches!(
            self.phase,
            CloudLifecyclePhase::Pending
                | CloudLifecyclePhase::Ready
                | CloudLifecyclePhase::Applied
                | CloudLifecyclePhase::ProviderError
                | CloudLifecyclePhase::CanceledLocally {
                    provider_may_continue: true
                }
        );
        if requires_task != task_id.is_some()
            || matches!(
                self.phase,
                CloudLifecyclePhase::CanceledLocally {
                    provider_may_continue: false
                }
            ) && task_id.is_some()
        {
            return Err(invalid_ledger());
        }

        if self.candidate_task_ids.len() > MAX_RECONCILIATION_CANDIDATES {
            return Err(invalid_ledger());
        }
        let mut candidates = HashSet::with_capacity(self.candidate_task_ids.len());
        for candidate in &self.candidate_task_ids {
            let task_id = CloudTaskId::try_new(candidate).map_err(|_| invalid_ledger())?;
            if !candidates.insert(task_id) {
                return Err(invalid_ledger());
            }
        }
        if self.reconciliation_generation == 0 {
            if self.reconciliation_complete.is_some() || !self.candidate_task_ids.is_empty() {
                return Err(invalid_ledger());
            }
        } else if self.reconciliation_complete.is_none()
            || !self.history.contains(&CloudLifecyclePhase::OutcomeUnknown)
        {
            return Err(invalid_ledger());
        }
        Ok(())
    }
}

fn phase_for_status(status: CloudTaskStatus) -> CloudLifecyclePhase {
    match status {
        CloudTaskStatus::Pending => CloudLifecyclePhase::Pending,
        CloudTaskStatus::Ready => CloudLifecyclePhase::Ready,
        CloudTaskStatus::Applied => CloudLifecyclePhase::Applied,
        CloudTaskStatus::Error => CloudLifecyclePhase::ProviderError,
    }
}

fn valid_transition(from: CloudLifecyclePhase, to: CloudLifecyclePhase) -> bool {
    match from {
        CloudLifecyclePhase::Submitting => matches!(
            to,
            CloudLifecyclePhase::FailedBeforeSubmit
                | CloudLifecyclePhase::Pending
                | CloudLifecyclePhase::OutcomeUnknown
                | CloudLifecyclePhase::CanceledLocally {
                    provider_may_continue: false
                }
        ),
        CloudLifecyclePhase::Pending => matches!(
            to,
            CloudLifecyclePhase::Pending
                | CloudLifecyclePhase::Ready
                | CloudLifecyclePhase::Applied
                | CloudLifecyclePhase::ProviderError
                | CloudLifecyclePhase::CanceledLocally {
                    provider_may_continue: true
                }
        ),
        CloudLifecyclePhase::OutcomeUnknown => matches!(
            to,
            CloudLifecyclePhase::OutcomeUnknown
                | CloudLifecyclePhase::Pending
                | CloudLifecyclePhase::Ready
                | CloudLifecyclePhase::Applied
                | CloudLifecyclePhase::ProviderError
                | CloudLifecyclePhase::AbandonedUnknown
        ),
        CloudLifecyclePhase::FailedBeforeSubmit
        | CloudLifecyclePhase::Ready
        | CloudLifecyclePhase::Applied
        | CloudLifecyclePhase::ProviderError
        | CloudLifecyclePhase::CanceledLocally { .. }
        | CloudLifecyclePhase::AbandonedUnknown => false,
    }
}

pub(crate) fn load_lifecycle_ledger(
    state_dir: &Path,
    runner_uid: u32,
) -> Result<Option<CloudLifecycleLedger>, CloudLifecycleError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state_dir, runner_uid);
        Err(CloudLifecycleError::new(CloudLifecycleErrorCategory::Scope))
    }

    #[cfg(target_os = "linux")]
    {
        let path = state_dir.join(CLOUD_LIFECYCLE_FILE_NAME);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                return Err(invalid_ledger());
            }
            Err(_) => return Err(unavailable_ledger()),
        };

        validate_lifecycle_file(&file, runner_uid)?;
        let metadata = file.metadata().map_err(|_| unavailable_ledger())?;
        if metadata.len() > MAX_LIFECYCLE_BYTES {
            return Err(invalid_ledger());
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_LIFECYCLE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable_ledger())?;
        if bytes.len() as u64 > MAX_LIFECYCLE_BYTES {
            return Err(invalid_ledger());
        }
        let ledger: CloudLifecycleLedger =
            serde_json::from_slice(&bytes).map_err(|_| invalid_ledger())?;
        ledger.validate()?;
        Ok(Some(ledger))
    }
}

pub(crate) fn persist_lifecycle_ledger(
    state_dir: &Path,
    runner_uid: u32,
    ledger: &CloudLifecycleLedger,
) -> Result<(), CloudLifecycleError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state_dir, runner_uid, ledger);
        Err(CloudLifecycleError::new(CloudLifecycleErrorCategory::Scope))
    }

    #[cfg(target_os = "linux")]
    {
        ledger.validate()?;
        let bytes = serde_json::to_vec(ledger).map_err(|_| invalid_ledger())?;
        if bytes.len() as u64 > MAX_LIFECYCLE_BYTES {
            return Err(invalid_ledger());
        }
        let final_path = state_dir.join(CLOUD_LIFECYCLE_FILE_NAME);
        let temporary_path = state_dir.join(format!(".cloud-lifecycle.{}.tmp", Uuid::new_v4()));
        let result =
            persist_replacement(state_dir, runner_uid, &temporary_path, &final_path, &bytes);
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary_path);
        }
        result
    }
}

pub(crate) fn remove_lifecycle_ledger(
    state_dir: &Path,
    runner_uid: u32,
) -> Result<(), CloudLifecycleError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (state_dir, runner_uid);
        Err(CloudLifecycleError::new(CloudLifecycleErrorCategory::Scope))
    }

    #[cfg(target_os = "linux")]
    {
        let path = state_dir.join(CLOUD_LIFECYCLE_FILE_NAME);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options.open(&path).map_err(|_| unavailable_ledger())?;
        validate_lifecycle_file(&file, runner_uid)?;
        drop(file);
        std::fs::remove_file(path).map_err(|_| unavailable_ledger())?;
        File::open(state_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| unavailable_ledger())
    }
}

#[cfg(target_os = "linux")]
fn persist_replacement(
    state_dir: &Path,
    runner_uid: u32,
    temporary_path: &Path,
    final_path: &Path,
    bytes: &[u8],
) -> Result<(), CloudLifecycleError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(temporary_path)
        .map_err(|_| unavailable_ledger())?;
    // SAFETY: `file` owns a valid descriptor and `fchmod` retains no pointer.
    if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
        return Err(unavailable_ledger());
    }
    validate_lifecycle_file(&file, runner_uid)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| unavailable_ledger())?;
    std::fs::rename(temporary_path, final_path).map_err(|_| unavailable_ledger())?;
    File::open(state_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| unavailable_ledger())
}

#[cfg(target_os = "linux")]
fn validate_lifecycle_file(file: &File, runner_uid: u32) -> Result<(), CloudLifecycleError> {
    let metadata = file.metadata().map_err(|_| unavailable_ledger())?;
    if !metadata.is_file()
        || metadata.uid() != runner_uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(invalid_ledger());
    }
    Ok(())
}

fn invalid_ledger() -> CloudLifecycleError {
    CloudLifecycleError::new(CloudLifecycleErrorCategory::LedgerInvalid)
}

fn unavailable_ledger() -> CloudLifecycleError {
    CloudLifecycleError::new(CloudLifecycleErrorCategory::LedgerUnavailable)
}
