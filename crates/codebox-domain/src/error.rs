use thiserror::Error;

/// A UUID failed validation at a domain-value boundary.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum IdError {
    /// The nil UUID cannot identify a domain entity.
    #[error("{id_type} cannot be nil")]
    Nil { id_type: &'static str },
}

/// A workspace-relative path failed portable boundary validation.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum WorkspacePathError {
    /// The input had no usable path component.
    #[error("workspace path is empty")]
    Empty,
    /// The input began at a filesystem root.
    #[error("workspace path must be relative")]
    Absolute,
    /// The input contained a parent traversal component.
    #[error("workspace path contains a parent traversal component")]
    ParentTraversal,
    /// The input contained a NUL byte.
    #[error("workspace path contains a NUL byte")]
    ContainsNul,
    /// Backslash is not a portable path separator at this boundary.
    #[error("workspace path cannot contain a backslash separator")]
    BackslashNotAllowed,
    /// The input looked like a Windows drive-prefixed path.
    #[error("workspace path cannot contain a drive prefix")]
    DrivePrefix,
    /// The normalized or input path exceeded the portable bound.
    #[error("workspace path exceeds {max_bytes} bytes (received {actual_bytes})")]
    TooLong {
        max_bytes: usize,
        actual_bytes: usize,
    },
}

/// Failure while advancing a domain event sequence.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum EventSeqError {
    /// The sequence is already at its representable maximum.
    #[error("event sequence cannot advance beyond u64::MAX")]
    Overflow,
}

/// The small base taxonomy for errors produced by this value-only domain crate.
///
/// Storage, network, provider, and sandbox boundaries must define their own richer error enums;
/// this type is intentionally limited to value construction and validation.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DomainError {
    /// A UUID failed validation.
    #[error("invalid identifier: {0}")]
    InvalidId(#[from] IdError),
    /// A workspace-relative path failed validation.
    #[error("invalid workspace path: {0}")]
    InvalidWorkspacePath(#[from] WorkspacePathError),
    /// An event sequence cannot be advanced further.
    #[error("invalid event sequence: {0}")]
    InvalidEventSequence(#[from] EventSeqError),
}
