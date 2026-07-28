use std::fmt;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::cloud_lifecycle_ledger::{
    CloudLifecycleLedger, CloudLifecyclePhase, load_lifecycle_ledger, persist_lifecycle_ledger,
    remove_lifecycle_ledger,
};
use crate::cloud_lifecycle_types::{
    CloudLifecycle, CloudLifecycleError, CloudLifecycleErrorCategory, UnknownSubmitDecision,
};
use crate::cloud_runner_types::{CloudSubmitObservation, CloudUnknownResolution};
use crate::scope::OwnedCredentialScopeLease;
use crate::{
    CloudCancellation, CloudPrompt, CloudReconciliation, CloudRunnerConfig, CloudRunnerError,
    CloudRunnerErrorCategory, CloudSubmission, CloudSubmitOperationId, CloudSubmitRequest,
    CloudTaskId, CloudTaskRunner, CloudTaskStatus,
};

pub(crate) trait LifecycleCloudRunner: Send + Sync {
    fn acquire_scope(&self) -> Result<OwnedCredentialScopeLease, CloudRunnerError>;
    fn observe_submit(
        &self,
        operation_id: CloudSubmitOperationId,
    ) -> Result<CloudSubmitObservation, CloudRunnerError>;
    fn submit(
        &self,
        request: CloudSubmitRequest,
        cancellation: CloudCancellation,
    ) -> Result<CloudSubmission, CloudRunnerError>;
    fn status(&self, task_id: &CloudTaskId) -> Result<CloudTaskStatus, CloudRunnerError>;
    fn reconcile_unknown(&self) -> Result<CloudReconciliation, CloudRunnerError>;
    fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        resolution: CloudUnknownResolution,
    ) -> Result<CloudSubmitObservation, CloudRunnerError>;
}

impl LifecycleCloudRunner for CloudTaskRunner {
    fn acquire_scope(&self) -> Result<OwnedCredentialScopeLease, CloudRunnerError> {
        self.acquire_scope()
    }

    fn observe_submit(
        &self,
        operation_id: CloudSubmitOperationId,
    ) -> Result<CloudSubmitObservation, CloudRunnerError> {
        self.observe_submit(operation_id)
    }

    fn submit(
        &self,
        request: CloudSubmitRequest,
        cancellation: CloudCancellation,
    ) -> Result<CloudSubmission, CloudRunnerError> {
        self.submit(request, cancellation)
    }

    fn status(&self, task_id: &CloudTaskId) -> Result<CloudTaskStatus, CloudRunnerError> {
        self.status(task_id)
    }

    fn reconcile_unknown(&self) -> Result<CloudReconciliation, CloudRunnerError> {
        self.reconcile_unknown()
    }

    fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        resolution: CloudUnknownResolution,
    ) -> Result<CloudSubmitObservation, CloudRunnerError> {
        self.resolve_unknown(operation_id, resolution)
    }
}

struct OrchestratorState {
    ledger: Option<CloudLifecycleLedger>,
    cancellation: Option<CloudCancellation>,
}

/// Serialized, crash-safe local lifecycle for one provider-specific Codex Cloud task.
///
/// Contract: `CU-AGT-P0-02`. This type has no polling loop, browser lease, local diff application,
/// or provider-side cancellation authority.
pub struct CloudTaskOrchestrator {
    runner: Arc<dyn LifecycleCloudRunner>,
    state: Mutex<OrchestratorState>,
    changed: Condvar,
}

impl fmt::Debug for CloudTaskOrchestrator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudTaskOrchestrator")
            .finish_non_exhaustive()
    }
}

impl CloudTaskOrchestrator {
    /// Constructs an orchestrator and durably repairs any interrupted local submission.
    ///
    /// Contract: `CU-AGT-P0-02`. Construction performs no provider command.
    pub fn new(config: CloudRunnerConfig) -> Result<Self, CloudLifecycleError> {
        let runner = CloudTaskRunner::new(config).map_err(map_runner_error)?;
        Self::from_runner(Arc::new(runner))
    }

    #[cfg(test)]
    pub(crate) fn with_runner(
        runner: Arc<dyn LifecycleCloudRunner>,
    ) -> Result<Self, CloudLifecycleError> {
        Self::from_runner(runner)
    }

    fn from_runner(runner: Arc<dyn LifecycleCloudRunner>) -> Result<Self, CloudLifecycleError> {
        let ledger = load_with_runner(runner.as_ref())?;
        let orchestrator = Self {
            runner,
            state: Mutex::new(OrchestratorState {
                ledger,
                cancellation: None,
            }),
            changed: Condvar::new(),
        };
        orchestrator.repair_interrupted_state()?;
        Ok(orchestrator)
    }

    /// Starts exactly one explicit Cloud submit and returns its durable local projection.
    ///
    /// Contract: `CU-AGT-P0-02`. Unknown outcomes are never submitted again automatically.
    pub fn start(&self, prompt: CloudPrompt) -> Result<CloudLifecycle, CloudLifecycleError> {
        let mut state = self.lock_state()?;
        if state
            .ledger
            .as_ref()
            .is_some_and(|ledger| !ledger.phase().is_terminal())
        {
            return Err(current_error(
                CloudLifecycleErrorCategory::TurnAlreadyRunning,
                state.ledger.as_ref(),
            ));
        }

        let operation_id = CloudSubmitOperationId::new();
        match self.runner.observe_submit(operation_id) {
            Ok(CloudSubmitObservation::Absent) => {}
            Ok(_) => {
                return Err(CloudLifecycleError::for_operation(
                    CloudLifecycleErrorCategory::OperationConflict,
                    operation_id,
                ));
            }
            Err(error) => return Err(map_runner_error(error)),
        }

        let prior = state.ledger.clone();
        let submitting = CloudLifecycleLedger::submitting(operation_id);
        self.persist(&submitting)?;
        let cancellation = CloudCancellation::new();
        state.ledger = Some(submitting);
        state.cancellation = Some(cancellation.clone());
        drop(state);

        let result = self
            .runner
            .submit(CloudSubmitRequest::new(operation_id, prompt), cancellation);

        let mut state = self.lock_state()?;
        if !matches!(
            state.ledger.as_ref(),
            Some(ledger)
                if ledger.operation_id() == operation_id
                    && ledger.phase() == CloudLifecyclePhase::Submitting
        ) {
            state.cancellation = None;
            self.changed.notify_all();
            return Err(CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::RecoveryRequired,
                operation_id,
            ));
        }

        if let Some(reported) = result
            .as_ref()
            .err()
            .and_then(CloudRunnerError::operation_id)
            .filter(|reported| *reported != operation_id)
        {
            return self.rollback_after_conflict(state, prior, operation_id, reported);
        }

        let mut ledger = state.ledger.clone().ok_or_else(|| {
            CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::RecoveryRequired,
                operation_id,
            )
        })?;
        match result {
            Ok(submission) => {
                if submission.operation_id() != operation_id {
                    return self.rollback_after_conflict(
                        state,
                        prior,
                        operation_id,
                        submission.operation_id(),
                    );
                }
                ledger.record_status(submission.task_id(), CloudTaskStatus::Pending)?;
            }
            Err(error) => match error.category() {
                CloudRunnerErrorCategory::CanceledBeforeStart => {
                    ledger.record_canceled_without_task()?;
                }
                CloudRunnerErrorCategory::OutcomeUnknown
                | CloudRunnerErrorCategory::RecoveryRequired => {
                    if error.operation_id() != Some(operation_id) {
                        state.cancellation = None;
                        self.changed.notify_all();
                        return Err(CloudLifecycleError::for_operation(
                            CloudLifecycleErrorCategory::RecoveryRequired,
                            operation_id,
                        ));
                    }
                    ledger.record_outcome_unknown()?;
                }
                CloudRunnerErrorCategory::OperationConflict => {
                    return self.rollback_after_conflict(
                        state,
                        prior,
                        operation_id,
                        error.operation_id().unwrap_or(operation_id),
                    );
                }
                _ => ledger.record_failed_before_submit()?,
            },
        }
        if let Err(error) = self.persist_after_effect(&ledger) {
            state.cancellation = None;
            self.changed.notify_all();
            return Err(error);
        }
        let lifecycle = ledger.lifecycle()?;
        state.ledger = Some(ledger);
        state.cancellation = None;
        self.changed.notify_all();
        Ok(lifecycle)
    }

    /// Performs at most one provider status read for a pending task.
    ///
    /// Contract: `CU-AGT-P0-02`.
    pub fn inspect(&self) -> Result<CloudLifecycle, CloudLifecycleError> {
        let mut state = self.lock_state()?;
        let current = state
            .ledger
            .as_ref()
            .ok_or_else(|| {
                CloudLifecycleError::new(CloudLifecycleErrorCategory::NoCurrentOperation)
            })?
            .lifecycle()?;
        let task_id = match &current {
            CloudLifecycle::Pending { task_id, .. } => task_id.clone(),
            CloudLifecycle::Submitting { operation_id } => {
                return Err(CloudLifecycleError::for_operation(
                    CloudLifecycleErrorCategory::TurnAlreadyRunning,
                    *operation_id,
                ));
            }
            CloudLifecycle::OutcomeUnknown { operation_id } => {
                return Err(CloudLifecycleError::for_operation(
                    CloudLifecycleErrorCategory::OutcomeUnknown,
                    *operation_id,
                ));
            }
            _ => return Ok(current),
        };
        let status = self
            .runner
            .status(&task_id)
            .map_err(|error| map_provider_read(error, current.operation_id()))?;
        let mut ledger = state.ledger.clone().ok_or_else(|| {
            CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::RecoveryRequired,
                current.operation_id(),
            )
        })?;
        ledger.record_status(&task_id, status)?;
        self.persist_after_effect(&ledger)?;
        let lifecycle = ledger.lifecycle()?;
        state.ledger = Some(ledger);
        Ok(lifecycle)
    }

    /// Records one bounded reconciliation for the current unknown submit.
    ///
    /// Contract: `CU-AGT-P0-02`. Reconciliation alone never authorizes another submit.
    pub fn reconcile_unknown(&self) -> Result<CloudReconciliation, CloudLifecycleError> {
        let mut state = self.lock_state()?;
        let operation_id = require_phase(&state, CloudLifecyclePhase::OutcomeUnknown)?;
        let reconciliation = self.runner.reconcile_unknown().map_err(map_runner_error)?;
        if reconciliation.operation_id() != operation_id {
            return Err(CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::OperationConflict,
                reconciliation.operation_id(),
            ));
        }
        let task_ids: Vec<_> = reconciliation
            .tasks()
            .iter()
            .map(|task| task.id().clone())
            .collect();
        let mut ledger = state.ledger.clone().ok_or_else(|| {
            CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::RecoveryRequired,
                operation_id,
            )
        })?;
        ledger.record_reconciliation(&task_ids, reconciliation.is_complete())?;
        self.persist_after_effect(&ledger)?;
        state.ledger = Some(ledger);
        Ok(reconciliation)
    }

    /// Applies one operation-bound explicit unknown-submit decision.
    ///
    /// Contract: `CU-AGT-P0-02`. The lower submit is terminalized before the local projection.
    pub fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    ) -> Result<CloudLifecycle, CloudLifecycleError> {
        let mut state = self.lock_state()?;
        let ledger = state.ledger.as_ref().ok_or_else(|| {
            CloudLifecycleError::new(CloudLifecycleErrorCategory::NoCurrentOperation)
        })?;
        if ledger.operation_id() != operation_id {
            return Err(CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::StaleDecision,
                operation_id,
            ));
        }
        if ledger.phase().is_terminal() || ledger.phase() == CloudLifecyclePhase::Pending {
            return replay_resolution(ledger, &decision);
        }
        if ledger.phase() != CloudLifecyclePhase::OutcomeUnknown {
            return Err(CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::WrongState,
                operation_id,
            ));
        }
        if ledger.reconciliation_generation() == 0 {
            return Err(CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::WrongState,
                operation_id,
            ));
        }

        let (resolution, adopted_status) = match &decision {
            UnknownSubmitDecision::AdoptListedTask(task_id) => {
                if ledger.reconciliation_complete() != Some(true) || !ledger.has_candidate(task_id)
                {
                    return Err(CloudLifecycleError::for_operation(
                        CloudLifecycleErrorCategory::TaskNotListed,
                        operation_id,
                    ));
                }
                let status = self
                    .runner
                    .status(task_id)
                    .map_err(|error| map_provider_read(error, operation_id))?;
                (
                    CloudUnknownResolution::AdoptListedTask(task_id.clone()),
                    Some((task_id.clone(), status)),
                )
            }
            UnknownSubmitDecision::AbandonAfterReconciliation(acknowledgement) => {
                if acknowledgement.operation_id() != operation_id {
                    return Err(CloudLifecycleError::for_operation(
                        CloudLifecycleErrorCategory::AcknowledgementRequired,
                        operation_id,
                    ));
                }
                (CloudUnknownResolution::ExplicitlyAbandon, None)
            }
        };

        let lower_observation = self
            .runner
            .resolve_unknown(operation_id, resolution)
            .map_err(map_runner_error)?;
        match (&adopted_status, lower_observation) {
            (Some((task_id, _)), CloudSubmitObservation::TaskRecorded(submission))
                if submission.operation_id() == operation_id && submission.task_id() == task_id => {
            }
            (None, CloudSubmitObservation::ExplicitlyAbandoned) => {}
            _ => {
                return Err(CloudLifecycleError::for_operation(
                    CloudLifecycleErrorCategory::OperationConflict,
                    operation_id,
                ));
            }
        }
        let mut next = state.ledger.clone().ok_or_else(|| {
            CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::RecoveryRequired,
                operation_id,
            )
        })?;
        match adopted_status {
            Some((task_id, status)) => next
                .record_status(&task_id, status)
                .map_err(|_| recovery_required(operation_id))?,
            None => next
                .record_abandoned_unknown()
                .map_err(|_| recovery_required(operation_id))?,
        }
        self.persist_after_effect(&next)?;
        let lifecycle = next.lifecycle()?;
        state.ledger = Some(next);
        Ok(lifecycle)
    }

    /// Requests local cancellation without claiming provider-side termination.
    ///
    /// Contract: `CU-AGT-P0-02`.
    pub fn cancel(&self) -> Result<CloudLifecycle, CloudLifecycleError> {
        let mut state = self.lock_state()?;
        loop {
            let current = state
                .ledger
                .as_ref()
                .ok_or_else(|| {
                    CloudLifecycleError::new(CloudLifecycleErrorCategory::NoCurrentOperation)
                })?
                .lifecycle()?;
            match current {
                CloudLifecycle::Submitting { operation_id } => {
                    let cancellation = state.cancellation.clone().ok_or_else(|| {
                        CloudLifecycleError::for_operation(
                            CloudLifecycleErrorCategory::RecoveryRequired,
                            operation_id,
                        )
                    })?;
                    cancellation.cancel();
                    state = self.changed.wait(state).map_err(|_| {
                        CloudLifecycleError::for_operation(
                            CloudLifecycleErrorCategory::RecoveryRequired,
                            operation_id,
                        )
                    })?;
                }
                CloudLifecycle::Pending { operation_id, .. } => {
                    let mut ledger = state.ledger.clone().ok_or_else(|| {
                        CloudLifecycleError::for_operation(
                            CloudLifecycleErrorCategory::RecoveryRequired,
                            operation_id,
                        )
                    })?;
                    ledger.record_canceled_with_task()?;
                    self.persist_after_effect(&ledger)?;
                    let lifecycle = ledger.lifecycle()?;
                    state.ledger = Some(ledger);
                    return Ok(lifecycle);
                }
                CloudLifecycle::OutcomeUnknown { .. }
                | CloudLifecycle::FailedBeforeSubmit { .. }
                | CloudLifecycle::Ready { .. }
                | CloudLifecycle::Applied { .. }
                | CloudLifecycle::ProviderError { .. }
                | CloudLifecycle::CanceledLocally { .. }
                | CloudLifecycle::AbandonedUnknown { .. } => return Ok(current),
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn current_for_test(&self) -> Result<Option<CloudLifecycle>, CloudLifecycleError> {
        self.lock_state()?
            .ledger
            .as_ref()
            .map(CloudLifecycleLedger::lifecycle)
            .transpose()
    }

    fn repair_interrupted_state(&self) -> Result<(), CloudLifecycleError> {
        let mut state = self.lock_state()?;
        let Some(current) = state.ledger.as_ref() else {
            return Ok(());
        };
        if !matches!(
            current.phase(),
            CloudLifecyclePhase::Submitting | CloudLifecyclePhase::OutcomeUnknown
        ) {
            return Ok(());
        }
        let operation_id = current.operation_id();
        let observation = self
            .runner
            .observe_submit(operation_id)
            .map_err(map_runner_error)?;
        let mut repaired = current.clone();
        let changed = match (current.phase(), observation) {
            (
                CloudLifecyclePhase::Submitting,
                CloudSubmitObservation::Absent | CloudSubmitObservation::FailedBeforeSpawn,
            ) => {
                repaired.record_failed_before_submit()?;
                true
            }
            (CloudLifecyclePhase::Submitting, CloudSubmitObservation::OutcomeUnknown) => {
                repaired.record_outcome_unknown()?;
                true
            }
            (
                CloudLifecyclePhase::Submitting | CloudLifecyclePhase::OutcomeUnknown,
                CloudSubmitObservation::TaskRecorded(submission),
            ) => {
                if submission.operation_id() != operation_id {
                    return Err(CloudLifecycleError::for_operation(
                        CloudLifecycleErrorCategory::OperationConflict,
                        submission.operation_id(),
                    ));
                }
                repaired.record_status(submission.task_id(), CloudTaskStatus::Pending)?;
                true
            }
            (
                CloudLifecyclePhase::Submitting | CloudLifecyclePhase::OutcomeUnknown,
                CloudSubmitObservation::ExplicitlyAbandoned,
            ) => {
                if current.phase() == CloudLifecyclePhase::Submitting {
                    repaired.record_outcome_unknown()?;
                }
                repaired.record_abandoned_unknown()?;
                true
            }
            (CloudLifecyclePhase::OutcomeUnknown, CloudSubmitObservation::OutcomeUnknown) => false,
            (
                CloudLifecyclePhase::OutcomeUnknown,
                CloudSubmitObservation::Absent | CloudSubmitObservation::FailedBeforeSpawn,
            ) => {
                return Err(CloudLifecycleError::for_operation(
                    CloudLifecycleErrorCategory::OperationConflict,
                    operation_id,
                ));
            }
            _ => {
                return Err(CloudLifecycleError::for_operation(
                    CloudLifecycleErrorCategory::OperationConflict,
                    operation_id,
                ));
            }
        };
        if changed {
            self.persist(&repaired)?;
            state.ledger = Some(repaired);
        }
        Ok(())
    }

    fn rollback_after_conflict(
        &self,
        mut state: MutexGuard<'_, OrchestratorState>,
        prior: Option<CloudLifecycleLedger>,
        operation_id: CloudSubmitOperationId,
        conflicting_operation_id: CloudSubmitOperationId,
    ) -> Result<CloudLifecycle, CloudLifecycleError> {
        let restored = match &prior {
            Some(ledger) => self.persist(ledger),
            None => self.remove(),
        };
        state.cancellation = None;
        self.changed.notify_all();
        match restored {
            Ok(()) => {
                state.ledger = prior;
                Err(CloudLifecycleError::for_operation(
                    CloudLifecycleErrorCategory::OperationConflict,
                    conflicting_operation_id,
                ))
            }
            Err(_) => Err(CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::RecoveryRequired,
                operation_id,
            )),
        }
    }

    fn persist(&self, ledger: &CloudLifecycleLedger) -> Result<(), CloudLifecycleError> {
        let lease = self.runner.acquire_scope().map_err(map_runner_error)?;
        persist_lifecycle_ledger(lease.state_dir(), lease.runner_uid(), ledger)
    }

    fn persist_after_effect(
        &self,
        ledger: &CloudLifecycleLedger,
    ) -> Result<(), CloudLifecycleError> {
        self.persist(ledger).map_err(|_| {
            CloudLifecycleError::for_operation(
                CloudLifecycleErrorCategory::RecoveryRequired,
                ledger.operation_id(),
            )
        })
    }

    fn remove(&self) -> Result<(), CloudLifecycleError> {
        let lease = self.runner.acquire_scope().map_err(map_runner_error)?;
        remove_lifecycle_ledger(lease.state_dir(), lease.runner_uid())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, OrchestratorState>, CloudLifecycleError> {
        self.state
            .lock()
            .map_err(|_| CloudLifecycleError::new(CloudLifecycleErrorCategory::RecoveryRequired))
    }
}

fn load_with_runner(
    runner: &dyn LifecycleCloudRunner,
) -> Result<Option<CloudLifecycleLedger>, CloudLifecycleError> {
    let lease = runner.acquire_scope().map_err(map_runner_error)?;
    load_lifecycle_ledger(lease.state_dir(), lease.runner_uid())
}

fn require_phase(
    state: &OrchestratorState,
    expected: CloudLifecyclePhase,
) -> Result<CloudSubmitOperationId, CloudLifecycleError> {
    let ledger = state
        .ledger
        .as_ref()
        .ok_or_else(|| CloudLifecycleError::new(CloudLifecycleErrorCategory::NoCurrentOperation))?;
    if ledger.phase() == expected {
        Ok(ledger.operation_id())
    } else {
        Err(CloudLifecycleError::for_operation(
            CloudLifecycleErrorCategory::WrongState,
            ledger.operation_id(),
        ))
    }
}

fn replay_resolution(
    ledger: &CloudLifecycleLedger,
    decision: &UnknownSubmitDecision,
) -> Result<CloudLifecycle, CloudLifecycleError> {
    let lifecycle = ledger.lifecycle()?;
    let exact = ledger.reconciliation_generation() > 0
        && match (decision, &lifecycle) {
            (
                UnknownSubmitDecision::AdoptListedTask(requested),
                CloudLifecycle::Pending { task_id, .. }
                | CloudLifecycle::Ready { task_id, .. }
                | CloudLifecycle::Applied { task_id, .. }
                | CloudLifecycle::ProviderError { task_id, .. },
            ) => requested == task_id,
            (
                UnknownSubmitDecision::AbandonAfterReconciliation(acknowledgement),
                CloudLifecycle::AbandonedUnknown { operation_id },
            ) => acknowledgement.operation_id() == *operation_id,
            _ => false,
        };
    if exact {
        Ok(lifecycle)
    } else {
        Err(CloudLifecycleError::for_operation(
            CloudLifecycleErrorCategory::StaleDecision,
            ledger.operation_id(),
        ))
    }
}

fn current_error(
    category: CloudLifecycleErrorCategory,
    ledger: Option<&CloudLifecycleLedger>,
) -> CloudLifecycleError {
    match ledger {
        Some(ledger) => CloudLifecycleError::for_operation(category, ledger.operation_id()),
        None => CloudLifecycleError::new(category),
    }
}

fn map_provider_read(
    _error: CloudRunnerError,
    operation_id: CloudSubmitOperationId,
) -> CloudLifecycleError {
    CloudLifecycleError::for_operation(CloudLifecycleErrorCategory::ProviderRead, operation_id)
}

fn recovery_required(operation_id: CloudSubmitOperationId) -> CloudLifecycleError {
    CloudLifecycleError::for_operation(CloudLifecycleErrorCategory::RecoveryRequired, operation_id)
}

fn map_runner_error(error: CloudRunnerError) -> CloudLifecycleError {
    let category = match error.category() {
        CloudRunnerErrorCategory::Scope => CloudLifecycleErrorCategory::Scope,
        CloudRunnerErrorCategory::Busy => CloudLifecycleErrorCategory::Busy,
        CloudRunnerErrorCategory::OperationConflict => {
            CloudLifecycleErrorCategory::OperationConflict
        }
        CloudRunnerErrorCategory::OutcomeUnknown => CloudLifecycleErrorCategory::OutcomeUnknown,
        CloudRunnerErrorCategory::RecoveryRequired => CloudLifecycleErrorCategory::RecoveryRequired,
        CloudRunnerErrorCategory::LedgerInvalid => CloudLifecycleErrorCategory::LedgerInvalid,
        CloudRunnerErrorCategory::LedgerUnavailable => {
            CloudLifecycleErrorCategory::LedgerUnavailable
        }
        _ => CloudLifecycleErrorCategory::LowerRunner,
    };
    match error.operation_id() {
        Some(operation_id) => CloudLifecycleError::for_operation(category, operation_id),
        None => CloudLifecycleError::new(category),
    }
}
