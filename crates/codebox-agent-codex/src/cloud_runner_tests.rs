use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use std::os::unix::fs::PermissionsExt;

use serde_json::json;

use crate::cloud_runner::{
    CloudCaptureLimits, CloudCliRuntime, CloudCommandCompletion, CloudCommandEnd,
    CloudCommandSupervisor, CloudStartFailure,
};
use crate::cloud_submit_ledger::{
    CLOUD_LEDGER_FILE_NAME, CloudLedgerPhase, CloudSubmitLedger, load_cloud_ledger,
    persist_cloud_ledger,
};
use crate::ledger::{LoginLedger, ProcessIdentity, persist_ledger, read_process_identity};
use crate::parser::PinnedStatus;
use crate::runtime::{
    SharedCapture,
    test_support::{drain_with_limit, parent_death_kills_child},
};
use crate::scope::OwnedCredentialScopeLease;
use crate::{
    CloudBranch, CloudCancellation, CloudCapture, CloudEnvironmentId, CloudInvocation, CloudPrompt,
    CloudRunnerConfig, CloudRunnerError, CloudRunnerErrorCategory, CloudSubmitOperationId,
    CloudSubmitRequest, CloudTaskId, CloudTaskRunner, CloudTaskStatus, CredentialScope,
    CredentialScopeConfig, LoginOperationId,
};

const ENVIRONMENT: &str = "env_test";
const BRANCH: &str = "main";
const TASK_ID: &str = "task_test_123";
const TASK_URL: &str = "https://chatgpt.com/codex/tasks/task_test_123";
const CLOUD_CAPTURE_LIMIT: usize = 64 * 1024;

static NEXT_LAYOUT: AtomicUsize = AtomicUsize::new(0);

struct TestLayout {
    root: PathBuf,
    executable: PathBuf,
    codex_home: PathBuf,
    state_dir: PathBuf,
    working_dir: PathBuf,
}

impl TestLayout {
    fn new() -> Self {
        let sequence = NEXT_LAYOUT.fetch_add(1, Ordering::Relaxed);
        let root = Path::new("/dev/shm").join(format!(
            "codebox-cloud-runner-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("create test root");
        set_mode(&root, 0o700);
        let executable = root.join("codex-native");
        fs::write(&executable, b"trusted fake executable").expect("write fake executable");
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

    fn scope(&self) -> CredentialScope {
        CredentialScope::validate(CredentialScopeConfig::new(
            self.executable.clone(),
            self.codex_home.clone(),
            self.state_dir.clone(),
            self.working_dir.clone(),
        ))
        .expect("valid cloud test scope")
    }

    fn config(&self) -> CloudRunnerConfig {
        CloudRunnerConfig::new(
            self.scope(),
            environment(),
            CloudBranch::try_new(BRANCH).expect("valid branch"),
        )
    }

    fn persist_cloud(&self, ledger: &CloudSubmitLedger) {
        persist_cloud_ledger(&self.state_dir, runner_uid(), ledger)
            .expect("persist cloud test ledger");
    }

    fn load_cloud(&self) -> CloudSubmitLedger {
        load_cloud_ledger(&self.state_dir, runner_uid())
            .expect("load cloud test ledger")
            .expect("cloud test ledger exists")
    }

    fn persist_login_intent(&self) {
        persist_ledger(
            &self.state_dir,
            runner_uid(),
            &LoginLedger::new(LoginOperationId::new()),
        )
        .expect("persist login intent");
    }

    fn install_fake_cli(&self) -> (PathBuf, PathBuf) {
        let observation = self.root.join("observed.txt");
        let exec_count = self.root.join("exec-count.txt");
        let script = format!(
            "#!/bin/sh\n\
             observation='{observation}'\n\
             exec_count='{exec_count}'\n\
             {{\n\
               printf 'cwd=%s\\n' \"$PWD\"\n\
               printf 'codex_home=%s\\n' \"$CODEX_HOME\"\n\
               for arg in \"$@\"; do printf 'arg=%s\\n' \"$arg\"; done\n\
             }} >> \"$observation\"\n\
             if [ \"$1\" = '--version' ]; then\n\
               printf 'codex-cli 0.145.0\\n'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = '-c' ]; then\n\
               printf 'Logged in using ChatGPT\\n' >&2\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = 'cloud' ] && [ \"$2\" = 'exec' ]; then\n\
               count=0\n\
               if [ -r \"$exec_count\" ]; then IFS= read -r count < \"$exec_count\"; fi\n\
               count=$((count + 1))\n\
               printf '%s\\n' \"$count\" > \"$exec_count\"\n\
               printf '{TASK_URL}\\n'\n\
               exit 0\n\
             fi\n\
             exit 9\n",
            observation = observation.display(),
            exec_count = exec_count.display(),
        );
        fs::write(&self.executable, script).expect("install fake CLI");
        set_mode(&self.executable, 0o700);
        (observation, exec_count)
    }
}

impl Drop for TestLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn private_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).expect("create private directory");
    set_mode(&path, 0o700);
    path
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set test mode");
}

fn runner_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and dereferences no pointers.
    unsafe { libc::geteuid() }
}

fn environment() -> CloudEnvironmentId {
    CloudEnvironmentId::try_new(ENVIRONMENT).expect("valid environment")
}

fn prompt(value: &str) -> CloudPrompt {
    CloudPrompt::try_new(value).expect("valid prompt")
}

fn request(operation_id: CloudSubmitOperationId, value: &str) -> CloudSubmitRequest {
    CloudSubmitRequest::new(operation_id, prompt(value))
}

fn process_identity() -> ProcessIdentity {
    read_process_identity(std::process::id()).expect("read test process identity")
}

fn exec_capture() -> CloudCapture {
    CloudCapture::new(
        format!("{TASK_URL}\n").into_bytes(),
        Vec::new(),
        false,
        false,
        Some(0),
    )
}

fn status_capture(status: CloudTaskStatus) -> CloudCapture {
    let (label, exit) = match status {
        CloudTaskStatus::Pending => ("PENDING", 1),
        CloudTaskStatus::Ready => ("READY", 0),
        CloudTaskStatus::Applied => ("APPLIED", 1),
        CloudTaskStatus::Error => ("ERROR", 1),
    };
    CloudCapture::new(
        format!("[{label}] Test task\nCloud Env  •  1m ago\n+1/-0 • 1 file\n").into_bytes(),
        Vec::new(),
        false,
        false,
        Some(exit),
    )
}

fn list_capture(task_ids: &[String], cursor: Option<&str>) -> CloudCapture {
    let tasks: Vec<_> = task_ids
        .iter()
        .map(|task_id| {
            json!({
                "id": task_id,
                "url": format!("https://chatgpt.com/codex/tasks/{task_id}"),
                "title": "Candidate",
                "status": "pending",
                "updated_at": "2026-07-28T12:00:00Z",
                "environment_id": ENVIRONMENT,
                "environment_label": "Test",
                "summary": {
                    "files_changed": 0,
                    "lines_added": 0,
                    "lines_removed": 0
                },
                "is_review": false,
                "attempt_total": 1
            })
        })
        .collect();
    let bytes = serde_json::to_vec(&json!({ "tasks": tasks, "cursor": cursor }))
        .expect("serialize list fixture");
    CloudCapture::new(bytes, Vec::new(), false, false, Some(0))
}

fn empty_capture() -> CloudCapture {
    CloudCapture::new(Vec::new(), Vec::new(), false, false, None)
}

fn unknown_ledger(operation_id: CloudSubmitOperationId) -> CloudSubmitLedger {
    let mut ledger = CloudSubmitLedger::new(
        operation_id,
        &environment(),
        &CloudBranch::try_new(BRANCH).expect("valid branch"),
    );
    ledger
        .append(CloudLedgerPhase::Authorized)
        .expect("authorize test ledger");
    ledger
        .append(CloudLedgerPhase::OutcomeUnknown)
        .expect("mark test ledger unknown");
    ledger
}

enum FakePlan {
    Complete {
        end: CloudCommandEnd,
        capture: CloudCapture,
        make_state_read_only: bool,
    },
    WaitFailure,
    StartFailure {
        may_have_started: bool,
        category: CloudRunnerErrorCategory,
    },
}

impl FakePlan {
    fn exited(capture: CloudCapture) -> Self {
        Self::Complete {
            end: CloudCommandEnd::Exited,
            capture,
            make_state_read_only: false,
        }
    }
}

struct FakeState {
    plans: VecDeque<FakePlan>,
    version_result: Result<(), CloudRunnerError>,
    login_result: Result<PinnedStatus, CloudRunnerError>,
    version_calls: usize,
    login_calls: usize,
    start_calls: usize,
    invocations: Vec<Vec<String>>,
    start_ledger_snapshots: Vec<String>,
    wait_ledger_snapshots: Vec<String>,
}

struct FakeRuntime {
    state: Arc<Mutex<FakeState>>,
    state_dir: PathBuf,
}

impl FakeRuntime {
    fn scripted(
        layout: &TestLayout,
        plans: impl IntoIterator<Item = FakePlan>,
    ) -> (Self, Arc<Mutex<FakeState>>) {
        let state = Arc::new(Mutex::new(FakeState {
            plans: plans.into_iter().collect(),
            version_result: Ok(()),
            login_result: Ok(PinnedStatus::LoggedIn),
            version_calls: 0,
            login_calls: 0,
            start_calls: 0,
            invocations: Vec::new(),
            start_ledger_snapshots: Vec::new(),
            wait_ledger_snapshots: Vec::new(),
        }));
        (
            Self {
                state: Arc::clone(&state),
                state_dir: layout.state_dir.clone(),
            },
            state,
        )
    }

    fn from_state(layout: &TestLayout, state: Arc<Mutex<FakeState>>) -> Self {
        Self {
            state,
            state_dir: layout.state_dir.clone(),
        }
    }

    fn with_login_result(self, result: Result<PinnedStatus, CloudRunnerError>) -> Self {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .login_result = result;
        self
    }
}

impl CloudCliRuntime for FakeRuntime {
    fn verify_version(&self, _lease: &OwnedCredentialScopeLease) -> Result<(), CloudRunnerError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.version_calls += 1;
        state.version_result
    }

    fn login_status(
        &self,
        _lease: &OwnedCredentialScopeLease,
    ) -> Result<PinnedStatus, CloudRunnerError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.login_calls += 1;
        state.login_result
    }

    fn start(
        &self,
        _lease: &OwnedCredentialScopeLease,
        invocation: &CloudInvocation,
        _timeout: Duration,
        _cancellation: CloudCancellation,
        _capture_limits: CloudCaptureLimits,
    ) -> Result<Box<dyn CloudCommandSupervisor>, CloudStartFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.start_calls += 1;
        state.invocations.push(invocation.args().to_vec());
        state.start_ledger_snapshots.push(
            fs::read_to_string(self.state_dir.join(CLOUD_LEDGER_FILE_NAME)).unwrap_or_default(),
        );
        let plan = state.plans.pop_front().unwrap_or(FakePlan::StartFailure {
            may_have_started: false,
            category: CloudRunnerErrorCategory::Process,
        });
        drop(state);

        match plan {
            FakePlan::StartFailure {
                may_have_started,
                category,
            } => Err(CloudStartFailure {
                error: CloudRunnerError::new(category),
                may_have_started,
            }),
            plan => Ok(Box::new(FakeSupervisor {
                plan: Some(plan),
                state: Arc::clone(&self.state),
                state_dir: self.state_dir.clone(),
            })),
        }
    }
}

struct FakeSupervisor {
    plan: Option<FakePlan>,
    state: Arc<Mutex<FakeState>>,
    state_dir: PathBuf,
}

impl CloudCommandSupervisor for FakeSupervisor {
    fn process_identity(&self) -> ProcessIdentity {
        process_identity()
    }

    fn wait(&mut self) -> Result<CloudCommandCompletion, CloudRunnerError> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .wait_ledger_snapshots
            .push(
                fs::read_to_string(self.state_dir.join(CLOUD_LEDGER_FILE_NAME)).unwrap_or_default(),
            );
        match self.plan.take().expect("fake plan consumed once") {
            FakePlan::Complete {
                end,
                capture,
                make_state_read_only,
            } => {
                if make_state_read_only {
                    set_mode(&self.state_dir, 0o500);
                }
                Ok(CloudCommandCompletion { end, capture })
            }
            FakePlan::WaitFailure => Err(CloudRunnerError::new(CloudRunnerErrorCategory::Process)),
            FakePlan::StartFailure { .. } => panic!("start failure reached supervisor"),
        }
    }

    fn cancel_and_wait(&mut self) -> Result<CloudCommandCompletion, CloudRunnerError> {
        Ok(CloudCommandCompletion {
            end: CloudCommandEnd::Canceled,
            capture: empty_capture(),
        })
    }
}

#[test]
fn cloud_runner_executes_only_pinned_invocations() {
    let layout = TestLayout::new();
    let (runtime, state) = FakeRuntime::scripted(
        &layout,
        [
            FakePlan::exited(exec_capture()),
            FakePlan::exited(status_capture(CloudTaskStatus::Ready)),
            FakePlan::exited(list_capture(&[], None)),
        ],
    );
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let operation_id = CloudSubmitOperationId::new();
    let task_id = CloudTaskId::try_new(TASK_ID).expect("valid task ID");

    runner
        .submit(
            request(operation_id, "fix the tests"),
            CloudCancellation::new(),
        )
        .expect("submit through fake runtime");
    assert_eq!(
        runner.status(&task_id).expect("read fake status"),
        CloudTaskStatus::Ready
    );
    layout.persist_cloud(&unknown_ledger(CloudSubmitOperationId::new()));
    runner
        .reconcile_unknown()
        .expect("list through fake runtime");

    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(state.start_calls, 3);
    assert_eq!(
        state.invocations,
        vec![
            CloudInvocation::exec(
                &environment(),
                &CloudBranch::try_new(BRANCH).unwrap(),
                &prompt("fix the tests")
            )
            .args(),
            CloudInvocation::status(&task_id).args(),
            CloudInvocation::list(&environment(), None).args(),
        ]
        .into_iter()
        .map(<[String]>::to_vec)
        .collect::<Vec<_>>()
    );
    assert!(state.invocations.iter().flatten().all(|arg| arg != "apply"));
}

#[test]
fn cloud_submit_fails_closed_before_spawn() {
    let layout = TestLayout::new();
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runtime = runtime.with_login_result(Ok(PinnedStatus::LoggedOut));
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(CloudSubmitOperationId::new(), "do not spawn"),
            CloudCancellation::new(),
        )
        .expect_err("logged-out preflight must fail");
    assert_eq!(error.category(), CloudRunnerErrorCategory::NotAuthenticated);
    assert_eq!(
        layout.load_cloud().latest(),
        CloudLedgerPhase::FailedBeforeSpawn
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );

    let layout = TestLayout::new();
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let cancellation = CloudCancellation::new();
    cancellation.cancel();
    let error = runner
        .submit(
            request(
                CloudSubmitOperationId::new(),
                "canceled before authorization",
            ),
            cancellation,
        )
        .expect_err("pre-authorization cancellation must not spawn");
    assert_eq!(
        error.category(),
        CloudRunnerErrorCategory::CanceledBeforeStart
    );
    assert_eq!(
        layout.load_cloud().latest(),
        CloudLedgerPhase::FailedBeforeSpawn
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );

    let layout = TestLayout::new();
    layout.persist_login_intent();
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(CloudSubmitOperationId::new(), "blocked by login recovery"),
            CloudCancellation::new(),
        )
        .expect_err("unknown login ledger must block");
    assert_eq!(
        error.category(),
        CloudRunnerErrorCategory::LoginOutcomeUnknown
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );

    let layout = TestLayout::new();
    let (runtime, state) = FakeRuntime::scripted(
        &layout,
        [FakePlan::StartFailure {
            may_have_started: false,
            category: CloudRunnerErrorCategory::Process,
        }],
    );
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(CloudSubmitOperationId::new(), "spawn must fail closed"),
            CloudCancellation::new(),
        )
        .expect_err("proven pre-spawn failure must be terminal");
    assert_eq!(error.category(), CloudRunnerErrorCategory::Process);
    assert_eq!(
        layout.load_cloud().latest(),
        CloudLedgerPhase::FailedBeforeSpawn
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        1
    );
}

#[test]
fn cloud_runner_blocks_pinned_error_log_write() {
    let layout = TestLayout::new();
    let (runtime, _) = FakeRuntime::scripted(&layout, [FakePlan::exited(exec_capture())]);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    runner
        .submit(
            request(CloudSubmitOperationId::new(), "install sentinel"),
            CloudCancellation::new(),
        )
        .expect("submit creates sentinel");
    let sentinel = layout.working_dir.join("error.log");
    assert!(sentinel.is_dir());
    assert_eq!(
        fs::metadata(&sentinel)
            .expect("sentinel metadata")
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
    assert!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sentinel)
            .is_err()
    );

    let layout = TestLayout::new();
    fs::write(layout.working_dir.join("error.log"), b"unsafe").expect("write unsafe sentinel");
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(CloudSubmitOperationId::new(), "fail sentinel"),
            CloudCancellation::new(),
        )
        .expect_err("file sentinel must fail");
    assert_eq!(
        error.category(),
        CloudRunnerErrorCategory::DiagnosticBoundary
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );
}

#[test]
fn cloud_submit_ledger_precedes_each_effect_boundary() {
    let layout = TestLayout::new();
    let (runtime, state) = FakeRuntime::scripted(&layout, [FakePlan::exited(exec_capture())]);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    runner
        .submit(
            request(CloudSubmitOperationId::new(), "check ordering"),
            CloudCancellation::new(),
        )
        .expect("ordered submit");

    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert!(state.start_ledger_snapshots[0].contains("\"authorized\""));
    assert!(state.wait_ledger_snapshots[0].contains("\"started\""));
    assert!(!state.start_ledger_snapshots[0].contains("\"started\""));
}

#[test]
fn cloud_submit_returns_only_durable_task() {
    let layout = TestLayout::new();
    let (runtime, _) = FakeRuntime::scripted(&layout, [FakePlan::exited(exec_capture())]);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let operation_id = CloudSubmitOperationId::new();
    let submission = runner
        .submit(
            request(operation_id, "durable result"),
            CloudCancellation::new(),
        )
        .expect("successful submit");
    let ledger = layout.load_cloud();

    assert_eq!(submission.operation_id(), operation_id);
    assert_eq!(submission.task_id().as_str(), TASK_ID);
    assert_eq!(ledger.latest(), CloudLedgerPhase::TaskRecorded);
    assert_eq!(
        ledger
            .recorded_task()
            .expect("valid recorded task")
            .expect("task is recorded")
            .as_str(),
        TASK_ID
    );
}

#[test]
fn cloud_submit_replay_returns_recorded_task_without_exec() {
    let layout = TestLayout::new();
    let (runtime, state) = FakeRuntime::scripted(&layout, [FakePlan::exited(exec_capture())]);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let operation_id = CloudSubmitOperationId::new();
    let first = runner
        .submit(
            request(operation_id, "first prompt"),
            CloudCancellation::new(),
        )
        .expect("first submit");
    let replay = runner
        .submit(
            request(operation_id, "different prompt is observe-only"),
            CloudCancellation::new(),
        )
        .expect("same-ID replay");

    assert_eq!(first, replay);
    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(state.start_calls, 1);
    assert_eq!(state.version_calls, 1);
    assert_eq!(state.login_calls, 1);
}

#[test]
fn cloud_submit_crash_matrix_is_reconcilable() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist_cloud(&CloudSubmitLedger::new(
        operation_id,
        &environment(),
        &CloudBranch::try_new(BRANCH).unwrap(),
    ));
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(operation_id, "intent replay"),
            CloudCancellation::new(),
        )
        .expect_err("intent recovery is terminal pre-start");
    assert_eq!(
        error.category(),
        CloudRunnerErrorCategory::PriorFailedBeforeSpawn
    );
    assert_eq!(
        layout.load_cloud().latest(),
        CloudLedgerPhase::FailedBeforeSpawn
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let mut ledger = CloudSubmitLedger::new(
        operation_id,
        &environment(),
        &CloudBranch::try_new(BRANCH).unwrap(),
    );
    ledger.append(CloudLedgerPhase::Authorized).unwrap();
    layout.persist_cloud(&ledger);
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(operation_id, "authorized replay"),
            CloudCancellation::new(),
        )
        .expect_err("authorized recovery is unknown");
    assert_eq!(error.category(), CloudRunnerErrorCategory::OutcomeUnknown);
    assert_eq!(
        layout.load_cloud().latest(),
        CloudLedgerPhase::OutcomeUnknown
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let mut ledger = CloudSubmitLedger::new(
        operation_id,
        &environment(),
        &CloudBranch::try_new(BRANCH).unwrap(),
    );
    ledger.append(CloudLedgerPhase::Authorized).unwrap();
    ledger.record_started(process_identity()).unwrap();
    layout.persist_cloud(&ledger);
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(operation_id, "started replay"),
            CloudCancellation::new(),
        )
        .expect_err("started recovery is unknown");
    assert_eq!(error.category(), CloudRunnerErrorCategory::OutcomeUnknown);
    assert_eq!(
        layout.load_cloud().latest(),
        CloudLedgerPhase::OutcomeUnknown
    );
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );
}

#[test]
fn cloud_submit_cancel_and_timeout_reap_as_unknown() {
    for end in [CloudCommandEnd::Canceled, CloudCommandEnd::TimedOut] {
        let layout = TestLayout::new();
        let (runtime, _) = FakeRuntime::scripted(
            &layout,
            [FakePlan::Complete {
                end,
                capture: empty_capture(),
                make_state_read_only: false,
            }],
        );
        let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
        let error = runner
            .submit(
                request(CloudSubmitOperationId::new(), "interrupted"),
                CloudCancellation::new(),
            )
            .expect_err("interrupted submit is unknown");
        assert_eq!(error.category(), CloudRunnerErrorCategory::OutcomeUnknown);
        assert_eq!(
            layout.load_cloud().latest(),
            CloudLedgerPhase::OutcomeUnknown
        );
    }

    let layout = TestLayout::new();
    let (runtime, _) = FakeRuntime::scripted(&layout, [FakePlan::WaitFailure]);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(CloudSubmitOperationId::new(), "supervisor failure"),
            CloudCancellation::new(),
        )
        .expect_err("post-spawn supervisor failure is unknown");
    assert_eq!(error.category(), CloudRunnerErrorCategory::OutcomeUnknown);
    assert_eq!(
        layout.load_cloud().latest(),
        CloudLedgerPhase::OutcomeUnknown
    );
}

#[test]
fn cloud_submit_parent_death_does_not_leave_runnable_child() {
    assert!(parent_death_kills_child().expect("shared parent-death policy"));
}

#[test]
fn regression_cloud_output_drains_after_truncation() {
    let output = drain_with_limit(vec![b'x'; CLOUD_CAPTURE_LIMIT + 8192], CLOUD_CAPTURE_LIMIT)
        .expect("drain bounded Cloud output");
    assert_eq!(output.stdout.len(), CLOUD_CAPTURE_LIMIT);
    assert!(output.stdout_overflow);
}

#[test]
fn cloud_runner_capture_is_chunk_partition_invariant() {
    let bytes: Vec<u8> = (0..=255).cycle().take(2048).collect();
    for chunk_size in 1..=bytes.len() {
        let capture = SharedCapture::with_limit(CLOUD_CAPTURE_LIMIT);
        for chunk in bytes.chunks(chunk_size) {
            capture.push(chunk);
        }
        let (actual, overflow) = capture.snapshot();
        assert_eq!(actual, bytes);
        assert!(!overflow);
    }
}

#[test]
fn cloud_runner_status_uses_pinned_exit_mapping() {
    let layout = TestLayout::new();
    let statuses = [
        CloudTaskStatus::Pending,
        CloudTaskStatus::Ready,
        CloudTaskStatus::Applied,
        CloudTaskStatus::Error,
    ];
    let plans = statuses
        .iter()
        .copied()
        .map(status_capture)
        .map(FakePlan::exited);
    let (runtime, _) = FakeRuntime::scripted(&layout, plans);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let task_id = CloudTaskId::try_new(TASK_ID).unwrap();

    for expected in statuses {
        assert_eq!(runner.status(&task_id).expect("pinned status"), expected);
    }
}

#[test]
fn cloud_reconciliation_is_bounded_and_detects_cycles() {
    let layout = TestLayout::new();
    layout.persist_cloud(&unknown_ledger(CloudSubmitOperationId::new()));
    let mut plans = Vec::new();
    for page in 0..5 {
        let ids: Vec<_> = (0..20)
            .map(|row| format!("task_page_{page}_{row}"))
            .collect();
        plans.push(FakePlan::exited(list_capture(
            &ids,
            Some(&format!("cursor-{page}")),
        )));
    }
    let (runtime, state) = FakeRuntime::scripted(&layout, plans);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let reconciliation = runner.reconcile_unknown().expect("bounded reconciliation");
    assert_eq!(reconciliation.tasks().len(), 100);
    assert!(!reconciliation.is_complete());
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        5
    );

    let layout = TestLayout::new();
    layout.persist_cloud(&unknown_ledger(CloudSubmitOperationId::new()));
    let (runtime, _) = FakeRuntime::scripted(
        &layout,
        [
            FakePlan::exited(list_capture(&["task_cycle_1".to_owned()], Some("same"))),
            FakePlan::exited(list_capture(&["task_cycle_2".to_owned()], Some("same"))),
        ],
    );
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .reconcile_unknown()
        .expect_err("cursor cycle must fail");
    assert_eq!(
        error.category(),
        CloudRunnerErrorCategory::ReconciliationCycle
    );
}

#[test]
fn cloud_reconciliation_never_infers_task_identity() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist_cloud(&unknown_ledger(operation_id));
    let (runtime, state) = FakeRuntime::scripted(
        &layout,
        [FakePlan::exited(list_capture(
            &["task_candidate".to_owned()],
            None,
        ))],
    );
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let result = runner
        .reconcile_unknown()
        .expect("one candidate is still ambiguous");
    assert_eq!(result.tasks().len(), 1);
    assert_eq!(
        layout.load_cloud().latest(),
        CloudLedgerPhase::ReconciliationObserved
    );
    let error = runner
        .submit(
            request(CloudSubmitOperationId::new(), "must not infer"),
            CloudCancellation::new(),
        )
        .expect_err("reconciliation does not permit submit");
    assert_eq!(error.category(), CloudRunnerErrorCategory::OutcomeUnknown);
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        1
    );
}

#[test]
fn cloud_unknown_submit_blocks_another_exec() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist_cloud(&unknown_ledger(operation_id));
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));

    for requested in [operation_id, CloudSubmitOperationId::new()] {
        let error = runner
            .submit(request(requested, "blocked"), CloudCancellation::new())
            .expect_err("unknown blocks every submit");
        assert_eq!(error.category(), CloudRunnerErrorCategory::OutcomeUnknown);
    }
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );
}

#[test]
fn cloud_submit_ledger_fails_closed() {
    assert!(
        serde_json::from_str::<CloudSubmitOperationId>("\"00000000-0000-0000-0000-000000000000\"")
            .is_err()
    );

    let layout = TestLayout::new();
    let ledger_path = layout.state_dir.join(CLOUD_LEDGER_FILE_NAME);
    fs::write(&ledger_path, b"{not-json").expect("write malformed ledger");
    set_mode(&ledger_path, 0o600);
    let (runtime, state) = FakeRuntime::scripted(&layout, []);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let error = runner
        .submit(
            request(CloudSubmitOperationId::new(), "invalid ledger"),
            CloudCancellation::new(),
        )
        .expect_err("malformed ledger fails closed");
    assert_eq!(error.category(), CloudRunnerErrorCategory::LedgerInvalid);
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );

    set_mode(&ledger_path, 0o644);
    let error =
        load_cloud_ledger(&layout.state_dir, runner_uid()).expect_err("unsafe mode fails closed");
    assert_eq!(error.category(), CloudRunnerErrorCategory::LedgerInvalid);
}

#[test]
fn cloud_runner_outputs_and_state_are_redacted() {
    let layout = TestLayout::new();
    let canary = "T004A_SECRET_PROMPT_CANARY";
    let capture = CloudCapture::new(
        Vec::new(),
        canary.as_bytes().to_vec(),
        false,
        false,
        Some(1),
    );
    let (runtime, _) = FakeRuntime::scripted(&layout, [FakePlan::exited(capture)]);
    let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
    let request = request(CloudSubmitOperationId::new(), canary);
    assert!(!format!("{request:?}").contains(canary));
    let error = runner
        .submit(request, CloudCancellation::new())
        .expect_err("malformed provider output");
    let ledger = fs::read_to_string(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
        .expect("read redacted ledger");

    assert_eq!(error.category(), CloudRunnerErrorCategory::OutcomeUnknown);
    assert!(!error.to_string().contains(canary));
    assert!(!format!("{runner:?}").contains(ENVIRONMENT));
    assert!(!ledger.contains(canary));
    assert!(!ledger.contains(TASK_URL));
}

#[test]
fn regression_cloud_runner_never_executes_repository_code() {
    let layout = TestLayout::new();
    let (observation_path, exec_count_path) = layout.install_fake_cli();
    let repository = layout.root.join("hostile-repository");
    fs::create_dir(&repository).expect("create hostile repository");
    fs::create_dir(repository.join(".git")).expect("create git marker");
    let marker = repository.join("repository-code-ran");
    let hook = repository.join("hook.sh");
    fs::write(
        &hook,
        format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
    )
    .expect("write hostile hook");
    set_mode(&hook, 0o700);
    let credential_canary = "T004A_AUTH_CANARY";
    fs::write(layout.codex_home.join("auth.json"), credential_canary).expect("write auth canary");
    let runner = CloudTaskRunner::new(layout.config()).expect("production Cloud runner");
    let prompt = "fix tests; $(touch SHOULD_NOT_RUN)";
    let submission = runner
        .submit(
            request(CloudSubmitOperationId::new(), prompt),
            CloudCancellation::new(),
        )
        .expect("fixed fake CLI submit");
    let observations = fs::read_to_string(observation_path).expect("read fake CLI observations");
    let ledger = fs::read_to_string(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
        .expect("read Cloud ledger");

    assert_eq!(submission.task_id().as_str(), TASK_ID);
    assert!(!marker.exists());
    assert!(!observations.contains(&repository.to_string_lossy().into_owned()));
    assert!(!observations.contains(credential_canary));
    assert!(!observations.contains("cloud\narg=apply"));
    assert!(observations.contains(&format!("cwd={}", layout.working_dir.display())));
    assert!(observations.contains(&format!("codex_home={}", layout.codex_home.display())));
    assert!(layout.working_dir.join("error.log").is_dir());
    assert_eq!(
        fs::read_to_string(exec_count_path)
            .expect("read exec count")
            .trim(),
        "1"
    );
    assert!(!ledger.contains(prompt));
    assert!(!ledger.contains(credential_canary));
}

#[test]
fn regression_unknown_cloud_submit_reconciles_before_retry() {
    for candidate_ids in [
        Vec::new(),
        vec!["task_candidate".to_owned()],
        vec![
            "task_candidate_one".to_owned(),
            "task_candidate_two".to_owned(),
        ],
    ] {
        let layout = TestLayout::new();
        let (runtime, state) = FakeRuntime::scripted(
            &layout,
            [
                FakePlan::Complete {
                    end: CloudCommandEnd::Exited,
                    capture: exec_capture(),
                    make_state_read_only: true,
                },
                FakePlan::exited(list_capture(&candidate_ids, None)),
            ],
        );
        let runner = CloudTaskRunner::with_runtime(layout.config(), Box::new(runtime));
        let operation_id = CloudSubmitOperationId::new();
        let error = runner
            .submit(
                request(operation_id, "provider records before disk failure"),
                CloudCancellation::new(),
            )
            .expect_err("task record persistence must fail");
        assert_eq!(error.category(), CloudRunnerErrorCategory::RecoveryRequired);
        drop(runner);
        set_mode(&layout.state_dir, 0o700);

        let restarted = CloudTaskRunner::with_runtime(
            layout.config(),
            Box::new(FakeRuntime::from_state(&layout, Arc::clone(&state))),
        );
        let reconciliation = restarted.reconcile_unknown().unwrap_or_else(|error| {
            panic!(
                "reconcile after restart with {} candidate(s): {error:?}",
                candidate_ids.len()
            )
        });
        assert_eq!(reconciliation.operation_id(), operation_id);
        assert_eq!(reconciliation.tasks().len(), candidate_ids.len());
        assert!(reconciliation.is_complete());

        let error = restarted
            .submit(
                request(
                    CloudSubmitOperationId::new(),
                    "reconciliation cannot authorize another exec",
                ),
                CloudCancellation::new(),
            )
            .expect_err("unknown submit remains blocked after reconciliation");
        assert_eq!(error.category(), CloudRunnerErrorCategory::OutcomeUnknown);

        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let exec_calls = state
            .invocations
            .iter()
            .filter(|args| args.get(1).is_some_and(|arg| arg == "exec"))
            .count();
        assert_eq!(exec_calls, 1);
        assert_eq!(state.start_calls, 2);
    }
}
