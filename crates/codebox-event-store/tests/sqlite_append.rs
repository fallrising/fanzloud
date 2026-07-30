use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use codebox_domain::{
    ApprovalDecision, ApprovalId, DOMAIN_EVENT_SCHEMA_V1, DomainEvent, EventSeq, NewDomainEvent,
    SandboxId, SessionId, TurnId,
};
use codebox_event_store::{
    DatabasePathErrorKind, EventStoreError, MAX_APPEND_EVENTS, MAX_EVENT_PAYLOAD_BYTES,
    SQLITE_BUSY_TIMEOUT, SqliteEventStore,
};
use proptest::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use tempfile::{TempDir, tempdir};
use tokio::time::{sleep, timeout};
use uuid::Uuid;

fn private_root() -> TempDir {
    let root = tempdir().expect("private temporary root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private root permissions");
    root
}

fn database_path(root: &TempDir) -> PathBuf {
    root.path().join("events.sqlite")
}

fn event(seed: u64, payload: DomainEvent) -> NewDomainEvent {
    NewDomainEvent {
        schema_version: DOMAIN_EVENT_SCHEMA_V1,
        occurred_at: Utc
            .timestamp_opt((seed % 1_000_000) as i64, 0)
            .single()
            .expect("valid fixed timestamp"),
        causation_id: Some(Uuid::from_u128(u128::from(seed) + 10)),
        correlation_id: Uuid::from_u128(u128::from(seed) + 20),
        payload,
    }
}

fn row_count(path: &Path) -> u64 {
    let count: i64 = Connection::open(path)
        .expect("open test database")
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .expect("count event rows");
    u64::try_from(count).expect("non-negative row count")
}

fn raw_high_water(path: &Path, stream: SessionId) -> Option<EventSeq> {
    let connection = Connection::open(path).expect("open test database");
    let encoded: Option<Vec<u8>> = connection
        .query_row(
            "SELECT seq FROM events WHERE stream_id = ?1 ORDER BY seq DESC LIMIT 1",
            params![stream.as_uuid().as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .expect("query high-water");
    encoded.map(|bytes| {
        let fixed: [u8; 8] = bytes.try_into().expect("fixed-width sequence");
        EventSeq::new(u64::from_be_bytes(fixed))
    })
}

#[tokio::test]
async fn append_rejects_empty_batch_without_database_access() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    std::fs::remove_file(&path).expect("remove isolated test database");

    assert_eq!(
        store
            .append(SessionId::new(), EventSeq::initial(), Vec::new())
            .await,
        Err(EventStoreError::EmptyBatch)
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn new_stream_append_assigns_contiguous_unique_envelopes() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    let stream = SessionId::new();
    let inputs = vec![
        event(1, DomainEvent::SessionCreated),
        event(2, DomainEvent::ProvisioningFailed),
    ];

    let appended = store
        .append(stream, EventSeq::initial(), inputs.clone())
        .await
        .expect("append new stream");

    assert_eq!(appended.len(), 2);
    assert_eq!(appended[0].stream_id, stream);
    assert_eq!(appended[1].stream_id, stream);
    assert_eq!(appended[0].seq, EventSeq::new(1));
    assert_eq!(appended[1].seq, EventSeq::new(2));
    assert_ne!(appended[0].event_id, appended[1].event_id);
    assert!(appended.iter().all(|item| !item.event_id.is_nil()));
    for (envelope, input) in appended.iter().zip(inputs) {
        assert_eq!(envelope.schema_version, input.schema_version);
        assert_eq!(envelope.occurred_at, input.occurred_at);
        assert_eq!(envelope.causation_id, input.causation_id);
        assert_eq!(envelope.correlation_id, input.correlation_id);
        assert_eq!(envelope.payload, input.payload);
    }
    assert_eq!(raw_high_water(&path, stream), Some(EventSeq::new(2)));
}

#[tokio::test]
async fn append_rejects_oversized_batch_and_all_v1_payloads_fit_bound() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    let oversized = (0..=MAX_APPEND_EVENTS)
        .map(|index| event(index as u64 + 1, DomainEvent::SessionCreated))
        .collect::<Vec<_>>();

    assert_eq!(
        store
            .append(SessionId::new(), EventSeq::initial(), oversized)
            .await,
        Err(EventStoreError::BatchTooLarge {
            max: MAX_APPEND_EVENTS,
            actual: MAX_APPEND_EVENTS + 1,
        })
    );

    for payload in every_payload_variant() {
        assert!(
            serde_json::to_vec(&payload)
                .expect("serialize typed payload")
                .len()
                <= MAX_EVENT_PAYLOAD_BYTES
        );
    }
    assert_eq!(row_count(&path), 0);
}

#[tokio::test]
async fn append_rejects_unsupported_event_schema() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    let mut unsupported = event(1, DomainEvent::SessionCreated);
    unsupported.schema_version = DOMAIN_EVENT_SCHEMA_V1 + 1;

    assert_eq!(
        store
            .append(SessionId::new(), EventSeq::initial(), vec![unsupported],)
            .await,
        Err(EventStoreError::UnsupportedEventSchema {
            index: 0,
            supported: DOMAIN_EVENT_SCHEMA_V1,
            actual: DOMAIN_EVENT_SCHEMA_V1 + 1,
        })
    );
    assert_eq!(row_count(&path), 0);
}

#[tokio::test]
async fn append_rejects_sequence_overflow_without_database_change() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");

    assert_eq!(
        store
            .append(
                SessionId::new(),
                EventSeq::new(u64::MAX),
                vec![event(1, DomainEvent::SessionCreated)],
            )
            .await,
        Err(EventStoreError::SequenceOverflow {
            expected: EventSeq::new(u64::MAX),
            count: 1,
        })
    );
    assert_eq!(row_count(&path), 0);
}

#[tokio::test]
async fn sequence_conflict_returns_actual_without_change() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    let stream = SessionId::new();
    store
        .append(
            stream,
            EventSeq::initial(),
            vec![event(1, DomainEvent::SessionCreated)],
        )
        .await
        .expect("first append");

    assert_eq!(
        store
            .append(
                stream,
                EventSeq::initial(),
                vec![event(2, DomainEvent::ProvisioningFailed)],
            )
            .await,
        Err(EventStoreError::SequenceConflict {
            expected: EventSeq::initial(),
            actual: EventSeq::new(1),
        })
    );
    assert_eq!(row_count(&path), 1);
    assert_eq!(raw_high_water(&path, stream), Some(EventSeq::new(1)));
}

#[tokio::test]
async fn append_does_not_interpret_reducer_transitions() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path).await.expect("open store");

    let appended = store
        .append(
            SessionId::new(),
            EventSeq::initial(),
            vec![event(1, DomainEvent::ProvisioningFailed)],
        )
        .await
        .expect("storage does not own reducer semantics");
    assert_eq!(appended[0].payload, DomainEvent::ProvisioningFailed);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn append_model_keeps_sequences_contiguous_and_metadata_exact(
        batch_sizes in prop::collection::vec(1usize..8, 1..8)
    ) {
        let runtime = tokio::runtime::Runtime::new().expect("test runtime");
        runtime.block_on(async move {
            let root = private_root();
            let path = database_path(&root);
            let store = SqliteEventStore::open(path.clone()).await.expect("open store");
            let stream = SessionId::new();
            let mut expected = EventSeq::initial();
            let mut seed = 1u64;

            for size in batch_sizes {
                let inputs = (0..size)
                    .map(|_| {
                        let input = event(seed, DomainEvent::SessionCreated);
                        seed += 1;
                        input
                    })
                    .collect::<Vec<_>>();
                let appended = store
                    .append(stream, expected, inputs.clone())
                    .await
                    .expect("model append");
                for (offset, (envelope, input)) in appended.iter().zip(inputs).enumerate() {
                    prop_assert_eq!(
                        envelope.seq,
                        EventSeq::new(expected.value() + offset as u64 + 1)
                    );
                    prop_assert_eq!(&envelope.payload, &input.payload);
                    prop_assert_eq!(envelope.correlation_id, input.correlation_id);
                }
                expected = EventSeq::new(expected.value() + size as u64);
            }

            prop_assert_eq!(raw_high_water(&path, stream), Some(expected));
            Ok(())
        })?;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_writers_same_expected_sequence_exactly_one_succeeds() {
    let root = private_root();
    let path = database_path(&root);
    let store = Arc::new(
        SqliteEventStore::open(path.clone())
            .await
            .expect("open store"),
    );
    let stream = SessionId::new();
    let first = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .append(
                    stream,
                    EventSeq::initial(),
                    vec![event(1, DomainEvent::SessionCreated)],
                )
                .await
        })
    };
    let second = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .append(
                    stream,
                    EventSeq::initial(),
                    vec![event(2, DomainEvent::ProvisioningFailed)],
                )
                .await
        })
    };

    let results = [
        first.await.expect("first writer joined"),
        second.await.expect("second writer joined"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| {
                matches!(
                    result,
                    Err(EventStoreError::SequenceConflict {
                        expected,
                        actual
                    }) if *expected == EventSeq::initial() && *actual == EventSeq::new(1)
                )
            })
            .count(),
        1
    );
    assert_eq!(row_count(&path), 1);
}

#[tokio::test]
async fn committed_batch_persists_across_store_restart() {
    let root = private_root();
    let path = database_path(&root);
    let stream = SessionId::new();
    {
        let store = SqliteEventStore::open(path.clone())
            .await
            .expect("open store");
        store
            .append(
                stream,
                EventSeq::initial(),
                vec![
                    event(1, DomainEvent::SessionCreated),
                    event(2, DomainEvent::ProvisioningFailed),
                ],
            )
            .await
            .expect("append before restart");
    }

    let reopened = SqliteEventStore::open(path.clone())
        .await
        .expect("reopen store");
    reopened
        .append(
            stream,
            EventSeq::new(2),
            vec![event(3, DomainEvent::SessionArchivingStarted)],
        )
        .await
        .expect("append after restart");
    assert_eq!(row_count(&path), 3);
    assert_eq!(raw_high_water(&path, stream), Some(EventSeq::new(3)));
}

#[tokio::test]
async fn initialized_database_has_fixed_identity_and_private_mode() {
    let root = private_root();
    let path = database_path(&root);
    SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    let connection = Connection::open(&path).expect("open initialized database");
    let application_id: u32 = connection
        .query_row("PRAGMA application_id", [], |row| row.get(0))
        .expect("application ID");
    let user_version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("user version");
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal mode");

    assert_eq!(application_id, 0x4342_5831);
    assert_eq!(user_version, 1);
    assert_eq!(journal_mode, "wal");
    assert_eq!(
        std::fs::metadata(&path)
            .expect("database metadata")
            .permissions()
            .mode()
            & 0o077,
        0
    );
}

#[tokio::test]
async fn database_schema_drift_fails_closed() {
    let root = private_root();
    let path = database_path(&root);
    SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    Connection::open(&path)
        .expect("open schema-drift connection")
        .execute_batch(
            "
            DROP TABLE events;
            CREATE TABLE events (
                event_id BLOB,
                stream_id BLOB,
                seq BLOB,
                schema_version INTEGER,
                occurred_at TEXT,
                causation_id BLOB,
                correlation_id BLOB,
                payload BLOB
            ) STRICT;
            ",
        )
        .expect("replace schema without required constraints");

    assert_eq!(
        SqliteEventStore::open(path)
            .await
            .expect_err("schema drift rejected"),
        EventStoreError::CorruptStore {
            stage: codebox_event_store::CorruptStoreStage::Schema,
        }
    );
}

#[tokio::test]
async fn mid_transaction_failure_rolls_back_entire_batch() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    Connection::open(&path)
        .expect("open fault connection")
        .execute_batch(
            "
            CREATE TRIGGER test_abort_second
            BEFORE INSERT ON events
            WHEN (SELECT COUNT(*) FROM events) = 1
            BEGIN
                SELECT RAISE(ABORT, 'injected');
            END;
            ",
        )
        .expect("install fault trigger");

    assert!(matches!(
        store
            .append(
                SessionId::new(),
                EventSeq::initial(),
                vec![
                    event(1, DomainEvent::SessionCreated),
                    event(2, DomainEvent::ProvisioningFailed),
                ],
            )
            .await,
        Err(EventStoreError::Storage { .. })
    ));
    assert_eq!(row_count(&path), 0);
}

#[tokio::test]
async fn busy_timeout_leaves_stream_unchanged() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    let lock = Connection::open(&path).expect("open lock connection");
    lock.execute_batch("BEGIN IMMEDIATE")
        .expect("hold SQLite writer lock");

    let result = timeout(
        SQLITE_BUSY_TIMEOUT + Duration::from_secs(2),
        store.append(
            SessionId::new(),
            EventSeq::initial(),
            vec![event(1, DomainEvent::SessionCreated)],
        ),
    )
    .await
    .expect("busy timeout is bounded");
    assert_eq!(result, Err(EventStoreError::Busy));
    lock.execute_batch("ROLLBACK").expect("release writer lock");
    assert_eq!(row_count(&path), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_future_cancellation_requires_same_expected_reconciliation() {
    let root = private_root();
    let path = database_path(&root);
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    let stream = SessionId::new();
    let lock = Connection::open(&path).expect("open lock connection");
    lock.execute_batch("BEGIN IMMEDIATE")
        .expect("hold SQLite writer lock");

    let detached_store = store.clone();
    let append = tokio::spawn(async move {
        detached_store
            .append(
                stream,
                EventSeq::initial(),
                vec![event(1, DomainEvent::SessionCreated)],
            )
            .await
    });
    sleep(Duration::from_millis(100)).await;
    append.abort();
    assert!(
        append
            .await
            .expect_err("append future cancelled")
            .is_cancelled()
    );
    lock.execute_batch("ROLLBACK").expect("release writer lock");

    timeout(Duration::from_secs(3), async {
        loop {
            if raw_high_water(&path, stream) == Some(EventSeq::new(1)) {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("detached blocking transaction completed");

    assert_eq!(
        store
            .append(
                stream,
                EventSeq::initial(),
                vec![event(2, DomainEvent::ProvisioningFailed)],
            )
            .await,
        Err(EventStoreError::SequenceConflict {
            expected: EventSeq::initial(),
            actual: EventSeq::new(1),
        })
    );
}

#[tokio::test]
async fn database_path_rejects_relative_symlink_and_open_permissions() {
    assert_eq!(
        SqliteEventStore::open(PathBuf::from("relative.sqlite"))
            .await
            .expect_err("relative path rejected"),
        EventStoreError::InvalidDatabasePath {
            reason: DatabasePathErrorKind::Relative,
        }
    );

    let root = private_root();
    let target = root.path().join("target.sqlite");
    std::fs::write(&target, []).expect("create target");
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
        .expect("private target");
    let link = root.path().join("link.sqlite");
    symlink(&target, &link).expect("create test symlink");
    assert_eq!(
        SqliteEventStore::open(link)
            .await
            .expect_err("symlink rejected"),
        EventStoreError::InvalidDatabasePath {
            reason: DatabasePathErrorKind::TargetSymlink,
        }
    );

    let open = root.path().join("open.sqlite");
    std::fs::write(&open, []).expect("create open-permission file");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644))
        .expect("set open permissions");
    assert_eq!(
        SqliteEventStore::open(open)
            .await
            .expect_err("open permissions rejected"),
        EventStoreError::InvalidDatabasePath {
            reason: DatabasePathErrorKind::TargetOpenPermissions,
        }
    );

    let open_parent = private_root();
    std::fs::set_permissions(open_parent.path(), std::fs::Permissions::from_mode(0o755))
        .expect("set open parent permissions");
    assert_eq!(
        SqliteEventStore::open(database_path(&open_parent))
            .await
            .expect_err("open parent permissions rejected"),
        EventStoreError::InvalidDatabasePath {
            reason: DatabasePathErrorKind::ParentOpenPermissions,
        }
    );
}

#[tokio::test]
async fn store_debug_and_errors_do_not_expose_path_or_payload() {
    let root = private_root();
    let path = root.path().join("private-canary-events.sqlite");
    let store = SqliteEventStore::open(path.clone())
        .await
        .expect("open store");
    let debug = format!("{store:?}");
    assert!(!debug.contains(path.to_string_lossy().as_ref()));
    assert!(debug.contains("<redacted>"));

    let mut unsupported = event(1, DomainEvent::SessionCreated);
    unsupported.schema_version += 1;
    let error = store
        .append(SessionId::new(), EventSeq::initial(), vec![unsupported])
        .await
        .expect_err("unsupported version");
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(path.to_string_lossy().as_ref()));
    assert!(!rendered.contains("private-canary"));
    assert!(!rendered.contains("session_created"));
}

fn every_payload_variant() -> Vec<DomainEvent> {
    let turn_id = TurnId::new();
    let approval_id = ApprovalId::new();
    vec![
        DomainEvent::SessionCreated,
        DomainEvent::SandboxProvisioned {
            sandbox_id: SandboxId::new(),
        },
        DomainEvent::ProvisioningFailed,
        DomainEvent::TurnStarted { turn_id },
        DomainEvent::ApprovalRequested {
            turn_id,
            approval_id,
        },
        DomainEvent::ApprovalResolved {
            turn_id,
            approval_id,
            decision: ApprovalDecision::Denied,
        },
        DomainEvent::TurnCancellationRequested { turn_id },
        DomainEvent::TurnCancelled { turn_id },
        DomainEvent::TurnCompleted { turn_id },
        DomainEvent::TurnFailed { turn_id },
        DomainEvent::SessionIdled,
        DomainEvent::SessionResumed,
        DomainEvent::SessionArchivingStarted,
        DomainEvent::SessionArchived,
    ]
}
