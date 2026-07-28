use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use proptest::prelude::*;

use crate::cloud_lifecycle::LifecycleCloudRunner;
use crate::cloud_lifecycle_ledger::{CloudLifecycleLedger, persist_lifecycle_ledger};
use crate::cloud_runner::{
    CloudCaptureLimits, CloudCliRuntime, CloudCommandCompletion, CloudCommandEnd,
    CloudCommandSupervisor, CloudStartFailure,
};
use crate::cloud_submit_ledger::{CloudLedgerPhase, CloudSubmitLedger, persist_cloud_ledger};
use crate::ledger::{ProcessIdentity, read_process_identity};
use crate::parser::PinnedStatus;
use crate::runtime::SharedCapture;
use crate::scope::OwnedCredentialScopeLease;
use crate::{
    CloudBranch, CloudCancellation, CloudCapture, CloudDiffReadErrorCategory, CloudEnvironmentId,
    CloudInvocation, CloudRunnerConfig, CloudRunnerError, CloudRunnerErrorCategory,
    CloudSubmitOperationId, CloudTaskId, CloudTaskOrchestrator, CloudTaskRunner, CloudTaskStatus,
    CredentialScope, CredentialScopeConfig, decode_cloud_diff,
};

const ENVIRONMENT: &str = "env_diff";
const BRANCH: &str = "main";
const TASK_ONE: &str = "task_diff_one";
const TASK_TWO: &str = "task_diff_two";
const DIFF_TEXT: &str =
    "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -0,0 +1 @@\n+safe\n";

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
            "codebox-cloud-diff-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("create diff test root");
        set_mode(&root, 0o700);
        let executable = root.join("codex-native");
        fs::write(&executable, b"trusted diff test executable").expect("write test executable");
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
        .expect("valid diff test scope")
    }

    fn config(&self) -> CloudRunnerConfig {
        CloudRunnerConfig::new(self.scope(), environment(), branch())
    }

    fn install_sentinel(&self) {
        let sentinel = self.working_dir.join("error.log");
        fs::create_dir(&sentinel).expect("create diagnostic sentinel");
        set_mode(&sentinel, 0o700);
    }

    fn persist_accepted(
        &self,
        operation_id: CloudSubmitOperationId,
        task_id: &CloudTaskId,
        status: CloudTaskStatus,
        adopted: bool,
    ) {
        let mut submit = CloudSubmitLedger::new(operation_id, &environment(), &branch());
        submit
            .append(CloudLedgerPhase::Authorized)
            .expect("authorize test submit");
        let mut lifecycle = CloudLifecycleLedger::submitting(operation_id);
        if adopted {
            submit
                .append(CloudLedgerPhase::OutcomeUnknown)
                .expect("mark submit unknown");
            submit
                .record_reconciliation(std::slice::from_ref(task_id), true)
                .expect("record submit reconciliation");
            submit
                .record_adopted_task(task_id)
                .expect("record adopted task");
            lifecycle
                .record_outcome_unknown()
                .expect("mark lifecycle unknown");
            lifecycle
                .record_reconciliation(std::slice::from_ref(task_id), true)
                .expect("record lifecycle reconciliation");
        } else {
            submit
                .record_started(process_identity())
                .expect("record test process");
            submit.record_task(task_id).expect("record submitted task");
            lifecycle
                .record_status(task_id, CloudTaskStatus::Pending)
                .expect("record pending lifecycle");
        }
        lifecycle
            .record_status(task_id, status)
            .expect("record accepted lifecycle");
        persist_cloud_ledger(&self.state_dir, runner_uid(), &submit)
            .expect("persist diff submit ledger");
        persist_lifecycle_ledger(&self.state_dir, runner_uid(), &lifecycle)
            .expect("persist diff lifecycle ledger");
    }

    fn persist_pending(&self, operation_id: CloudSubmitOperationId, task_id: &CloudTaskId) {
        let mut submit = CloudSubmitLedger::new(operation_id, &environment(), &branch());
        submit
            .append(CloudLedgerPhase::Authorized)
            .expect("authorize pending submit");
        submit
            .record_started(process_identity())
            .expect("record pending process");
        submit.record_task(task_id).expect("record pending task");
        persist_cloud_ledger(&self.state_dir, runner_uid(), &submit)
            .expect("persist pending submit");

        let mut lifecycle = CloudLifecycleLedger::submitting(operation_id);
        lifecycle
            .record_status(task_id, CloudTaskStatus::Pending)
            .expect("record pending lifecycle");
        persist_lifecycle_ledger(&self.state_dir, runner_uid(), &lifecycle)
            .expect("persist pending lifecycle");
    }

    fn install_cli(&self, body: &str) {
        fs::write(&self.executable, body).expect("install fake diff CLI");
        set_mode(&self.executable, 0o700);
    }

    fn snapshot(&self) -> ManagedSnapshot {
        let mut entries = Vec::new();
        snapshot_tree(&self.root, &self.root, &self.codex_home, &mut entries);
        entries.sort();
        ManagedSnapshot(entries)
    }

    fn durable_snapshot(&self) -> ManagedSnapshot {
        let mut entries = Vec::new();
        snapshot_tree(&self.root, &self.state_dir, &self.codex_home, &mut entries);
        snapshot_tree(
            &self.root,
            &self.working_dir,
            &self.codex_home,
            &mut entries,
        );
        entries.sort();
        ManagedSnapshot(entries)
    }
}

impl Drop for TestLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ManagedSnapshot(Vec<(PathBuf, u32, Vec<u8>)>);

fn snapshot_tree(
    root: &Path,
    path: &Path,
    excluded: &Path,
    entries: &mut Vec<(PathBuf, u32, Vec<u8>)>,
) {
    let mut children: Vec<_> = fs::read_dir(path)
        .expect("read managed snapshot directory")
        .map(|entry| entry.expect("read managed snapshot entry").path())
        .collect();
    children.sort();
    for child in children {
        if child == excluded {
            continue;
        }
        let metadata = fs::symlink_metadata(&child).expect("stat managed snapshot entry");
        let relative = child
            .strip_prefix(root)
            .expect("managed path under root")
            .to_owned();
        let mode = metadata.permissions().mode();
        let bytes = if metadata.file_type().is_symlink() {
            fs::read_link(&child)
                .expect("read managed symlink")
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else if metadata.is_file() {
            fs::read(&child).expect("read managed file")
        } else {
            Vec::new()
        };
        entries.push((relative, mode, bytes));
        if metadata.is_dir() {
            snapshot_tree(root, &child, excluded, entries);
        }
    }
}

#[derive(Clone)]
enum TestPlan {
    Complete(CloudCommandEnd, CloudCapture),
    StartFailure(CloudRunnerErrorCategory),
    WaitFailure(CloudRunnerErrorCategory),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StartRecord {
    args: Vec<String>,
    timeout: Duration,
    capture_limits: CloudCaptureLimits,
}

struct TestRuntimeState {
    plans: VecDeque<TestPlan>,
    version_result: Result<(), CloudRunnerError>,
    version_calls: usize,
    starts: Vec<StartRecord>,
}

struct TestRuntime {
    state: Arc<Mutex<TestRuntimeState>>,
}

impl TestRuntime {
    fn scripted(plans: impl IntoIterator<Item = TestPlan>) -> (Self, Arc<Mutex<TestRuntimeState>>) {
        let state = Arc::new(Mutex::new(TestRuntimeState {
            plans: plans.into_iter().collect(),
            version_result: Ok(()),
            version_calls: 0,
            starts: Vec::new(),
        }));
        (
            Self {
                state: Arc::clone(&state),
            },
            state,
        )
    }
}

impl CloudCliRuntime for TestRuntime {
    fn verify_version(&self, _lease: &OwnedCredentialScopeLease) -> Result<(), CloudRunnerError> {
        let mut state = self.state.lock().expect("diff runtime state");
        state.version_calls += 1;
        state.version_result
    }

    fn login_status(
        &self,
        _lease: &OwnedCredentialScopeLease,
    ) -> Result<PinnedStatus, CloudRunnerError> {
        Ok(PinnedStatus::LoggedIn)
    }

    fn start(
        &self,
        _lease: &OwnedCredentialScopeLease,
        invocation: &CloudInvocation,
        timeout: Duration,
        _cancellation: CloudCancellation,
        capture_limits: CloudCaptureLimits,
    ) -> Result<Box<dyn CloudCommandSupervisor>, CloudStartFailure> {
        let mut state = self.state.lock().expect("diff runtime state");
        state.starts.push(StartRecord {
            args: invocation.args().to_vec(),
            timeout,
            capture_limits,
        });
        let plan = state.plans.pop_front().unwrap_or_else(|| {
            TestPlan::Complete(CloudCommandEnd::Exited, diff_capture(DIFF_TEXT))
        });
        match plan {
            TestPlan::StartFailure(category) => Err(CloudStartFailure {
                error: CloudRunnerError::new(category),
                may_have_started: false,
            }),
            plan => Ok(Box::new(TestSupervisor { plan: Some(plan) })),
        }
    }
}

struct TestSupervisor {
    plan: Option<TestPlan>,
}

impl CloudCommandSupervisor for TestSupervisor {
    fn process_identity(&self) -> ProcessIdentity {
        process_identity()
    }

    fn wait(&mut self) -> Result<CloudCommandCompletion, CloudRunnerError> {
        match self.plan.take().expect("one diff supervisor result") {
            TestPlan::Complete(end, capture) => Ok(CloudCommandCompletion { end, capture }),
            TestPlan::WaitFailure(category) => Err(CloudRunnerError::new(category)),
            TestPlan::StartFailure(_) => unreachable!("start failures do not create supervisors"),
        }
    }

    fn cancel_and_wait(&mut self) -> Result<CloudCommandCompletion, CloudRunnerError> {
        Ok(CloudCommandCompletion {
            end: CloudCommandEnd::Canceled,
            capture: CloudCapture::default(),
        })
    }
}

fn fake_orchestrator(
    layout: &TestLayout,
    plans: impl IntoIterator<Item = TestPlan>,
) -> (CloudTaskOrchestrator, Arc<Mutex<TestRuntimeState>>) {
    let (runtime, state) = TestRuntime::scripted(plans);
    let runner: Arc<dyn LifecycleCloudRunner> = Arc::new(CloudTaskRunner::with_runtime(
        layout.config(),
        Box::new(runtime),
    ));
    (
        CloudTaskOrchestrator::with_runner(runner).expect("construct diff orchestrator"),
        state,
    )
}

fn accepted_fake(
    layout: &TestLayout,
    status: CloudTaskStatus,
    adopted: bool,
    plans: impl IntoIterator<Item = TestPlan>,
) -> (
    CloudTaskOrchestrator,
    Arc<Mutex<TestRuntimeState>>,
    CloudSubmitOperationId,
    CloudTaskId,
) {
    let operation_id = CloudSubmitOperationId::new();
    let task_id = task(TASK_ONE);
    layout.install_sentinel();
    layout.persist_accepted(operation_id, &task_id, status, adopted);
    let (orchestrator, state) = fake_orchestrator(layout, plans);
    (orchestrator, state, operation_id, task_id)
}

fn diff_capture(value: &str) -> CloudCapture {
    CloudCapture::new(value.as_bytes().to_vec(), Vec::new(), false, false, Some(0))
}

fn environment() -> CloudEnvironmentId {
    CloudEnvironmentId::try_new(ENVIRONMENT).expect("valid diff environment")
}

fn branch() -> CloudBranch {
    CloudBranch::try_new(BRANCH).expect("valid diff branch")
}

fn task(value: &str) -> CloudTaskId {
    CloudTaskId::try_new(value).expect("valid diff task")
}

fn private_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).expect("create private diff directory");
    set_mode(&path, 0o700);
    path
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set diff test mode");
}

fn runner_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and dereferences no pointers.
    unsafe { libc::geteuid() }
}

fn process_identity() -> ProcessIdentity {
    read_process_identity(std::process::id()).expect("read diff test process identity")
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(path.exists(), "expected process marker was not created");
}

fn read_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .expect("read process ID")
        .trim()
        .parse()
        .expect("parse process ID")
}

fn process_alive(pid: u32) -> bool {
    let pid = i32::try_from(pid).expect("test PID in range");
    // SAFETY: signal zero performs an existence check and uses no pointers.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn wait_for_process_exit(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!process_alive(pid), "process {pid} remained alive");
}

#[test]
fn cloud_diff_requires_accepted_task_reference() {
    let first = TestLayout::new();
    let (orchestrator, first_state, operation_id, _) = accepted_fake(
        &first,
        CloudTaskStatus::Ready,
        false,
        [TestPlan::Complete(
            CloudCommandEnd::Exited,
            diff_capture(DIFF_TEXT),
        )],
    );
    let eligible = orchestrator
        .diff_eligible_task()
        .expect("mint accepted diff task");
    let debug = format!("{eligible:?}");
    assert!(!debug.contains(TASK_ONE));

    let second = TestLayout::new();
    let (other, state, _, _) = accepted_fake(
        &second,
        CloudTaskStatus::Ready,
        false,
        [TestPlan::Complete(
            CloudCommandEnd::Exited,
            diff_capture(DIFF_TEXT),
        )],
    );
    let error = other
        .diff_reader()
        .retrieve(&eligible, CloudCancellation::new())
        .expect_err("different scope must reject opaque authority");
    assert_eq!(
        error.category(),
        CloudDiffReadErrorCategory::AuthorityMismatch
    );
    assert!(state.lock().expect("diff runtime state").starts.is_empty());

    let mut wrong_config = CloudSubmitLedger::new(
        operation_id,
        &CloudEnvironmentId::try_new("env_other").expect("valid other environment"),
        &branch(),
    );
    wrong_config
        .append(CloudLedgerPhase::Authorized)
        .expect("authorize wrong-config submit");
    wrong_config
        .record_started(process_identity())
        .expect("record wrong-config process");
    wrong_config
        .record_task(&task(TASK_ONE))
        .expect("record wrong-config task");
    persist_cloud_ledger(&first.state_dir, runner_uid(), &wrong_config)
        .expect("persist wrong-config submit");
    let error = orchestrator
        .diff_reader()
        .retrieve(&eligible, CloudCancellation::new())
        .expect_err("same paths with different administrator config must reject");
    assert_eq!(
        error.category(),
        CloudDiffReadErrorCategory::AuthorityMismatch
    );
    assert!(
        first_state
            .lock()
            .expect("diff runtime state")
            .starts
            .is_empty()
    );
}

#[test]
fn cloud_diff_runner_executes_only_pinned_attempt() {
    let layout = TestLayout::new();
    let (orchestrator, state, _, _) = accepted_fake(
        &layout,
        CloudTaskStatus::Ready,
        false,
        [TestPlan::Complete(
            CloudCommandEnd::Exited,
            diff_capture(DIFF_TEXT),
        )],
    );
    let eligible = orchestrator.diff_eligible_task().expect("eligible task");
    let diff = orchestrator
        .diff_reader()
        .retrieve(&eligible, CloudCancellation::new())
        .expect("retrieve fixed diff");
    assert_eq!(diff.as_str(), DIFF_TEXT);

    let state = state.lock().expect("diff runtime state");
    assert_eq!(state.version_calls, 1);
    assert_eq!(
        state.starts,
        vec![StartRecord {
            args: vec![
                "cloud".to_owned(),
                "diff".to_owned(),
                "--attempt=1".to_owned(),
                TASK_ONE.to_owned(),
            ],
            timeout: Duration::from_secs(60),
            capture_limits: CloudCaptureLimits::diff(),
        }]
    );
}

#[test]
fn cloud_diff_rejects_ineligible_lifecycle() {
    let pending = TestLayout::new();
    pending.install_sentinel();
    pending.persist_pending(CloudSubmitOperationId::new(), &task(TASK_ONE));
    let (orchestrator, state) = fake_orchestrator(&pending, []);
    let error = orchestrator
        .diff_eligible_task()
        .expect_err("pending task must be ineligible");
    assert_eq!(
        error.category(),
        CloudDiffReadErrorCategory::IneligibleLifecycle
    );
    assert!(state.lock().expect("diff runtime state").starts.is_empty());

    for phase in ["failed", "provider-error", "canceled", "abandoned"] {
        let layout = TestLayout::new();
        let operation_id = CloudSubmitOperationId::new();
        let task_id = task(TASK_ONE);
        let mut lifecycle = CloudLifecycleLedger::submitting(operation_id);
        match phase {
            "failed" => lifecycle
                .record_failed_before_submit()
                .expect("record failed lifecycle"),
            "provider-error" => {
                lifecycle
                    .record_status(&task_id, CloudTaskStatus::Pending)
                    .expect("record pending lifecycle");
                lifecycle
                    .record_status(&task_id, CloudTaskStatus::Error)
                    .expect("record provider-error lifecycle");
            }
            "canceled" => lifecycle
                .record_canceled_without_task()
                .expect("record canceled lifecycle"),
            "abandoned" => {
                lifecycle
                    .record_outcome_unknown()
                    .expect("record unknown lifecycle");
                lifecycle
                    .record_abandoned_unknown()
                    .expect("record abandoned lifecycle");
            }
            _ => unreachable!(),
        }
        persist_lifecycle_ledger(&layout.state_dir, runner_uid(), &lifecycle)
            .expect("persist ineligible lifecycle");
        let (orchestrator, _) = fake_orchestrator(&layout, []);
        let error = orchestrator
            .diff_eligible_task()
            .expect_err("terminal non-diff lifecycle must be ineligible");
        assert_eq!(
            error.category(),
            CloudDiffReadErrorCategory::IneligibleLifecycle,
            "{phase}"
        );
    }

    for (status, adopted) in [
        (CloudTaskStatus::Ready, false),
        (CloudTaskStatus::Applied, false),
        (CloudTaskStatus::Ready, true),
    ] {
        let layout = TestLayout::new();
        let (orchestrator, _, _, _) = accepted_fake(&layout, status, adopted, []);
        orchestrator
            .diff_eligible_task()
            .expect("Ready/Applied task must be eligible");
    }

    let stale = TestLayout::new();
    let (orchestrator, state, _, _) = accepted_fake(
        &stale,
        CloudTaskStatus::Ready,
        false,
        [TestPlan::Complete(
            CloudCommandEnd::Exited,
            diff_capture(DIFF_TEXT),
        )],
    );
    let old = orchestrator
        .diff_eligible_task()
        .expect("old eligible task");
    stale.persist_accepted(
        CloudSubmitOperationId::new(),
        &task(TASK_TWO),
        CloudTaskStatus::Ready,
        false,
    );
    let error = orchestrator
        .diff_reader()
        .retrieve(&old, CloudCancellation::new())
        .expect_err("replaced lifecycle must invalidate old authority");
    assert_eq!(
        error.category(),
        CloudDiffReadErrorCategory::AuthorityMismatch
    );
    assert!(state.lock().expect("diff runtime state").starts.is_empty());
}

#[test]
fn cloud_diff_success_leaves_managed_state_identical() {
    let layout = TestLayout::new();
    let (orchestrator, _, _, _) = accepted_fake(
        &layout,
        CloudTaskStatus::Applied,
        true,
        [TestPlan::Complete(
            CloudCommandEnd::Exited,
            diff_capture(DIFF_TEXT),
        )],
    );
    let eligible = orchestrator.diff_eligible_task().expect("eligible task");
    let before = layout.snapshot();
    let diff = orchestrator
        .diff_reader()
        .retrieve(&eligible, CloudCancellation::new())
        .expect("successful E0 diff");
    assert_eq!(diff.as_str(), DIFF_TEXT);
    assert_eq!(layout.snapshot(), before);
}

#[test]
fn cloud_diff_failure_leaves_managed_state_identical() {
    let failures = [
        TestPlan::StartFailure(CloudRunnerErrorCategory::Process),
        TestPlan::WaitFailure(CloudRunnerErrorCategory::Process),
        TestPlan::Complete(CloudCommandEnd::TimedOut, CloudCapture::default()),
        TestPlan::Complete(CloudCommandEnd::Canceled, CloudCapture::default()),
        TestPlan::Complete(
            CloudCommandEnd::Exited,
            CloudCapture::new(Vec::new(), Vec::new(), false, false, Some(7)),
        ),
        TestPlan::Complete(
            CloudCommandEnd::Exited,
            CloudCapture::new(Vec::new(), Vec::new(), false, false, None),
        ),
        TestPlan::Complete(
            CloudCommandEnd::Exited,
            CloudCapture::new(vec![0xff], Vec::new(), false, false, Some(0)),
        ),
        TestPlan::Complete(
            CloudCommandEnd::Exited,
            CloudCapture::new(Vec::new(), Vec::new(), true, false, Some(0)),
        ),
    ];

    for plan in failures {
        let layout = TestLayout::new();
        let (orchestrator, _, _, _) = accepted_fake(&layout, CloudTaskStatus::Ready, false, [plan]);
        let eligible = orchestrator.diff_eligible_task().expect("eligible task");
        let before = layout.snapshot();
        orchestrator
            .diff_reader()
            .retrieve(&eligible, CloudCancellation::new())
            .expect_err("injected diff failure");
        assert_eq!(layout.snapshot(), before);
    }

    let layout = TestLayout::new();
    let (orchestrator, state, _, _) = accepted_fake(&layout, CloudTaskStatus::Ready, false, []);
    let eligible = orchestrator.diff_eligible_task().expect("eligible task");
    state.lock().expect("diff runtime state").version_result =
        Err(CloudRunnerError::new(CloudRunnerErrorCategory::Version));
    let before = layout.snapshot();
    let error = orchestrator
        .diff_reader()
        .retrieve(&eligible, CloudCancellation::new())
        .expect_err("version failure");
    assert_eq!(error.category(), CloudDiffReadErrorCategory::Version);
    assert_eq!(layout.snapshot(), before);
}

#[test]
fn cloud_diff_cannot_append_pinned_error_log() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let task_id = task(TASK_ONE);
    layout.install_sentinel();
    layout.persist_accepted(operation_id, &task_id, CloudTaskStatus::Ready, false);
    layout.install_cli(
        "#!/bin/sh\n\
         if [ \"$1\" = '--version' ]; then\n\
           printf 'codex-cli 0.145.0\\n'\n\
           exit 0\n\
         fi\n\
         if [ \"$1\" = 'cloud' ] && [ \"$2\" = 'diff' ]; then\n\
           (printf 'account-secret\\n' >> error.log) 2>/dev/null || :\n\
           printf 'diff --git a/a b/a\\n--- a/a\\n+++ b/a\\n'\n\
           exit 0\n\
         fi\n\
         exit 9\n",
    );
    let orchestrator =
        CloudTaskOrchestrator::new(layout.config()).expect("construct process diff orchestrator");
    let eligible = orchestrator.diff_eligible_task().expect("eligible task");
    let before = layout.snapshot();
    orchestrator
        .diff_reader()
        .retrieve(&eligible, CloudCancellation::new())
        .expect("sentinel blocks append without blocking diff");
    assert_eq!(layout.snapshot(), before);
    assert!(
        fs::read_dir(layout.working_dir.join("error.log"))
            .expect("read diagnostic sentinel")
            .next()
            .is_none()
    );

    for unsafe_kind in ["missing", "file", "symlink", "mode"] {
        let layout = TestLayout::new();
        let (orchestrator, state, _, _) = accepted_fake(&layout, CloudTaskStatus::Ready, false, []);
        let eligible = orchestrator.diff_eligible_task().expect("eligible task");
        let sentinel = layout.working_dir.join("error.log");
        match unsafe_kind {
            "missing" => fs::remove_dir(&sentinel).expect("remove sentinel"),
            "file" => {
                fs::remove_dir(&sentinel).expect("remove sentinel");
                fs::write(&sentinel, b"unsafe").expect("write unsafe sentinel file");
                set_mode(&sentinel, 0o600);
            }
            "symlink" => {
                fs::remove_dir(&sentinel).expect("remove sentinel");
                let target = private_directory(&layout.root, "sentinel-target");
                symlink(target, &sentinel).expect("create sentinel symlink");
            }
            "mode" => set_mode(&sentinel, 0o755),
            _ => unreachable!(),
        }
        let before = layout.snapshot();
        let error = orchestrator
            .diff_reader()
            .retrieve(&eligible, CloudCancellation::new())
            .expect_err("unsafe sentinel must fail closed");
        assert_eq!(
            error.category(),
            CloudDiffReadErrorCategory::DiagnosticBoundary
        );
        assert_eq!(layout.snapshot(), before);
        assert!(state.lock().expect("diff runtime state").starts.is_empty());
    }
}

#[test]
fn cloud_diff_runner_drains_after_limit() {
    for stream in ["stdout", "stderr"] {
        let layout = TestLayout::new();
        let marker = layout.root.join(format!("{stream}-drained"));
        let operation_id = CloudSubmitOperationId::new();
        let task_id = task(TASK_ONE);
        layout.install_sentinel();
        layout.persist_accepted(operation_id, &task_id, CloudTaskStatus::Ready, false);
        let body = if stream == "stdout" {
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = '--version' ]; then printf 'codex-cli 0.145.0\\n'; exit 0; fi\n\
                 if [ \"$1\" = 'cloud' ] && [ \"$2\" = 'diff' ]; then\n\
                   /usr/bin/head -c 2097153 /dev/zero | /usr/bin/tr '\\000' x\n\
                   : > '{marker}'\n\
                   exit 0\n\
                 fi\n\
                 exit 9\n",
                marker = marker.display(),
            )
        } else {
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = '--version' ]; then printf 'codex-cli 0.145.0\\n'; exit 0; fi\n\
                 if [ \"$1\" = 'cloud' ] && [ \"$2\" = 'diff' ]; then\n\
                   /usr/bin/head -c 65537 /dev/zero | /usr/bin/tr '\\000' e >&2\n\
                   : > '{marker}'\n\
                   exit 0\n\
                 fi\n\
                 exit 9\n",
                marker = marker.display(),
            )
        };
        layout.install_cli(&body);
        let orchestrator =
            CloudTaskOrchestrator::new(layout.config()).expect("construct drain orchestrator");
        let eligible = orchestrator.diff_eligible_task().expect("eligible task");
        let error = orchestrator
            .diff_reader()
            .retrieve(&eligible, CloudCancellation::new())
            .expect_err("overflow must not return a partial diff");
        assert_eq!(error.category(), CloudDiffReadErrorCategory::OutputLimit);
        assert!(
            marker.exists(),
            "{stream} was not drained after its retained limit"
        );
    }
}

#[test]
fn cloud_diff_cancel_reaps_without_partial_result() {
    let layout = TestLayout::new();
    let entered = layout.root.join("diff-entered");
    let parent_pid_file = layout.root.join("diff-parent-pid");
    let child_pid_file = layout.root.join("diff-child-pid");
    let operation_id = CloudSubmitOperationId::new();
    let task_id = task(TASK_ONE);
    layout.install_sentinel();
    layout.persist_accepted(operation_id, &task_id, CloudTaskStatus::Ready, false);
    layout.install_cli(&format!(
        "#!/bin/sh\n\
         if [ \"$1\" = '--version' ]; then printf 'codex-cli 0.145.0\\n'; exit 0; fi\n\
         if [ \"$1\" = 'cloud' ] && [ \"$2\" = 'diff' ]; then\n\
           printf '%s\\n' \"$$\" > '{parent_pid_file}'\n\
           /usr/bin/sleep 300 &\n\
           printf '%s\\n' \"$!\" > '{child_pid_file}'\n\
           : > '{entered}'\n\
           wait\n\
         fi\n\
         exit 9\n",
        entered = entered.display(),
        parent_pid_file = parent_pid_file.display(),
        child_pid_file = child_pid_file.display(),
    ));
    let orchestrator =
        CloudTaskOrchestrator::new(layout.config()).expect("construct cancellation orchestrator");
    let eligible = orchestrator.diff_eligible_task().expect("eligible task");
    let reader = orchestrator.diff_reader();
    let before = layout.durable_snapshot();
    let cancellation = CloudCancellation::new();
    let cancel_handle = cancellation.clone();
    let retrieval = thread::spawn(move || reader.retrieve(&eligible, cancellation));
    wait_for_file(&entered);
    let parent_pid = read_pid(&parent_pid_file);
    let child_pid = read_pid(&child_pid_file);
    cancel_handle.cancel();
    let error = retrieval
        .join()
        .expect("join diff retrieval")
        .expect_err("canceled diff returns no partial result");
    assert_eq!(error.category(), CloudDiffReadErrorCategory::Canceled);
    wait_for_process_exit(parent_pid);
    wait_for_process_exit(child_pid);
    assert_eq!(layout.durable_snapshot(), before);

    let canceled = CloudCancellation::new();
    canceled.cancel();
    let error = orchestrator
        .diff_reader()
        .retrieve(
            &orchestrator.diff_eligible_task().expect("eligible task"),
            canceled,
        )
        .expect_err("pre-cancel must not spawn");
    assert_eq!(error.category(), CloudDiffReadErrorCategory::Canceled);
}

proptest! {
    #[test]
    fn cloud_diff_runner_is_chunk_partition_invariant(
        bytes in prop::collection::vec(
            prop_oneof![Just(b'a'), Just(b' '), Just(b'\n'), Just(b'\t'), Just(b'+')],
            0..2048,
        ),
        chunk_sizes in prop::collection::vec(1_usize..128, 1..32),
    ) {
        let expected = decode_cloud_diff(&CloudCapture::new(
            bytes.clone(),
            Vec::new(),
            false,
            false,
            Some(0),
        ));
        let capture = SharedCapture::with_limit(2 * 1024 * 1024);
        let mut offset = 0;
        for size in chunk_sizes.iter().cycle() {
            if offset == bytes.len() {
                break;
            }
            let end = (offset + size).min(bytes.len());
            capture.push(&bytes[offset..end]);
            offset = end;
        }
        let (partitioned, overflow) = capture.snapshot();
        let actual = decode_cloud_diff(&CloudCapture::new(
            partitioned,
            Vec::new(),
            overflow,
            false,
            Some(0),
        ));
        prop_assert_eq!(actual, expected);
    }
}

#[test]
fn cloud_diff_runner_errors_and_debug_are_redacted() {
    let canaries = [
        TASK_ONE,
        "https://chatgpt.com/codex/tasks/task_diff_one",
        DIFF_TEXT,
        "/private/codex-home",
        "account-secret",
        ENVIRONMENT,
    ];
    for category in [
        CloudDiffReadErrorCategory::IneligibleLifecycle,
        CloudDiffReadErrorCategory::AuthorityMismatch,
        CloudDiffReadErrorCategory::Scope,
        CloudDiffReadErrorCategory::Busy,
        CloudDiffReadErrorCategory::Version,
        CloudDiffReadErrorCategory::DiagnosticBoundary,
        CloudDiffReadErrorCategory::Process,
        CloudDiffReadErrorCategory::Timeout,
        CloudDiffReadErrorCategory::Canceled,
        CloudDiffReadErrorCategory::OutputLimit,
        CloudDiffReadErrorCategory::ProviderDrift,
        CloudDiffReadErrorCategory::InvalidDiff,
    ] {
        let error = crate::CloudDiffReadError::new(category);
        let rendered = format!("{error:?} {error}");
        for canary in canaries {
            assert!(!rendered.contains(canary));
        }
    }

    let layout = TestLayout::new();
    let (orchestrator, _, _, _) = accepted_fake(
        &layout,
        CloudTaskStatus::Ready,
        false,
        [TestPlan::Complete(
            CloudCommandEnd::Exited,
            CloudCapture::new(
                b"secret-diff\0account-secret".to_vec(),
                Vec::new(),
                false,
                false,
                Some(0),
            ),
        )],
    );
    let eligible = orchestrator.diff_eligible_task().expect("eligible task");
    let rendered = format!(
        "{:?} {:?}",
        orchestrator.diff_reader(),
        orchestrator
            .diff_reader()
            .retrieve(&eligible, CloudCancellation::new())
            .expect_err("invalid diff")
    );
    assert!(!rendered.contains("secret-diff"));
    assert!(!rendered.contains("account-secret"));
    assert_eq!(
        orchestrator
            .diff_reader()
            .retrieve(&eligible, CloudCancellation::new())
            .expect("default second diff plan")
            .as_str(),
        DIFF_TEXT
    );
}

#[test]
fn cloud_diff_has_no_local_application_surface() {
    let layout = TestLayout::new();
    let marker = layout.root.join("must-not-exist");
    let malicious = format!(
        "diff --git a/../../escape b/../../escape\n--- a/../../escape\n+++ b/../../escape\n@@ -0,0 +1 @@\n+$({} )\n",
        marker.display()
    );
    let (orchestrator, state, _, _) = accepted_fake(
        &layout,
        CloudTaskStatus::Ready,
        false,
        [TestPlan::Complete(
            CloudCommandEnd::Exited,
            diff_capture(&malicious),
        )],
    );
    let eligible = orchestrator.diff_eligible_task().expect("eligible task");
    let before = layout.snapshot();
    let diff = orchestrator
        .diff_reader()
        .retrieve(&eligible, CloudCancellation::new())
        .expect("malicious bytes remain display data");
    assert_eq!(diff.as_str(), malicious);
    assert_eq!(layout.snapshot(), before);
    assert!(!marker.exists());
    let starts = &state.lock().expect("diff runtime state").starts;
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].args[0..3], ["cloud", "diff", "--attempt=1"]);
    assert!(!starts[0].args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "apply" | "exec" | "checkout" | "git" | "sh" | "bash"
        )
    }));
}
