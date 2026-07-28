//! Trusted local boundary for the pinned Codex adapter.
//!
//! T002A owns credential-scope validation, fixed command policy, and an exclusive local lease.
//! T002B owns the bounded, crash-safe device-login lifecycle without exposing provider credentials
//! or arbitrary process output.
//! T003 owns side-effect-free, version-pinned Cloud values, fixed argv, and completed-capture
//! decoders. It exposes no process, credential, repository-execution, retry, or diff-application
//! authority.

mod broker;
mod cloud;
mod error;
mod invocation;
mod ledger;
mod login_error;
mod login_types;
mod parser;
mod runtime;
mod scope;

#[cfg(test)]
mod cloud_tests;
#[cfg(test)]
mod t002b_tests;

pub use broker::LoginBroker;
pub use cloud::{
    CloudAdapterError, CloudBranch, CloudCapture, CloudCursor, CloudDiff, CloudEnvironmentId,
    CloudErrorCategory, CloudField, CloudInvocation, CloudPrompt, CloudTaskId, CloudTaskListPage,
    CloudTaskStatus, CloudTaskSummary, CloudTaskUrl, decode_cloud_diff, decode_cloud_exec,
    decode_cloud_list, decode_cloud_status, decode_cloud_version,
};
pub use error::{
    CredentialDirectory, CredentialPath, CredentialScopeError, DirectoryViolation,
    ExecutableViolation, LeaseViolation,
};
pub use invocation::{CodexCommand, CodexInvocation};
pub use login_error::LoginBrokerError;
pub use login_types::{
    LoginInteraction, LoginOperationId, LoginStatus, VerificationCode, VerificationUrl,
};
pub use scope::{CredentialScope, CredentialScopeConfig, CredentialScopeLease};
