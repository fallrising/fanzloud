use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::ledger::{LedgerPhase, LoginLedger, ProcessIdentity, load_ledger, persist_ledger};
use crate::parser::{
    CapturedOutput, MAX_CAPTURE_BYTES, PinnedStatus, PromptParseError, parse_device_prompt,
    parse_login_completion, parse_status, parse_version,
};
use crate::runtime::test_support::{
    drain_more_than_limit, mismatched_parent_fails_spawn, parent_death_kills_child,
};
use crate::runtime::{CliRuntime, LoginSupervisor, SupervisorEvent};
use crate::scope::OwnedCredentialScopeLease;
use crate::{
    CodexCommand, CredentialScope, CredentialScopeConfig, LoginBroker, LoginBrokerError,
    LoginOperationId, LoginStatus, VerificationCode,
};

const DEVICE_PROMPT_FIXTURE: &str =
    include_str!("../../../docs/fixtures/codex-0.145.0/login/device-login.stdout");
const STATUS_CHATGPT_FIXTURE: &str =
    include_str!("../../../docs/fixtures/codex-0.145.0/login/login-status.chatgpt.stderr");
const STATUS_LOGGED_OUT_FIXTURE: &str =
    include_str!("../../../docs/fixtures/codex-0.145.0/login/login-status.logged-out.stderr");
const LOGIN_SUCCESS_FIXTURE: &str =
    include_str!("../../../docs/fixtures/codex-0.145.0/login/device-login.success.stderr");
const DEVICE_CODE: &str = "A1B2-3456C";

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
                "codebox-login-test-{}-{sequence}",
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
        fs::write(&executable, b"pinned fake runtime executable").expect("create fake executable");
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

    fn scope(&self) -> CredentialScope {
        CredentialScope::validate(CredentialScopeConfig::new(
            self.executable.clone(),
            self.codex_home.clone(),
            self.state_dir.clone(),
            self.working_dir.clone(),
        ))
        .expect("valid test scope")
    }

    fn persist(&self, ledger: &LoginLedger) {
        // SAFETY: `geteuid` has no preconditions and uses no pointers.
        let runner_uid = unsafe { libc::geteuid() };
        persist_ledger(&self.state_dir, runner_uid, ledger).expect("persist test ledger");
    }

    fn load(&self) -> LoginLedger {
        // SAFETY: `geteuid` has no preconditions and uses no pointers.
        let runner_uid = unsafe { libc::geteuid() };
        load_ledger(&self.state_dir, runner_uid)
            .expect("load test ledger")
            .expect("test ledger exists")
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

#[derive(Clone, Copy)]
enum FakeStatus {
    LoggedOut,
    LoggedIn,
    Unavailable,
    Invalid,
}

struct FakeState {
    statuses: VecDeque<FakeStatus>,
    events: Option<VecDeque<SupervisorEvent>>,
    cancel_event: Option<SupervisorEvent>,
    version_calls: usize,
    status_calls: usize,
    start_calls: usize,
}

struct FakeRuntime {
    state: Arc<Mutex<FakeState>>,
}

impl FakeRuntime {
    fn scripted(
        statuses: impl IntoIterator<Item = FakeStatus>,
        events: impl IntoIterator<Item = SupervisorEvent>,
    ) -> (Self, Arc<Mutex<FakeState>>) {
        let state = Arc::new(Mutex::new(FakeState {
            statuses: statuses.into_iter().collect(),
            events: Some(events.into_iter().collect()),
            cancel_event: None,
            version_calls: 0,
            status_calls: 0,
            start_calls: 0,
        }));
        (
            Self {
                state: Arc::clone(&state),
            },
            state,
        )
    }

    fn with_cancel_event(self, event: SupervisorEvent) -> Self {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .cancel_event = Some(event);
        self
    }
}

impl CliRuntime for FakeRuntime {
    fn verify_version(&self, _lease: &OwnedCredentialScopeLease) -> Result<(), LoginBrokerError> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .version_calls += 1;
        Ok(())
    }

    fn status(&self, _lease: &OwnedCredentialScopeLease) -> Result<PinnedStatus, LoginBrokerError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.status_calls += 1;
        match state
            .statuses
            .pop_front()
            .unwrap_or(FakeStatus::Unavailable)
        {
            FakeStatus::LoggedOut => Ok(PinnedStatus::LoggedOut),
            FakeStatus::LoggedIn => Ok(PinnedStatus::LoggedIn),
            FakeStatus::Unavailable => Err(LoginBrokerError::StatusUnavailable),
            FakeStatus::Invalid => Err(LoginBrokerError::ProviderOutputInvalid),
        }
    }

    fn start(
        &self,
        _lease: &OwnedCredentialScopeLease,
    ) -> Result<Box<dyn LoginSupervisor>, LoginBrokerError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.start_calls += 1;
        Ok(Box::new(FakeSupervisor {
            events: state.events.take().unwrap_or_default(),
            cancel_event: state.cancel_event.take(),
        }))
    }
}

struct FakeSupervisor {
    events: VecDeque<SupervisorEvent>,
    cancel_event: Option<SupervisorEvent>,
}

impl LoginSupervisor for FakeSupervisor {
    fn receive(&mut self, _timeout: Duration) -> Result<SupervisorEvent, LoginBrokerError> {
        self.events
            .pop_front()
            .ok_or(LoginBrokerError::StatusUnavailable)
    }

    fn try_receive(&mut self) -> Result<Option<SupervisorEvent>, LoginBrokerError> {
        Ok(self.events.pop_front())
    }

    fn cancel(&mut self) -> Result<SupervisorEvent, LoginBrokerError> {
        Ok(self
            .cancel_event
            .take()
            .unwrap_or_else(|| SupervisorEvent::Canceled(unknown_exit_output())))
    }
}

fn process_identity() -> ProcessIdentity {
    crate::ledger::read_process_identity(std::process::id()).expect("read test process identity")
}

fn dead_process_identity() -> ProcessIdentity {
    ProcessIdentity {
        pid: u32::MAX,
        start_time_ticks: 1,
    }
}

fn verification_code() -> VerificationCode {
    VerificationCode::from_validated(DEVICE_CODE.to_owned())
}

fn success_output() -> CapturedOutput {
    CapturedOutput {
        stdout: DEVICE_PROMPT_FIXTURE.as_bytes().to_vec(),
        stderr: LOGIN_SUCCESS_FIXTURE.as_bytes().to_vec(),
        exit_code: Some(0),
        ..CapturedOutput::default()
    }
}

fn unknown_exit_output() -> CapturedOutput {
    CapturedOutput {
        exit_code: None,
        ..CapturedOutput::default()
    }
}

fn lifecycle_events(final_event: SupervisorEvent) -> Vec<SupervisorEvent> {
    vec![
        SupervisorEvent::Started(process_identity()),
        SupervisorEvent::Instructions(verification_code()),
        final_event,
    ]
}

#[test]
fn fake_cli_login_lifecycle() {
    let layout = TestLayout::new();
    let (runtime, state) = FakeRuntime::scripted(
        [FakeStatus::LoggedOut, FakeStatus::LoggedIn],
        lifecycle_events(SupervisorEvent::Exited(success_output())),
    );
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    let interaction = broker.start_device_login().expect("start fake login");
    assert_eq!(interaction.verification_code().expose(), DEVICE_CODE);
    assert_eq!(interaction.expires_in_seconds(), 900);
    assert_eq!(
        interaction.verification_url().as_str(),
        "https://auth.openai.com/codex/device"
    );
    assert_eq!(
        broker.status().expect("reconcile completed fake login"),
        LoginStatus::LoggedIn
    );
    assert_eq!(layout.load().latest(), LedgerPhase::LoggedIn);

    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(state.start_calls, 1);
    assert_eq!(state.status_calls, 2);
}

#[test]
fn login_status_reconciles_after_interruption() {
    let layout = TestLayout::new();
    let mut ledger = LoginLedger::new(LoginOperationId::new());
    ledger
        .record_started(dead_process_identity())
        .expect("record dead child");
    ledger
        .append(LedgerPhase::OutcomeUnknown)
        .expect("record unknown outcome");
    layout.persist(&ledger);
    let (runtime, _) = FakeRuntime::scripted([FakeStatus::LoggedOut], []);
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    assert_eq!(
        broker.reconcile().expect("reconcile interrupted login"),
        LoginStatus::LoggedOut
    );
    assert_eq!(layout.load().latest(), LedgerPhase::LoggedOut);
}

#[test]
fn login_output_is_bounded_and_redacted() {
    let output = CapturedOutput {
        stderr: b"T002B_SECRET_CANARY".to_vec(),
        stderr_overflow: true,
        exit_code: Some(1),
        ..CapturedOutput::default()
    };
    let error = parse_status(&output).expect_err("overflow must fail");
    let code = verification_code();

    assert!(matches!(error, LoginBrokerError::OutputLimitExceeded));
    assert!(!error.to_string().contains("T002B_SECRET_CANARY"));
    assert!(!format!("{code:?}").contains(DEVICE_CODE));
}

#[test]
fn login_runner_never_executes_repository_commands() {
    assert_fixed_login_commands();
}

#[test]
fn regression_cloud_runner_never_executes_repository_code() {
    assert_fixed_login_commands();
}

fn assert_fixed_login_commands() {
    let layout = TestLayout::new();
    let scope = layout.scope();
    let lease = scope.try_acquire().expect("exclusive scope");
    for command in [
        CodexCommand::Version,
        CodexCommand::LoginStatus,
        CodexCommand::DeviceLogin,
    ] {
        let invocation = lease.invocation(command).expect("fixed invocation");
        let args = invocation.args().join(" ");
        assert_eq!(invocation.executable(), layout.executable);
        assert!(!args.contains("exec"));
        assert!(!args.contains("cloud"));
        assert!(!args.contains(&layout.root.to_string_lossy().into_owned()));
    }
}

#[test]
fn login_credentials_never_reach_events_or_artifacts() {
    let layout = TestLayout::new();
    let canary = "T002B_AUTH_CACHE_CANARY";
    fs::write(layout.codex_home.join("auth.json"), canary).expect("write auth canary");
    let (runtime, _) = FakeRuntime::scripted(
        [FakeStatus::LoggedOut],
        [
            SupervisorEvent::Started(process_identity()),
            SupervisorEvent::Instructions(verification_code()),
        ],
    );
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));
    let interaction = broker.start_device_login().expect("start fake login");
    let ledger = fs::read_to_string(layout.state_dir.join("login-ledger.json"))
        .expect("read redacted ledger");

    assert!(!ledger.contains(canary));
    assert!(!ledger.contains(interaction.verification_code().expose()));
    assert!(!format!("{broker:?}").contains(canary));
    assert!(!format!("{broker:?}").contains(DEVICE_CODE));
}

#[test]
fn login_unknown_outcome_is_not_retried() {
    let layout = TestLayout::new();
    layout.persist(&LoginLedger::new(LoginOperationId::new()));
    let (runtime, state) = FakeRuntime::scripted([FakeStatus::LoggedOut], []);
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    assert!(matches!(
        broker.start_device_login(),
        Err(LoginBrokerError::OutcomeUnknown)
    ));
    let state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(state.start_calls, 0);
    assert_eq!(state.status_calls, 0);
}

#[test]
fn login_crash_leaves_reconcilable_ledger() {
    let layout = TestLayout::new();
    let mut ledger = LoginLedger::new(LoginOperationId::new());
    ledger
        .record_started(dead_process_identity())
        .expect("record attempted child");
    ledger
        .append(LedgerPhase::OutcomeUnknown)
        .expect("record crash outcome");
    layout.persist(&ledger);
    let (runtime, _) = FakeRuntime::scripted([FakeStatus::LoggedOut], []);
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    assert_eq!(
        broker.reconcile().expect("reconcile crashed login"),
        LoginStatus::LoggedOut
    );
    assert_eq!(layout.load().latest(), LedgerPhase::LoggedOut);
}

#[test]
fn pinned_login_status_fixtures_are_exact() {
    let logged_in = CapturedOutput {
        stderr: STATUS_CHATGPT_FIXTURE.as_bytes().to_vec(),
        exit_code: Some(0),
        ..CapturedOutput::default()
    };
    let logged_out = CapturedOutput {
        stderr: STATUS_LOGGED_OUT_FIXTURE.as_bytes().to_vec(),
        exit_code: Some(1),
        ..CapturedOutput::default()
    };

    assert_eq!(
        parse_status(&logged_in).expect("parse logged-in fixture"),
        PinnedStatus::LoggedIn
    );
    assert_eq!(
        parse_status(&logged_out).expect("parse logged-out fixture"),
        PinnedStatus::LoggedOut
    );

    let mut drifted = logged_out;
    drifted.stderr.extend_from_slice(b"extra");
    assert!(matches!(
        parse_status(&drifted),
        Err(LoginBrokerError::StatusUnavailable)
    ));

    let unsupported_mode = CapturedOutput {
        stderr: b"Logged in using an API key\n".to_vec(),
        exit_code: Some(0),
        ..CapturedOutput::default()
    };
    assert!(matches!(
        parse_status(&unsupported_mode),
        Err(LoginBrokerError::ProviderOutputInvalid)
    ));
}

#[test]
fn pinned_cli_version_and_completion_fixtures_are_exact() {
    let version = CapturedOutput {
        stdout: b"codex-cli 0.145.0\n".to_vec(),
        exit_code: Some(0),
        ..CapturedOutput::default()
    };
    parse_version(&version).expect("parse pinned version");

    let mut drifted_version = version;
    drifted_version.stdout = b"codex-cli 0.145.1\n".to_vec();
    assert!(matches!(
        parse_version(&drifted_version),
        Err(LoginBrokerError::VersionMismatch)
    ));

    let mut drifted_completion = success_output();
    drifted_completion.stdout.extend_from_slice(b"unexpected\n");
    assert!(matches!(
        parse_login_completion(&drifted_completion),
        Err(LoginBrokerError::ProviderOutputInvalid)
    ));
}

#[test]
fn pinned_device_prompt_fixture_is_parsed() {
    let code = parse_device_prompt(DEVICE_PROMPT_FIXTURE.as_bytes(), false)
        .expect("parse pinned prompt fixture");

    assert_eq!(code.expose(), DEVICE_CODE);
}

#[test]
fn logged_in_status_does_not_spawn_device_login() {
    let layout = TestLayout::new();
    let (runtime, state) = FakeRuntime::scripted([FakeStatus::LoggedIn], []);
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    assert!(matches!(
        broker.start_device_login(),
        Err(LoginBrokerError::AlreadyLoggedIn)
    ));
    assert_eq!(
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start_calls,
        0
    );
}

#[test]
fn device_prompt_parser_is_chunk_boundary_invariant() {
    let bytes = DEVICE_PROMPT_FIXTURE.as_bytes();
    for boundary in 0..bytes.len() {
        assert!(matches!(
            parse_device_prompt(&bytes[..boundary], false),
            Err(PromptParseError::Incomplete)
        ));
    }
    assert_eq!(
        parse_device_prompt(bytes, false)
            .expect("complete fixture")
            .expose(),
        DEVICE_CODE
    );
}

#[test]
fn login_cancellation_reaps_and_reconciles() {
    let layout = TestLayout::new();
    let (runtime, _) = FakeRuntime::scripted(
        [FakeStatus::LoggedOut, FakeStatus::LoggedOut],
        [
            SupervisorEvent::Started(process_identity()),
            SupervisorEvent::Instructions(verification_code()),
        ],
    );
    let runtime = runtime.with_cancel_event(SupervisorEvent::Canceled(unknown_exit_output()));
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    broker.start_device_login().expect("start fake login");
    assert_eq!(
        broker.cancel().expect("cancel and reconcile"),
        LoginStatus::LoggedOut
    );
    assert_eq!(layout.load().latest(), LedgerPhase::LoggedOut);
}

#[test]
fn regression_login_output_drains_after_truncation() {
    let output =
        drain_more_than_limit(vec![b'x'; MAX_CAPTURE_BYTES + 8192]).expect("drain test stream");

    assert_eq!(output.stdout.len(), MAX_CAPTURE_BYTES);
    assert!(output.stdout_overflow);
}

#[test]
fn login_parent_crash_does_not_leave_runnable_orphan() {
    assert!(parent_death_kills_child().expect("exercise Linux parent-death binding"));
}

#[test]
fn login_parent_death_binding_race_fails_closed() {
    assert!(mismatched_parent_fails_spawn());
}

#[test]
fn login_instruction_deadline_kills_group_and_reconciles() {
    let layout = TestLayout::new();
    let (runtime, _) = FakeRuntime::scripted(
        [FakeStatus::LoggedOut, FakeStatus::LoggedOut],
        [
            SupervisorEvent::Started(process_identity()),
            SupervisorEvent::InstructionDeadline(unknown_exit_output()),
        ],
    );
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    assert!(matches!(
        broker.start_device_login(),
        Err(LoginBrokerError::LoginFailed)
    ));
    assert_eq!(layout.load().latest(), LedgerPhase::LoggedOut);
}

#[test]
fn login_overall_deadline_kills_group_and_reconciles() {
    let layout = TestLayout::new();
    let (runtime, _) = FakeRuntime::scripted(
        [FakeStatus::LoggedOut, FakeStatus::LoggedOut],
        [
            SupervisorEvent::Started(process_identity()),
            SupervisorEvent::OverallDeadline(unknown_exit_output()),
        ],
    );
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    assert!(matches!(
        broker.start_device_login(),
        Err(LoginBrokerError::LoginFailed)
    ));
    assert_eq!(layout.load().latest(), LedgerPhase::LoggedOut);
}

#[test]
fn inconclusive_preflight_status_fails_closed() {
    for status in [FakeStatus::Unavailable, FakeStatus::Invalid] {
        let layout = TestLayout::new();
        let (runtime, state) = FakeRuntime::scripted([status], []);
        let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

        assert!(broker.start_device_login().is_err());
        assert_eq!(
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .start_calls,
            0
        );
    }
}

#[test]
fn post_spawn_output_failure_is_reconciled_without_losing_its_type() {
    let layout = TestLayout::new();
    let (runtime, _) = FakeRuntime::scripted(
        [FakeStatus::LoggedOut, FakeStatus::LoggedOut],
        [
            SupervisorEvent::Started(process_identity()),
            SupervisorEvent::Uncertain(LoginBrokerError::OutputLimitExceeded),
        ],
    );
    let mut broker = LoginBroker::with_runtime(layout.scope(), Box::new(runtime));

    assert!(matches!(
        broker.start_device_login(),
        Err(LoginBrokerError::OutputLimitExceeded)
    ));
    assert_eq!(layout.load().latest(), LedgerPhase::LoggedOut);
}

#[test]
fn login_ledger_rejects_malformed_and_permission_unsafe_files() {
    let layout = TestLayout::new();
    let ledger_path = layout.state_dir.join("login-ledger.json");
    fs::write(&ledger_path, b"{not-json").expect("write malformed ledger");
    set_mode(&ledger_path, 0o600);
    // SAFETY: `geteuid` has no preconditions and uses no pointers.
    let runner_uid = unsafe { libc::geteuid() };
    assert!(matches!(
        load_ledger(&layout.state_dir, runner_uid),
        Err(LoginBrokerError::LedgerInvalid)
    ));

    fs::write(
        &ledger_path,
        serde_json::to_vec(&LoginLedger::new(LoginOperationId::new()))
            .expect("serialize valid ledger"),
    )
    .expect("replace ledger");
    set_mode(&ledger_path, 0o644);
    assert!(matches!(
        load_ledger(&layout.state_dir, runner_uid),
        Err(LoginBrokerError::LedgerInvalid)
    ));
}
