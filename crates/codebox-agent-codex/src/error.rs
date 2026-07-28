use std::io;

use thiserror::Error;

/// One administrator-configured private directory in the credential scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialDirectory {
    /// The operator-owned Codex credential directory.
    CodexHome,
    /// Codebox-owned durable broker state.
    State,
    /// A non-repository working directory for the pinned CLI.
    Working,
}

/// One configured path checked for repository ancestry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialPath {
    /// The pinned native Codex executable.
    Executable,
    /// The operator-owned Codex credential directory.
    CodexHome,
    /// Codebox-owned durable broker state.
    State,
    /// A non-repository working directory for the pinned CLI.
    Working,
}

/// A private directory violated the CU-AUTH-P0-02 safety policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectoryViolation {
    /// The configured path was not absolute.
    NotAbsolute,
    /// The configured path contained a symlink or non-canonical component.
    NotCanonical,
    /// The final path component was a symbolic link.
    Symlink,
    /// The path did not identify a directory.
    NotDirectory,
    /// The directory was not owned by the effective runner user.
    WrongOwner,
    /// The directory mode was not exactly `0700`.
    Permissions,
}

/// The configured native executable violated the CU-AUTH-P0-02 safety policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutableViolation {
    /// The configured path was not absolute.
    NotAbsolute,
    /// The configured path contained a symlink or non-canonical component.
    NotCanonical,
    /// The final path component was a symbolic link.
    Symlink,
    /// The path did not identify a regular file.
    NotRegularFile,
    /// The file was owned by neither root nor the effective runner user.
    WrongOwner,
    /// The file was not executable or was writable by group/other.
    Permissions,
}

/// The durable lease file violated the CU-AUTH-P0-02 safety policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseViolation {
    /// The lease path was a symbolic link.
    Symlink,
    /// The opened lease was not a regular file.
    NotRegularFile,
    /// The lease was not owned by the effective runner user.
    WrongOwner,
    /// The lease mode was not exactly `0600`.
    Permissions,
    /// The lease had another hard link.
    MultipleLinks,
}

/// A credential scope could not be validated or exclusively leased.
///
/// Contract: `CU-AUTH-P0-02`. Display strings identify operator action without embedding configured
/// paths, environment values, credential content, or arbitrary OS error text.
#[derive(Debug, Error)]
pub enum CredentialScopeError {
    /// T002A currently supports only the Linux P0 deployment.
    #[error("credential scopes are unsupported on this platform")]
    UnsupportedPlatform,
    /// A private directory failed a deterministic safety check.
    #[error("a configured credential directory is unsafe")]
    DirectoryUnsafe {
        directory: CredentialDirectory,
        violation: DirectoryViolation,
    },
    /// Metadata for a configured directory was unavailable.
    #[error("a configured credential directory is unavailable")]
    DirectoryUnavailable {
        directory: CredentialDirectory,
        #[source]
        source: io::Error,
    },
    /// The native executable failed a deterministic safety check.
    #[error("the configured Codex executable is unsafe")]
    ExecutableUnsafe { violation: ExecutableViolation },
    /// Metadata for the native executable was unavailable.
    #[error("the configured Codex executable is unavailable")]
    ExecutableUnavailable {
        #[source]
        source: io::Error,
    },
    /// Two private directories overlapped.
    #[error("configured credential directories must not overlap")]
    DirectoryOverlap,
    /// A configured path was inside a repository.
    #[error("a configured credential path is inside a repository")]
    RepositoryPathRejected { path: CredentialPath },
    /// Repository ancestry could not be inspected safely.
    #[error("repository ancestry could not be inspected")]
    RepositoryInspectionUnavailable {
        path: CredentialPath,
        #[source]
        source: io::Error,
    },
    /// The persistent lease file failed a safety check.
    #[error("the credential-scope lease file is unsafe")]
    LeaseUnsafe { violation: LeaseViolation },
    /// The lease file could not be opened or locked.
    #[error("the credential-scope lease operation failed")]
    LeaseUnavailable {
        #[source]
        source: io::Error,
    },
    /// Another process or broker instance currently owns the scope.
    #[error("another login operation already owns the credential scope")]
    LoginAlreadyRunning,
}
