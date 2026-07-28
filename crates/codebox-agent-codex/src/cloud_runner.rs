use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::process::{Child, ExitStatus};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use crate::cloud_runner_types::{
    CloudCancellation, CloudReconciliation, CloudRunnerConfig, CloudRunnerError,
    CloudRunnerErrorCategory, CloudSubmission, CloudSubmitObservation, CloudSubmitOperationId,
    CloudSubmitRequest, CloudUnknownResolution,
};
use crate::cloud_submit_ledger::{
    CloudLedgerPhase, CloudSubmitLedger, load_cloud_ledger, persist_cloud_ledger,
};
use crate::ledger::{ProcessIdentity, load_ledger, read_process_identity};
use crate::parser::PinnedStatus;
use crate::runtime::{
    CliRuntime, CommandSpec, ProcessRuntime, SharedCapture, drain_stream, finish_capture,
    spawn_bound_child, terminate_group,
};
use crate::scope::OwnedCredentialScopeLease;
use crate::{
    CloudCapture, CloudInvocation, CloudTaskId, CloudTaskStatus, CredentialScope,
    CredentialScopeError, LoginBrokerError, decode_cloud_exec, decode_cloud_list,
    decode_cloud_status,
};

const CLOUD_CAPTURE_BYTES: usize = 64 * 1024;
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(60);
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
const RECONCILIATION_TIMEOUT: Duration = Duration::from_secs(60);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_RECONCILIATION_PAGES: usize = 5;
const MAX_RECONCILIATION_TASKS: usize = 100;
const DIAGNOSTIC_SENTINEL_NAME: &str = "error.log";

/// Trusted single-operator runner for the pinned Codex Cloud submit/status/list surface.
///
/// Contract: `CU-CLOUD-P0-01`. No public method accepts an executable, path, environment entry,
/// arbitrary argv, shell, repository, retry callback, or diff-application authority.
pub struct CloudTaskRunner {
    scope: CredentialScope,
    environment: crate::CloudEnvironmentId,
    branch: crate::CloudBranch,
    runtime: Box<dyn CloudCliRuntime>,
}

impl fmt::Debug for CloudTaskRunner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudTaskRunner")
            .finish_non_exhaustive()
    }
}

impl CloudTaskRunner {
    /// Creates a runner from trusted typed administrator configuration.
    pub fn new(config: CloudRunnerConfig) -> Result<Self, CloudRunnerError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = config;
            Err(CloudRunnerError::new(CloudRunnerErrorCategory::Scope))
        }

        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                scope: config.scope,
                environment: config.environment,
                branch: config.branch,
                runtime: Box::new(ProcessCloudRuntime),
            })
        }
    }

    #[cfg(test)]
    pub(crate) fn with_runtime(
        config: CloudRunnerConfig,
        runtime: Box<dyn CloudCliRuntime>,
    ) -> Self {
        Self {
            scope: config.scope,
            environment: config.environment,
            branch: config.branch,
            runtime,
        }
    }

    /// Executes at most one fixed Cloud submit and returns only a durably recorded task.
    ///
    /// Contract: `CU-CLOUD-P0-01`. Replaying a recorded operation ID is observation-only.
    pub fn submit(
        &self,
        request: CloudSubmitRequest,
        cancellation: CloudCancellation,
    ) -> Result<CloudSubmission, CloudRunnerError> {
        let operation_id = request.operation_id;
        let lease = self.acquire_scope()?;
        let existing = self.recover_current_ledger(&lease)?;
        if let Some(result) = self.dispatch_existing(operation_id, existing.as_ref()) {
            return result;
        }

        self.runtime.verify_version(&lease)?;

        let mut ledger = CloudSubmitLedger::new(operation_id, &self.environment, &self.branch);
        persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;

        if cancellation.is_cancelled() {
            ledger.append(CloudLedgerPhase::FailedBeforeSpawn)?;
            persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
            return Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::CanceledBeforeStart,
                operation_id,
            ));
        }

        if let Err(error) = self.authorize_login(&lease) {
            ledger.append(CloudLedgerPhase::FailedBeforeSpawn)?;
            persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
            return Err(error.with_operation(operation_id));
        }
        if let Err(error) = ensure_diagnostic_sentinel(&lease) {
            ledger.append(CloudLedgerPhase::FailedBeforeSpawn)?;
            persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
            return Err(error.with_operation(operation_id));
        }

        ledger.append(CloudLedgerPhase::Authorized)?;
        persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
        if cancellation.is_cancelled() {
            self.persist_unknown(&lease, &mut ledger)?;
            return Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::OutcomeUnknown,
                operation_id,
            ));
        }

        let invocation = CloudInvocation::exec(&self.environment, &self.branch, &request.prompt);
        let mut supervisor =
            match self
                .runtime
                .start(&lease, &invocation, SUBMIT_TIMEOUT, cancellation)
            {
                Ok(supervisor) => supervisor,
                Err(failure) if failure.may_have_started => {
                    self.persist_unknown(&lease, &mut ledger)?;
                    return Err(CloudRunnerError::for_operation(
                        CloudRunnerErrorCategory::OutcomeUnknown,
                        operation_id,
                    ));
                }
                Err(failure) => {
                    ledger.append(CloudLedgerPhase::FailedBeforeSpawn)?;
                    persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
                    return Err(failure.error.with_operation(operation_id));
                }
            };

        ledger.record_started(supervisor.process_identity())?;
        if persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger).is_err() {
            let _ = supervisor.cancel_and_wait();
            return Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::RecoveryRequired,
                operation_id,
            ));
        }

        let completion = match supervisor.wait() {
            Ok(completion) => completion,
            Err(_) => {
                self.persist_unknown(&lease, &mut ledger)?;
                return Err(CloudRunnerError::for_operation(
                    CloudRunnerErrorCategory::OutcomeUnknown,
                    operation_id,
                ));
            }
        };
        if completion.end != CloudCommandEnd::Exited {
            self.persist_unknown(&lease, &mut ledger)?;
            return Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::OutcomeUnknown,
                operation_id,
            ));
        }

        let task_url = match decode_cloud_exec(&completion.capture) {
            Ok(task_url) => task_url,
            Err(_) => {
                self.persist_unknown(&lease, &mut ledger)?;
                return Err(CloudRunnerError::for_operation(
                    CloudRunnerErrorCategory::OutcomeUnknown,
                    operation_id,
                ));
            }
        };
        let task_id = task_url.task_id().clone();
        let mut recorded_ledger = ledger.clone();
        recorded_ledger.record_task(&task_id)?;
        if persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &recorded_ledger).is_err() {
            let _ = self.persist_unknown(&lease, &mut ledger);
            return Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::RecoveryRequired,
                operation_id,
            ));
        }

        Ok(CloudSubmission::new(operation_id, task_id))
    }

    /// Reads one normalized provider task status through the fixed pinned command.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub fn status(&self, task_id: &CloudTaskId) -> Result<CloudTaskStatus, CloudRunnerError> {
        let lease = self.acquire_scope()?;
        self.runtime.verify_version(&lease)?;
        ensure_diagnostic_sentinel(&lease)?;
        let invocation = CloudInvocation::status(task_id);
        let completion = self.run_read(&lease, &invocation, STATUS_TIMEOUT)?;
        decode_cloud_status(&completion.capture)
            .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::ProviderOutput))
    }

    /// Lists bounded candidates for the current unknown submission without authorizing a retry.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub fn reconcile_unknown(&self) -> Result<CloudReconciliation, CloudRunnerError> {
        let lease = self.acquire_scope()?;
        let mut ledger = self
            .recover_current_ledger(&lease)?
            .ok_or_else(|| CloudRunnerError::new(CloudRunnerErrorCategory::NoUnknownOperation))?;
        if !ledger.latest().is_unknown() {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::NoUnknownOperation,
            ));
        }
        self.require_matching_config(&ledger)?;
        self.runtime.verify_version(&lease)?;

        let started_at = Instant::now();
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut seen_tasks = HashSet::new();
        let mut tasks = Vec::new();
        let mut complete = false;

        for page_index in 0..MAX_RECONCILIATION_PAGES {
            if started_at.elapsed() >= RECONCILIATION_TIMEOUT {
                break;
            }
            ensure_diagnostic_sentinel(&lease)?;
            let invocation = CloudInvocation::list(&self.environment, cursor.as_ref());
            let remaining = RECONCILIATION_TIMEOUT.saturating_sub(started_at.elapsed());
            let completion = self.run_read(&lease, &invocation, remaining.min(STATUS_TIMEOUT))?;
            let page = decode_cloud_list(&completion.capture)
                .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::ProviderOutput))?;

            for task in page.tasks() {
                if !seen_tasks.insert(task.id().clone()) {
                    return Err(CloudRunnerError::new(
                        CloudRunnerErrorCategory::ReconciliationCycle,
                    ));
                }
                tasks.push(task.clone());
            }

            match page.cursor().cloned() {
                None => {
                    complete = true;
                    break;
                }
                Some(next_cursor) => {
                    if !seen_cursors.insert(next_cursor.as_str().to_owned()) {
                        return Err(CloudRunnerError::new(
                            CloudRunnerErrorCategory::ReconciliationCycle,
                        ));
                    }
                    cursor = Some(next_cursor);
                }
            }

            if page_index + 1 == MAX_RECONCILIATION_PAGES || tasks.len() >= MAX_RECONCILIATION_TASKS
            {
                break;
            }
        }

        let task_ids: Vec<_> = tasks.iter().map(|task| task.id().clone()).collect();
        ledger.record_reconciliation(&task_ids, complete)?;
        persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
        Ok(CloudReconciliation::new(
            ledger.operation_id(),
            tasks,
            complete,
        ))
    }

    /// Observes and crash-classifies one durable submit without executing a Cloud command.
    ///
    /// Contract: `CU-CLOUD-P0-01`. T004A1 stages this crate-private bridge for T004B.
    #[allow(dead_code)]
    pub(crate) fn observe_submit(
        &self,
        operation_id: CloudSubmitOperationId,
    ) -> Result<CloudSubmitObservation, CloudRunnerError> {
        let lease = self.acquire_scope()?;
        let ledger = self.recover_current_ledger(&lease)?;
        self.classify_observation(operation_id, ledger.as_ref())
    }

    /// Durably applies one T004B-authorized resolution without executing a Cloud command.
    ///
    /// Contract: `CU-CLOUD-P0-01`. T004A1 stages this crate-private bridge for T004B.
    #[allow(dead_code)]
    pub(crate) fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        resolution: CloudUnknownResolution,
    ) -> Result<CloudSubmitObservation, CloudRunnerError> {
        let lease = self.acquire_scope()?;
        let mut ledger = self.recover_current_ledger(&lease)?.ok_or_else(|| {
            CloudRunnerError::new(CloudRunnerErrorCategory::ResolutionUnavailable)
        })?;
        if ledger.operation_id() != operation_id {
            return Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::OperationConflict,
                ledger.operation_id(),
            ));
        }

        match ledger.latest() {
            CloudLedgerPhase::TaskAdopted => {
                let recorded = ledger.recorded_task()?.ok_or_else(|| {
                    CloudRunnerError::new(CloudRunnerErrorCategory::LedgerInvalid)
                })?;
                return match resolution {
                    CloudUnknownResolution::AdoptListedTask(task_id) if task_id == recorded => {
                        Ok(CloudSubmitObservation::TaskRecorded(CloudSubmission::new(
                            operation_id,
                            recorded,
                        )))
                    }
                    CloudUnknownResolution::AdoptListedTask(_)
                    | CloudUnknownResolution::ExplicitlyAbandon => {
                        Err(CloudRunnerError::for_operation(
                            CloudRunnerErrorCategory::ResolutionConflict,
                            operation_id,
                        ))
                    }
                };
            }
            CloudLedgerPhase::ExplicitlyAbandoned => {
                return match resolution {
                    CloudUnknownResolution::ExplicitlyAbandon => {
                        Ok(CloudSubmitObservation::ExplicitlyAbandoned)
                    }
                    CloudUnknownResolution::AdoptListedTask(_) => {
                        Err(CloudRunnerError::for_operation(
                            CloudRunnerErrorCategory::ResolutionConflict,
                            operation_id,
                        ))
                    }
                };
            }
            CloudLedgerPhase::ReconciliationObserved => {}
            CloudLedgerPhase::Intent
            | CloudLedgerPhase::Authorized
            | CloudLedgerPhase::Started
            | CloudLedgerPhase::TaskRecorded
            | CloudLedgerPhase::FailedBeforeSpawn
            | CloudLedgerPhase::OutcomeUnknown => {
                return Err(CloudRunnerError::for_operation(
                    CloudRunnerErrorCategory::ResolutionUnavailable,
                    operation_id,
                ));
            }
        }

        match resolution {
            CloudUnknownResolution::AdoptListedTask(task_id) => {
                ledger.record_adopted_task(&task_id)?;
            }
            CloudUnknownResolution::ExplicitlyAbandon => {
                ledger.record_explicitly_abandoned()?;
            }
        }
        persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
        self.classify_observation(operation_id, Some(&ledger))
    }

    fn acquire_scope(&self) -> Result<OwnedCredentialScopeLease, CloudRunnerError> {
        self.scope.acquire_owned().map_err(map_scope_error)
    }

    fn authorize_login(&self, lease: &OwnedCredentialScopeLease) -> Result<(), CloudRunnerError> {
        if let Some(login_ledger) =
            load_ledger(lease.state_dir(), lease.runner_uid()).map_err(map_login_ledger_error)?
            && login_ledger.latest().is_uncertain()
        {
            return Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LoginOutcomeUnknown,
            ));
        }
        match self.runtime.login_status(lease)? {
            PinnedStatus::LoggedIn => Ok(()),
            PinnedStatus::LoggedOut => Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::NotAuthenticated,
            )),
        }
    }

    fn recover_current_ledger(
        &self,
        lease: &OwnedCredentialScopeLease,
    ) -> Result<Option<CloudSubmitLedger>, CloudRunnerError> {
        let Some(mut ledger) = load_cloud_ledger(lease.state_dir(), lease.runner_uid())? else {
            return Ok(None);
        };
        self.require_matching_config(&ledger)?;
        match ledger.latest() {
            CloudLedgerPhase::Intent => {
                ledger.append(CloudLedgerPhase::FailedBeforeSpawn)?;
                persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
            }
            CloudLedgerPhase::Authorized | CloudLedgerPhase::Started => {
                ledger.append(CloudLedgerPhase::OutcomeUnknown)?;
                persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
            }
            CloudLedgerPhase::TaskRecorded
            | CloudLedgerPhase::FailedBeforeSpawn
            | CloudLedgerPhase::OutcomeUnknown
            | CloudLedgerPhase::ReconciliationObserved
            | CloudLedgerPhase::TaskAdopted
            | CloudLedgerPhase::ExplicitlyAbandoned => {}
        }
        Ok(Some(ledger))
    }

    fn require_matching_config(&self, ledger: &CloudSubmitLedger) -> Result<(), CloudRunnerError> {
        if ledger.matches_config(&self.environment, &self.branch) {
            Ok(())
        } else {
            Err(CloudRunnerError::new(
                CloudRunnerErrorCategory::LedgerInvalid,
            ))
        }
    }

    fn dispatch_existing(
        &self,
        requested: CloudSubmitOperationId,
        ledger: Option<&CloudSubmitLedger>,
    ) -> Option<Result<CloudSubmission, CloudRunnerError>> {
        let ledger = ledger?;
        if ledger.latest().is_unknown() {
            return Some(Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::OutcomeUnknown,
                ledger.operation_id(),
            )));
        }
        if ledger.operation_id() != requested {
            return if ledger.latest().permits_new_operation() {
                None
            } else {
                Some(Err(CloudRunnerError::for_operation(
                    CloudRunnerErrorCategory::OutcomeUnknown,
                    ledger.operation_id(),
                )))
            };
        }
        match ledger.latest() {
            CloudLedgerPhase::TaskRecorded | CloudLedgerPhase::TaskAdopted => Some(
                ledger
                    .recorded_task()
                    .and_then(|task| {
                        task.ok_or_else(|| {
                            CloudRunnerError::new(CloudRunnerErrorCategory::LedgerInvalid)
                        })
                    })
                    .map(|task| CloudSubmission::new(requested, task)),
            ),
            CloudLedgerPhase::FailedBeforeSpawn => Some(Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::PriorFailedBeforeSpawn,
                requested,
            ))),
            CloudLedgerPhase::ExplicitlyAbandoned => Some(Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::PriorExplicitlyAbandoned,
                requested,
            ))),
            CloudLedgerPhase::Intent
            | CloudLedgerPhase::Authorized
            | CloudLedgerPhase::Started
            | CloudLedgerPhase::OutcomeUnknown
            | CloudLedgerPhase::ReconciliationObserved => {
                Some(Err(CloudRunnerError::for_operation(
                    CloudRunnerErrorCategory::OutcomeUnknown,
                    requested,
                )))
            }
        }
    }

    // T004A1 intentionally stages this crate-private bridge before T004B consumes it.
    #[allow(dead_code)]
    fn classify_observation(
        &self,
        requested: CloudSubmitOperationId,
        ledger: Option<&CloudSubmitLedger>,
    ) -> Result<CloudSubmitObservation, CloudRunnerError> {
        let Some(ledger) = ledger else {
            return Ok(CloudSubmitObservation::Absent);
        };
        if ledger.operation_id() != requested {
            return if ledger.latest().permits_new_operation() {
                Ok(CloudSubmitObservation::Absent)
            } else {
                Err(CloudRunnerError::for_operation(
                    CloudRunnerErrorCategory::OperationConflict,
                    ledger.operation_id(),
                ))
            };
        }

        match ledger.latest() {
            CloudLedgerPhase::FailedBeforeSpawn => Ok(CloudSubmitObservation::FailedBeforeSpawn),
            CloudLedgerPhase::OutcomeUnknown | CloudLedgerPhase::ReconciliationObserved => {
                Ok(CloudSubmitObservation::OutcomeUnknown)
            }
            CloudLedgerPhase::TaskRecorded | CloudLedgerPhase::TaskAdopted => {
                let task_id = ledger.recorded_task()?.ok_or_else(|| {
                    CloudRunnerError::new(CloudRunnerErrorCategory::LedgerInvalid)
                })?;
                Ok(CloudSubmitObservation::TaskRecorded(CloudSubmission::new(
                    requested, task_id,
                )))
            }
            CloudLedgerPhase::ExplicitlyAbandoned => {
                Ok(CloudSubmitObservation::ExplicitlyAbandoned)
            }
            CloudLedgerPhase::Intent | CloudLedgerPhase::Authorized | CloudLedgerPhase::Started => {
                Err(CloudRunnerError::new(
                    CloudRunnerErrorCategory::LedgerInvalid,
                ))
            }
        }
    }

    fn persist_unknown(
        &self,
        lease: &OwnedCredentialScopeLease,
        ledger: &mut CloudSubmitLedger,
    ) -> Result<(), CloudRunnerError> {
        if !ledger.latest().is_unknown() {
            ledger.append(CloudLedgerPhase::OutcomeUnknown)?;
        }
        persist_cloud_ledger(lease.state_dir(), lease.runner_uid(), ledger).map_err(|_| {
            CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::RecoveryRequired,
                ledger.operation_id(),
            )
        })
    }

    fn run_read(
        &self,
        lease: &OwnedCredentialScopeLease,
        invocation: &CloudInvocation,
        timeout: Duration,
    ) -> Result<CloudCommandCompletion, CloudRunnerError> {
        let mut supervisor = self
            .runtime
            .start(lease, invocation, timeout, CloudCancellation::new())
            .map_err(|failure| failure.error)?;
        let completion = supervisor.wait()?;
        match completion.end {
            CloudCommandEnd::Exited => Ok(completion),
            CloudCommandEnd::TimedOut => {
                Err(CloudRunnerError::new(CloudRunnerErrorCategory::Timeout))
            }
            CloudCommandEnd::Canceled => {
                Err(CloudRunnerError::new(CloudRunnerErrorCategory::Process))
            }
        }
    }
}

trait ErrorOperation {
    fn with_operation(self, operation_id: CloudSubmitOperationId) -> Self;
}

impl ErrorOperation for CloudRunnerError {
    fn with_operation(self, operation_id: CloudSubmitOperationId) -> Self {
        CloudRunnerError::for_operation(self.category(), operation_id)
    }
}

fn map_scope_error(error: CredentialScopeError) -> CloudRunnerError {
    let category = if matches!(error, CredentialScopeError::LoginAlreadyRunning) {
        CloudRunnerErrorCategory::Busy
    } else {
        CloudRunnerErrorCategory::Scope
    };
    CloudRunnerError::new(category)
}

fn map_login_ledger_error(error: LoginBrokerError) -> CloudRunnerError {
    let category = match error {
        LoginBrokerError::LedgerInvalid | LoginBrokerError::OutcomeUnknown => {
            CloudRunnerErrorCategory::LoginOutcomeUnknown
        }
        _ => CloudRunnerErrorCategory::Process,
    };
    CloudRunnerError::new(category)
}

fn map_runtime_error(error: LoginBrokerError) -> CloudRunnerError {
    let category = match error {
        LoginBrokerError::VersionMismatch => CloudRunnerErrorCategory::Version,
        LoginBrokerError::StatusUnavailable => CloudRunnerErrorCategory::NotAuthenticated,
        LoginBrokerError::ProviderOutputInvalid | LoginBrokerError::OutputLimitExceeded => {
            CloudRunnerErrorCategory::ProviderOutput
        }
        LoginBrokerError::CredentialScope(error) => return map_scope_error(error),
        LoginBrokerError::LoginAlreadyRunning => CloudRunnerErrorCategory::Busy,
        LoginBrokerError::LedgerInvalid
        | LoginBrokerError::LedgerUnavailable { .. }
        | LoginBrokerError::OutcomeUnknown => CloudRunnerErrorCategory::LoginOutcomeUnknown,
        LoginBrokerError::AlreadyLoggedIn
        | LoginBrokerError::LoginFailed
        | LoginBrokerError::Process { .. } => CloudRunnerErrorCategory::Process,
    };
    CloudRunnerError::new(category)
}

fn ensure_diagnostic_sentinel(lease: &OwnedCredentialScopeLease) -> Result<(), CloudRunnerError> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = lease;
        Err(CloudRunnerError::new(CloudRunnerErrorCategory::Scope))
    }

    #[cfg(target_os = "linux")]
    {
        let path = lease.working_dir().join(DIAGNOSTIC_SENTINEL_NAME);
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(
                        |_| CloudRunnerError::new(CloudRunnerErrorCategory::DiagnosticBoundary),
                    )?;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => {
                    return Err(CloudRunnerError::new(
                        CloudRunnerErrorCategory::DiagnosticBoundary,
                    ));
                }
            },
            Err(_) => {
                return Err(CloudRunnerError::new(
                    CloudRunnerErrorCategory::DiagnosticBoundary,
                ));
            }
        }

        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let directory = options
            .open(path)
            .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::DiagnosticBoundary))?;
        validate_diagnostic_sentinel(&directory, lease.runner_uid())
    }
}

#[cfg(target_os = "linux")]
fn validate_diagnostic_sentinel(directory: &File, runner_uid: u32) -> Result<(), CloudRunnerError> {
    let metadata = directory
        .metadata()
        .map_err(|_| CloudRunnerError::new(CloudRunnerErrorCategory::DiagnosticBoundary))?;
    if !metadata.is_dir() || metadata.uid() != runner_uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(CloudRunnerError::new(
            CloudRunnerErrorCategory::DiagnosticBoundary,
        ));
    }
    Ok(())
}

pub(crate) trait CloudCliRuntime: Send + Sync {
    fn verify_version(&self, lease: &OwnedCredentialScopeLease) -> Result<(), CloudRunnerError>;

    fn login_status(
        &self,
        lease: &OwnedCredentialScopeLease,
    ) -> Result<PinnedStatus, CloudRunnerError>;

    fn start(
        &self,
        lease: &OwnedCredentialScopeLease,
        invocation: &CloudInvocation,
        timeout: Duration,
        cancellation: CloudCancellation,
    ) -> Result<Box<dyn CloudCommandSupervisor>, CloudStartFailure>;
}

pub(crate) trait CloudCommandSupervisor: Send {
    fn process_identity(&self) -> ProcessIdentity;
    fn wait(&mut self) -> Result<CloudCommandCompletion, CloudRunnerError>;
    fn cancel_and_wait(&mut self) -> Result<CloudCommandCompletion, CloudRunnerError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CloudCommandEnd {
    Exited,
    Canceled,
    TimedOut,
}

pub(crate) struct CloudCommandCompletion {
    pub(crate) end: CloudCommandEnd,
    pub(crate) capture: CloudCapture,
}

pub(crate) struct CloudStartFailure {
    pub(crate) error: CloudRunnerError,
    pub(crate) may_have_started: bool,
}

struct ProcessCloudRuntime;

impl CloudCliRuntime for ProcessCloudRuntime {
    fn verify_version(&self, lease: &OwnedCredentialScopeLease) -> Result<(), CloudRunnerError> {
        ProcessRuntime::default()
            .verify_version(lease)
            .map_err(map_runtime_error)
    }

    fn login_status(
        &self,
        lease: &OwnedCredentialScopeLease,
    ) -> Result<PinnedStatus, CloudRunnerError> {
        ProcessRuntime::default()
            .status(lease)
            .map_err(map_runtime_error)
    }

    fn start(
        &self,
        lease: &OwnedCredentialScopeLease,
        invocation: &CloudInvocation,
        timeout: Duration,
        cancellation: CloudCancellation,
    ) -> Result<Box<dyn CloudCommandSupervisor>, CloudStartFailure> {
        let invocation = lease
            .cloud_invocation(invocation)
            .map_err(|error| CloudStartFailure {
                error: map_scope_error(error),
                may_have_started: false,
            })?;
        let spec = CommandSpec::from_cloud(&invocation);
        let mut child = spawn_bound_child(&spec).map_err(|error| CloudStartFailure {
            error: map_runtime_error(error),
            may_have_started: false,
        })?;
        let process = read_process_identity(child.id()).map_err(|error| {
            let _ = terminate_group(&mut child, TERMINATION_GRACE);
            CloudStartFailure {
                error: map_runtime_error(error),
                may_have_started: true,
            }
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            let _ = terminate_group(&mut child, TERMINATION_GRACE);
            CloudStartFailure {
                error: CloudRunnerError::new(CloudRunnerErrorCategory::Process),
                may_have_started: true,
            }
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            let _ = terminate_group(&mut child, TERMINATION_GRACE);
            CloudStartFailure {
                error: CloudRunnerError::new(CloudRunnerErrorCategory::Process),
                may_have_started: true,
            }
        })?;
        let stdout_capture = SharedCapture::with_limit(CLOUD_CAPTURE_BYTES);
        let stderr_capture = SharedCapture::with_limit(CLOUD_CAPTURE_BYTES);
        let stdout_thread = drain_stream(stdout, stdout_capture.clone());
        let stderr_thread = drain_stream(stderr, stderr_capture.clone());

        Ok(Box::new(ProcessCloudSupervisor {
            child,
            process,
            stdout_capture,
            stderr_capture,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            started_at: Instant::now(),
            timeout,
            cancellation,
            finished: false,
        }))
    }
}

struct ProcessCloudSupervisor {
    child: Child,
    process: ProcessIdentity,
    stdout_capture: SharedCapture,
    stderr_capture: SharedCapture,
    stdout_thread: Option<JoinHandle<Result<(), io::Error>>>,
    stderr_thread: Option<JoinHandle<Result<(), io::Error>>>,
    started_at: Instant,
    timeout: Duration,
    cancellation: CloudCancellation,
    finished: bool,
}

impl ProcessCloudSupervisor {
    fn complete(
        &mut self,
        end: CloudCommandEnd,
        status: Result<ExitStatus, io::Error>,
    ) -> Result<CloudCommandCompletion, CloudRunnerError> {
        let stdout_thread = self
            .stdout_thread
            .take()
            .ok_or_else(|| CloudRunnerError::new(CloudRunnerErrorCategory::Process))?;
        let stderr_thread = self
            .stderr_thread
            .take()
            .ok_or_else(|| CloudRunnerError::new(CloudRunnerErrorCategory::Process))?;
        let output = finish_capture(
            status,
            &self.stdout_capture,
            &self.stderr_capture,
            stdout_thread,
            stderr_thread,
        )
        .map_err(map_runtime_error)?;
        self.finished = true;
        Ok(CloudCommandCompletion {
            end,
            capture: CloudCapture::new(
                output.stdout,
                output.stderr,
                output.stdout_overflow,
                output.stderr_overflow,
                output.exit_code,
            ),
        })
    }
}

impl CloudCommandSupervisor for ProcessCloudSupervisor {
    fn process_identity(&self) -> ProcessIdentity {
        self.process
    }

    fn wait(&mut self) -> Result<CloudCommandCompletion, CloudRunnerError> {
        loop {
            if self.cancellation.is_cancelled() {
                let status = terminate_group(&mut self.child, TERMINATION_GRACE);
                return self.complete(CloudCommandEnd::Canceled, status);
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    return self.complete(CloudCommandEnd::Exited, Ok(status));
                }
                Ok(None) if self.started_at.elapsed() < self.timeout => {
                    thread::sleep(PROCESS_POLL_INTERVAL);
                }
                Ok(None) => {
                    let status = terminate_group(&mut self.child, TERMINATION_GRACE);
                    return self.complete(CloudCommandEnd::TimedOut, status);
                }
                Err(error) => {
                    let _ = terminate_group(&mut self.child, TERMINATION_GRACE);
                    return Err(CloudRunnerError::new(
                        if error.kind() == io::ErrorKind::TimedOut {
                            CloudRunnerErrorCategory::Timeout
                        } else {
                            CloudRunnerErrorCategory::Process
                        },
                    ));
                }
            }
        }
    }

    fn cancel_and_wait(&mut self) -> Result<CloudCommandCompletion, CloudRunnerError> {
        let status = terminate_group(&mut self.child, TERMINATION_GRACE);
        self.complete(CloudCommandEnd::Canceled, status)
    }
}

impl Drop for ProcessCloudSupervisor {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let status = terminate_group(&mut self.child, TERMINATION_GRACE);
        let _ = self.complete(CloudCommandEnd::Canceled, status);
    }
}
