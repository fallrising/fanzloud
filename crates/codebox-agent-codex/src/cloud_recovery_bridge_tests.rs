use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use crate::cloud_runner_types::{CloudSubmitObservation, CloudUnknownResolution};
use crate::cloud_submit_ledger::{
    CLOUD_LEDGER_FILE_NAME, CloudLedgerPhase, CloudSubmitLedger, load_cloud_ledger,
    persist_cloud_ledger,
};
use crate::ledger::{ProcessIdentity, read_process_identity};
use crate::{
    CloudBranch, CloudCancellation, CloudEnvironmentId, CloudPrompt, CloudRunnerConfig,
    CloudRunnerErrorCategory, CloudSubmitOperationId, CloudSubmitRequest, CloudTaskId,
    CloudTaskRunner, CredentialScope, CredentialScopeConfig,
};

const ENVIRONMENT: &str = "env_bridge";
const BRANCH: &str = "main";
const TASK_ONE: &str = "task_bridge_one";
const TASK_TWO: &str = "task_bridge_two";
const NEW_TASK: &str = "task_bridge_new";
const NEW_TASK_URL: &str = "https://chatgpt.com/codex/tasks/task_bridge_new";

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
            "codebox-cloud-recovery-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("create bridge test root");
        set_mode(&root, 0o700);
        let executable = root.join("codex-native");
        fs::write(&executable, b"trusted bridge test executable")
            .expect("write bridge test executable");
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

    fn config(&self) -> CloudRunnerConfig {
        CloudRunnerConfig::new(
            CredentialScope::validate(CredentialScopeConfig::new(
                self.executable.clone(),
                self.codex_home.clone(),
                self.state_dir.clone(),
                self.working_dir.clone(),
            ))
            .expect("valid bridge test scope"),
            environment(),
            branch(),
        )
    }

    fn persist(&self, ledger: &CloudSubmitLedger) {
        persist_cloud_ledger(&self.state_dir, runner_uid(), ledger).expect("persist bridge ledger");
    }

    fn load(&self) -> CloudSubmitLedger {
        load_cloud_ledger(&self.state_dir, runner_uid())
            .expect("load bridge ledger")
            .expect("bridge ledger exists")
    }

    fn install_fake_cli(&self) -> PathBuf {
        let exec_count = self.root.join("exec-count.txt");
        let script = format!(
            "#!/bin/sh\n\
             exec_count='{exec_count}'\n\
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
               printf '{NEW_TASK_URL}\\n'\n\
               exit 0\n\
             fi\n\
             exit 9\n",
            exec_count = exec_count.display(),
        );
        fs::write(&self.executable, script).expect("install bridge fake CLI");
        set_mode(&self.executable, 0o700);
        exec_count
    }
}

impl Drop for TestLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn private_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).expect("create private bridge directory");
    set_mode(&path, 0o700);
    path
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set bridge test mode");
}

fn runner_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and dereferences no pointers.
    unsafe { libc::geteuid() }
}

fn environment() -> CloudEnvironmentId {
    CloudEnvironmentId::try_new(ENVIRONMENT).expect("valid bridge environment")
}

fn branch() -> CloudBranch {
    CloudBranch::try_new(BRANCH).expect("valid bridge branch")
}

fn task(value: &str) -> CloudTaskId {
    CloudTaskId::try_new(value).expect("valid bridge task")
}

fn process_identity() -> ProcessIdentity {
    read_process_identity(std::process::id()).expect("read bridge process identity")
}

fn unknown_ledger(operation_id: CloudSubmitOperationId) -> CloudSubmitLedger {
    let mut ledger = CloudSubmitLedger::new(operation_id, &environment(), &branch());
    ledger
        .append(CloudLedgerPhase::Authorized)
        .expect("authorize bridge ledger");
    ledger
        .append(CloudLedgerPhase::OutcomeUnknown)
        .expect("mark bridge ledger unknown");
    ledger
}

fn reconciled_ledger(
    operation_id: CloudSubmitOperationId,
    candidates: &[CloudTaskId],
    complete: bool,
) -> CloudSubmitLedger {
    let mut ledger = unknown_ledger(operation_id);
    ledger
        .record_reconciliation(candidates, complete)
        .expect("record bridge reconciliation");
    ledger
}

fn recorded_ledger(
    operation_id: CloudSubmitOperationId,
    task_id: &CloudTaskId,
) -> CloudSubmitLedger {
    let mut ledger = CloudSubmitLedger::new(operation_id, &environment(), &branch());
    ledger
        .append(CloudLedgerPhase::Authorized)
        .expect("authorize recorded bridge ledger");
    ledger
        .record_started(process_identity())
        .expect("start recorded bridge ledger");
    ledger.record_task(task_id).expect("record bridge task");
    ledger
}

fn assert_task_observation(observation: CloudSubmitObservation, expected: &CloudTaskId) {
    match observation {
        CloudSubmitObservation::TaskRecorded(submission) => {
            assert_eq!(submission.task_id(), expected);
        }
        other => panic!("expected task observation, got {other:?}"),
    }
}

#[test]
fn cloud_submit_observation_recovers_every_durable_phase() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
    assert_eq!(
        runner
            .observe_submit(operation_id)
            .expect("observe absent submit"),
        CloudSubmitObservation::Absent
    );

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist(&CloudSubmitLedger::new(
        operation_id,
        &environment(),
        &branch(),
    ));
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
    assert_eq!(
        runner.observe_submit(operation_id).expect("recover intent"),
        CloudSubmitObservation::FailedBeforeSpawn
    );
    assert_eq!(layout.load().latest(), CloudLedgerPhase::FailedBeforeSpawn);

    for started in [false, true] {
        let layout = TestLayout::new();
        let operation_id = CloudSubmitOperationId::new();
        let mut ledger = CloudSubmitLedger::new(operation_id, &environment(), &branch());
        ledger
            .append(CloudLedgerPhase::Authorized)
            .expect("authorize matrix ledger");
        if started {
            ledger
                .record_started(process_identity())
                .expect("start matrix ledger");
        }
        layout.persist(&ledger);
        let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
        assert_eq!(
            runner
                .observe_submit(operation_id)
                .expect("recover ambiguous submit"),
            CloudSubmitObservation::OutcomeUnknown
        );
        assert_eq!(layout.load().latest(), CloudLedgerPhase::OutcomeUnknown);
    }

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let task_id = task(TASK_ONE);
    layout.persist(&recorded_ledger(operation_id, &task_id));
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
    assert_task_observation(
        runner
            .observe_submit(operation_id)
            .expect("observe recorded task"),
        &task_id,
    );

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let mut failed = CloudSubmitLedger::new(operation_id, &environment(), &branch());
    failed
        .append(CloudLedgerPhase::FailedBeforeSpawn)
        .expect("fail bridge ledger");
    layout.persist(&failed);
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
    assert_eq!(
        runner
            .observe_submit(operation_id)
            .expect("observe failed submit"),
        CloudSubmitObservation::FailedBeforeSpawn
    );

    for reconciled in [false, true] {
        let layout = TestLayout::new();
        let operation_id = CloudSubmitOperationId::new();
        let ledger = if reconciled {
            reconciled_ledger(operation_id, &[task(TASK_ONE)], true)
        } else {
            unknown_ledger(operation_id)
        };
        layout.persist(&ledger);
        let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
        assert_eq!(
            runner
                .observe_submit(operation_id)
                .expect("observe unknown submit"),
            CloudSubmitObservation::OutcomeUnknown
        );
    }

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let task_id = task(TASK_ONE);
    let mut adopted = reconciled_ledger(operation_id, std::slice::from_ref(&task_id), true);
    adopted
        .record_adopted_task(&task_id)
        .expect("adopt bridge task");
    layout.persist(&adopted);
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
    assert_task_observation(
        runner
            .observe_submit(operation_id)
            .expect("observe adopted task"),
        &task_id,
    );

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let mut abandoned = reconciled_ledger(operation_id, &[], false);
    abandoned
        .record_explicitly_abandoned()
        .expect("abandon bridge submit");
    layout.persist(&abandoned);
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
    assert_eq!(
        runner
            .observe_submit(operation_id)
            .expect("observe abandoned submit"),
        CloudSubmitObservation::ExplicitlyAbandoned
    );
}

#[test]
fn cloud_submit_observation_never_executes_cli() {
    let layout = TestLayout::new();
    let marker = layout.root.join("cli-executed");
    fs::write(
        &layout.executable,
        format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()),
    )
    .expect("install observation canary CLI");
    set_mode(&layout.executable, 0o700);
    let operation_id = CloudSubmitOperationId::new();
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");

    assert_eq!(
        runner
            .observe_submit(operation_id)
            .expect("observe absent submit"),
        CloudSubmitObservation::Absent
    );

    layout.persist(&reconciled_ledger(operation_id, &[], false));
    assert_eq!(
        runner
            .observe_submit(operation_id)
            .expect("observe reconciled submit"),
        CloudSubmitObservation::OutcomeUnknown
    );
    assert_eq!(
        runner
            .resolve_unknown(operation_id, CloudUnknownResolution::ExplicitlyAbandon)
            .expect("resolve reconciled submit"),
        CloudSubmitObservation::ExplicitlyAbandoned
    );
    assert!(!marker.exists());
}

#[test]
fn cloud_submit_observation_rejects_different_unknown_operation() {
    for reconciled in [false, true] {
        let layout = TestLayout::new();
        let current = CloudSubmitOperationId::new();
        let ledger = if reconciled {
            reconciled_ledger(current, &[task(TASK_ONE)], false)
        } else {
            unknown_ledger(current)
        };
        layout.persist(&ledger);
        let before = fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
            .expect("read lower ledger before conflict");
        let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");

        let error = runner
            .observe_submit(CloudSubmitOperationId::new())
            .expect_err("different unknown must block readiness");
        assert_eq!(
            error.category(),
            CloudRunnerErrorCategory::OperationConflict
        );
        assert_eq!(error.operation_id(), Some(current));
        assert_eq!(
            fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
                .expect("read lower ledger after conflict"),
            before
        );
        if reconciled {
            let error = runner
                .resolve_unknown(
                    CloudSubmitOperationId::new(),
                    CloudUnknownResolution::ExplicitlyAbandon,
                )
                .expect_err("different resolution operation must fail");
            assert_eq!(
                error.category(),
                CloudRunnerErrorCategory::OperationConflict
            );
            assert_eq!(error.operation_id(), Some(current));
            assert_eq!(
                fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
                    .expect("read lower ledger after resolution conflict"),
                before
            );
        }
    }

    let layout = TestLayout::new();
    let terminal_operation = CloudSubmitOperationId::new();
    let mut terminal = CloudSubmitLedger::new(terminal_operation, &environment(), &branch());
    terminal
        .append(CloudLedgerPhase::FailedBeforeSpawn)
        .expect("terminalize prior bridge operation");
    layout.persist(&terminal);
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
    assert_eq!(
        runner
            .observe_submit(CloudSubmitOperationId::new())
            .expect("different terminal permits a new operation"),
        CloudSubmitObservation::Absent
    );
}

#[test]
fn cloud_submit_resolution_adopts_only_latest_candidate() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let old_task = task(TASK_ONE);
    let latest_task = task(TASK_TWO);
    let mut ledger = reconciled_ledger(operation_id, std::slice::from_ref(&old_task), true);
    ledger
        .record_reconciliation(std::slice::from_ref(&latest_task), true)
        .expect("replace latest reconciliation");
    layout.persist(&ledger);
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");

    let error = runner
        .resolve_unknown(
            operation_id,
            CloudUnknownResolution::AdoptListedTask(old_task),
        )
        .expect_err("stale candidate must fail");
    assert_eq!(
        error.category(),
        CloudRunnerErrorCategory::CandidateNotRecorded
    );
    assert_eq!(
        layout.load().latest(),
        CloudLedgerPhase::ReconciliationObserved
    );

    assert_task_observation(
        runner
            .resolve_unknown(
                operation_id,
                CloudUnknownResolution::AdoptListedTask(latest_task.clone()),
            )
            .expect("adopt latest candidate"),
        &latest_task,
    );
    assert_eq!(layout.load().latest(), CloudLedgerPhase::TaskAdopted);
}

#[test]
fn cloud_submit_resolution_requires_reconciliation() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist(&unknown_ledger(operation_id));
    let before = fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
        .expect("read unresolved lower ledger");
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");

    let error = runner
        .resolve_unknown(operation_id, CloudUnknownResolution::ExplicitlyAbandon)
        .expect_err("abandonment requires reconciliation");
    assert_eq!(
        error.category(),
        CloudRunnerErrorCategory::ResolutionUnavailable
    );
    assert_eq!(
        fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
            .expect("read unchanged unresolved ledger"),
        before
    );
    let error = runner
        .resolve_unknown(
            operation_id,
            CloudUnknownResolution::AdoptListedTask(task(TASK_ONE)),
        )
        .expect_err("adoption also requires reconciliation");
    assert_eq!(
        error.category(),
        CloudRunnerErrorCategory::ResolutionUnavailable
    );
    assert_eq!(
        fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
            .expect("read unchanged unresolved ledger after adoption"),
        before
    );

    let reconciled = reconciled_ledger(operation_id, &[], false);
    layout.persist(&reconciled);
    assert_eq!(
        runner
            .resolve_unknown(operation_id, CloudUnknownResolution::ExplicitlyAbandon,)
            .expect("abandon reconciled submit"),
        CloudSubmitObservation::ExplicitlyAbandoned
    );
}

#[test]
fn cloud_submit_resolution_is_replay_idempotent_and_conflict_safe() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let adopted_task = task(TASK_ONE);
    let other_task = task(TASK_TWO);
    layout.persist(&reconciled_ledger(
        operation_id,
        &[adopted_task.clone(), other_task.clone()],
        true,
    ));
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");

    runner
        .resolve_unknown(
            operation_id,
            CloudUnknownResolution::AdoptListedTask(adopted_task.clone()),
        )
        .expect("first adoption");
    let terminal_bytes = fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
        .expect("read adopted terminal ledger");
    assert_task_observation(
        runner
            .resolve_unknown(
                operation_id,
                CloudUnknownResolution::AdoptListedTask(adopted_task.clone()),
            )
            .expect("replay exact adoption"),
        &adopted_task,
    );
    assert_eq!(
        fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
            .expect("read replayed terminal ledger"),
        terminal_bytes
    );

    for conflict in [
        CloudUnknownResolution::AdoptListedTask(other_task),
        CloudUnknownResolution::ExplicitlyAbandon,
    ] {
        let error = runner
            .resolve_unknown(operation_id, conflict)
            .expect_err("conflicting resolution must fail");
        assert_eq!(
            error.category(),
            CloudRunnerErrorCategory::ResolutionConflict
        );
    }

    let layout = TestLayout::new();
    let abandoned_operation = CloudSubmitOperationId::new();
    layout.persist(&reconciled_ledger(abandoned_operation, &[], false));
    let runner = CloudTaskRunner::new(layout.config()).expect("abandon bridge runner");
    runner
        .resolve_unknown(
            abandoned_operation,
            CloudUnknownResolution::ExplicitlyAbandon,
        )
        .expect("first abandonment");
    let terminal_bytes = fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
        .expect("read abandoned terminal ledger");
    assert_eq!(
        runner
            .resolve_unknown(
                abandoned_operation,
                CloudUnknownResolution::ExplicitlyAbandon,
            )
            .expect("replay exact abandonment"),
        CloudSubmitObservation::ExplicitlyAbandoned
    );
    assert_eq!(
        fs::read(layout.state_dir.join(CLOUD_LEDGER_FILE_NAME))
            .expect("read replayed abandonment ledger"),
        terminal_bytes
    );
}

#[test]
fn cloud_submit_resolution_terminal_allows_new_operation_without_auto_exec() {
    let layout = TestLayout::new();
    let exec_count = layout.install_fake_cli();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist(&reconciled_ledger(operation_id, &[], true));
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");

    assert_eq!(
        runner
            .resolve_unknown(operation_id, CloudUnknownResolution::ExplicitlyAbandon,)
            .expect("explicitly abandon lower submit"),
        CloudSubmitObservation::ExplicitlyAbandoned
    );
    assert!(!exec_count.exists());
    let replay_error = runner
        .submit(
            CloudSubmitRequest::new(
                operation_id,
                CloudPrompt::try_new("same operation remains observation-only")
                    .expect("valid bridge prompt"),
            ),
            CloudCancellation::new(),
        )
        .expect_err("abandoned operation cannot replay as exec");
    assert_eq!(
        replay_error.category(),
        CloudRunnerErrorCategory::PriorExplicitlyAbandoned
    );
    assert!(!exec_count.exists());

    let submission = runner
        .submit(
            CloudSubmitRequest::new(
                CloudSubmitOperationId::new(),
                CloudPrompt::try_new("explicit later operation").expect("valid bridge prompt"),
            ),
            CloudCancellation::new(),
        )
        .expect("explicit new operation is permitted");
    assert_eq!(submission.task_id().as_str(), NEW_TASK);
    assert_eq!(
        fs::read_to_string(exec_count)
            .expect("read explicit exec count")
            .trim(),
        "1"
    );
}

#[test]
fn cloud_submit_resolution_restarts_from_terminal_observation() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let task_id = task(TASK_ONE);
    layout.persist(&reconciled_ledger(
        operation_id,
        std::slice::from_ref(&task_id),
        true,
    ));
    let first = CloudTaskRunner::new(layout.config()).expect("first bridge runner");
    first
        .resolve_unknown(
            operation_id,
            CloudUnknownResolution::AdoptListedTask(task_id.clone()),
        )
        .expect("commit lower adoption");
    drop(first);

    let restarted = CloudTaskRunner::new(layout.config()).expect("restarted bridge runner");
    assert_task_observation(
        restarted
            .observe_submit(operation_id)
            .expect("observe terminal lower adoption"),
        &task_id,
    );

    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    layout.persist(&reconciled_ledger(operation_id, &[], false));
    let first = CloudTaskRunner::new(layout.config()).expect("first abandon bridge runner");
    first
        .resolve_unknown(operation_id, CloudUnknownResolution::ExplicitlyAbandon)
        .expect("commit lower abandonment");
    drop(first);

    let restarted = CloudTaskRunner::new(layout.config()).expect("restarted abandon bridge runner");
    assert_eq!(
        restarted
            .observe_submit(operation_id)
            .expect("observe terminal lower abandonment"),
        CloudSubmitObservation::ExplicitlyAbandoned
    );
}

#[test]
fn cloud_submit_resolution_ledger_fails_closed_and_redacts() {
    let layout = TestLayout::new();
    let operation_id = CloudSubmitOperationId::new();
    let task_id = task(TASK_ONE);
    layout.persist(&reconciled_ledger(
        operation_id,
        std::slice::from_ref(&task_id),
        true,
    ));
    let runner = CloudTaskRunner::new(layout.config()).expect("bridge runner");
    runner
        .resolve_unknown(
            operation_id,
            CloudUnknownResolution::AdoptListedTask(task_id),
        )
        .expect("commit valid adopted ledger");

    let ledger_path = layout.state_dir.join(CLOUD_LEDGER_FILE_NAME);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).expect("read adopted ledger"))
            .expect("parse adopted ledger");
    value["candidateTaskIds"] = json!(["task_unrelated"]);
    fs::write(
        &ledger_path,
        serde_json::to_vec(&value).expect("serialize impossible adopted ledger"),
    )
    .expect("write impossible adopted ledger");
    set_mode(&ledger_path, 0o600);

    let error = runner
        .observe_submit(operation_id)
        .expect_err("impossible adopted ledger must fail closed");
    assert_eq!(error.category(), CloudRunnerErrorCategory::LedgerInvalid);
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(TASK_ONE));
    assert!(!rendered.contains("task_unrelated"));
    assert!(!rendered.contains(&layout.state_dir.to_string_lossy().into_owned()));
}
