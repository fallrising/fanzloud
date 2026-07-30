use codebox_domain::EventSeq;
use thiserror::Error;

/// A bounded reason that an administrator SQLite path failed validation.
///
/// Contract: `CU-EVT-01`. Variants deliberately omit the supplied filesystem path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabasePathErrorKind {
    Relative,
    MissingFileName,
    ParentUnavailable,
    ParentNotDirectory,
    ParentNotCanonical,
    ParentWrongOwner,
    ParentOpenPermissions,
    TargetSymlink,
    TargetNotRegular,
    TargetWrongOwner,
    TargetOpenPermissions,
    CreateFailed,
}

/// The bounded event-store operation associated with a storage failure.
///
/// Contract: `CU-EVT-01`. SQL text and database paths are never represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageOperation {
    Open,
    Configure,
    Initialize,
    Begin,
    ReadHighWater,
    CheckEventId,
    Insert,
    VerifyInserted,
    Commit,
}

/// A safe classification of a SQLite/storage failure.
///
/// Contract: `CU-EVT-01`. Raw SQLite messages are intentionally not caller-visible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageErrorKind {
    ReadOnly,
    Full,
    Io,
    Constraint,
    Other,
}

/// The bounded stage at which persisted store structure was inconsistent.
///
/// Contract: `CU-EVT-01`. It carries no malformed bytes or SQL diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorruptStoreStage {
    Schema,
    Sequence,
    EventId,
    StreamId,
    SchemaVersion,
    Timestamp,
    CausationId,
    CorrelationId,
    Payload,
}

/// A checked failure from the T030A SQLite append boundary.
///
/// Contract: `CU-EVT-01`. Every variant gives a caller action without exposing paths, SQL,
/// payloads, or SQLite diagnostic text.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum EventStoreError {
    #[error("event append batch cannot be empty")]
    EmptyBatch,
    #[error("event append batch exceeds its configured count limit")]
    BatchTooLarge { max: usize, actual: usize },
    #[error("event payload exceeds its configured byte limit")]
    EventPayloadTooLarge {
        index: usize,
        max: usize,
        actual: usize,
    },
    #[error("event schema version is unsupported by this store")]
    UnsupportedEventSchema {
        index: usize,
        supported: u16,
        actual: u16,
    },
    #[error("event sequence range cannot be represented")]
    SequenceOverflow { expected: EventSeq, count: usize },
    #[error("event stream sequence does not match the caller expectation")]
    SequenceConflict {
        expected: EventSeq,
        actual: EventSeq,
    },
    #[error("generated event identity is nil or already exists")]
    DuplicateEventId,
    #[error("SQLite writer remained busy beyond the bounded wait")]
    Busy,
    #[error("administrator database path failed validation")]
    InvalidDatabasePath { reason: DatabasePathErrorKind },
    #[error("database application or schema version is unsupported")]
    UnsupportedDatabaseSchema {
        expected_application_id: u32,
        actual_application_id: u32,
        expected_user_version: u32,
        actual_user_version: u32,
    },
    #[error("persisted event-store structure is corrupt or inconsistent")]
    CorruptStore { stage: CorruptStoreStage },
    #[error("SQLite storage operation failed")]
    Storage {
        operation: StorageOperation,
        kind: StorageErrorKind,
    },
    #[error("blocking SQLite worker could not be joined")]
    WorkerUnavailable,
}
