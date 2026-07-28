use std::io;

use thiserror::Error;

use crate::CredentialScopeError;

/// A pinned Codex login operation failed at a typed caller-action boundary.
///
/// Contract: `CU-AUTH-P0-01`. No variant contains raw provider output, credential bytes, a
/// verification code, or a configured filesystem path.
#[derive(Debug, Error)]
pub enum LoginBrokerError {
    /// The credential scope failed validation or exclusive acquisition.
    #[error("the credential scope rejected the login operation")]
    CredentialScope(#[source] CredentialScopeError),
    /// The pinned executable reported a different version.
    #[error("the configured Codex CLI version does not match the pin")]
    VersionMismatch,
    /// Another login operation owns this broker or scope.
    #[error("a login operation is already running")]
    LoginAlreadyRunning,
    /// ChatGPT authentication is already present, so destructive relogin was not started.
    #[error("Codex is already logged in with ChatGPT")]
    AlreadyLoggedIn,
    /// Human-readable CLI output did not match the exact pinned fixture.
    #[error("the pinned Codex CLI returned unrecognized output")]
    ProviderOutputInvalid,
    /// One process stream exceeded its retained-output bound.
    #[error("the pinned Codex CLI exceeded the output limit")]
    OutputLimitExceeded,
    /// Login status could not prove logged-in or logged-out state.
    #[error("Codex login status is unavailable")]
    StatusUnavailable,
    /// A started login reached a proven terminal failure.
    #[error("the Codex device-login operation failed")]
    LoginFailed,
    /// A prior side effect may have occurred and must be reconciled before retry.
    #[error("the Codex login outcome is unknown and must be reconciled")]
    OutcomeUnknown,
    /// The trusted child process could not be started, supervised, or reaped.
    #[error("the Codex login process supervisor failed")]
    Process {
        #[source]
        source: io::Error,
    },
    /// The durable ledger could not be read or updated.
    #[error("the Codex login ledger is unavailable")]
    LedgerUnavailable {
        #[source]
        source: io::Error,
    },
    /// The durable ledger was malformed, unsafe, or from an unsupported version.
    #[error("the Codex login ledger is invalid")]
    LedgerInvalid,
}

impl From<CredentialScopeError> for LoginBrokerError {
    fn from(error: CredentialScopeError) -> Self {
        match error {
            CredentialScopeError::LoginAlreadyRunning => Self::LoginAlreadyRunning,
            error => Self::CredentialScope(error),
        }
    }
}
