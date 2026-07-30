use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::{
    ApprovalId, DOMAIN_EVENT_SCHEMA_V1, DomainEvent, DomainEventEnvelope, DomainEventKind,
    EventSeq, SandboxId, SessionId, TurnId,
};

/// The replayed lifecycle state of one P1 session.
///
/// Contract: `CU-PROTO-01`. These variants project exactly the TD §4.2 state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatus {
    Provisioning,
    Ready,
    Running,
    WaitingApproval,
    Cancelling,
    Idle,
    Failed,
    Archiving,
    Archived,
}

/// The deterministic session projection produced from durable version-1 events.
///
/// Contract: `CU-PROTO-01`. Fields are exposed through read-only projections so callers cannot
/// manufacture a state that bypasses reducer transition checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionProjection {
    session_id: SessionId,
    status: SessionStatus,
    active_turn: Option<TurnId>,
    pending_approval: Option<ApprovalId>,
    sandbox_id: Option<SandboxId>,
    last_activity: DateTime<Utc>,
    last_seq: EventSeq,
}

impl SessionProjection {
    /// Returns the session stream represented by this projection.
    ///
    /// Contract: `CU-PROTO-01`.
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Returns the current TD §4.2 lifecycle state.
    ///
    /// Contract: `CU-PROTO-01`.
    pub const fn status(&self) -> SessionStatus {
        self.status
    }

    /// Returns the one active turn, if the replayed state permits one.
    ///
    /// Contract: `CU-PROTO-01`.
    pub const fn active_turn(&self) -> Option<TurnId> {
        self.active_turn
    }

    /// Returns the one pending approval, if the session is waiting for it.
    ///
    /// Contract: `CU-PROTO-01`.
    pub const fn pending_approval(&self) -> Option<ApprovalId> {
        self.pending_approval
    }

    /// Returns the sandbox identity established during provisioning, if any.
    ///
    /// Contract: `CU-PROTO-01`. This does not prove that a runtime still exists.
    pub const fn sandbox_id(&self) -> Option<SandboxId> {
        self.sandbox_id
    }

    /// Returns the occurrence time attached to the highest accepted event sequence.
    ///
    /// Contract: `CU-PROTO-01`. Event sequence, not wall-clock order, is authoritative.
    pub const fn last_activity(&self) -> DateTime<Utc> {
        self.last_activity
    }

    /// Returns the highest contiguous event sequence included in this projection.
    ///
    /// Contract: `CU-PROTO-01`.
    pub const fn last_seq(&self) -> EventSeq {
        self.last_seq
    }
}

/// An immutable reducer for one session event stream.
///
/// Contract: `CU-PROTO-01`. `apply` returns a new value, so every checked failure is E0 and leaves
/// the source reducer unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionReducer {
    stream_id: SessionId,
    last_seq: EventSeq,
    projection: Option<SessionProjection>,
}

impl SessionReducer {
    /// Creates the empty pre-creation reducer for one validated session stream.
    ///
    /// Contract: `CU-PROTO-01`. The first accepted event must be `SessionCreated` at sequence one.
    pub const fn new(stream_id: SessionId) -> Self {
        Self {
            stream_id,
            last_seq: EventSeq::initial(),
            projection: None,
        }
    }

    /// Returns the stream identity this reducer accepts.
    ///
    /// Contract: `CU-PROTO-01`.
    pub const fn stream_id(&self) -> SessionId {
        self.stream_id
    }

    /// Returns the current contiguous high-water, including zero for an empty reducer.
    ///
    /// Contract: `CU-PROTO-01`.
    pub const fn last_seq(&self) -> EventSeq {
        self.last_seq
    }

    /// Returns the replayed projection, or `None` before `SessionCreated`.
    ///
    /// Contract: `CU-PROTO-01`.
    pub const fn projection(&self) -> Option<&SessionProjection> {
        self.projection.as_ref()
    }

    /// Applies one complete version-1 event envelope and returns a new reducer.
    ///
    /// Contract: `CU-PROTO-01`. Wrong stream, overflow, non-contiguous sequence, unsupported
    /// version, missing creation, illegal transition, and identity mismatch failures are typed and
    /// leave this reducer unchanged.
    pub fn apply(&self, event: &DomainEventEnvelope) -> Result<Self, SessionReducerError> {
        if event.stream_id != self.stream_id {
            return Err(SessionReducerError::WrongStream {
                expected: self.stream_id,
                actual: event.stream_id,
            });
        }

        let expected_seq =
            self.last_seq
                .checked_next()
                .map_err(|_| SessionReducerError::SequenceOverflow {
                    current: self.last_seq,
                })?;
        if event.seq != expected_seq {
            return Err(SessionReducerError::UnexpectedSequence {
                expected: expected_seq,
                actual: event.seq,
            });
        }
        if event.schema_version != DOMAIN_EVENT_SCHEMA_V1 {
            return Err(SessionReducerError::UnsupportedSchemaVersion {
                supported: DOMAIN_EVENT_SCHEMA_V1,
                actual: event.schema_version,
            });
        }

        let projection = match &self.projection {
            None => {
                if event.payload != DomainEvent::SessionCreated {
                    return Err(SessionReducerError::SessionNotCreated {
                        event: event.payload.kind(),
                    });
                }
                SessionProjection {
                    session_id: self.stream_id,
                    status: SessionStatus::Provisioning,
                    active_turn: None,
                    pending_approval: None,
                    sandbox_id: None,
                    last_activity: event.occurred_at,
                    last_seq: event.seq,
                }
            }
            Some(current) => {
                let mut next = current.clone();
                apply_transition(&mut next, &event.payload)?;
                next.last_activity = event.occurred_at;
                next.last_seq = event.seq;
                next
            }
        };

        Ok(Self {
            stream_id: self.stream_id,
            last_seq: event.seq,
            projection: Some(projection),
        })
    }
}

fn apply_transition(
    projection: &mut SessionProjection,
    event: &DomainEvent,
) -> Result<(), SessionReducerError> {
    let event_kind = event.kind();
    match event {
        DomainEvent::SessionCreated => invalid_transition(projection.status, event_kind),
        DomainEvent::SandboxProvisioned { sandbox_id } => {
            require_status(projection, SessionStatus::Provisioning, event_kind)?;
            projection.status = SessionStatus::Ready;
            projection.sandbox_id = Some(*sandbox_id);
            Ok(())
        }
        DomainEvent::ProvisioningFailed => {
            require_status(projection, SessionStatus::Provisioning, event_kind)?;
            projection.status = SessionStatus::Failed;
            Ok(())
        }
        DomainEvent::TurnStarted { turn_id } => {
            require_status(projection, SessionStatus::Ready, event_kind)?;
            projection.status = SessionStatus::Running;
            projection.active_turn = Some(*turn_id);
            Ok(())
        }
        DomainEvent::ApprovalRequested {
            turn_id,
            approval_id,
        } => {
            require_status(projection, SessionStatus::Running, event_kind)?;
            require_active_turn(projection, *turn_id)?;
            projection.status = SessionStatus::WaitingApproval;
            projection.pending_approval = Some(*approval_id);
            Ok(())
        }
        DomainEvent::ApprovalResolved {
            turn_id,
            approval_id,
            decision: _,
        } => {
            require_status(projection, SessionStatus::WaitingApproval, event_kind)?;
            require_active_turn(projection, *turn_id)?;
            require_pending_approval(projection, *approval_id)?;
            projection.status = SessionStatus::Running;
            projection.pending_approval = None;
            Ok(())
        }
        DomainEvent::TurnCancellationRequested { turn_id } => {
            require_status(projection, SessionStatus::Running, event_kind)?;
            require_active_turn(projection, *turn_id)?;
            projection.status = SessionStatus::Cancelling;
            Ok(())
        }
        DomainEvent::TurnCancelled { turn_id } => {
            require_status(projection, SessionStatus::Cancelling, event_kind)?;
            require_active_turn(projection, *turn_id)?;
            projection.status = SessionStatus::Ready;
            projection.active_turn = None;
            Ok(())
        }
        DomainEvent::TurnCompleted { turn_id } => {
            require_status(projection, SessionStatus::Running, event_kind)?;
            require_active_turn(projection, *turn_id)?;
            projection.status = SessionStatus::Ready;
            projection.active_turn = None;
            Ok(())
        }
        DomainEvent::TurnFailed { turn_id } => {
            require_status(projection, SessionStatus::Running, event_kind)?;
            require_active_turn(projection, *turn_id)?;
            projection.status = SessionStatus::Failed;
            projection.active_turn = None;
            Ok(())
        }
        DomainEvent::SessionIdled => {
            require_status(projection, SessionStatus::Ready, event_kind)?;
            projection.status = SessionStatus::Idle;
            Ok(())
        }
        DomainEvent::SessionResumed => {
            require_status(projection, SessionStatus::Idle, event_kind)?;
            projection.status = SessionStatus::Ready;
            Ok(())
        }
        DomainEvent::SessionArchivingStarted => {
            if !matches!(
                projection.status,
                SessionStatus::Ready | SessionStatus::Idle | SessionStatus::Failed
            ) {
                return invalid_transition(projection.status, event_kind);
            }
            projection.status = SessionStatus::Archiving;
            Ok(())
        }
        DomainEvent::SessionArchived => {
            require_status(projection, SessionStatus::Archiving, event_kind)?;
            projection.status = SessionStatus::Archived;
            Ok(())
        }
    }
}

fn require_status(
    projection: &SessionProjection,
    expected: SessionStatus,
    event: DomainEventKind,
) -> Result<(), SessionReducerError> {
    if projection.status == expected {
        Ok(())
    } else {
        invalid_transition(projection.status, event)
    }
}

fn invalid_transition<T>(
    state: SessionStatus,
    event: DomainEventKind,
) -> Result<T, SessionReducerError> {
    Err(SessionReducerError::InvalidTransition { state, event })
}

fn require_active_turn(
    projection: &SessionProjection,
    actual: TurnId,
) -> Result<(), SessionReducerError> {
    if projection.active_turn == Some(actual) {
        Ok(())
    } else {
        Err(SessionReducerError::ActiveTurnMismatch {
            expected: projection.active_turn,
            actual,
        })
    }
}

fn require_pending_approval(
    projection: &SessionProjection,
    actual: ApprovalId,
) -> Result<(), SessionReducerError> {
    if projection.pending_approval == Some(actual) {
        Ok(())
    } else {
        Err(SessionReducerError::PendingApprovalMismatch {
            expected: projection.pending_approval,
            actual,
        })
    }
}

/// A checked failure while replaying one session event.
///
/// Contract: `CU-PROTO-01`. Variants expose only bounded identifiers, state, event kind, sequence,
/// and version values; no arbitrary event content is included.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SessionReducerError {
    #[error("event belongs to a different session stream")]
    WrongStream {
        expected: SessionId,
        actual: SessionId,
    },
    #[error("event sequence cannot advance beyond its representable maximum")]
    SequenceOverflow { current: EventSeq },
    #[error("event sequence is not the next contiguous value")]
    UnexpectedSequence {
        expected: EventSeq,
        actual: EventSeq,
    },
    #[error("domain event schema version is unsupported")]
    UnsupportedSchemaVersion { supported: u16, actual: u16 },
    #[error("session stream does not begin with SessionCreated")]
    SessionNotCreated { event: DomainEventKind },
    #[error("domain event is invalid for the current session state")]
    InvalidTransition {
        state: SessionStatus,
        event: DomainEventKind,
    },
    #[error("turn-bound event does not name the active turn")]
    ActiveTurnMismatch {
        expected: Option<TurnId>,
        actual: TurnId,
    },
    #[error("approval resolution does not name the pending approval")]
    PendingApprovalMismatch {
        expected: Option<ApprovalId>,
        actual: ApprovalId,
    },
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn session_reducer_rejects_sequence_overflow() {
        let stream_id = SessionId::new();
        let reducer = SessionReducer {
            stream_id,
            last_seq: EventSeq::new(u64::MAX),
            projection: None,
        };
        let event = DomainEventEnvelope {
            event_id: Uuid::from_u128(1),
            stream_id,
            seq: EventSeq::new(u64::MAX),
            schema_version: DOMAIN_EVENT_SCHEMA_V1,
            occurred_at: Utc
                .timestamp_opt(1, 0)
                .single()
                .expect("valid fixed timestamp"),
            causation_id: None,
            correlation_id: Uuid::from_u128(2),
            payload: DomainEvent::SessionCreated,
        };

        assert_eq!(
            reducer.apply(&event),
            Err(SessionReducerError::SequenceOverflow {
                current: EventSeq::new(u64::MAX),
            }),
        );
        assert_eq!(reducer.last_seq(), EventSeq::new(u64::MAX));
        assert!(reducer.projection().is_none());
    }
}
