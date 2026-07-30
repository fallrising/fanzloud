use chrono::{TimeZone, Utc};
use codebox_domain::{
    ApprovalDecision, ApprovalId, DOMAIN_EVENT_SCHEMA_V1, DomainEvent, DomainEventEnvelope,
    DomainEventKind, EventPersistence, EventSeq, RuntimeEventKind, SandboxId, SessionId,
    SessionReducer, SessionReducerError, SessionStatus, TurnId,
};
use proptest::prelude::*;
use uuid::Uuid;

fn session_id(value: u128) -> SessionId {
    SessionId::try_from_uuid(Uuid::from_u128(value)).expect("non-nil session ID")
}

fn turn_id(value: u128) -> TurnId {
    TurnId::try_from_uuid(Uuid::from_u128(value)).expect("non-nil turn ID")
}

fn approval_id(value: u128) -> ApprovalId {
    ApprovalId::try_from_uuid(Uuid::from_u128(value)).expect("non-nil approval ID")
}

fn sandbox_id(value: u128) -> SandboxId {
    SandboxId::try_from_uuid(Uuid::from_u128(value)).expect("non-nil sandbox ID")
}

fn envelope(
    stream_id: SessionId,
    seq: u64,
    schema_version: u16,
    payload: DomainEvent,
) -> DomainEventEnvelope {
    DomainEventEnvelope {
        event_id: Uuid::from_u128(10_000 + u128::from(seq)),
        stream_id,
        seq: EventSeq::new(seq),
        schema_version,
        occurred_at: Utc
            .timestamp_opt((seq % 1_000_000) as i64, 0)
            .single()
            .expect("valid fixed timestamp"),
        causation_id: Some(Uuid::from_u128(20_000 + u128::from(seq))),
        correlation_id: Uuid::from_u128(30_000 + u128::from(seq)),
        payload,
    }
}

fn next(reducer: &SessionReducer, payload: DomainEvent) -> SessionReducer {
    let seq = reducer
        .last_seq()
        .checked_next()
        .expect("test sequence remains bounded");
    reducer
        .apply(&envelope(
            reducer.stream_id(),
            seq.value(),
            DOMAIN_EVENT_SCHEMA_V1,
            payload,
        ))
        .expect("legal test transition")
}

fn created(stream_id: SessionId) -> SessionReducer {
    next(&SessionReducer::new(stream_id), DomainEvent::SessionCreated)
}

fn ready(stream_id: SessionId, sandbox: SandboxId) -> SessionReducer {
    next(
        &created(stream_id),
        DomainEvent::SandboxProvisioned {
            sandbox_id: sandbox,
        },
    )
}

fn assert_status(reducer: &SessionReducer, expected: SessionStatus) {
    assert_eq!(
        reducer.projection().expect("created projection").status(),
        expected,
    );
}

#[test]
fn domain_event_envelope_v1_round_trips_without_field_drift() {
    let original = envelope(
        session_id(1),
        7,
        DOMAIN_EVENT_SCHEMA_V1,
        DomainEvent::ApprovalResolved {
            turn_id: turn_id(2),
            approval_id: approval_id(3),
            decision: ApprovalDecision::Denied,
        },
    );

    let encoded = serde_json::to_value(&original).expect("event serialization");
    assert_eq!(encoded["schema_version"], DOMAIN_EVENT_SCHEMA_V1);
    assert_eq!(encoded["seq"], 7);
    assert_eq!(encoded["payload"]["type"], "approval_resolved");
    assert_eq!(encoded["payload"]["data"]["decision"], "denied");

    let decoded: DomainEventEnvelope =
        serde_json::from_value(encoded).expect("event deserialization");
    assert_eq!(decoded, original);
}

#[test]
fn domain_event_v1_variant_schema_is_locked() {
    let turn = turn_id(1);
    let approval = approval_id(2);
    let sandbox = sandbox_id(3);
    let fixtures = [
        (
            DomainEvent::SessionCreated,
            serde_json::json!({"type": "session_created"}),
        ),
        (
            DomainEvent::SandboxProvisioned {
                sandbox_id: sandbox,
            },
            serde_json::json!({
                "type": "sandbox_provisioned",
                "data": {"sandbox_id": sandbox},
            }),
        ),
        (
            DomainEvent::ProvisioningFailed,
            serde_json::json!({"type": "provisioning_failed"}),
        ),
        (
            DomainEvent::TurnStarted { turn_id: turn },
            serde_json::json!({
                "type": "turn_started",
                "data": {"turn_id": turn},
            }),
        ),
        (
            DomainEvent::ApprovalRequested {
                turn_id: turn,
                approval_id: approval,
            },
            serde_json::json!({
                "type": "approval_requested",
                "data": {"turn_id": turn, "approval_id": approval},
            }),
        ),
        (
            DomainEvent::ApprovalResolved {
                turn_id: turn,
                approval_id: approval,
                decision: ApprovalDecision::Approved,
            },
            serde_json::json!({
                "type": "approval_resolved",
                "data": {
                    "turn_id": turn,
                    "approval_id": approval,
                    "decision": "approved",
                },
            }),
        ),
        (
            DomainEvent::TurnCancellationRequested { turn_id: turn },
            serde_json::json!({
                "type": "turn_cancellation_requested",
                "data": {"turn_id": turn},
            }),
        ),
        (
            DomainEvent::TurnCancelled { turn_id: turn },
            serde_json::json!({
                "type": "turn_cancelled",
                "data": {"turn_id": turn},
            }),
        ),
        (
            DomainEvent::TurnCompleted { turn_id: turn },
            serde_json::json!({
                "type": "turn_completed",
                "data": {"turn_id": turn},
            }),
        ),
        (
            DomainEvent::TurnFailed { turn_id: turn },
            serde_json::json!({
                "type": "turn_failed",
                "data": {"turn_id": turn},
            }),
        ),
        (
            DomainEvent::SessionIdled,
            serde_json::json!({"type": "session_idled"}),
        ),
        (
            DomainEvent::SessionResumed,
            serde_json::json!({"type": "session_resumed"}),
        ),
        (
            DomainEvent::SessionArchivingStarted,
            serde_json::json!({"type": "session_archiving_started"}),
        ),
        (
            DomainEvent::SessionArchived,
            serde_json::json!({"type": "session_archived"}),
        ),
    ];

    for (event, expected) in fixtures {
        assert_eq!(
            serde_json::to_value(&event).expect("event serialization"),
            expected,
        );
        let decoded: DomainEvent =
            serde_json::from_value(expected).expect("event fixture deserialization");
        assert_eq!(decoded, event);
    }
}

#[test]
fn domain_event_serde_rejects_unknown_fields_and_variants() {
    let valid = envelope(
        session_id(1),
        1,
        DOMAIN_EVENT_SCHEMA_V1,
        DomainEvent::SessionCreated,
    );
    let mut unknown_field = serde_json::to_value(&valid).expect("event serialization");
    unknown_field
        .as_object_mut()
        .expect("envelope object")
        .insert("future_field".to_owned(), serde_json::json!(true));
    assert!(serde_json::from_value::<DomainEventEnvelope>(unknown_field).is_err());

    let mut unknown_variant = serde_json::to_value(&valid).expect("event serialization");
    unknown_variant["payload"]["type"] = serde_json::json!("future_event");
    assert!(serde_json::from_value::<DomainEventEnvelope>(unknown_variant).is_err());
}

#[test]
fn session_reducer_applies_every_legal_transition() {
    let stream = session_id(1);
    let sandbox = sandbox_id(2);
    let turn = turn_id(3);
    let approval = approval_id(4);

    let provisioning = created(stream);
    assert_status(&provisioning, SessionStatus::Provisioning);
    let ready_state = next(
        &provisioning,
        DomainEvent::SandboxProvisioned {
            sandbox_id: sandbox,
        },
    );
    assert_status(&ready_state, SessionStatus::Ready);
    let running = next(&ready_state, DomainEvent::TurnStarted { turn_id: turn });
    assert_status(&running, SessionStatus::Running);
    let waiting = next(
        &running,
        DomainEvent::ApprovalRequested {
            turn_id: turn,
            approval_id: approval,
        },
    );
    assert_status(&waiting, SessionStatus::WaitingApproval);
    let running = next(
        &waiting,
        DomainEvent::ApprovalResolved {
            turn_id: turn,
            approval_id: approval,
            decision: ApprovalDecision::Approved,
        },
    );
    assert_status(&running, SessionStatus::Running);
    let cancelling = next(
        &running,
        DomainEvent::TurnCancellationRequested { turn_id: turn },
    );
    assert_status(&cancelling, SessionStatus::Cancelling);
    let ready_state = next(&cancelling, DomainEvent::TurnCancelled { turn_id: turn });
    assert_status(&ready_state, SessionStatus::Ready);
    let idle = next(&ready_state, DomainEvent::SessionIdled);
    assert_status(&idle, SessionStatus::Idle);
    let ready_state = next(&idle, DomainEvent::SessionResumed);
    assert_status(&ready_state, SessionStatus::Ready);
    let archiving = next(&ready_state, DomainEvent::SessionArchivingStarted);
    assert_status(&archiving, SessionStatus::Archiving);
    let archived = next(&archiving, DomainEvent::SessionArchived);
    assert_status(&archived, SessionStatus::Archived);

    let failed_provision = next(&created(session_id(10)), DomainEvent::ProvisioningFailed);
    assert_status(&failed_provision, SessionStatus::Failed);
    assert_status(
        &next(&failed_provision, DomainEvent::SessionArchivingStarted),
        SessionStatus::Archiving,
    );

    let completed_turn = next(
        &next(
            &ready(session_id(20), sandbox_id(21)),
            DomainEvent::TurnStarted {
                turn_id: turn_id(22),
            },
        ),
        DomainEvent::TurnCompleted {
            turn_id: turn_id(22),
        },
    );
    assert_status(&completed_turn, SessionStatus::Ready);

    let failed_turn = next(
        &next(
            &ready(session_id(30), sandbox_id(31)),
            DomainEvent::TurnStarted {
                turn_id: turn_id(32),
            },
        ),
        DomainEvent::TurnFailed {
            turn_id: turn_id(32),
        },
    );
    assert_status(&failed_turn, SessionStatus::Failed);

    let idle_archiving = next(
        &next(
            &ready(session_id(40), sandbox_id(41)),
            DomainEvent::SessionIdled,
        ),
        DomainEvent::SessionArchivingStarted,
    );
    assert_status(&idle_archiving, SessionStatus::Archiving);
}

#[test]
fn session_projection_tracks_status_turn_approval_sandbox_activity_and_high_water() {
    let stream = session_id(1);
    let sandbox = sandbox_id(2);
    let turn = turn_id(3);
    let approval = approval_id(4);
    let reducer = next(
        &next(
            &ready(stream, sandbox),
            DomainEvent::TurnStarted { turn_id: turn },
        ),
        DomainEvent::ApprovalRequested {
            turn_id: turn,
            approval_id: approval,
        },
    );
    let projection = reducer.projection().expect("created projection");

    assert_eq!(projection.session_id(), stream);
    assert_eq!(projection.status(), SessionStatus::WaitingApproval);
    assert_eq!(projection.active_turn(), Some(turn));
    assert_eq!(projection.pending_approval(), Some(approval));
    assert_eq!(projection.sandbox_id(), Some(sandbox));
    assert_eq!(projection.last_seq(), EventSeq::new(4));
    assert_eq!(
        projection.last_activity(),
        Utc.timestamp_opt(4, 0)
            .single()
            .expect("valid fixed timestamp"),
    );
}

#[test]
fn session_reducer_rejects_wrong_stream() {
    let reducer = SessionReducer::new(session_id(1));
    let event = envelope(
        session_id(2),
        1,
        DOMAIN_EVENT_SCHEMA_V1,
        DomainEvent::SessionCreated,
    );

    assert_eq!(
        reducer.apply(&event),
        Err(SessionReducerError::WrongStream {
            expected: session_id(1),
            actual: session_id(2),
        }),
    );
    assert!(reducer.projection().is_none());
    assert_eq!(reducer.last_seq(), EventSeq::initial());
}

#[test]
fn session_reducer_rejects_duplicate_gap_and_out_of_order_sequences() {
    let stream = session_id(1);
    let reducer = created(stream);
    let payload = DomainEvent::SandboxProvisioned {
        sandbox_id: sandbox_id(2),
    };

    for actual in [1, 3, 0] {
        assert_eq!(
            reducer.apply(&envelope(
                stream,
                actual,
                DOMAIN_EVENT_SCHEMA_V1,
                payload.clone(),
            )),
            Err(SessionReducerError::UnexpectedSequence {
                expected: EventSeq::new(2),
                actual: EventSeq::new(actual),
            }),
        );
    }
    assert_eq!(reducer.last_seq(), EventSeq::new(1));
    assert_status(&reducer, SessionStatus::Provisioning);
}

#[test]
fn session_reducer_rejects_unsupported_schema_version() {
    let stream = session_id(1);
    let reducer = SessionReducer::new(stream);

    assert_eq!(
        reducer.apply(&envelope(stream, 1, 2, DomainEvent::SessionCreated)),
        Err(SessionReducerError::UnsupportedSchemaVersion {
            supported: DOMAIN_EVENT_SCHEMA_V1,
            actual: 2,
        }),
    );
    assert!(reducer.projection().is_none());
}

#[test]
fn session_reducer_requires_creation_first() {
    let stream = session_id(1);
    let reducer = SessionReducer::new(stream);

    assert_eq!(
        reducer.apply(&envelope(
            stream,
            1,
            DOMAIN_EVENT_SCHEMA_V1,
            DomainEvent::SandboxProvisioned {
                sandbox_id: sandbox_id(2),
            },
        )),
        Err(SessionReducerError::SessionNotCreated {
            event: DomainEventKind::SandboxProvisioned,
        }),
    );
    assert!(reducer.projection().is_none());
}

fn assert_invalid_transition(reducer: &SessionReducer, payload: DomainEvent) {
    let state = reducer.projection().expect("created projection").status();
    let event_kind = payload.kind();
    let seq = reducer
        .last_seq()
        .checked_next()
        .expect("bounded test sequence");

    assert_eq!(
        reducer.apply(&envelope(
            reducer.stream_id(),
            seq.value(),
            DOMAIN_EVENT_SCHEMA_V1,
            payload,
        )),
        Err(SessionReducerError::InvalidTransition {
            state,
            event: event_kind,
        }),
    );
}

#[test]
fn session_reducer_rejects_every_illegal_transition() {
    let provisioning = created(session_id(1));
    assert_invalid_transition(&provisioning, DomainEvent::SessionArchived);
    assert_invalid_transition(&provisioning, DomainEvent::SessionCreated);

    let ready_state = ready(session_id(10), sandbox_id(11));
    assert_invalid_transition(&ready_state, DomainEvent::SessionResumed);
    assert_invalid_transition(
        &ready_state,
        DomainEvent::TurnCompleted {
            turn_id: turn_id(900),
        },
    );
    assert_invalid_transition(
        &ready_state,
        DomainEvent::ApprovalResolved {
            turn_id: turn_id(901),
            approval_id: approval_id(902),
            decision: ApprovalDecision::Denied,
        },
    );

    let running = next(
        &ready_state,
        DomainEvent::TurnStarted {
            turn_id: turn_id(12),
        },
    );
    assert_invalid_transition(&running, DomainEvent::SessionIdled);

    let waiting = next(
        &running,
        DomainEvent::ApprovalRequested {
            turn_id: turn_id(12),
            approval_id: approval_id(13),
        },
    );
    assert_invalid_transition(
        &waiting,
        DomainEvent::TurnCompleted {
            turn_id: turn_id(12),
        },
    );

    let running_after_approval = next(
        &waiting,
        DomainEvent::ApprovalResolved {
            turn_id: turn_id(12),
            approval_id: approval_id(13),
            decision: ApprovalDecision::Denied,
        },
    );
    let cancelling = next(
        &running_after_approval,
        DomainEvent::TurnCancellationRequested {
            turn_id: turn_id(12),
        },
    );
    assert_invalid_transition(
        &cancelling,
        DomainEvent::TurnStarted {
            turn_id: turn_id(14),
        },
    );

    let idle = next(&ready_state, DomainEvent::SessionIdled);
    assert_invalid_transition(
        &idle,
        DomainEvent::TurnStarted {
            turn_id: turn_id(15),
        },
    );

    let failed = next(&created(session_id(20)), DomainEvent::ProvisioningFailed);
    assert_invalid_transition(&failed, DomainEvent::SessionResumed);

    let archiving = next(&ready_state, DomainEvent::SessionArchivingStarted);
    assert_invalid_transition(&archiving, DomainEvent::SessionIdled);

    let archived = next(&archiving, DomainEvent::SessionArchived);
    assert_invalid_transition(&archived, DomainEvent::SessionResumed);
}

#[test]
fn session_reducer_rejects_wrong_turn_and_approval() {
    let stream = session_id(1);
    let active_turn = turn_id(2);
    let wrong_turn = turn_id(3);
    let pending = approval_id(4);
    let wrong_approval = approval_id(5);
    let running = next(
        &ready(stream, sandbox_id(6)),
        DomainEvent::TurnStarted {
            turn_id: active_turn,
        },
    );

    let seq = running
        .last_seq()
        .checked_next()
        .expect("bounded sequence")
        .value();
    assert_eq!(
        running.apply(&envelope(
            stream,
            seq,
            DOMAIN_EVENT_SCHEMA_V1,
            DomainEvent::ApprovalRequested {
                turn_id: wrong_turn,
                approval_id: pending,
            },
        )),
        Err(SessionReducerError::ActiveTurnMismatch {
            expected: Some(active_turn),
            actual: wrong_turn,
        }),
    );

    let waiting = next(
        &running,
        DomainEvent::ApprovalRequested {
            turn_id: active_turn,
            approval_id: pending,
        },
    );
    let seq = waiting
        .last_seq()
        .checked_next()
        .expect("bounded sequence")
        .value();
    assert_eq!(
        waiting.apply(&envelope(
            stream,
            seq,
            DOMAIN_EVENT_SCHEMA_V1,
            DomainEvent::ApprovalResolved {
                turn_id: active_turn,
                approval_id: wrong_approval,
                decision: ApprovalDecision::Approved,
            },
        )),
        Err(SessionReducerError::PendingApprovalMismatch {
            expected: Some(pending),
            actual: wrong_approval,
        }),
    );
    assert_eq!(
        waiting.apply(&envelope(
            stream,
            seq,
            DOMAIN_EVENT_SCHEMA_V1,
            DomainEvent::ApprovalResolved {
                turn_id: wrong_turn,
                approval_id: pending,
                decision: ApprovalDecision::Approved,
            },
        )),
        Err(SessionReducerError::ActiveTurnMismatch {
            expected: Some(active_turn),
            actual: wrong_turn,
        }),
    );
}

#[test]
fn session_reducer_errors_have_bounded_safe_debug() {
    let errors = [
        SessionReducerError::WrongStream {
            expected: session_id(1),
            actual: session_id(2),
        },
        SessionReducerError::SequenceOverflow {
            current: EventSeq::new(u64::MAX),
        },
        SessionReducerError::UnexpectedSequence {
            expected: EventSeq::new(2),
            actual: EventSeq::new(4),
        },
        SessionReducerError::UnsupportedSchemaVersion {
            supported: 1,
            actual: 2,
        },
        SessionReducerError::SessionNotCreated {
            event: DomainEventKind::TurnStarted,
        },
        SessionReducerError::InvalidTransition {
            state: SessionStatus::Archived,
            event: DomainEventKind::TurnStarted,
        },
        SessionReducerError::ActiveTurnMismatch {
            expected: Some(turn_id(3)),
            actual: turn_id(4),
        },
        SessionReducerError::PendingApprovalMismatch {
            expected: Some(approval_id(5)),
            actual: approval_id(6),
        },
    ];

    for error in errors {
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(display.len() < 160);
        assert!(debug.len() < 320);
        assert!(!display.contains("arbitrary-sensitive-content"));
        assert!(!debug.contains("arbitrary-sensitive-content"));
    }
}

#[test]
fn runtime_event_kind_classification_is_total() {
    assert_eq!(
        RuntimeEventKind::Domain(DomainEventKind::SessionCreated).persistence(),
        EventPersistence::Durable,
    );
    for kind in [
        RuntimeEventKind::TextDelta,
        RuntimeEventKind::ReasoningSummaryDelta,
        RuntimeEventKind::TerminalDelta,
        RuntimeEventKind::ToolOutputDelta,
    ] {
        assert_eq!(kind.persistence(), EventPersistence::Ephemeral);
    }
}

fn replay(stream: SessionId, events: &[DomainEvent]) -> SessionReducer {
    events
        .iter()
        .cloned()
        .fold(SessionReducer::new(stream), |reducer, payload| {
            next(&reducer, payload)
        })
}

proptest! {
    #[test]
    fn session_reducer_replay_is_deterministic(actions in prop::collection::vec(0_u8..3, 0..32)) {
        let stream = session_id(1);
        let mut events = vec![
            DomainEvent::SessionCreated,
            DomainEvent::SandboxProvisioned {
                sandbox_id: sandbox_id(2),
            },
        ];

        for (index, action) in actions.into_iter().enumerate() {
            let turn = turn_id(100 + index as u128);
            events.push(DomainEvent::TurnStarted { turn_id: turn });
            match action {
                0 => events.push(DomainEvent::TurnCompleted { turn_id: turn }),
                1 => {
                    let approval = approval_id(10_000 + index as u128);
                    events.push(DomainEvent::ApprovalRequested {
                        turn_id: turn,
                        approval_id: approval,
                    });
                    events.push(DomainEvent::ApprovalResolved {
                        turn_id: turn,
                        approval_id: approval,
                        decision: ApprovalDecision::Denied,
                    });
                    events.push(DomainEvent::TurnCompleted { turn_id: turn });
                }
                2 => {
                    events.push(DomainEvent::TurnCancellationRequested { turn_id: turn });
                    events.push(DomainEvent::TurnCancelled { turn_id: turn });
                }
                _ => unreachable!("generator range is exhaustive"),
            }
        }

        let first = replay(stream, &events);
        let second = replay(stream, &events);
        prop_assert_eq!(first, second);
    }

    #[test]
    fn session_reducer_checked_failures_are_e0(mode in 0_u8..5, noise in any::<u64>()) {
        let stream = session_id(1);
        let reducer = ready(stream, sandbox_id(2));
        let snapshot = reducer.clone();
        let expected = reducer
            .last_seq()
            .checked_next()
            .expect("bounded sequence")
            .value();
        let event = match mode {
            0 => envelope(
                session_id(9),
                expected,
                DOMAIN_EVENT_SCHEMA_V1,
                DomainEvent::SessionIdled,
            ),
            1 => {
                let actual = if noise == expected {
                    expected + 1
                } else {
                    noise
                };
                envelope(
                    stream,
                    actual,
                    DOMAIN_EVENT_SCHEMA_V1,
                    DomainEvent::SessionIdled,
                )
            }
            2 => envelope(stream, expected, 2, DomainEvent::SessionIdled),
            3 => envelope(
                stream,
                expected,
                DOMAIN_EVENT_SCHEMA_V1,
                DomainEvent::SessionArchived,
            ),
            4 => envelope(
                stream,
                expected,
                DOMAIN_EVENT_SCHEMA_V1,
                DomainEvent::TurnCompleted {
                    turn_id: turn_id(7),
                },
            ),
            _ => unreachable!("generator range is exhaustive"),
        };

        prop_assert!(reducer.apply(&event).is_err());
        prop_assert_eq!(reducer, snapshot);
    }
}

#[test]
fn regression_ephemeral_not_persisted() {
    let runtime_events = [
        RuntimeEventKind::TextDelta,
        RuntimeEventKind::Domain(DomainEventKind::TurnStarted),
        RuntimeEventKind::ReasoningSummaryDelta,
        RuntimeEventKind::TerminalDelta,
        RuntimeEventKind::ToolOutputDelta,
    ];
    let durable: Vec<_> = runtime_events
        .into_iter()
        .filter(|event| event.persistence() == EventPersistence::Durable)
        .collect();

    assert_eq!(
        durable,
        vec![RuntimeEventKind::Domain(DomainEventKind::TurnStarted)],
    );
}
