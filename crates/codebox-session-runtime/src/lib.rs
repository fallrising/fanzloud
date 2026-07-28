//! Process-lifetime P0 session lifecycle.
//!
//! T005A owns one single-writer session projection, bounded ordered event history, rate-limited
//! monitoring, explicit-only cancellation/recovery, and subscriber isolation over the accepted
//! provider-specific Codex Cloud orchestrator.

mod config;
mod error;
mod runtime;
mod types;

pub use config::{P0SessionConfig, P0SessionConfigField};
pub use error::{P0LiveReceiveError, P0SessionError, P0SessionErrorCategory};
pub use runtime::{P0LiveReceiver, P0SessionRuntime, P0SessionSubscription};
pub use types::{
    P0Actor, P0CloudLifecycle, P0CloudTaskId, P0InstanceId, P0RecoveryCandidates,
    P0RecoveryDecisionKind, P0RuntimeStopReason, P0SessionEvent, P0SessionEventEnvelope,
    P0SessionIdentity, P0SessionSnapshot, P0SessionState, P0TurnProjection, P0TurnReceipt,
    P0TurnSnapshot,
};

#[cfg(test)]
mod tests;
