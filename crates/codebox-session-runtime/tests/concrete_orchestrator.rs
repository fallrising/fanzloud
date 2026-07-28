#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use codebox_agent_codex::{
    CloudBranch, CloudEnvironmentId, CloudPrompt, CloudRunnerConfig, CloudTaskOrchestrator,
    CredentialScope, CredentialScopeConfig,
};
use codebox_session_runtime::{
    P0SessionConfig, P0SessionRuntime, P0SessionState, P0TurnProjection,
};

static NEXT_LAYOUT: AtomicU64 = AtomicU64::new(0);
const WAIT_LIMIT: Duration = Duration::from_secs(5);

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
            let sequence = NEXT_LAYOUT.fetch_add(1, Ordering::Relaxed);
            let candidate = Path::new("/dev/shm").join(format!(
                "codebox-t005a-concrete-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create concrete fixture root: {error}"),
            }
        };
        set_mode(&root, 0o700);
        let executable = root.join("codex-fixture");
        fs::write(&executable, fixture_script()).expect("write concrete Codex fixture");
        set_mode(&executable, 0o700);
        let codex_home = private_directory(&root, "codex-home");
        let state_dir = private_directory(&root, "state");
        let working_dir = private_directory(&root, "working");
        Self {
            root,
            executable,
            codex_home,
            state_dir,
            working_dir,
        }
    }

    fn runner_config(&self) -> CloudRunnerConfig {
        let scope = CredentialScope::validate(CredentialScopeConfig::new(
            self.executable.clone(),
            self.codex_home.clone(),
            self.state_dir.clone(),
            self.working_dir.clone(),
        ))
        .expect("valid concrete credential scope");
        CloudRunnerConfig::new(
            scope,
            CloudEnvironmentId::try_new("env_synthetic").expect("valid environment"),
            CloudBranch::try_new("main").expect("valid branch"),
        )
    }
}

impl Drop for TestLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn p0_session_composes_with_concrete_accepted_orchestrator() {
    let layout = TestLayout::new();
    let orchestrator =
        CloudTaskOrchestrator::new(layout.runner_config()).expect("concrete orchestrator");
    let config = P0SessionConfig::try_new(Duration::from_millis(250), 64, 8, 8)
        .expect("valid session bounds");
    let runtime = P0SessionRuntime::new(orchestrator, config).expect("concrete session runtime");

    runtime
        .start_turn(CloudPrompt::try_new("update the synthetic documentation").expect("prompt"))
        .expect("queue concrete turn");
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let snapshot = runtime.snapshot().expect("snapshot");
        if snapshot.state == P0SessionState::Ready
            && snapshot.current_turn.as_ref().is_some_and(|turn| {
                matches!(
                    turn.projection,
                    P0TurnProjection::Cloud {
                        lifecycle: codebox_session_runtime::P0CloudLifecycle::Ready { .. },
                        ..
                    }
                )
            })
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "concrete orchestrator did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let diff = runtime.read_diff().expect("concrete diff read");
    assert_eq!(
        diff.as_str(),
        "diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n+Synthetic text\n"
    );
    runtime.shutdown().expect("join concrete runtime");
}

fn private_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).expect("create private fixture directory");
    set_mode(&path, 0o700);
    path
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set fixture mode");
}

fn fixture_script() -> &'static [u8] {
    r#"#!/bin/sh
case " $* " in
  " --version ")
    printf 'codex-cli 0.145.0\n'
    exit 0
    ;;
  *" login status "*)
    printf 'Logged in using ChatGPT\n' >&2
    exit 0
    ;;
  *" cloud exec "*)
    printf 'https://chatgpt.com/codex/tasks/task_i_abc123\n'
    exit 0
    ;;
  *" cloud status "*)
    printf '[READY] Synthetic documentation update\nprivate-environment  •  10s ago\n+5/-1 • 1 file\n'
    exit 0
    ;;
  *" cloud diff "*)
    printf 'diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n+Synthetic text\n'
    exit 0
    ;;
esac
exit 97
"#
    .as_bytes()
}
