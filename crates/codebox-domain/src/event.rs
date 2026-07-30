use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApprovalId, EventSeq, SandboxId, SessionId, TurnId};

/// The only durable domain-event schema understood by T020.
///
/// Contract: `CU-PROTO-01`. A different value must be handled by a later explicit upcaster or
/// migration rather than interpreted as version 1.
pub const DOMAIN_EVENT_SCHEMA_V1: u16 = 1;

/// The durable decision recorded when one pending approval is resolved.
///
/// Contract: `CU-PROTO-01`. Tool and policy consequences remain outside the session reducer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

/// Stable names for the version-1 durable session-event variants.
///
/// Contract: `CU-PROTO-01`. This bounded classification is safe to include in reducer errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainEventKind {
    SessionCreated,
    SandboxProvisioned,
    ProvisioningFailed,
    TurnStarted,
    ApprovalRequested,
    ApprovalResolved,
    TurnCancellationRequested,
    TurnCancelled,
    TurnCompleted,
    TurnFailed,
    SessionIdled,
    SessionResumed,
    SessionArchivingStarted,
    SessionArchived,
}

/// One immutable semantic event in the version-1 session schema.
///
/// Contract: `CU-PROTO-01`. The enum contains state-transition identifiers only; prompts, model
/// text, provider output, tool output, credentials, paths, and web frames are separate types.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DomainEvent {
    SessionCreated,
    SandboxProvisioned {
        sandbox_id: SandboxId,
    },
    ProvisioningFailed,
    TurnStarted {
        turn_id: TurnId,
    },
    ApprovalRequested {
        turn_id: TurnId,
        approval_id: ApprovalId,
    },
    ApprovalResolved {
        turn_id: TurnId,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    },
    TurnCancellationRequested {
        turn_id: TurnId,
    },
    TurnCancelled {
        turn_id: TurnId,
    },
    TurnCompleted {
        turn_id: TurnId,
    },
    TurnFailed {
        turn_id: TurnId,
    },
    SessionIdled,
    SessionResumed,
    SessionArchivingStarted,
    SessionArchived,
}

impl DomainEvent {
    /// Returns the bounded variant classification without exposing event payload values.
    ///
    /// Contract: `CU-PROTO-01`. Classification is deterministic and has no side effect.
    pub const fn kind(&self) -> DomainEventKind {
        match self {
            Self::SessionCreated => DomainEventKind::SessionCreated,
            Self::SandboxProvisioned { .. } => DomainEventKind::SandboxProvisioned,
            Self::ProvisioningFailed => DomainEventKind::ProvisioningFailed,
            Self::TurnStarted { .. } => DomainEventKind::TurnStarted,
            Self::ApprovalRequested { .. } => DomainEventKind::ApprovalRequested,
            Self::ApprovalResolved { .. } => DomainEventKind::ApprovalResolved,
            Self::TurnCancellationRequested { .. } => DomainEventKind::TurnCancellationRequested,
            Self::TurnCancelled { .. } => DomainEventKind::TurnCancelled,
            Self::TurnCompleted { .. } => DomainEventKind::TurnCompleted,
            Self::TurnFailed { .. } => DomainEventKind::TurnFailed,
            Self::SessionIdled => DomainEventKind::SessionIdled,
            Self::SessionResumed => DomainEventKind::SessionResumed,
            Self::SessionArchivingStarted => DomainEventKind::SessionArchivingStarted,
            Self::SessionArchived => DomainEventKind::SessionArchived,
        }
    }
}

/// A semantic event before an event store assigns stream sequence and event identity.
///
/// Contract: `CU-PROTO-01`. T020 defines this value shape but performs no append or persistence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewDomainEvent {
    pub schema_version: u16,
    pub occurred_at: DateTime<Utc>,
    pub causation_id: Option<Uuid>,
    pub correlation_id: Uuid,
    pub payload: DomainEvent,
}

/// One fully identified immutable event from a session stream.
///
/// Contract: `CU-PROTO-01`. The reducer validates stream, sequence, schema, and transition before
/// returning a new projection; this value does not claim it was durably stored.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DomainEventEnvelope {
    pub event_id: Uuid,
    pub stream_id: SessionId,
    pub seq: EventSeq,
    pub schema_version: u16,
    pub occurred_at: DateTime<Utc>,
    pub causation_id: Option<Uuid>,
    pub correlation_id: Uuid,
    pub payload: DomainEvent,
}

/// Whether a runtime event category is eligible for durable event storage.
///
/// Contract: `CU-PROTO-03`. Classification does not itself store or discard an event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventPersistence {
    Durable,
    Ephemeral,
}

/// The runtime categories whose persistence policy is fixed by TD §4.4.
///
/// Contract: `CU-PROTO-03`. High-frequency delta categories deliberately carry no payload here;
/// their future transport types remain separate from the durable domain schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeEventKind {
    Domain(DomainEventKind),
    TextDelta,
    ReasoningSummaryDelta,
    TerminalDelta,
    ToolOutputDelta,
}

impl RuntimeEventKind {
    /// Classifies semantic domain events as durable and high-frequency deltas as ephemeral.
    ///
    /// Contract: `CU-PROTO-03`. The total pure mapping prevents a delta category from being
    /// mistaken for a durable event by callers that honor this contract.
    pub const fn persistence(self) -> EventPersistence {
        match self {
            Self::Domain(_) => EventPersistence::Durable,
            Self::TextDelta
            | Self::ReasoningSummaryDelta
            | Self::TerminalDelta
            | Self::ToolOutputDelta => EventPersistence::Ephemeral,
        }
    }
}
