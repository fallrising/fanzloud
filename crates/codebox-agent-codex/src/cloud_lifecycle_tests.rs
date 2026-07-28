use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use serde_json::json;

use crate::cloud_lifecycle::LifecycleCloudRunner;
use crate::cloud_lifecycle_ledger::{
    CLOUD_LIFECYCLE_FILE_NAME, CloudLifecycleLedger, CloudLifecyclePhase, load_lifecycle_ledger,
    persist_lifecycle_ledger,
};
use crate::cloud_runner_types::{CloudSubmitObservation, CloudUnknownResolution};
use crate::scope::OwnedCredentialScopeLease;
use crate::{
    CloudBranch, CloudCancellation, CloudCapture, CloudDiff, CloudDiffReadError,
    CloudDiffReadErrorCategory, CloudEnvironmentId, CloudLifecycle, CloudLifecycleErrorCategory,
    CloudPrompt, CloudReconciliation, CloudRunnerConfig, CloudRunnerError,
    CloudRunnerErrorCategory, CloudSubmission, CloudSubmitOperationId, CloudSubmitRequest,
    CloudTaskId, CloudTaskOrchestrator, CloudTaskStatus, CloudTaskSummary, CredentialScope,
    CredentialScopeConfig, CredentialScopeError, DiffEligibleCloudTask,
    DuplicateRiskAcknowledgement, UnknownSubmitDecision, decode_cloud_list,
};

const ENVIRONMENT: &str = "env_lifecycle";
const BRANCH: &str = "main";
const TASK_ONE: &str = "task_lifecycle_one";
const TASK_TWO: &str = "task_lifecycle_two";
const TASK_THREE: &str = "task_lifecycle_three";

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
            "codebox-cloud-lifecycle-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("create lifecycle test root");
        set_mode(&root, 0o700);
        let executable = root.join("codex-native");
        fs::write(&executable, b"trusted lifecycle test executable")
            .expect("write lifecycle executable");
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
        .expect("valid lifecycle scope")
    }

    fn config(&self) -> CloudRunnerConfig {
        CloudRunnerConfig::new(self.scope(), environment(), branch())
    }

    fn fake_runner(&self) -> Arc<FakeRunner> {
        Arc::new(FakeRunner::new(self.scope(), self.state_dir.clone()))
    }

    fn persist_lifecycle(&self, ledger: &CloudLifecycleLedger) {
        persist_lifecycle_ledger(&self.state_dir, runner_uid(), ledger)
            .expect("persist lifecycle test ledger");
    }

    fn lifecycle_bytes(&self) -> Vec<u8> {
        fs::read(self.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME)).expect("read lifecycle bytes")
    }

    fn install_cancel_cli(&self) -> (PathBuf, PathBuf) {
        let entered = self.root.join("submit-entered");
        let pid_file = self.root.join("submit-pid");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = '--version' ]; then\n\
               printf 'codex-cli 0.145.0\\n'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = '-c' ]; then\n\
               printf 'Logged in using ChatGPT\\n' >&2\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = 'cloud' ] && [ \"$2\" = 'exec' ]; then\n\
               printf '%s\\n' \"$$\" > '{pid_file}'\n\
               : > '{entered}'\n\
               while :; do sleep 1; done\n\
             fi\n\
             if [ \"$1\" = 'cloud' ] && [ \"$2\" = 'list' ]; then\n\
               printf '%s\\n' '{{\"cursor\":null,\"tasks\":[]}}'\n\
               exit 0\n\
             fi\n\
             exit 9\n",
            entered = entered.display(),
            pid_file = pid_file.display(),
        );
        fs::write(&self.executable, script).expect("install lifecycle cancellation CLI");
        set_mode(&self.executable, 0o700);
        (entered, pid_file)
    }
}

impl Drop for TestLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

enum SubmitBehavior {
    Success(CloudTaskId),
    OutcomeUnknown,
    WaitForRelease(CloudTaskId),
    WaitForCancellation {
        after_authorization: bool,
    },
    Conflict {
        operation_id: CloudSubmitOperationId,
        fail_restore: bool,
    },
}

struct ReconciliationResult {
    operation_id: Option<CloudSubmitOperationId>,
    tasks: Vec<CloudTaskSummary>,
    complete: bool,
}

struct FakeRunnerState {
    observation: Option<Result<CloudSubmitObservation, CloudRunnerError>>,
    submit_behaviors: VecDeque<SubmitBehavior>,
    statuses: VecDeque<Result<CloudTaskStatus, CloudRunnerError>>,
    reconciliations: VecDeque<ReconciliationResult>,
    last_operation: Option<CloudSubmitOperationId>,
    submit_count: usize,
    status_count: usize,
    reconciliation_count: usize,
    resolution_count: usize,
    submit_entered: bool,
    release_submit: bool,
    fail_next_acquire: bool,
    resolution_saw_upper_unknown: bool,
}

struct FakeRunner {
    scope: CredentialScope,
    state_dir: PathBuf,
    state: Mutex<FakeRunnerState>,
    changed: Condvar,
}

impl FakeRunner {
    fn new(scope: CredentialScope, state_dir: PathBuf) -> Self {
        Self {
            scope,
            state_dir,
            state: Mutex::new(FakeRunnerState {
                observation: None,
                submit_behaviors: VecDeque::new(),
                statuses: VecDeque::new(),
                reconciliations: VecDeque::new(),
                last_operation: None,
                submit_count: 0,
                status_count: 0,
                reconciliation_count: 0,
                resolution_count: 0,
                submit_entered: false,
                release_submit: false,
                fail_next_acquire: false,
                resolution_saw_upper_unknown: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn set_observation(&self, observation: Result<CloudSubmitObservation, CloudRunnerError>) {
        self.state.lock().expect("fake runner state").observation = Some(observation);
    }

    fn push_submit(&self, behavior: SubmitBehavior) {
        self.state
            .lock()
            .expect("fake runner state")
            .submit_behaviors
            .push_back(behavior);
    }

    fn push_status(&self, status: CloudTaskStatus) {
        self.state
            .lock()
            .expect("fake runner state")
            .statuses
            .push_back(Ok(status));
    }

    fn push_reconciliation(
        &self,
        operation_id: Option<CloudSubmitOperationId>,
        task_ids: &[&str],
        complete: bool,
    ) {
        self.state
            .lock()
            .expect("fake runner state")
            .reconciliations
            .push_back(ReconciliationResult {
                operation_id,
                tasks: task_summaries(task_ids),
                complete,
            });
    }

    fn wait_until_submit_entered(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = self.state.lock().expect("fake runner state");
        while !state.submit_entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "fake submit did not start");
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .expect("wait for fake submit");
            state = next;
            assert!(!timeout.timed_out(), "fake submit did not start");
        }
    }

    fn release_submit(&self) {
        let mut state = self.state.lock().expect("fake runner state");
        state.release_submit = true;
        self.changed.notify_all();
    }

    fn counts(&self) -> (usize, usize, usize, usize) {
        let state = self.state.lock().expect("fake runner state");
        (
            state.submit_count,
            state.status_count,
            state.reconciliation_count,
            state.resolution_count,
        )
    }

    fn resolution_saw_upper_unknown(&self) -> bool {
        self.state
            .lock()
            .expect("fake runner state")
            .resolution_saw_upper_unknown
    }
}

impl LifecycleCloudRunner for FakeRunner {
    fn acquire_scope(&self) -> Result<OwnedCredentialScopeLease, CloudRunnerError> {
        {
            let mut state = self.state.lock().expect("fake runner state");
            if state.fail_next_acquire {
                state.fail_next_acquire = false;
                return Err(CloudRunnerError::new(
                    CloudRunnerErrorCategory::LedgerUnavailable,
                ));
            }
        }
        self.scope.acquire_owned().map_err(|error| {
            let category = if matches!(error, CredentialScopeError::LoginAlreadyRunning) {
                CloudRunnerErrorCategory::Busy
            } else {
                CloudRunnerErrorCategory::Scope
            };
            CloudRunnerError::new(category)
        })
    }

    fn observe_submit(
        &self,
        _operation_id: CloudSubmitOperationId,
    ) -> Result<CloudSubmitObservation, CloudRunnerError> {
        self.state
            .lock()
            .expect("fake runner state")
            .observation
            .clone()
            .unwrap_or(Ok(CloudSubmitObservation::Absent))
    }

    fn submit(
        &self,
        request: CloudSubmitRequest,
        cancellation: CloudCancellation,
    ) -> Result<CloudSubmission, CloudRunnerError> {
        let operation_id = request.operation_id();
        let behavior = {
            let mut state = self.state.lock().expect("fake runner state");
            state.submit_count += 1;
            state.last_operation = Some(operation_id);
            state.submit_entered = true;
            self.changed.notify_all();
            state
                .submit_behaviors
                .pop_front()
                .unwrap_or_else(|| SubmitBehavior::Success(task(TASK_ONE)))
        };
        match behavior {
            SubmitBehavior::Success(task_id) => Ok(CloudSubmission::new(operation_id, task_id)),
            SubmitBehavior::OutcomeUnknown => Err(CloudRunnerError::for_operation(
                CloudRunnerErrorCategory::OutcomeUnknown,
                operation_id,
            )),
            SubmitBehavior::WaitForRelease(task_id) => {
                let mut state = self.state.lock().expect("fake runner state");
                while !state.release_submit {
                    state = self.changed.wait(state).expect("wait to release submit");
                }
                state.release_submit = false;
                Ok(CloudSubmission::new(operation_id, task_id))
            }
            SubmitBehavior::WaitForCancellation {
                after_authorization,
            } => {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !cancellation.is_cancelled() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(2));
                }
                assert!(
                    cancellation.is_cancelled(),
                    "cancel signal was not delivered"
                );
                let category = if after_authorization {
                    CloudRunnerErrorCategory::OutcomeUnknown
                } else {
                    CloudRunnerErrorCategory::CanceledBeforeStart
                };
                Err(CloudRunnerError::for_operation(category, operation_id))
            }
            SubmitBehavior::Conflict {
                operation_id: current,
                fail_restore,
            } => {
                if fail_restore {
                    self.state
                        .lock()
                        .expect("fake runner state")
                        .fail_next_acquire = true;
                }
                Err(CloudRunnerError::for_operation(
                    CloudRunnerErrorCategory::OutcomeUnknown,
                    current,
                ))
            }
        }
    }

    fn status(&self, _task_id: &CloudTaskId) -> Result<CloudTaskStatus, CloudRunnerError> {
        let mut state = self.state.lock().expect("fake runner state");
        state.status_count += 1;
        state
            .statuses
            .pop_front()
            .unwrap_or(Ok(CloudTaskStatus::Pending))
    }

    fn reconcile_unknown(&self) -> Result<CloudReconciliation, CloudRunnerError> {
        let mut state = self.state.lock().expect("fake runner state");
        state.reconciliation_count += 1;
        let result = state
            .reconciliations
            .pop_front()
            .unwrap_or(ReconciliationResult {
                operation_id: None,
                tasks: Vec::new(),
                complete: true,
            });
        let operation_id = result
            .operation_id
            .or(state.last_operation)
            .expect("fake reconciliation operation");
        Ok(CloudReconciliation::new(
            operation_id,
            result.tasks,
            result.complete,
        ))
    }

    fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        resolution: CloudUnknownResolution,
    ) -> Result<CloudSubmitObservation, CloudRunnerError> {
        let upper = fs::read_to_string(self.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME))
            .expect("read upper lifecycle during lower resolution");
        let mut state = self.state.lock().expect("fake runner state");
        state.resolution_count += 1;
        state.resolution_saw_upper_unknown = upper.contains("outcomeUnknown");
        match resolution {
            CloudUnknownResolution::AdoptListedTask(task_id) => Ok(
                CloudSubmitObservation::TaskRecorded(CloudSubmission::new(operation_id, task_id)),
            ),
            CloudUnknownResolution::ExplicitlyAbandon => {
                Ok(CloudSubmitObservation::ExplicitlyAbandoned)
            }
        }
    }

    fn retrieve_diff(
        &self,
        _task: &DiffEligibleCloudTask,
        _cancellation: CloudCancellation,
    ) -> Result<CloudDiff, CloudDiffReadError> {
        Err(CloudDiffReadError::new(CloudDiffReadErrorCategory::Process))
    }
}

fn private_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).expect("create lifecycle private directory");
    set_mode(&path, 0o700);
    path
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set lifecycle test mode");
}

fn runner_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and dereferences no pointers.
    unsafe { libc::geteuid() }
}

fn environment() -> CloudEnvironmentId {
    CloudEnvironmentId::try_new(ENVIRONMENT).expect("valid lifecycle environment")
}

fn branch() -> CloudBranch {
    CloudBranch::try_new(BRANCH).expect("valid lifecycle branch")
}

fn task(value: &str) -> CloudTaskId {
    CloudTaskId::try_new(value).expect("valid lifecycle task")
}

fn prompt(value: &str) -> CloudPrompt {
    CloudPrompt::try_new(value).expect("valid lifecycle prompt")
}

fn task_summaries(task_ids: &[&str]) -> Vec<CloudTaskSummary> {
    let tasks: Vec<_> = task_ids
        .iter()
        .enumerate()
        .map(|(index, task_id)| {
            json!({
                "attempt_total": 1,
                "environment_id": ENVIRONMENT,
                "environment_label": "private-environment",
                "id": task_id,
                "is_review": false,
                "status": "pending",
                "summary": {
                    "files_changed": 1,
                    "lines_added": 2,
                    "lines_removed": 0
                },
                "title": format!("sensitive-provider-title-{index}"),
                "updated_at": "2026-07-28T12:00:00Z",
                "url": format!("https://chatgpt.com/codex/tasks/{task_id}")
            })
        })
        .collect();
    let capture = CloudCapture::new(
        serde_json::to_vec(&json!({"cursor": null, "tasks": tasks}))
            .expect("serialize fake reconciliation"),
        Vec::new(),
        false,
        false,
        Some(0),
    );
    decode_cloud_list(&capture)
        .expect("decode fake reconciliation")
        .tasks()
        .to_vec()
}

fn unknown_orchestrator(
    layout: &TestLayout,
) -> (
    Arc<FakeRunner>,
    CloudTaskOrchestrator,
    CloudSubmitOperationId,
) {
    let runner = layout.fake_runner();
    runner.push_submit(SubmitBehavior::OutcomeUnknown);
    let orchestrator =
        CloudTaskOrchestrator::with_runner(runner.clone()).expect("construct fake orchestrator");
    let lifecycle = orchestrator
        .start(prompt("unknown lifecycle operation"))
        .expect("project unknown submit");
    let operation_id = lifecycle.operation_id();
    assert!(matches!(lifecycle, CloudLifecycle::OutcomeUnknown { .. }));
    (runner, orchestrator, operation_id)
}

fn pending_ledger(
    operation_id: CloudSubmitOperationId,
    task_id: &CloudTaskId,
) -> CloudLifecycleLedger {
    let mut ledger = CloudLifecycleLedger::submitting(operation_id);
    ledger
        .record_status(task_id, CloudTaskStatus::Pending)
        .expect("record pending lifecycle");
    ledger
}

fn terminal_ledger(operation_id: CloudSubmitOperationId) -> CloudLifecycleLedger {
    let mut ledger = CloudLifecycleLedger::submitting(operation_id);
    ledger
        .record_failed_before_submit()
        .expect("record terminal lifecycle");
    ledger
}

fn assert_task_state(
    lifecycle: &CloudLifecycle,
    operation_id: CloudSubmitOperationId,
    task_id: &CloudTaskId,
    expected: CloudTaskStatus,
) {
    let matches = match (expected, lifecycle) {
        (
            CloudTaskStatus::Pending,
            CloudLifecycle::Pending {
                operation_id: actual_operation,
                task_id: actual_task,
            },
        )
        | (
            CloudTaskStatus::Ready,
            CloudLifecycle::Ready {
                operation_id: actual_operation,
                task_id: actual_task,
            },
        )
        | (
            CloudTaskStatus::Applied,
            CloudLifecycle::Applied {
                operation_id: actual_operation,
                task_id: actual_task,
            },
        )
        | (
            CloudTaskStatus::Error,
            CloudLifecycle::ProviderError {
                operation_id: actual_operation,
                task_id: actual_task,
            },
        ) => *actual_operation == operation_id && actual_task == task_id,
        _ => false,
    };
    assert!(matches, "unexpected lifecycle: {lifecycle:?}");
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(path.exists(), "timed out waiting for {}", path.display());
}

fn process_exists(pid: i32) -> bool {
    // SAFETY: signal zero performs an existence check and dereferences no pointers.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[test]
fn cloud_lifecycle_maps_all_pinned_statuses() {
    for status in [
        CloudTaskStatus::Pending,
        CloudTaskStatus::Ready,
        CloudTaskStatus::Applied,
        CloudTaskStatus::Error,
    ] {
        let layout = TestLayout::new();
        let runner = layout.fake_runner();
        runner.push_submit(SubmitBehavior::Success(task(TASK_ONE)));
        runner.push_status(status);
        let orchestrator =
            CloudTaskOrchestrator::with_runner(runner.clone()).expect("mapping orchestrator");
        let pending = orchestrator
            .start(prompt("map provider status"))
            .expect("submit");
        let operation_id = pending.operation_id();
        let inspected = orchestrator.inspect().expect("inspect one status");
        assert_task_state(&inspected, operation_id, &task(TASK_ONE), status);
        assert_eq!(runner.counts(), (1, 1, 0, 0));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn cloud_lifecycle_never_moves_backward(statuses in proptest::collection::vec(0u8..4, 0..24)) {
        let operation_id = CloudSubmitOperationId::new();
        let task_id = task(TASK_ONE);
        let mut ledger = pending_ledger(operation_id, &task_id);
        let mut prior_rank = 1u8;
        for value in statuses {
            if ledger.phase().is_terminal() {
                break;
            }
            let status = match value {
                0 => CloudTaskStatus::Pending,
                1 => CloudTaskStatus::Ready,
                2 => CloudTaskStatus::Applied,
                _ => CloudTaskStatus::Error,
            };
            ledger.record_status(&task_id, status).expect("valid monotonic status");
            let rank = if ledger.phase() == CloudLifecyclePhase::Pending { 1 } else { 2 };
            prop_assert!(rank >= prior_rank);
            prop_assert_eq!(ledger.operation_id(), operation_id);
            let lifecycle = ledger.lifecycle().expect("valid projected lifecycle");
            prop_assert_eq!(lifecycle.task_id(), Some(&task_id));
            prior_rank = rank;
        }
    }
}

#[test]
fn cloud_lifecycle_rejects_concurrent_start() {
    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    runner.push_submit(SubmitBehavior::WaitForRelease(task(TASK_ONE)));
    let orchestrator = Arc::new(
        CloudTaskOrchestrator::with_runner(runner.clone()).expect("concurrency orchestrator"),
    );
    let first = {
        let orchestrator = orchestrator.clone();
        thread::spawn(move || orchestrator.start(prompt("first concurrent submit")))
    };
    runner.wait_until_submit_entered();
    let error = orchestrator
        .start(prompt("second concurrent submit"))
        .expect_err("second start must fail");
    assert_eq!(
        error.category(),
        CloudLifecycleErrorCategory::TurnAlreadyRunning
    );
    assert_eq!(runner.counts().0, 1);
    runner.release_submit();
    assert!(matches!(
        first
            .join()
            .expect("join first submit")
            .expect("first submit"),
        CloudLifecycle::Pending { .. }
    ));
}

#[test]
fn cloud_disconnect_does_not_cancel_or_resubmit() {
    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    runner.push_submit(SubmitBehavior::WaitForRelease(task(TASK_ONE)));
    let orchestrator = Arc::new(
        CloudTaskOrchestrator::with_runner(runner.clone())
            .expect("submitting disconnect orchestrator"),
    );
    let start = {
        let orchestrator = orchestrator.clone();
        thread::spawn(move || orchestrator.start(prompt("disconnect during submit")))
    };
    runner.wait_until_submit_entered();
    let browser_handle = orchestrator.clone();
    drop(browser_handle);
    assert_eq!(runner.counts().0, 1);
    runner.release_submit();
    assert!(matches!(
        start
            .join()
            .expect("join disconnected submit")
            .expect("disconnected submit result"),
        CloudLifecycle::Pending { .. }
    ));
    assert_eq!(runner.counts(), (1, 0, 0, 0));

    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    let orchestrator =
        CloudTaskOrchestrator::with_runner(runner.clone()).expect("disconnect orchestrator");
    let lifecycle = orchestrator
        .start(prompt("browser may disconnect"))
        .expect("submit");
    let expected = lifecycle.clone();
    drop(lifecycle);
    drop(orchestrator);

    let restarted =
        CloudTaskOrchestrator::with_runner(runner.clone()).expect("restart after disconnect");
    assert_eq!(
        restarted.current_for_test().expect("current lifecycle"),
        Some(expected)
    );
    assert_eq!(runner.counts(), (1, 0, 0, 0));
}

#[test]
fn cloud_unknown_requires_explicit_recovery() {
    let layout = TestLayout::new();
    let (runner, orchestrator, operation_id) = unknown_orchestrator(&layout);
    for action in [
        orchestrator.start(prompt("must not retry")).map(|_| ()),
        orchestrator.inspect().map(|_| ()),
    ] {
        let error = action.expect_err("unknown operation must stay blocked");
        assert!(matches!(
            error.category(),
            CloudLifecycleErrorCategory::TurnAlreadyRunning
                | CloudLifecycleErrorCategory::OutcomeUnknown
        ));
    }
    runner.push_reconciliation(Some(operation_id), &[], true);
    orchestrator
        .reconcile_unknown()
        .expect("explicit reconciliation");
    assert!(matches!(
        orchestrator.current_for_test().expect("unknown current"),
        Some(CloudLifecycle::OutcomeUnknown { .. })
    ));
    assert_eq!(runner.counts(), (1, 0, 1, 0));
}

#[test]
fn cloud_recovery_adopts_only_recorded_candidate() {
    let layout = TestLayout::new();
    let (runner, orchestrator, operation_id) = unknown_orchestrator(&layout);
    runner.push_reconciliation(Some(operation_id), &[TASK_ONE], true);
    orchestrator
        .reconcile_unknown()
        .expect("first reconciliation");
    runner.push_reconciliation(Some(operation_id), &[TASK_TWO], true);
    orchestrator
        .reconcile_unknown()
        .expect("latest reconciliation");

    let error = orchestrator
        .resolve_unknown(
            operation_id,
            UnknownSubmitDecision::AdoptListedTask(task(TASK_ONE)),
        )
        .expect_err("stale candidate must fail");
    assert_eq!(error.category(), CloudLifecycleErrorCategory::TaskNotListed);
    assert_eq!(runner.counts(), (1, 0, 2, 0));

    runner.push_status(CloudTaskStatus::Ready);
    let adopted = orchestrator
        .resolve_unknown(
            operation_id,
            UnknownSubmitDecision::AdoptListedTask(task(TASK_TWO)),
        )
        .expect("adopt latest candidate");
    assert_task_state(
        &adopted,
        operation_id,
        &task(TASK_TWO),
        CloudTaskStatus::Ready,
    );
    assert_eq!(runner.counts(), (1, 1, 2, 1));
    let terminal_bytes = layout.lifecycle_bytes();
    assert_eq!(
        orchestrator
            .resolve_unknown(
                operation_id,
                UnknownSubmitDecision::AdoptListedTask(task(TASK_TWO)),
            )
            .expect("replay exact adoption"),
        adopted
    );
    assert_eq!(layout.lifecycle_bytes(), terminal_bytes);
    assert_eq!(runner.counts(), (1, 1, 2, 1));
}

#[test]
fn cloud_incomplete_reconciliation_does_not_infer_absence() {
    let layout = TestLayout::new();
    let (runner, orchestrator, operation_id) = unknown_orchestrator(&layout);
    runner.push_reconciliation(Some(operation_id), &[], false);
    let reconciliation = orchestrator
        .reconcile_unknown()
        .expect("incomplete reconciliation");
    assert!(!reconciliation.is_complete());
    let error = orchestrator
        .resolve_unknown(
            operation_id,
            UnknownSubmitDecision::AdoptListedTask(task(TASK_ONE)),
        )
        .expect_err("incomplete evidence cannot adopt");
    assert_eq!(error.category(), CloudLifecycleErrorCategory::TaskNotListed);
    assert!(matches!(
        orchestrator.current_for_test().expect("current unknown"),
        Some(CloudLifecycle::OutcomeUnknown { .. })
    ));
}

#[test]
fn cloud_abandon_requires_reconciliation_and_duplicate_risk_ack() {
    let layout = TestLayout::new();
    let (runner, orchestrator, operation_id) = unknown_orchestrator(&layout);
    let acknowledgement = DuplicateRiskAcknowledgement::for_operation(operation_id);
    let error = orchestrator
        .resolve_unknown(
            operation_id,
            UnknownSubmitDecision::AbandonAfterReconciliation(acknowledgement.clone()),
        )
        .expect_err("abandonment requires reconciliation");
    assert_eq!(error.category(), CloudLifecycleErrorCategory::WrongState);

    runner.push_reconciliation(Some(operation_id), &[], false);
    orchestrator
        .reconcile_unknown()
        .expect("record reconciliation");
    let error = orchestrator
        .resolve_unknown(
            operation_id,
            UnknownSubmitDecision::AbandonAfterReconciliation(
                DuplicateRiskAcknowledgement::for_operation(CloudSubmitOperationId::new()),
            ),
        )
        .expect_err("mismatched acknowledgement must fail");
    assert_eq!(
        error.category(),
        CloudLifecycleErrorCategory::AcknowledgementRequired
    );

    let abandoned = orchestrator
        .resolve_unknown(
            operation_id,
            UnknownSubmitDecision::AbandonAfterReconciliation(acknowledgement.clone()),
        )
        .expect("explicitly abandon");
    assert_eq!(abandoned, CloudLifecycle::AbandonedUnknown { operation_id });
    assert_eq!(runner.counts().3, 1);
    let terminal_bytes = layout.lifecycle_bytes();
    assert_eq!(
        orchestrator
            .resolve_unknown(
                operation_id,
                UnknownSubmitDecision::AbandonAfterReconciliation(acknowledgement),
            )
            .expect("replay exact abandonment"),
        abandoned
    );
    assert_eq!(layout.lifecycle_bytes(), terminal_bytes);
    assert_eq!(runner.counts().3, 1);
}

#[test]
fn cloud_cancel_during_submit_reaps_and_reconciles() {
    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    runner.push_submit(SubmitBehavior::WaitForCancellation {
        after_authorization: false,
    });
    let orchestrator = Arc::new(
        CloudTaskOrchestrator::with_runner(runner.clone())
            .expect("pre-authorization cancel orchestrator"),
    );
    let start = {
        let orchestrator = orchestrator.clone();
        thread::spawn(move || orchestrator.start(prompt("cancel before authorization")))
    };
    runner.wait_until_submit_entered();
    let canceled = orchestrator.cancel().expect("cancel before authorization");
    let started = start
        .join()
        .expect("join pre-start cancel")
        .expect("start result");
    assert_eq!(canceled, started);
    assert!(matches!(
        canceled,
        CloudLifecycle::CanceledLocally {
            task_id: None,
            provider_may_continue: false,
            ..
        }
    ));

    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    runner.push_submit(SubmitBehavior::WaitForCancellation {
        after_authorization: true,
    });
    let orchestrator = Arc::new(
        CloudTaskOrchestrator::with_runner(runner.clone())
            .expect("post-authorization cancel orchestrator"),
    );
    let start = {
        let orchestrator = orchestrator.clone();
        thread::spawn(move || orchestrator.start(prompt("cancel after authorization")))
    };
    runner.wait_until_submit_entered();
    let canceled = orchestrator.cancel().expect("cancel after authorization");
    let started = start
        .join()
        .expect("join post-authorization cancel")
        .expect("post-authorization start result");
    assert_eq!(canceled, started);
    assert!(matches!(canceled, CloudLifecycle::OutcomeUnknown { .. }));

    let layout = TestLayout::new();
    let (entered, pid_file) = layout.install_cancel_cli();
    let orchestrator =
        Arc::new(CloudTaskOrchestrator::new(layout.config()).expect("real cancel orchestrator"));
    let start = {
        let orchestrator = orchestrator.clone();
        thread::spawn(move || orchestrator.start(prompt("cancel real Cloud submit")))
    };
    wait_for_file(&entered);
    let pid: i32 = fs::read_to_string(&pid_file)
        .expect("read submit pid")
        .trim()
        .parse()
        .expect("parse submit pid");
    let canceled = orchestrator.cancel().expect("cancel submitting lifecycle");
    let started = start
        .join()
        .expect("join canceled submit")
        .expect("submit result");
    assert!(matches!(canceled, CloudLifecycle::OutcomeUnknown { .. }));
    assert_eq!(canceled, started);
    assert!(!process_exists(pid), "Cloud submit process must be reaped");
    let reconciliation = orchestrator
        .reconcile_unknown()
        .expect("reconcile canceled unknown");
    assert!(reconciliation.is_complete());
    assert!(reconciliation.tasks().is_empty());
}

#[test]
fn cloud_cancel_does_not_claim_provider_termination() {
    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    let orchestrator =
        CloudTaskOrchestrator::with_runner(runner.clone()).expect("known cancel orchestrator");
    let pending = orchestrator
        .start(prompt("known provider task"))
        .expect("submit");
    let canceled = orchestrator.cancel().expect("cancel local monitoring");
    assert_eq!(canceled.operation_id(), pending.operation_id());
    assert_eq!(canceled.task_id(), Some(&task(TASK_ONE)));
    assert!(matches!(
        canceled,
        CloudLifecycle::CanceledLocally {
            provider_may_continue: true,
            ..
        }
    ));
    assert_eq!(runner.counts(), (1, 0, 0, 0));
}

#[test]
fn cloud_cancel_is_replay_idempotent() {
    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    let orchestrator =
        CloudTaskOrchestrator::with_runner(runner.clone()).expect("cancel replay orchestrator");
    orchestrator.start(prompt("cancel replay")).expect("submit");
    let first = orchestrator.cancel().expect("first cancel");
    let bytes = layout.lifecycle_bytes();
    let second = orchestrator.cancel().expect("replayed cancel");
    assert_eq!(first, second);
    assert_eq!(layout.lifecycle_bytes(), bytes);
    assert_eq!(runner.counts(), (1, 0, 0, 0));

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist_lifecycle(&terminal_ledger(operation_id));
    let orchestrator = CloudTaskOrchestrator::with_runner(layout.fake_runner())
        .expect("terminal cancel orchestrator");
    let bytes = layout.lifecycle_bytes();
    assert_eq!(
        orchestrator.cancel().expect("cancel terminal lifecycle"),
        CloudLifecycle::FailedBeforeSubmit { operation_id }
    );
    assert_eq!(layout.lifecycle_bytes(), bytes);
}

#[test]
fn cloud_lifecycle_errors_and_debug_are_redacted() {
    let layout = TestLayout::new();
    let (runner, orchestrator, operation_id) = unknown_orchestrator(&layout);
    runner.push_reconciliation(Some(operation_id), &[TASK_ONE], true);
    let reconciliation = orchestrator.reconcile_unknown().expect("reconcile");
    let prompt_secret = "unknown lifecycle operation";
    let provider_title = "sensitive-provider-title-0";
    let rejected_task = task(TASK_THREE);
    let error = orchestrator
        .resolve_unknown(
            operation_id,
            UnknownSubmitDecision::AdoptListedTask(rejected_task.clone()),
        )
        .expect_err("unlisted task must fail");
    let rendered = format!("{error:?} {error} {reconciliation:?}");
    assert!(!rendered.contains(rejected_task.as_str()));
    assert!(!rendered.contains(provider_title));
    assert!(!rendered.contains(prompt_secret));
    assert!(!rendered.contains(&layout.state_dir.to_string_lossy().into_owned()));
    let ledger = String::from_utf8(layout.lifecycle_bytes()).expect("utf8 lifecycle ledger");
    assert!(!ledger.contains(prompt_secret));
    assert!(!ledger.contains(provider_title));
    assert!(!ledger.contains(&layout.state_dir.to_string_lossy().into_owned()));
}

#[test]
fn cloud_orchestrator_never_auto_resubmits_after_unknown() {
    let layout = TestLayout::new();
    let (runner, orchestrator, operation_id) = unknown_orchestrator(&layout);
    assert_eq!(runner.counts().0, 1);
    assert!(orchestrator.start(prompt("blocked retry")).is_err());
    assert!(orchestrator.inspect().is_err());
    runner.push_reconciliation(Some(operation_id), &[], true);
    orchestrator.reconcile_unknown().expect("reconcile");
    assert!(orchestrator.start(prompt("still blocked")).is_err());
    assert_eq!(runner.counts().0, 1);

    orchestrator
        .resolve_unknown(
            operation_id,
            UnknownSubmitDecision::AbandonAfterReconciliation(
                DuplicateRiskAcknowledgement::for_operation(operation_id),
            ),
        )
        .expect("explicit abandon");
    runner.push_submit(SubmitBehavior::Success(task(TASK_TWO)));
    let next = orchestrator
        .start(prompt("explicit independent operation"))
        .expect("explicit later submit");
    assert_ne!(next.operation_id(), operation_id);
    assert_eq!(runner.counts().0, 2);
}

#[test]
fn cloud_lifecycle_ledger_fails_closed() {
    fn assert_invalid(layout: &TestLayout) {
        let error = CloudTaskOrchestrator::with_runner(layout.fake_runner())
            .expect_err("unsafe lifecycle ledger must fail");
        assert_eq!(error.category(), CloudLifecycleErrorCategory::LedgerInvalid);
    }

    let layout = TestLayout::new();
    fs::write(
        layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME),
        b"{not-json",
    )
    .expect("write malformed lifecycle");
    set_mode(&layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME), 0o600);
    assert_invalid(&layout);

    let layout = TestLayout::new();
    fs::write(
        layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME),
        vec![b'x'; 64 * 1024 + 1],
    )
    .expect("write oversized lifecycle");
    set_mode(&layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME), 0o600);
    assert_invalid(&layout);

    let layout = TestLayout::new();
    layout.persist_lifecycle(&terminal_ledger(CloudSubmitOperationId::new()));
    set_mode(&layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME), 0o644);
    assert_invalid(&layout);

    let layout = TestLayout::new();
    layout.persist_lifecycle(&terminal_ledger(CloudSubmitOperationId::new()));
    fs::hard_link(
        layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME),
        layout.state_dir.join("lifecycle-hardlink"),
    )
    .expect("hard link lifecycle");
    assert_invalid(&layout);

    let layout = TestLayout::new();
    let target = layout.state_dir.join("lifecycle-target");
    fs::write(&target, b"{}").expect("write lifecycle symlink target");
    symlink(&target, layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME)).expect("symlink lifecycle");
    assert_invalid(&layout);

    let layout = TestLayout::new();
    layout.persist_lifecycle(&pending_ledger(
        CloudSubmitOperationId::new(),
        &task(TASK_ONE),
    ));
    let path = layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read lifecycle")).expect("parse lifecycle");
    value["schemaVersion"] = json!(99);
    fs::write(
        &path,
        serde_json::to_vec(&value).expect("serialize invalid version"),
    )
    .expect("write invalid version");
    set_mode(&path, 0o600);
    assert_invalid(&layout);

    let layout = TestLayout::new();
    layout.persist_lifecycle(&pending_ledger(
        CloudSubmitOperationId::new(),
        &task(TASK_ONE),
    ));
    let path = layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read history lifecycle"))
            .expect("parse history lifecycle");
    let mut history = vec![json!("submitting")];
    history.extend((0..32).map(|_| json!("pending")));
    value["history"] = serde_json::Value::Array(history);
    fs::write(
        &path,
        serde_json::to_vec(&value).expect("serialize oversized history"),
    )
    .expect("write oversized history");
    set_mode(&path, 0o600);
    assert_invalid(&layout);

    let mut ledger = CloudLifecycleLedger::submitting(CloudSubmitOperationId::new());
    ledger
        .record_outcome_unknown()
        .expect("record candidate-bound unknown");
    let candidates: Vec<_> = (0..101)
        .map(|index| task(&format!("task_candidate_{index}")))
        .collect();
    assert_eq!(
        ledger
            .record_reconciliation(&candidates, true)
            .expect_err("candidate bound must fail")
            .category(),
        CloudLifecycleErrorCategory::LedgerInvalid
    );
}

#[test]
fn cloud_submitting_restart_observes_lower_disposition() {
    enum Expected {
        Failed,
        Unknown,
        Pending,
        Abandoned,
        Conflict,
    }

    for (local_unknown, observation, expected) in [
        (false, CloudSubmitObservation::Absent, Expected::Failed),
        (
            false,
            CloudSubmitObservation::FailedBeforeSpawn,
            Expected::Failed,
        ),
        (
            false,
            CloudSubmitObservation::OutcomeUnknown,
            Expected::Unknown,
        ),
        (
            false,
            CloudSubmitObservation::TaskRecorded(CloudSubmission::new(
                CloudSubmitOperationId::new(),
                task(TASK_ONE),
            )),
            Expected::Pending,
        ),
        (
            false,
            CloudSubmitObservation::ExplicitlyAbandoned,
            Expected::Abandoned,
        ),
        (
            true,
            CloudSubmitObservation::OutcomeUnknown,
            Expected::Unknown,
        ),
        (
            true,
            CloudSubmitObservation::TaskRecorded(CloudSubmission::new(
                CloudSubmitOperationId::new(),
                task(TASK_ONE),
            )),
            Expected::Pending,
        ),
        (
            true,
            CloudSubmitObservation::ExplicitlyAbandoned,
            Expected::Abandoned,
        ),
        (true, CloudSubmitObservation::Absent, Expected::Conflict),
        (
            true,
            CloudSubmitObservation::FailedBeforeSpawn,
            Expected::Conflict,
        ),
    ] {
        let layout = TestLayout::new();
        let operation_id = CloudSubmitOperationId::new();
        let mut ledger = CloudLifecycleLedger::submitting(operation_id);
        if local_unknown {
            ledger
                .record_outcome_unknown()
                .expect("record restart unknown");
        }
        layout.persist_lifecycle(&ledger);
        let before = layout.lifecycle_bytes();
        let observation = match observation {
            CloudSubmitObservation::TaskRecorded(submission) => {
                CloudSubmitObservation::TaskRecorded(CloudSubmission::new(
                    operation_id,
                    submission.task_id().clone(),
                ))
            }
            other => other,
        };
        let runner = layout.fake_runner();
        runner.set_observation(Ok(observation));
        let result = CloudTaskOrchestrator::with_runner(runner);
        match expected {
            Expected::Conflict => {
                let error = result.expect_err("restart lower conflict");
                assert_eq!(
                    error.category(),
                    CloudLifecycleErrorCategory::OperationConflict
                );
                assert_eq!(layout.lifecycle_bytes(), before);
            }
            Expected::Failed => assert!(matches!(
                result
                    .expect("repair failed state")
                    .current_for_test()
                    .expect("current"),
                Some(CloudLifecycle::FailedBeforeSubmit { .. })
            )),
            Expected::Unknown => assert!(matches!(
                result
                    .expect("repair unknown state")
                    .current_for_test()
                    .expect("current"),
                Some(CloudLifecycle::OutcomeUnknown { .. })
            )),
            Expected::Pending => assert!(matches!(
                result
                    .expect("repair pending state")
                    .current_for_test()
                    .expect("current"),
                Some(CloudLifecycle::Pending { .. })
            )),
            Expected::Abandoned => assert!(matches!(
                result
                    .expect("repair abandoned state")
                    .current_for_test()
                    .expect("current"),
                Some(CloudLifecycle::AbandonedUnknown { .. })
            )),
        }
    }

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let mut ledger = CloudLifecycleLedger::submitting(operation_id);
    ledger
        .record_outcome_unknown()
        .expect("record byte-stable restart unknown");
    layout.persist_lifecycle(&ledger);
    let before = layout.lifecycle_bytes();
    let runner = layout.fake_runner();
    runner.set_observation(Ok(CloudSubmitObservation::OutcomeUnknown));
    CloudTaskOrchestrator::with_runner(runner).expect("restart unchanged unknown");
    assert_eq!(layout.lifecycle_bytes(), before);

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist_lifecycle(&CloudLifecycleLedger::submitting(operation_id));
    let before = layout.lifecycle_bytes();
    let different_operation = CloudSubmitOperationId::new();
    let runner = layout.fake_runner();
    runner.set_observation(Ok(CloudSubmitObservation::TaskRecorded(
        CloudSubmission::new(different_operation, task(TASK_ONE)),
    )));
    let error = CloudTaskOrchestrator::with_runner(runner)
        .expect_err("mismatched lower task operation must fail");
    assert_eq!(
        error.category(),
        CloudLifecycleErrorCategory::OperationConflict
    );
    assert_eq!(error.operation_id(), Some(different_operation));
    assert_eq!(layout.lifecycle_bytes(), before);
}

#[test]
fn cloud_resolution_terminalizes_lower_submit_before_lifecycle() {
    for adopt in [false, true] {
        let layout = TestLayout::new();
        let (runner, orchestrator, operation_id) = unknown_orchestrator(&layout);
        let candidates = if adopt { vec![TASK_ONE] } else { Vec::new() };
        runner.push_reconciliation(Some(operation_id), &candidates, true);
        orchestrator
            .reconcile_unknown()
            .expect("reconcile resolution");
        let decision = if adopt {
            runner.push_status(CloudTaskStatus::Pending);
            UnknownSubmitDecision::AdoptListedTask(task(TASK_ONE))
        } else {
            UnknownSubmitDecision::AbandonAfterReconciliation(
                DuplicateRiskAcknowledgement::for_operation(operation_id),
            )
        };
        orchestrator
            .resolve_unknown(operation_id, decision)
            .expect("resolve lower before upper");
        assert!(runner.resolution_saw_upper_unknown());
    }
}

#[test]
fn cloud_start_checks_lower_readiness_before_submitting() {
    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    let orchestrator =
        CloudTaskOrchestrator::with_runner(runner.clone()).expect("idle readiness orchestrator");
    assert!(matches!(
        orchestrator
            .start(prompt("idle lower readiness"))
            .expect("start"),
        CloudLifecycle::Pending { .. }
    ));

    let layout = TestLayout::new();
    let prior_operation = CloudSubmitOperationId::new();
    layout.persist_lifecycle(&terminal_ledger(prior_operation));
    let runner = layout.fake_runner();
    let orchestrator =
        CloudTaskOrchestrator::with_runner(runner).expect("terminal readiness orchestrator");
    let next = orchestrator
        .start(prompt("replace terminal lifecycle"))
        .expect("start after terminal");
    assert_ne!(next.operation_id(), prior_operation);

    for prior_terminal in [false, true] {
        let layout = TestLayout::new();
        let before = if prior_terminal {
            layout.persist_lifecycle(&terminal_ledger(CloudSubmitOperationId::new()));
            Some(layout.lifecycle_bytes())
        } else {
            None
        };
        let runner = layout.fake_runner();
        let lower_operation = CloudSubmitOperationId::new();
        runner.set_observation(Err(CloudRunnerError::for_operation(
            CloudRunnerErrorCategory::OperationConflict,
            lower_operation,
        )));
        let orchestrator =
            CloudTaskOrchestrator::with_runner(runner.clone()).expect("conflict orchestrator");
        let error = orchestrator
            .start(prompt("blocked lower readiness"))
            .expect_err("lower unknown must block");
        assert_eq!(
            error.category(),
            CloudLifecycleErrorCategory::OperationConflict
        );
        assert_eq!(error.operation_id(), Some(lower_operation));
        assert_eq!(runner.counts().0, 0);
        match before {
            Some(bytes) => assert_eq!(layout.lifecycle_bytes(), bytes),
            None => assert!(!layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME).exists()),
        }
    }
}

#[test]
fn cloud_start_restores_prior_lifecycle_on_lower_conflict() {
    for prior_terminal in [false, true] {
        let layout = TestLayout::new();
        let prior = if prior_terminal {
            let ledger = terminal_ledger(CloudSubmitOperationId::new());
            layout.persist_lifecycle(&ledger);
            Some(layout.lifecycle_bytes())
        } else {
            None
        };
        let runner = layout.fake_runner();
        runner.push_submit(SubmitBehavior::Conflict {
            operation_id: CloudSubmitOperationId::new(),
            fail_restore: false,
        });
        let orchestrator =
            CloudTaskOrchestrator::with_runner(runner.clone()).expect("rollback orchestrator");
        let error = orchestrator
            .start(prompt("post-gate lower conflict"))
            .expect_err("post-gate conflict must roll back");
        assert_eq!(
            error.category(),
            CloudLifecycleErrorCategory::OperationConflict
        );
        assert_eq!(runner.counts().0, 1);
        match prior {
            Some(bytes) => assert_eq!(layout.lifecycle_bytes(), bytes),
            None => assert!(!layout.state_dir.join(CLOUD_LIFECYCLE_FILE_NAME).exists()),
        }
    }

    let layout = TestLayout::new();
    let runner = layout.fake_runner();
    runner.push_submit(SubmitBehavior::Conflict {
        operation_id: CloudSubmitOperationId::new(),
        fail_restore: true,
    });
    let orchestrator =
        CloudTaskOrchestrator::with_runner(runner.clone()).expect("fault rollback orchestrator");
    let error = orchestrator
        .start(prompt("rollback storage fault"))
        .expect_err("failed rollback requires repair");
    assert_eq!(
        error.category(),
        CloudLifecycleErrorCategory::RecoveryRequired
    );
    let ledger = load_lifecycle_ledger(&layout.state_dir, runner_uid())
        .expect("load orphan submitting")
        .expect("orphan submitting exists");
    assert_eq!(ledger.phase(), CloudLifecyclePhase::Submitting);
    assert_eq!(runner.counts().0, 1);
}
