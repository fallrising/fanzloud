//! Validated domain values shared by Codebox boundaries.
//!
//! Cross-module identifiers intentionally use distinct Rust types. For example, a turn identifier
//! cannot be passed where a session identifier is required:
//!
//! ```compile_fail
//! use codebox_domain::{SessionId, TurnId};
//!
//! fn take_session(_: SessionId) {}
//!
//! take_session(TurnId::new());
//! ```

mod error;
mod id;
mod path;
mod sequence;

pub use error::{DomainError, EventSeqError, IdError, WorkspacePathError};
pub use id::{ApprovalId, ArtifactId, CommandId, SandboxId, SessionId, ToolCallId, TurnId};
pub use path::{MAX_WORKSPACE_PATH_BYTES, WorkspacePath};
pub use sequence::EventSeq;
