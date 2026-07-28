#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use codebox_agent_codex::{
    CodexCommand, CredentialDirectory, CredentialScope, CredentialScopeConfig,
    CredentialScopeError, DirectoryViolation, ExecutableViolation, LeaseViolation,
};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestLayout {
    root: PathBuf,
    executable: PathBuf,
    codex_home: PathBuf,
    state_dir: PathBuf,
    working_dir: PathBuf,
}

impl TestLayout {
    fn new() -> Self {
        let root = loop {
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let candidate = Path::new("/dev/shm").join(format!(
                "codebox-agent-codex-test-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create isolated test directory: {error}"),
            }
        };
        set_mode(&root, 0o700);

        let executable = root.join("codex-native");
        fs::write(&executable, b"pinned native fixture").expect("create executable fixture");
        set_mode(&executable, 0o700);

        let codex_home = create_private_directory(&root, "codex-home");
        let state_dir = create_private_directory(&root, "state");
        let working_dir = create_private_directory(&root, "working");

        Self {
            root,
            executable,
            codex_home,
            state_dir,
            working_dir,
        }
    }

    fn config(&self) -> CredentialScopeConfig {
        CredentialScopeConfig::new(
            self.executable.clone(),
            self.codex_home.clone(),
            self.state_dir.clone(),
            self.working_dir.clone(),
        )
    }
}

impl Drop for TestLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn create_private_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).expect("create private test directory");
    set_mode(&path, 0o700);
    path
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set fixture mode");
}

#[test]
fn credential_scope_accepts_private_non_repository_paths() {
    let layout = TestLayout::new();
    let scope = CredentialScope::validate(layout.config()).expect("valid credential scope");
    let lease = scope.try_acquire().expect("exclusive scope lease");

    assert_eq!(lease.codex_home(), layout.codex_home);
    assert_eq!(lease.state_dir(), layout.state_dir);
    assert_eq!(lease.working_dir(), layout.working_dir);
}

#[test]
fn login_home_permissions_are_rejected_when_unsafe() {
    let layout = TestLayout::new();
    set_mode(&layout.codex_home, 0o750);

    let error = CredentialScope::validate(layout.config()).expect_err("unsafe mode must fail");

    assert!(matches!(
        error,
        CredentialScopeError::DirectoryUnsafe {
            directory: CredentialDirectory::CodexHome,
            violation: DirectoryViolation::Permissions,
        }
    ));
}

#[test]
fn credential_scope_rejects_wrong_owner_and_symlinks() {
    let layout = TestLayout::new();
    fs::remove_file(&layout.executable).expect("remove regular executable fixture");
    symlink("/bin/false", &layout.executable).expect("create executable symlink");

    let error = CredentialScope::validate(layout.config()).expect_err("symlink must fail");

    assert!(matches!(
        error,
        CredentialScopeError::ExecutableUnsafe {
            violation: ExecutableViolation::Symlink,
        }
    ));
}

#[test]
fn credential_scope_rejects_repository_paths() {
    let layout = TestLayout::new();
    fs::write(layout.root.join(".git"), b"gitdir: elsewhere").expect("create repository marker");

    let error =
        CredentialScope::validate(layout.config()).expect_err("repository ancestry must fail");

    assert!(matches!(
        error,
        CredentialScopeError::RepositoryPathRejected { .. }
    ));
}

#[test]
fn credential_scope_rejects_overlapping_directories() {
    let layout = TestLayout::new();
    let config = CredentialScopeConfig::new(
        layout.executable.clone(),
        layout.codex_home.clone(),
        layout.codex_home.clone(),
        layout.working_dir.clone(),
    );

    assert!(matches!(
        CredentialScope::validate(config),
        Err(CredentialScopeError::DirectoryOverlap)
    ));
}

#[test]
fn login_command_is_not_user_controlled() {
    let layout = TestLayout::new();
    let scope = CredentialScope::validate(layout.config()).expect("valid credential scope");
    let lease = scope.try_acquire().expect("exclusive scope lease");

    let version = lease
        .invocation(CodexCommand::Version)
        .expect("version invocation");
    let status = lease
        .invocation(CodexCommand::LoginStatus)
        .expect("status invocation");
    let login = lease
        .invocation(CodexCommand::DeviceLogin)
        .expect("login invocation");

    assert_eq!(version.args(), ["--version"]);
    assert_eq!(
        status.args(),
        [
            "-c",
            "forced_login_method=\"chatgpt\"",
            "-c",
            "cli_auth_credentials_store=\"file\"",
            "login",
            "status",
        ]
    );
    assert_eq!(
        login.args(),
        [
            "-c",
            "forced_login_method=\"chatgpt\"",
            "-c",
            "cli_auth_credentials_store=\"file\"",
            "login",
            "--device-auth",
        ]
    );

    for invocation in [version, status, login] {
        assert_eq!(invocation.executable(), layout.executable);
        assert_eq!(invocation.working_dir(), layout.working_dir);
        assert_eq!(invocation.codex_home(), layout.codex_home);
        assert!(invocation.clears_environment());
    }
}

#[test]
fn login_scope_is_single_writer() {
    let layout = TestLayout::new();
    let first_scope = CredentialScope::validate(layout.config()).expect("first scope");
    let second_scope = CredentialScope::validate(layout.config()).expect("second scope");
    let first_lease = first_scope.try_acquire().expect("first lease");

    assert!(matches!(
        second_scope.try_acquire(),
        Err(CredentialScopeError::LoginAlreadyRunning)
    ));

    drop(first_lease);
    second_scope
        .try_acquire()
        .expect("lock is available after owner drops");
}

#[test]
fn credential_scope_rejects_unsafe_lease_file() {
    let layout = TestLayout::new();
    let lease_path = layout.state_dir.join("login.lock");
    fs::write(&lease_path, b"not trusted").expect("create existing lease fixture");
    set_mode(&lease_path, 0o644);
    let scope = CredentialScope::validate(layout.config()).expect("valid credential scope");

    assert!(matches!(
        scope.try_acquire(),
        Err(CredentialScopeError::LeaseUnsafe {
            violation: LeaseViolation::Permissions,
        })
    ));
}

#[test]
fn credential_scope_rechecks_paths_before_use() {
    let layout = TestLayout::new();
    let scope = CredentialScope::validate(layout.config()).expect("initially valid scope");
    set_mode(&layout.working_dir, 0o755);

    assert!(matches!(
        scope.try_acquire(),
        Err(CredentialScopeError::DirectoryUnsafe {
            directory: CredentialDirectory::Working,
            violation: DirectoryViolation::Permissions,
        })
    ));

    set_mode(&layout.working_dir, 0o700);
    let lease = scope.try_acquire().expect("lease after operator repair");
    set_mode(&layout.working_dir, 0o755);
    assert!(matches!(
        lease.invocation(CodexCommand::LoginStatus),
        Err(CredentialScopeError::DirectoryUnsafe {
            directory: CredentialDirectory::Working,
            violation: DirectoryViolation::Permissions,
        })
    ));
}

#[test]
fn released_lock_does_not_override_uncertain_ledger() {
    let layout = TestLayout::new();
    let ledger = layout.state_dir.join("login-ledger.json");
    let uncertain = br#"{"schemaVersion":1,"state":"outcomeUnknown"}"#;
    fs::write(&ledger, uncertain).expect("write uncertain ledger fixture");
    let scope = CredentialScope::validate(layout.config()).expect("valid credential scope");

    drop(scope.try_acquire().expect("first lease"));
    drop(scope.try_acquire().expect("second lease"));

    assert_eq!(
        fs::read(ledger).expect("read ledger after lease churn"),
        uncertain
    );
}

#[test]
fn regression_cloud_runner_never_executes_repository_code() {
    let layout = TestLayout::new();
    let scope = CredentialScope::validate(layout.config()).expect("valid credential scope");
    let lease = scope.try_acquire().expect("exclusive scope lease");

    for command in [
        CodexCommand::Version,
        CodexCommand::LoginStatus,
        CodexCommand::DeviceLogin,
    ] {
        let invocation = lease.invocation(command).expect("fixed invocation");
        let joined = invocation.args().join(" ");

        assert_eq!(invocation.executable(), layout.executable);
        assert!(!joined.contains("exec"));
        assert!(!joined.contains("cloud"));
        assert!(!joined.contains("sh"));
        assert!(!joined.contains(&layout.root.to_string_lossy().into_owned()));
    }
}

#[test]
fn credential_scope_never_reads_or_projects_auth_cache() {
    let layout = TestLayout::new();
    let auth_cache = layout.codex_home.join("auth.json");
    let canary = b"T002A_SECRET_CANARY";
    fs::write(&auth_cache, canary).expect("write auth canary");
    set_mode(&auth_cache, 0o000);

    let scope = CredentialScope::validate(layout.config()).expect("scope ignores auth contents");
    let lease = scope.try_acquire().expect("lease ignores auth contents");
    let invocation = lease
        .invocation(CodexCommand::LoginStatus)
        .expect("fixed status invocation");

    assert_eq!(invocation.codex_home(), layout.codex_home);
    assert!(!format!("{:?}", invocation.args()).contains("T002A_SECRET_CANARY"));
}
