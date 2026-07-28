use std::fmt;

use codebox_agent_codex::{CloudLifecycle, CloudSubmitOperationId, CloudTaskId, CloudTaskSummary};
use codebox_domain::{EventSeq, SessionId, TurnId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// Fresh identity for one control-plane process instance.
///
/// Contract: `CU-SES-P0-01`. A new value invalidates stale P0 mutation headers after restart.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct P0InstanceId(Uuid);

impl P0InstanceId {
    pub(crate) fn new() -> Self {
        loop {
            let value = Uuid::new_v4();
            if !value.is_nil() {
                return Self(value);
            }
        }
    }

    /// Validates a process-instance UUID received at a transport boundary.
    ///
    /// Contract: `CU-SES-P0-01`. Nil is rejected.
    pub fn try_from_uuid(value: Uuid) -> Result<Self, &'static str> {
        if value.is_nil() {
            Err("P0 instance ID cannot be nil")
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the underlying non-nil UUID.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Debug for P0InstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for P0InstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for P0InstanceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for P0InstanceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Uuid::deserialize(deserializer)?;
        Self::try_from_uuid(value).map_err(serde::de::Error::custom)
    }
}

/// The only actor recognized by the personal single-operator P0 runtime.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum P0Actor {
    Operator,
}

/// Stable identity returned by every P0 snapshot.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct P0SessionIdentity {
    pub session_id: SessionId,
    pub instance_id: P0InstanceId,
}

/// Reduced process-lifetime P0 session state.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum P0SessionState {
    Ready,
    Running,
    RecoveryRequired,
    MonitoringDegraded,
    Stopped,
}

/// Serializable provider task identity projected to the P0 web protocol.
///
/// Contract: `CU-SES-P0-01`. Construction remains private and accepts only an already-validated
/// T003 `CloudTaskId`.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct P0CloudTaskId(String);

impl P0CloudTaskId {
    fn from_cloud(value: CloudTaskId) -> Self {
        Self(value.as_str().to_owned())
    }

    /// Returns the accepted provider task identifier.
    ///
    /// Contract: `CU-SES-P0-01`. This is a display projection, not diff/recovery authority.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for P0CloudTaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("P0CloudTaskId")
            .field(&self.0)
            .finish()
    }
}

/// Serializable allowlisted projection of the accepted provider-specific Cloud lifecycle.
///
/// Contract: `CU-SES-P0-01`. It contains no prompt, URL, diff, raw output, configuration, or path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum P0CloudLifecycle {
    Submitting {
        operation_id: CloudSubmitOperationId,
    },
    FailedBeforeSubmit {
        operation_id: CloudSubmitOperationId,
    },
    OutcomeUnknown {
        operation_id: CloudSubmitOperationId,
    },
    Pending {
        operation_id: CloudSubmitOperationId,
        task_id: P0CloudTaskId,
    },
    Ready {
        operation_id: CloudSubmitOperationId,
        task_id: P0CloudTaskId,
    },
    Applied {
        operation_id: CloudSubmitOperationId,
        task_id: P0CloudTaskId,
    },
    ProviderError {
        operation_id: CloudSubmitOperationId,
        task_id: P0CloudTaskId,
    },
    CanceledLocally {
        operation_id: CloudSubmitOperationId,
        task_id: Option<P0CloudTaskId>,
        provider_may_continue: bool,
    },
    AbandonedUnknown {
        operation_id: CloudSubmitOperationId,
    },
}

impl P0CloudLifecycle {
    pub(crate) fn from_cloud(value: CloudLifecycle) -> Self {
        match value {
            CloudLifecycle::Submitting { operation_id } => Self::Submitting { operation_id },
            CloudLifecycle::FailedBeforeSubmit { operation_id } => {
                Self::FailedBeforeSubmit { operation_id }
            }
            CloudLifecycle::OutcomeUnknown { operation_id } => {
                Self::OutcomeUnknown { operation_id }
            }
            CloudLifecycle::Pending {
                operation_id,
                task_id,
            } => Self::Pending {
                operation_id,
                task_id: P0CloudTaskId::from_cloud(task_id),
            },
            CloudLifecycle::Ready {
                operation_id,
                task_id,
            } => Self::Ready {
                operation_id,
                task_id: P0CloudTaskId::from_cloud(task_id),
            },
            CloudLifecycle::Applied {
                operation_id,
                task_id,
            } => Self::Applied {
                operation_id,
                task_id: P0CloudTaskId::from_cloud(task_id),
            },
            CloudLifecycle::ProviderError {
                operation_id,
                task_id,
            } => Self::ProviderError {
                operation_id,
                task_id: P0CloudTaskId::from_cloud(task_id),
            },
            CloudLifecycle::CanceledLocally {
                operation_id,
                task_id,
                provider_may_continue,
            } => Self::CanceledLocally {
                operation_id,
                task_id: task_id.map(P0CloudTaskId::from_cloud),
                provider_may_continue,
            },
            CloudLifecycle::AbandonedUnknown { operation_id } => {
                Self::AbandonedUnknown { operation_id }
            }
        }
    }

    /// Returns the exact lower operation identity.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn operation_id(&self) -> CloudSubmitOperationId {
        match self {
            Self::Submitting { operation_id }
            | Self::FailedBeforeSubmit { operation_id }
            | Self::OutcomeUnknown { operation_id }
            | Self::Pending { operation_id, .. }
            | Self::Ready { operation_id, .. }
            | Self::Applied { operation_id, .. }
            | Self::ProviderError { operation_id, .. }
            | Self::CanceledLocally { operation_id, .. }
            | Self::AbandonedUnknown { operation_id } => *operation_id,
        }
    }

    pub(crate) const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    pub(crate) const fn is_unknown(&self) -> bool {
        matches!(self, Self::OutcomeUnknown { .. })
    }

    pub(crate) const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::FailedBeforeSubmit { .. }
                | Self::Ready { .. }
                | Self::Applied { .. }
                | Self::ProviderError { .. }
                | Self::CanceledLocally { .. }
                | Self::AbandonedUnknown { .. }
        )
    }

    pub(crate) const fn task_id(&self) -> Option<&P0CloudTaskId> {
        match self {
            Self::Pending { task_id, .. }
            | Self::Ready { task_id, .. }
            | Self::Applied { task_id, .. }
            | Self::ProviderError { task_id, .. } => Some(task_id),
            Self::CanceledLocally { task_id, .. } => task_id.as_ref(),
            Self::Submitting { .. }
            | Self::FailedBeforeSubmit { .. }
            | Self::OutcomeUnknown { .. }
            | Self::AbandonedUnknown { .. } => None,
        }
    }
}

/// Current process-lifetime turn projection.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum P0TurnProjection {
    Queued,
    Starting {
        cancel_requested: bool,
    },
    Cloud {
        lifecycle: P0CloudLifecycle,
        cancel_requested: bool,
    },
    MonitoringDegraded {
        operation_id: CloudSubmitOperationId,
        last_known_pending: P0CloudLifecycle,
        cancel_requested: bool,
    },
    CanceledBeforeCloudStart,
    StoppedBeforeCloudStart,
    StoppedAfterLowerFailure,
}

/// Snapshot of the current or most recent local turn.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct P0TurnSnapshot {
    pub turn_id: TurnId,
    pub projection: P0TurnProjection,
}

/// Complete process-lifetime P0 session snapshot.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct P0SessionSnapshot {
    pub identity: P0SessionIdentity,
    pub state: P0SessionState,
    pub current_turn: Option<P0TurnSnapshot>,
    pub high_water_seq: EventSeq,
}

/// Receipt proving that one local turn intent was committed and queued.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct P0TurnReceipt {
    pub turn_id: TurnId,
    pub high_water_seq: EventSeq,
}

/// Safe explicit recovery decision kind recorded in the session event.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum P0RecoveryDecisionKind {
    Adopt,
    Abandon,
}

/// Safe reason why the local runtime stopped.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum P0RuntimeStopReason {
    Shutdown,
    LowerFailure,
}

/// Bounded provider candidates from one explicit unknown-submit reconciliation.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct P0RecoveryCandidates {
    pub operation_id: CloudSubmitOperationId,
    pub task_ids: Vec<P0CloudTaskId>,
    pub complete: bool,
}

impl P0RecoveryCandidates {
    pub(crate) fn from_summaries(
        operation_id: CloudSubmitOperationId,
        tasks: &[CloudTaskSummary],
        complete: bool,
    ) -> Self {
        Self {
            operation_id,
            task_ids: tasks
                .iter()
                .map(|task| P0CloudTaskId::from_cloud(task.id().clone()))
                .collect(),
            complete,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        operation_id: CloudSubmitOperationId,
        task_ids: Vec<CloudTaskId>,
        complete: bool,
    ) -> Self {
        Self {
            operation_id,
            task_ids: task_ids
                .into_iter()
                .map(P0CloudTaskId::from_cloud)
                .collect(),
            complete,
        }
    }
}

/// Version-1 process-lifetime P0 session event.
///
/// Contract: `CU-SES-P0-01`. Payloads are allowlisted and never contain prompts, diffs, codes,
/// credentials, raw provider text, configuration, or internal paths.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum P0SessionEvent {
    TurnAccepted,
    TurnCanceledBeforeCloudStart,
    LifecycleChanged {
        lifecycle: P0CloudLifecycle,
    },
    CancelRequested {
        actor: P0Actor,
    },
    MonitoringDegraded,
    RecoveryObserved {
        actor: P0Actor,
        operation_id: CloudSubmitOperationId,
        task_ids: Vec<P0CloudTaskId>,
        complete: bool,
    },
    RecoveryResolved {
        actor: P0Actor,
        operation_id: CloudSubmitOperationId,
        decision: P0RecoveryDecisionKind,
    },
    RuntimeStopped {
        reason: P0RuntimeStopReason,
    },
}

/// Ordered versioned envelope for one P0 session event.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct P0SessionEventEnvelope {
    pub schema_version: u16,
    pub session_id: SessionId,
    pub seq: EventSeq,
    pub turn_id: Option<TurnId>,
    pub payload: P0SessionEvent,
}
