use std::fmt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use crate::{
    CodexCommand, CodexInvocation, CredentialDirectory, CredentialPath, CredentialScopeError,
    DirectoryViolation, ExecutableViolation, LeaseViolation,
};

const LEASE_FILE_NAME: &str = "login.lock";

/// Administrator-owned paths for one personal Codex credential scope.
///
/// Contract: `CU-AUTH-P0-02`. These values are deployment configuration, never browser input.
pub struct CredentialScopeConfig {
    executable: PathBuf,
    codex_home: PathBuf,
    state_dir: PathBuf,
    working_dir: PathBuf,
}

impl CredentialScopeConfig {
    /// Creates an unvalidated administrator configuration.
    ///
    /// Contract: `CU-AUTH-P0-02`. `CredentialScope::validate` must succeed before any value is used
    /// to construct a child command.
    pub fn new(
        executable: PathBuf,
        codex_home: PathBuf,
        state_dir: PathBuf,
        working_dir: PathBuf,
    ) -> Self {
        Self {
            executable,
            codex_home,
            state_dir,
            working_dir,
        }
    }
}

/// A validated local trust boundary for the pinned Codex CLI.
///
/// Contract: `CU-AUTH-P0-02`. The value does not start a process or inspect credential contents.
pub struct CredentialScope {
    executable: PathBuf,
    codex_home: PathBuf,
    state_dir: PathBuf,
    working_dir: PathBuf,
    #[cfg(target_os = "linux")]
    runner_uid: u32,
}

impl fmt::Debug for CredentialScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialScope")
            .finish_non_exhaustive()
    }
}

impl CredentialScope {
    /// Validates one Linux P0 credential scope.
    ///
    /// Contract: `CU-AUTH-P0-02`. Paths, ownership, modes, repository ancestry, and overlap are
    /// checked without reading `CODEX_HOME` files or starting a process.
    pub fn validate(config: CredentialScopeConfig) -> Result<Self, CredentialScopeError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = config;
            Err(CredentialScopeError::UnsupportedPlatform)
        }

        #[cfg(target_os = "linux")]
        {
            let runner_uid = effective_uid();
            validate_executable(&config.executable, runner_uid)?;
            validate_directory(
                &config.codex_home,
                CredentialDirectory::CodexHome,
                runner_uid,
            )?;
            validate_directory(&config.state_dir, CredentialDirectory::State, runner_uid)?;
            validate_directory(
                &config.working_dir,
                CredentialDirectory::Working,
                runner_uid,
            )?;

            reject_repository_ancestry(
                executable_ancestry_start(&config.executable),
                CredentialPath::Executable,
            )?;
            reject_repository_ancestry(&config.codex_home, CredentialPath::CodexHome)?;
            reject_repository_ancestry(&config.state_dir, CredentialPath::State)?;
            reject_repository_ancestry(&config.working_dir, CredentialPath::Working)?;

            if paths_overlap(&config.codex_home, &config.state_dir)
                || paths_overlap(&config.codex_home, &config.working_dir)
                || paths_overlap(&config.state_dir, &config.working_dir)
            {
                return Err(CredentialScopeError::DirectoryOverlap);
            }

            Ok(Self {
                executable: config.executable,
                codex_home: config.codex_home,
                state_dir: config.state_dir,
                working_dir: config.working_dir,
                runner_uid,
            })
        }
    }

    /// Obtains the exclusive E1 lease for this operator scope.
    ///
    /// Contract: `CU-AUTH-P0-02`. A second owner receives `LoginAlreadyRunning`; lock availability
    /// does not inspect or modify the T002B login ledger.
    pub fn try_acquire(&self) -> Result<CredentialScopeLease<'_>, CredentialScopeError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(CredentialScopeError::UnsupportedPlatform)
        }

        #[cfg(target_os = "linux")]
        {
            self.revalidate()?;
            let lease_path = self.state_dir.join(LEASE_FILE_NAME);
            let file = open_lease(&lease_path)?;
            validate_lease(&file, self.runner_uid)?;
            acquire_lock(&file)?;

            Ok(CredentialScopeLease {
                scope: self,
                _lock: file,
            })
        }
    }

    pub(crate) fn acquire_owned(&self) -> Result<OwnedCredentialScopeLease, CredentialScopeError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(CredentialScopeError::UnsupportedPlatform)
        }

        #[cfg(target_os = "linux")]
        {
            let borrowed = self.try_acquire()?;
            Ok(OwnedCredentialScopeLease {
                executable: self.executable.clone(),
                codex_home: self.codex_home.clone(),
                state_dir: self.state_dir.clone(),
                working_dir: self.working_dir.clone(),
                runner_uid: self.runner_uid,
                _lock: borrowed._lock,
            })
        }
    }

    #[cfg(target_os = "linux")]
    fn revalidate(&self) -> Result<(), CredentialScopeError> {
        validate_executable(&self.executable, self.runner_uid)?;
        validate_directory(
            &self.codex_home,
            CredentialDirectory::CodexHome,
            self.runner_uid,
        )?;
        validate_directory(&self.state_dir, CredentialDirectory::State, self.runner_uid)?;
        validate_directory(
            &self.working_dir,
            CredentialDirectory::Working,
            self.runner_uid,
        )?;
        reject_repository_ancestry(
            executable_ancestry_start(&self.executable),
            CredentialPath::Executable,
        )?;
        reject_repository_ancestry(&self.codex_home, CredentialPath::CodexHome)?;
        reject_repository_ancestry(&self.state_dir, CredentialPath::State)?;
        reject_repository_ancestry(&self.working_dir, CredentialPath::Working)
    }
}

/// Move-only ownership of one operator credential scope.
///
/// Contract: `CU-AUTH-P0-02`. Dropping the value closes the locked file descriptor and releases the
/// kernel lock without changing provider state or the T002B ledger.
pub struct CredentialScopeLease<'scope> {
    scope: &'scope CredentialScope,
    _lock: File,
}

pub(crate) struct OwnedCredentialScopeLease {
    executable: PathBuf,
    codex_home: PathBuf,
    state_dir: PathBuf,
    working_dir: PathBuf,
    runner_uid: u32,
    _lock: File,
}

impl OwnedCredentialScopeLease {
    pub(crate) fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub(crate) fn runner_uid(&self) -> u32 {
        self.runner_uid
    }

    pub(crate) fn invocation(
        &self,
        command: CodexCommand,
    ) -> Result<CodexInvocation<'_>, CredentialScopeError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = command;
            Err(CredentialScopeError::UnsupportedPlatform)
        }

        #[cfg(target_os = "linux")]
        {
            revalidate_owned(self)?;
            Ok(CodexInvocation::new(
                &self.executable,
                &self.working_dir,
                &self.codex_home,
                command,
            ))
        }
    }
}

impl CredentialScopeLease<'_> {
    /// Returns the operator credential directory without reading it.
    ///
    /// Contract: `CU-AUTH-P0-02`.
    pub fn codex_home(&self) -> &Path {
        &self.scope.codex_home
    }

    /// Returns the trusted broker state directory.
    ///
    /// Contract: `CU-AUTH-P0-02`.
    pub fn state_dir(&self) -> &Path {
        &self.scope.state_dir
    }

    /// Returns the trusted non-repository CLI working directory.
    ///
    /// Contract: `CU-AUTH-P0-02`.
    pub fn working_dir(&self) -> &Path {
        &self.scope.working_dir
    }

    /// Produces one fixed process policy after rechecking the mutable host boundary.
    ///
    /// Contract: `CU-AUTH-P0-02`. The result cannot carry arbitrary argv or environment values.
    pub fn invocation(
        &self,
        command: CodexCommand,
    ) -> Result<CodexInvocation<'_>, CredentialScopeError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = command;
            Err(CredentialScopeError::UnsupportedPlatform)
        }

        #[cfg(target_os = "linux")]
        {
            self.scope.revalidate()?;
            Ok(CodexInvocation::new(
                &self.scope.executable,
                &self.scope.working_dir,
                &self.scope.codex_home,
                command,
            ))
        }
    }
}

#[cfg(target_os = "linux")]
fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

#[cfg(target_os = "linux")]
fn validate_executable(path: &Path, runner_uid: u32) -> Result<(), CredentialScopeError> {
    if !path.is_absolute() {
        return Err(CredentialScopeError::ExecutableUnsafe {
            violation: ExecutableViolation::NotAbsolute,
        });
    }

    let metadata = std::fs::symlink_metadata(path)
        .map_err(|source| CredentialScopeError::ExecutableUnavailable { source })?;
    if metadata.file_type().is_symlink() {
        return Err(CredentialScopeError::ExecutableUnsafe {
            violation: ExecutableViolation::Symlink,
        });
    }
    if !metadata.is_file() {
        return Err(CredentialScopeError::ExecutableUnsafe {
            violation: ExecutableViolation::NotRegularFile,
        });
    }
    if !owner_is_allowed(metadata.uid(), runner_uid, true) {
        return Err(CredentialScopeError::ExecutableUnsafe {
            violation: ExecutableViolation::WrongOwner,
        });
    }

    let mode = metadata.mode();
    if mode & 0o111 == 0 || mode & 0o022 != 0 || mode & 0o7000 != 0 {
        return Err(CredentialScopeError::ExecutableUnsafe {
            violation: ExecutableViolation::Permissions,
        });
    }

    let canonical = std::fs::canonicalize(path)
        .map_err(|source| CredentialScopeError::ExecutableUnavailable { source })?;
    if canonical != path {
        return Err(CredentialScopeError::ExecutableUnsafe {
            violation: ExecutableViolation::NotCanonical,
        });
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_directory(
    path: &Path,
    directory: CredentialDirectory,
    runner_uid: u32,
) -> Result<(), CredentialScopeError> {
    if !path.is_absolute() {
        return Err(CredentialScopeError::DirectoryUnsafe {
            directory,
            violation: DirectoryViolation::NotAbsolute,
        });
    }

    let metadata = std::fs::symlink_metadata(path)
        .map_err(|source| CredentialScopeError::DirectoryUnavailable { directory, source })?;
    if metadata.file_type().is_symlink() {
        return Err(CredentialScopeError::DirectoryUnsafe {
            directory,
            violation: DirectoryViolation::Symlink,
        });
    }
    if !metadata.is_dir() {
        return Err(CredentialScopeError::DirectoryUnsafe {
            directory,
            violation: DirectoryViolation::NotDirectory,
        });
    }
    if !owner_is_allowed(metadata.uid(), runner_uid, false) {
        return Err(CredentialScopeError::DirectoryUnsafe {
            directory,
            violation: DirectoryViolation::WrongOwner,
        });
    }
    if metadata.mode() & 0o7777 != 0o700 {
        return Err(CredentialScopeError::DirectoryUnsafe {
            directory,
            violation: DirectoryViolation::Permissions,
        });
    }

    let canonical = std::fs::canonicalize(path)
        .map_err(|source| CredentialScopeError::DirectoryUnavailable { directory, source })?;
    if canonical != path {
        return Err(CredentialScopeError::DirectoryUnsafe {
            directory,
            violation: DirectoryViolation::NotCanonical,
        });
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn revalidate_owned(lease: &OwnedCredentialScopeLease) -> Result<(), CredentialScopeError> {
    validate_executable(&lease.executable, lease.runner_uid)?;
    validate_directory(
        &lease.codex_home,
        CredentialDirectory::CodexHome,
        lease.runner_uid,
    )?;
    validate_directory(
        &lease.state_dir,
        CredentialDirectory::State,
        lease.runner_uid,
    )?;
    validate_directory(
        &lease.working_dir,
        CredentialDirectory::Working,
        lease.runner_uid,
    )?;
    reject_repository_ancestry(
        executable_ancestry_start(&lease.executable),
        CredentialPath::Executable,
    )?;
    reject_repository_ancestry(&lease.codex_home, CredentialPath::CodexHome)?;
    reject_repository_ancestry(&lease.state_dir, CredentialPath::State)?;
    reject_repository_ancestry(&lease.working_dir, CredentialPath::Working)
}

#[cfg(target_os = "linux")]
fn owner_is_allowed(actual_uid: u32, runner_uid: u32, root_is_allowed: bool) -> bool {
    actual_uid == runner_uid || (root_is_allowed && actual_uid == 0)
}

#[cfg(target_os = "linux")]
fn executable_ancestry_start(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}

#[cfg(target_os = "linux")]
fn reject_repository_ancestry(
    start: &Path,
    configured_path: CredentialPath,
) -> Result<(), CredentialScopeError> {
    for ancestor in start.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => {
                return Err(CredentialScopeError::RepositoryPathRejected {
                    path: configured_path,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(CredentialScopeError::RepositoryInspectionUnavailable {
                    path: configured_path,
                    source,
                });
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[cfg(target_os = "linux")]
fn open_lease(path: &Path) -> Result<File, CredentialScopeError> {
    let mut create_options = OpenOptions::new();
    create_options
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);

    match create_options.open(path) {
        Ok(file) => {
            // SAFETY: `file` owns a valid descriptor; `fchmod` does not retain it or access Rust
            // memory. This makes the lease mode deterministic even under a restrictive umask.
            let result = unsafe { libc::fchmod(file.as_raw_fd(), 0o600) };
            if result == 0 {
                return Ok(file);
            }
            return Err(CredentialScopeError::LeaseUnavailable {
                source: std::io::Error::last_os_error(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(source) => return Err(map_lease_open_error(source)),
    }

    let mut existing_options = OpenOptions::new();
    existing_options
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    existing_options.open(path).map_err(map_lease_open_error)
}

#[cfg(target_os = "linux")]
fn map_lease_open_error(source: std::io::Error) -> CredentialScopeError {
    if source.raw_os_error() == Some(libc::ELOOP) {
        CredentialScopeError::LeaseUnsafe {
            violation: LeaseViolation::Symlink,
        }
    } else {
        CredentialScopeError::LeaseUnavailable { source }
    }
}

#[cfg(target_os = "linux")]
fn validate_lease(file: &File, runner_uid: u32) -> Result<(), CredentialScopeError> {
    let metadata = file
        .metadata()
        .map_err(|source| CredentialScopeError::LeaseUnavailable { source })?;
    if !metadata.is_file() {
        return Err(CredentialScopeError::LeaseUnsafe {
            violation: LeaseViolation::NotRegularFile,
        });
    }
    if metadata.uid() != runner_uid {
        return Err(CredentialScopeError::LeaseUnsafe {
            violation: LeaseViolation::WrongOwner,
        });
    }
    if metadata.mode() & 0o7777 != 0o600 {
        return Err(CredentialScopeError::LeaseUnsafe {
            violation: LeaseViolation::Permissions,
        });
    }
    if metadata.nlink() != 1 {
        return Err(CredentialScopeError::LeaseUnsafe {
            violation: LeaseViolation::MultipleLinks,
        });
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn acquire_lock(file: &File) -> Result<(), CredentialScopeError> {
    // SAFETY: the file descriptor is valid for the lifetime of `file`, and `flock` does not retain
    // the pointer or access Rust-managed memory.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(());
    }

    let source = std::io::Error::last_os_error();
    if matches!(
        source.raw_os_error(),
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
    ) {
        Err(CredentialScopeError::LoginAlreadyRunning)
    } else {
        Err(CredentialScopeError::LeaseUnavailable { source })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::owner_is_allowed;

    #[test]
    fn credential_scope_rejects_wrong_owner_policy() {
        assert!(!owner_is_allowed(2000, 1000, false));
        assert!(!owner_is_allowed(2000, 1000, true));
        assert!(!owner_is_allowed(0, 1000, false));
        assert!(owner_is_allowed(0, 1000, true));
        assert!(owner_is_allowed(1000, 1000, false));
    }
}
