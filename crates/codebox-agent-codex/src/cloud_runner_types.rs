use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    CloudBranch, CloudEnvironmentId, CloudPrompt, CloudTaskId, CloudTaskSummary, CredentialScope,
};

/// Strong identity and replay key for one non-idempotent Cloud submit.
///
/// Contract: `CU-CLOUD-P0-01`. The trusted caller creates this value before invoking the runner.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct CloudSubmitOperationId(Uuid);

impl CloudSubmitOperationId {
    /// Creates a fresh non-nil submit operation ID.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub fn new() -> Self {
        loop {
            let value = Uuid::new_v4();
            if !value.is_nil() {
                return Self(value);
            }
        }
    }

    pub(crate) const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Serialize for CloudSubmitOperationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CloudSubmitOperationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Uuid::deserialize(deserializer)?;
        if value.is_nil() {
            Err(serde::de::Error::custom(
                "Cloud submit operation ID cannot be nil",
            ))
        } else {
            Ok(Self(value))
        }
    }
}

impl Default for CloudSubmitOperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CloudSubmitOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CloudSubmitOperationId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for CloudSubmitOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Trusted administrator configuration for one Cloud runner.
///
/// Contract: `CU-CLOUD-P0-01`. It has no deserialization or browser-selected fallback surface.
pub struct CloudRunnerConfig {
    pub(crate) scope: CredentialScope,
    pub(crate) environment: CloudEnvironmentId,
    pub(crate) branch: CloudBranch,
}

impl CloudRunnerConfig {
    /// Combines an accepted credential scope with typed administrator Cloud configuration.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub fn new(
        scope: CredentialScope,
        environment: CloudEnvironmentId,
        branch: CloudBranch,
    ) -> Self {
        Self {
            scope,
            environment,
            branch,
        }
    }
}

impl fmt::Debug for CloudRunnerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudRunnerConfig")
            .finish_non_exhaustive()
    }
}

/// One caller-identified Cloud submit request.
///
/// Contract: `CU-CLOUD-P0-01`. Debug output never includes the prompt.
pub struct CloudSubmitRequest {
    pub(crate) operation_id: CloudSubmitOperationId,
    pub(crate) prompt: CloudPrompt,
}

impl CloudSubmitRequest {
    /// Creates a request whose operation ID is the durable replay identity.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub fn new(operation_id: CloudSubmitOperationId, prompt: CloudPrompt) -> Self {
        Self {
            operation_id,
            prompt,
        }
    }

    /// Returns the caller-created replay identity.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub const fn operation_id(&self) -> CloudSubmitOperationId {
        self.operation_id
    }
}

impl fmt::Debug for CloudSubmitRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudSubmitRequest")
            .field("operation_id", &self.operation_id)
            .field("prompt", &"[REDACTED]")
            .finish()
    }
}

/// Cloneable one-way cancellation signal for one fixed Cloud command.
///
/// Contract: `CU-CLOUD-P0-01`.
#[derive(Clone)]
pub struct CloudCancellation(Arc<AtomicBool>);

impl CloudCancellation {
    /// Creates an uncanceled signal.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Requests local process-group cancellation.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

impl Default for CloudCancellation {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for CloudCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudCancellation")
            .field("cancel_requested", &self.is_cancelled())
            .finish()
    }
}

/// Durable successful projection of one Cloud submit.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudSubmission {
    operation_id: CloudSubmitOperationId,
    task_id: CloudTaskId,
}

impl CloudSubmission {
    pub(crate) fn new(operation_id: CloudSubmitOperationId, task_id: CloudTaskId) -> Self {
        Self {
            operation_id,
            task_id,
        }
    }

    /// Returns the submit replay identity.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub const fn operation_id(&self) -> CloudSubmitOperationId {
        self.operation_id
    }

    /// Returns the durably recorded provider task ID.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub const fn task_id(&self) -> &CloudTaskId {
        &self.task_id
    }
}

impl fmt::Debug for CloudSubmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudSubmission")
            .field("operation_id", &self.operation_id)
            .field("task_id", &self.task_id)
            .finish()
    }
}

/// Bounded provider candidates observed for one unknown submit.
#[derive(Clone, Eq, PartialEq)]
pub struct CloudReconciliation {
    operation_id: CloudSubmitOperationId,
    tasks: Vec<CloudTaskSummary>,
    complete: bool,
}

impl CloudReconciliation {
    pub(crate) fn new(
        operation_id: CloudSubmitOperationId,
        tasks: Vec<CloudTaskSummary>,
        complete: bool,
    ) -> Self {
        Self {
            operation_id,
            tasks,
            complete,
        }
    }

    /// Returns the unknown submit operation being reconciled.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub const fn operation_id(&self) -> CloudSubmitOperationId {
        self.operation_id
    }

    /// Returns at most 100 bounded provider candidates.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub fn tasks(&self) -> &[CloudTaskSummary] {
        &self.tasks
    }

    /// Reports whether cursor exhaustion was observed within the reconciliation bounds.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

impl fmt::Debug for CloudReconciliation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudReconciliation")
            .field("operation_id", &self.operation_id)
            .field("task_count", &self.tasks.len())
            .field("complete", &self.complete)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
// T004A1 intentionally stages this crate-private bridge before T004B consumes it.
#[allow(dead_code)]
pub(crate) enum CloudSubmitObservation {
    Absent,
    FailedBeforeSpawn,
    OutcomeUnknown,
    TaskRecorded(CloudSubmission),
    ExplicitlyAbandoned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
// T004A1 intentionally stages this crate-private bridge before T004B consumes it.
#[allow(dead_code)]
pub(crate) enum CloudUnknownResolution {
    AdoptListedTask(CloudTaskId),
    ExplicitlyAbandon,
}

/// Redacted Cloud runner failure class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudRunnerErrorCategory {
    Scope,
    Busy,
    Version,
    NotAuthenticated,
    LoginOutcomeUnknown,
    DiagnosticBoundary,
    LedgerInvalid,
    LedgerUnavailable,
    Process,
    Timeout,
    CanceledBeforeStart,
    ProviderOutput,
    OutcomeUnknown,
    RecoveryRequired,
    PriorFailedBeforeSpawn,
    PriorExplicitlyAbandoned,
    ReconciliationCycle,
    NoUnknownOperation,
    OperationConflict,
    ResolutionUnavailable,
    ResolutionConflict,
    CandidateNotRecorded,
}

impl fmt::Display for CloudRunnerErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Scope => "credential scope is unavailable",
            Self::Busy => "credential scope is busy",
            Self::Version => "CLI version is not accepted",
            Self::NotAuthenticated => "ChatGPT login is required",
            Self::LoginOutcomeUnknown => "login outcome requires reconciliation",
            Self::DiagnosticBoundary => "diagnostic write boundary is unsafe",
            Self::LedgerInvalid => "submit ledger is invalid",
            Self::LedgerUnavailable => "submit ledger is unavailable",
            Self::Process => "Cloud process failed",
            Self::Timeout => "Cloud process timed out",
            Self::CanceledBeforeStart => "Cloud submit was canceled before authorization",
            Self::ProviderOutput => "provider output does not match the pinned contract",
            Self::OutcomeUnknown => "Cloud submit outcome requires reconciliation",
            Self::RecoveryRequired => "Cloud submit recovery requires operator action",
            Self::PriorFailedBeforeSpawn => "submit previously failed before spawn",
            Self::PriorExplicitlyAbandoned => "submit was explicitly abandoned",
            Self::ReconciliationCycle => "provider pagination contains a cycle",
            Self::NoUnknownOperation => "no unknown Cloud submit can be reconciled",
            Self::OperationConflict => "another Cloud submit operation owns the durable state",
            Self::ResolutionUnavailable => "Cloud submit cannot be resolved in its current state",
            Self::ResolutionConflict => "Cloud submit already has a different resolution",
            Self::CandidateNotRecorded => "Cloud task is not in the recorded candidate set",
        })
    }
}

/// Typed, redacted Cloud runner error.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("Codex Cloud runner: {category}")]
pub struct CloudRunnerError {
    category: CloudRunnerErrorCategory,
    operation_id: Option<CloudSubmitOperationId>,
}

impl CloudRunnerError {
    pub(crate) const fn new(category: CloudRunnerErrorCategory) -> Self {
        Self {
            category,
            operation_id: None,
        }
    }

    pub(crate) const fn for_operation(
        category: CloudRunnerErrorCategory,
        operation_id: CloudSubmitOperationId,
    ) -> Self {
        Self {
            category,
            operation_id: Some(operation_id),
        }
    }

    /// Returns the safe failure classification.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub const fn category(&self) -> CloudRunnerErrorCategory {
        self.category
    }

    /// Returns the affected operation ID when one was durably assigned.
    ///
    /// Contract: `CU-CLOUD-P0-01`.
    pub const fn operation_id(&self) -> Option<CloudSubmitOperationId> {
        self.operation_id
    }
}
