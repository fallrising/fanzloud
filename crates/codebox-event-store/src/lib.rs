//! SQLite persistence for versioned Codebox domain events.

mod codec;
mod error;
mod path;
mod sqlite;

pub use error::{
    CorruptStoreStage, DatabasePathErrorKind, EventStoreError, StorageErrorKind, StorageOperation,
};
pub use sqlite::{
    MAX_APPEND_EVENTS, MAX_EVENT_PAYLOAD_BYTES, SQLITE_BUSY_TIMEOUT, SqliteEventStore,
};
