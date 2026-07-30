use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use codebox_domain::{
    DOMAIN_EVENT_SCHEMA_V1, DomainEventEnvelope, EventSeq, NewDomainEvent, SessionId,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use tokio::task;
use uuid::Uuid;

use crate::codec::{
    EncodedNewEvent, decode_event, decode_sequence, encode_new, raw_event_from_row, sequence_bytes,
};
use crate::path::{cleanup_new_database, validate_and_prepare};
use crate::{CorruptStoreStage, EventStoreError, StorageErrorKind, StorageOperation};

/// Maximum semantic events accepted in one atomic append.
///
/// Contract: `CU-EVT-01`. The bound is checked before database access.
pub const MAX_APPEND_EVENTS: usize = 256;

/// Maximum serialized version-1 payload bytes accepted for one event.
///
/// Contract: `CU-EVT-01`. Large outputs belong in artifacts, not event rows.
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 65_536;

/// Maximum SQLite lock wait for one connection.
///
/// Contract: `CU-EVT-01`. Expiration returns `EventStoreError::Busy`.
pub const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const APPLICATION_ID: u32 = 0x4342_5831;
const DATABASE_SCHEMA_VERSION: u32 = 1;
const EVENTS_SCHEMA_SQL: &str = "CREATE TABLE events (
                    event_id BLOB NOT NULL PRIMARY KEY
                        CHECK (typeof(event_id) = 'blob' AND length(event_id) = 16),
                    stream_id BLOB NOT NULL
                        CHECK (typeof(stream_id) = 'blob' AND length(stream_id) = 16),
                    seq BLOB NOT NULL
                        CHECK (typeof(seq) = 'blob' AND length(seq) = 8),
                    schema_version INTEGER NOT NULL
                        CHECK (schema_version BETWEEN 0 AND 65535),
                    occurred_at TEXT NOT NULL,
                    causation_id BLOB
                        CHECK (causation_id IS NULL OR
                               (typeof(causation_id) = 'blob' AND length(causation_id) = 16)),
                    correlation_id BLOB NOT NULL
                        CHECK (typeof(correlation_id) = 'blob' AND length(correlation_id) = 16),
                    payload BLOB NOT NULL,
                    UNIQUE (stream_id, seq)
                ) STRICT, WITHOUT ROWID";

type EventIdSource = Arc<dyn Fn() -> Uuid + Send + Sync>;

/// The private P1 SQLite event-store adapter.
///
/// Contract: `CU-EVT-01`. T030A exposes only atomic append; replay and snapshots are added by
/// T030B–T030D. Debug output never reveals the administrator database path.
#[derive(Clone)]
pub struct SqliteEventStore {
    database_path: Arc<PathBuf>,
    event_id_source: EventIdSource,
}

impl fmt::Debug for SqliteEventStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteEventStore")
            .field("database_path", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl SqliteEventStore {
    /// Opens or initializes one private version-1 SQLite event database.
    ///
    /// Contract: `CU-EVT-01`. The path is validated as an administrator-owned private local file;
    /// incompatible or corrupt schema state fails closed with a typed redacted error.
    pub async fn open(database_path: PathBuf) -> Result<Self, EventStoreError> {
        Self::open_with_source(database_path, Arc::new(Uuid::new_v4)).await
    }

    async fn open_with_source(
        database_path: PathBuf,
        event_id_source: EventIdSource,
    ) -> Result<Self, EventStoreError> {
        let validated = task::spawn_blocking(move || {
            let validated = validate_and_prepare(database_path)?;
            match initialize_database(&validated.path) {
                Ok(()) => Ok(validated.path),
                Err(error) => {
                    if validated.created {
                        cleanup_new_database(&validated.path);
                    }
                    Err(error)
                }
            }
        })
        .await
        .map_err(|_| EventStoreError::WorkerUnavailable)??;

        Ok(Self {
            database_path: Arc::new(validated),
            event_id_source,
        })
    }

    /// Atomically appends one non-empty event batch at the exact expected stream sequence.
    ///
    /// Contract: `CU-EVT-01`. Success assigns globally unique event IDs and contiguous sequences;
    /// conflict returns the actual committed high-water and every failure leaves no partial batch.
    pub async fn append(
        &self,
        stream: SessionId,
        expected_seq: EventSeq,
        events: Vec<NewDomainEvent>,
    ) -> Result<Vec<DomainEventEnvelope>, EventStoreError> {
        let prepared = prepare_batch(stream, expected_seq, events, self.event_id_source.as_ref())?;
        let database_path = Arc::clone(&self.database_path);

        task::spawn_blocking(move || {
            append_prepared(&database_path, stream, expected_seq, prepared)
        })
        .await
        .map_err(|_| EventStoreError::WorkerUnavailable)?
    }
}

struct PreparedEvent {
    event_id: Uuid,
    stream_id: SessionId,
    seq: EventSeq,
    encoded: EncodedNewEvent,
}

fn prepare_batch(
    stream: SessionId,
    expected_seq: EventSeq,
    events: Vec<NewDomainEvent>,
    event_id_source: &(dyn Fn() -> Uuid + Send + Sync),
) -> Result<Vec<PreparedEvent>, EventStoreError> {
    if events.is_empty() {
        return Err(EventStoreError::EmptyBatch);
    }
    if events.len() > MAX_APPEND_EVENTS {
        return Err(EventStoreError::BatchTooLarge {
            max: MAX_APPEND_EVENTS,
            actual: events.len(),
        });
    }

    let count = events.len();
    let mut next = expected_seq;
    let mut event_ids = HashSet::with_capacity(count);
    let mut prepared = Vec::with_capacity(count);
    for (index, event) in events.into_iter().enumerate() {
        if event.schema_version != DOMAIN_EVENT_SCHEMA_V1 {
            return Err(EventStoreError::UnsupportedEventSchema {
                index,
                supported: DOMAIN_EVENT_SCHEMA_V1,
                actual: event.schema_version,
            });
        }
        let encoded = encode_new(&event, index)?;
        next = next
            .checked_next()
            .map_err(|_| EventStoreError::SequenceOverflow {
                expected: expected_seq,
                count,
            })?;
        let event_id = event_id_source();
        if event_id.is_nil() || !event_ids.insert(event_id) {
            return Err(EventStoreError::DuplicateEventId);
        }
        prepared.push(PreparedEvent {
            event_id,
            stream_id: stream,
            seq: next,
            encoded,
        });
    }
    Ok(prepared)
}

fn append_prepared(
    database_path: &Path,
    stream: SessionId,
    expected_seq: EventSeq,
    prepared: Vec<PreparedEvent>,
) -> Result<Vec<DomainEventEnvelope>, EventStoreError> {
    let mut connection = open_connection(database_path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| map_sqlite(error, StorageOperation::Begin))?;
    let actual = read_high_water(&transaction, stream)?;
    if actual != expected_seq {
        return Err(EventStoreError::SequenceConflict {
            expected: expected_seq,
            actual,
        });
    }

    for event in &prepared {
        if event_id_exists(&transaction, event.event_id)? {
            return Err(EventStoreError::DuplicateEventId);
        }
    }

    for event in &prepared {
        insert_event(&transaction, event)?;
    }

    let mut envelopes = Vec::with_capacity(prepared.len());
    for event in &prepared {
        envelopes.push(read_event_by_id(&transaction, event.event_id)?);
    }

    transaction
        .commit()
        .map_err(|error| map_sqlite(error, StorageOperation::Commit))?;
    Ok(envelopes)
}

fn initialize_database(path: &Path) -> Result<(), EventStoreError> {
    let connection = open_connection(path)?;
    let application_id = pragma_u32(&connection, "application_id")?;
    let user_version = pragma_u32(&connection, "user_version")?;
    let object_count: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| map_sqlite(error, StorageOperation::Initialize))?;

    if application_id == 0 && user_version == 0 && object_count == 0 {
        connection
            .execute_batch(
                "
                BEGIN IMMEDIATE;
                CREATE TABLE events (
                    event_id BLOB NOT NULL PRIMARY KEY
                        CHECK (typeof(event_id) = 'blob' AND length(event_id) = 16),
                    stream_id BLOB NOT NULL
                        CHECK (typeof(stream_id) = 'blob' AND length(stream_id) = 16),
                    seq BLOB NOT NULL
                        CHECK (typeof(seq) = 'blob' AND length(seq) = 8),
                    schema_version INTEGER NOT NULL
                        CHECK (schema_version BETWEEN 0 AND 65535),
                    occurred_at TEXT NOT NULL,
                    causation_id BLOB
                        CHECK (causation_id IS NULL OR
                               (typeof(causation_id) = 'blob' AND length(causation_id) = 16)),
                    correlation_id BLOB NOT NULL
                        CHECK (typeof(correlation_id) = 'blob' AND length(correlation_id) = 16),
                    payload BLOB NOT NULL,
                    UNIQUE (stream_id, seq)
                ) STRICT, WITHOUT ROWID;
                PRAGMA application_id = 1128421425;
                PRAGMA user_version = 1;
                COMMIT;
                ",
            )
            .map_err(|error| map_sqlite(error, StorageOperation::Initialize))?;
    } else if application_id != APPLICATION_ID || user_version != DATABASE_SCHEMA_VERSION {
        return Err(EventStoreError::UnsupportedDatabaseSchema {
            expected_application_id: APPLICATION_ID,
            actual_application_id: application_id,
            expected_user_version: DATABASE_SCHEMA_VERSION,
            actual_user_version: user_version,
        });
    }

    validate_schema(&connection)
}

fn open_connection(path: &Path) -> Result<Connection, EventStoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| map_sqlite(error, StorageOperation::Open))?;
    connection
        .busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(|error| map_sqlite(error, StorageOperation::Configure))?;
    connection
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")
        .map_err(|error| map_sqlite(error, StorageOperation::Configure))?;
    Ok(connection)
}

fn validate_schema(connection: &Connection) -> Result<(), EventStoreError> {
    let integrity: String = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(|error| map_sqlite(error, StorageOperation::Initialize))?;
    if integrity != "ok" {
        return Err(EventStoreError::CorruptStore {
            stage: CorruptStoreStage::Schema,
        });
    }
    let schema_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'events'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| EventStoreError::CorruptStore {
            stage: CorruptStoreStage::Schema,
        })?;
    if schema_sql != EVENTS_SCHEMA_SQL {
        return Err(EventStoreError::CorruptStore {
            stage: CorruptStoreStage::Schema,
        });
    }
    Ok(())
}

fn pragma_u32(connection: &Connection, name: &str) -> Result<u32, EventStoreError> {
    let query = match name {
        "application_id" => "PRAGMA application_id",
        "user_version" => "PRAGMA user_version",
        _ => {
            return Err(EventStoreError::CorruptStore {
                stage: CorruptStoreStage::Schema,
            });
        }
    };
    let value: i64 = connection
        .query_row(query, [], |row| row.get(0))
        .map_err(|error| map_sqlite(error, StorageOperation::Initialize))?;
    u32::try_from(value).map_err(|_| EventStoreError::CorruptStore {
        stage: CorruptStoreStage::Schema,
    })
}

fn read_high_water(
    transaction: &Transaction<'_>,
    stream: SessionId,
) -> Result<EventSeq, EventStoreError> {
    let encoded: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT seq FROM events WHERE stream_id = ?1 ORDER BY seq DESC LIMIT 1",
            params![stream.as_uuid().as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| map_sqlite(error, StorageOperation::ReadHighWater))?;
    encoded
        .as_deref()
        .map(decode_sequence)
        .transpose()
        .map(|value| value.unwrap_or_else(EventSeq::initial))
}

fn event_id_exists(transaction: &Transaction<'_>, event_id: Uuid) -> Result<bool, EventStoreError> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE event_id = ?1)",
            params![event_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(|error| map_sqlite(error, StorageOperation::CheckEventId))
}

fn insert_event(
    transaction: &Transaction<'_>,
    event: &PreparedEvent,
) -> Result<(), EventStoreError> {
    transaction
        .execute(
            "INSERT INTO events (
                event_id, stream_id, seq, schema_version, occurred_at, causation_id,
                correlation_id, payload
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.event_id.as_bytes().as_slice(),
                event.stream_id.as_uuid().as_bytes().as_slice(),
                sequence_bytes(event.seq).as_slice(),
                i64::from(event.encoded.schema_version),
                event.encoded.occurred_at,
                event
                    .encoded
                    .causation_id
                    .as_ref()
                    .map(|value| value.as_slice()),
                event.encoded.correlation_id.as_slice(),
                event.encoded.payload,
            ],
        )
        .map(|_| ())
        .map_err(|error| map_sqlite(error, StorageOperation::Insert))
}

fn read_event_by_id(
    transaction: &Transaction<'_>,
    event_id: Uuid,
) -> Result<DomainEventEnvelope, EventStoreError> {
    let raw = transaction
        .query_row(
            "SELECT event_id, stream_id, seq, schema_version, occurred_at, causation_id,
                    correlation_id, payload
             FROM events
             WHERE event_id = ?1",
            params![event_id.as_bytes().as_slice()],
            raw_event_from_row,
        )
        .map_err(|error| map_sqlite(error, StorageOperation::VerifyInserted))?;
    decode_event(raw)
}

fn map_sqlite(error: rusqlite::Error, operation: StorageOperation) -> EventStoreError {
    if let rusqlite::Error::SqliteFailure(failure, _) = &error {
        match failure.extended_code & 0xff {
            5 | 6 => return EventStoreError::Busy,
            11 | 26 => {
                return EventStoreError::CorruptStore {
                    stage: CorruptStoreStage::Schema,
                };
            }
            _ => {}
        }
    }

    let kind = match error {
        rusqlite::Error::SqliteFailure(failure, _) => match failure.extended_code & 0xff {
            8 => StorageErrorKind::ReadOnly,
            10 | 14 => StorageErrorKind::Io,
            13 => StorageErrorKind::Full,
            19 => StorageErrorKind::Constraint,
            _ => StorageErrorKind::Other,
        },
        _ => StorageErrorKind::Other,
    };
    EventStoreError::Storage { operation, kind }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::{TimeZone, Utc};
    use codebox_domain::{DomainEvent, NewDomainEvent};
    use tempfile::tempdir;

    use super::*;
    use crate::codec::{decode_sequence, sequence_bytes};

    fn event(seed: u128) -> NewDomainEvent {
        NewDomainEvent {
            schema_version: DOMAIN_EVENT_SCHEMA_V1,
            occurred_at: Utc
                .timestamp_opt(seed as i64, 0)
                .single()
                .expect("valid fixed timestamp"),
            causation_id: Some(Uuid::from_u128(seed + 10)),
            correlation_id: Uuid::from_u128(seed + 20),
            payload: DomainEvent::SessionCreated,
        }
    }

    #[test]
    fn sqlite_sequence_codec_preserves_full_u64_order() {
        let values = [0, 1, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX];
        let encoded: Vec<_> = values
            .iter()
            .copied()
            .map(EventSeq::new)
            .map(sequence_bytes)
            .collect();
        assert!(encoded.windows(2).all(|pair| pair[0] < pair[1]));
        for (value, bytes) in values.into_iter().zip(encoded) {
            assert_eq!(
                decode_sequence(&bytes).expect("fixed-width sequence"),
                EventSeq::new(value)
            );
        }
    }

    #[tokio::test]
    async fn sqlite_row_codec_preserves_every_envelope_field() {
        let root = tempdir().expect("private temp root");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private root mode");
        let store = SqliteEventStore::open(root.path().join("events.sqlite"))
            .await
            .expect("open store");
        let stream = SessionId::new();
        let input = event(1);
        let appended = store
            .append(stream, EventSeq::initial(), vec![input.clone()])
            .await
            .expect("append");
        assert_eq!(appended.len(), 1);
        let envelope = &appended[0];
        assert_eq!(envelope.stream_id, stream);
        assert_eq!(envelope.seq, EventSeq::new(1));
        assert_eq!(envelope.schema_version, input.schema_version);
        assert_eq!(envelope.occurred_at, input.occurred_at);
        assert_eq!(envelope.causation_id, input.causation_id);
        assert_eq!(envelope.correlation_id, input.correlation_id);
        assert_eq!(envelope.payload, input.payload);
        assert!(!envelope.event_id.is_nil());
    }

    #[tokio::test]
    async fn duplicate_event_id_rolls_back_entire_batch() {
        let root = tempdir().expect("private temp root");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private root mode");
        let event_ids = [
            Uuid::from_u128(500),
            Uuid::from_u128(501),
            Uuid::from_u128(500),
        ];
        let index = Arc::new(AtomicUsize::new(0));
        let source_index = Arc::clone(&index);
        let store = SqliteEventStore::open_with_source(
            root.path().join("events.sqlite"),
            Arc::new(move || event_ids[source_index.fetch_add(1, Ordering::SeqCst)]),
        )
        .await
        .expect("open store");
        let stream = SessionId::new();
        store
            .append(stream, EventSeq::initial(), vec![event(1)])
            .await
            .expect("seed existing event ID");
        let error = store
            .append(stream, EventSeq::new(1), vec![event(2), event(3)])
            .await
            .expect_err("duplicate source must fail");
        assert_eq!(error, EventStoreError::DuplicateEventId);
        let connection =
            Connection::open(root.path().join("events.sqlite")).expect("open test database");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .expect("count rows after duplicate");
        assert_eq!(count, 1);
    }

    #[test]
    fn event_store_errors_have_bounded_safe_debug() {
        let path_canary = "/private/operator/events.sqlite";
        let payload_canary = "secret-event-payload";
        let errors = [
            EventStoreError::EmptyBatch,
            EventStoreError::SequenceConflict {
                expected: EventSeq::new(1),
                actual: EventSeq::new(2),
            },
            EventStoreError::InvalidDatabasePath {
                reason: crate::DatabasePathErrorKind::TargetSymlink,
            },
            EventStoreError::Storage {
                operation: StorageOperation::Commit,
                kind: StorageErrorKind::Io,
            },
        ];
        for error in errors {
            let debug = format!("{error:?}");
            assert!(debug.len() < 256);
            assert!(!debug.contains(path_canary));
            assert!(!debug.contains(payload_canary));
        }
    }
}
