use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;

use crate::ledger::{ProcessIdentity, read_process_identity};
use crate::parser::{
    CapturedOutput, MAX_CAPTURE_BYTES, PinnedStatus, PromptParseError, parse_device_prompt,
    parse_status, parse_version,
};
use crate::scope::OwnedCredentialScopeLease;
use crate::{CodexCommand, CodexInvocation, LoginBrokerError, VerificationCode};

const SHORT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const INSTRUCTION_TIMEOUT: Duration = Duration::from_secs(30);
const OVERALL_LOGIN_TIMEOUT: Duration = Duration::from_secs(16 * 60);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) trait CliRuntime: Send + Sync {
    fn verify_version(&self, lease: &OwnedCredentialScopeLease) -> Result<(), LoginBrokerError>;

    fn status(&self, lease: &OwnedCredentialScopeLease) -> Result<PinnedStatus, LoginBrokerError>;

    fn start(
        &self,
        lease: &OwnedCredentialScopeLease,
    ) -> Result<Box<dyn LoginSupervisor>, LoginBrokerError>;
}

pub(crate) trait LoginSupervisor: Send {
    fn receive(&mut self, timeout: Duration) -> Result<SupervisorEvent, LoginBrokerError>;
    fn try_receive(&mut self) -> Result<Option<SupervisorEvent>, LoginBrokerError>;
    fn cancel(&mut self) -> Result<SupervisorEvent, LoginBrokerError>;
}

pub(crate) enum SupervisorEvent {
    Started(ProcessIdentity),
    Instructions(VerificationCode),
    Exited(CapturedOutput),
    InstructionDeadline(CapturedOutput),
    OverallDeadline(CapturedOutput),
    Canceled(CapturedOutput),
    Uncertain(LoginBrokerError),
    Failed(LoginBrokerError),
}

pub(crate) struct ProcessRuntime {
    short_timeout: Duration,
    instruction_timeout: Duration,
    overall_timeout: Duration,
    termination_grace: Duration,
}

impl Default for ProcessRuntime {
    fn default() -> Self {
        Self {
            short_timeout: SHORT_COMMAND_TIMEOUT,
            instruction_timeout: INSTRUCTION_TIMEOUT,
            overall_timeout: OVERALL_LOGIN_TIMEOUT,
            termination_grace: TERMINATION_GRACE,
        }
    }
}

impl CliRuntime for ProcessRuntime {
    fn verify_version(&self, lease: &OwnedCredentialScopeLease) -> Result<(), LoginBrokerError> {
        let invocation = lease.invocation(CodexCommand::Version)?;
        parse_version(&run_short_command(
            CommandSpec::from_invocation(&invocation),
            self.short_timeout,
            self.termination_grace,
        )?)
    }

    fn status(&self, lease: &OwnedCredentialScopeLease) -> Result<PinnedStatus, LoginBrokerError> {
        let invocation = lease.invocation(CodexCommand::LoginStatus)?;
        parse_status(&run_short_command(
            CommandSpec::from_invocation(&invocation),
            self.short_timeout,
            self.termination_grace,
        )?)
    }

    fn start(
        &self,
        lease: &OwnedCredentialScopeLease,
    ) -> Result<Box<dyn LoginSupervisor>, LoginBrokerError> {
        let invocation = lease.invocation(CodexCommand::DeviceLogin)?;
        Ok(Box::new(ProcessSupervisor::start(
            CommandSpec::from_invocation(&invocation),
            self.instruction_timeout,
            self.overall_timeout,
            self.termination_grace,
        )?))
    }
}

#[derive(Clone)]
struct CommandSpec {
    executable: PathBuf,
    working_dir: PathBuf,
    codex_home: PathBuf,
    args: Vec<OsString>,
}

impl CommandSpec {
    fn from_invocation(invocation: &CodexInvocation<'_>) -> Self {
        Self {
            executable: invocation.executable().to_owned(),
            working_dir: invocation.working_dir().to_owned(),
            codex_home: invocation.codex_home().to_owned(),
            args: invocation.args().iter().map(OsString::from).collect(),
        }
    }

    fn command(&self) -> Result<Command, LoginBrokerError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(crate::CredentialScopeError::UnsupportedPlatform.into())
        }

        #[cfg(target_os = "linux")]
        {
            // SAFETY: `getpid` has no preconditions and uses no pointers.
            let parent_pid = unsafe { libc::getpid() };
            self.command_with_parent_pid(parent_pid)
        }
    }

    #[cfg(target_os = "linux")]
    fn command_with_parent_pid(
        &self,
        parent_pid: libc::pid_t,
    ) -> Result<Command, LoginBrokerError> {
        let mut command = Command::new(&self.executable);
        command
            .args(&self.args)
            .current_dir(&self.working_dir)
            .env_clear()
            .env("CODEX_HOME", &self.codex_home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // The caller thread remains alive until this child is reaped. The closure uses only
        // async-signal-safe raw libc calls after fork.
        // SAFETY: the closure performs no allocation, locking, formatting, logging, panic, or
        // unwind after fork. All failures use a fixed raw OS error.
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::from_raw_os_error(libc::ECHILD));
                }
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(io::Error::from_raw_os_error(libc::ECHILD));
                }
                if libc::getppid() != parent_pid {
                    return Err(io::Error::from_raw_os_error(libc::ECHILD));
                }
                Ok(())
            });
        }

        Ok(command)
    }
}

struct ProcessSupervisor {
    commands: mpsc::Sender<SupervisorCommand>,
    events: mpsc::Receiver<SupervisorEvent>,
    thread: Option<JoinHandle<()>>,
    finished: bool,
}

impl ProcessSupervisor {
    fn start(
        spec: CommandSpec,
        instruction_timeout: Duration,
        overall_timeout: Duration,
        termination_grace: Duration,
    ) -> Result<Self, LoginBrokerError> {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("codex-login-supervisor".to_owned())
            .spawn(move || {
                supervise_device_login(
                    spec,
                    command_receiver,
                    event_sender,
                    instruction_timeout,
                    overall_timeout,
                    termination_grace,
                );
            })
            .map_err(|source| LoginBrokerError::Process { source })?;

        Ok(Self {
            commands: command_sender,
            events: event_receiver,
            thread: Some(thread),
            finished: false,
        })
    }

    fn finish_thread(&mut self) -> Result<(), LoginBrokerError> {
        self.finished = true;
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| LoginBrokerError::Process {
                source: io::Error::other("login supervisor thread panicked"),
            })?;
        }
        Ok(())
    }
}

impl LoginSupervisor for ProcessSupervisor {
    fn receive(&mut self, timeout: Duration) -> Result<SupervisorEvent, LoginBrokerError> {
        let event = self
            .events
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => LoginBrokerError::StatusUnavailable,
                mpsc::RecvTimeoutError::Disconnected => LoginBrokerError::Process {
                    source: io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "login supervisor disconnected",
                    ),
                },
            })?;
        if event_finishes_supervisor(&event) {
            self.finish_thread()?;
        }
        Ok(event)
    }

    fn try_receive(&mut self) -> Result<Option<SupervisorEvent>, LoginBrokerError> {
        match self.events.try_recv() {
            Ok(event) => {
                if event_finishes_supervisor(&event) {
                    self.finish_thread()?;
                }
                Ok(Some(event))
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) if self.finished => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(LoginBrokerError::Process {
                source: io::Error::new(io::ErrorKind::BrokenPipe, "login supervisor disconnected"),
            }),
        }
    }

    fn cancel(&mut self) -> Result<SupervisorEvent, LoginBrokerError> {
        self.commands
            .send(SupervisorCommand::Cancel)
            .map_err(|_| LoginBrokerError::Process {
                source: io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "login supervisor is unavailable",
                ),
            })?;
        loop {
            let event = self.receive(TERMINATION_GRACE + Duration::from_secs(1))?;
            if event_finishes_supervisor(&event) {
                return Ok(event);
            }
        }
    }
}

impl Drop for ProcessSupervisor {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = self.commands.send(SupervisorCommand::Cancel);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn event_finishes_supervisor(event: &SupervisorEvent) -> bool {
    matches!(
        event,
        SupervisorEvent::Exited(_)
            | SupervisorEvent::InstructionDeadline(_)
            | SupervisorEvent::OverallDeadline(_)
            | SupervisorEvent::Canceled(_)
            | SupervisorEvent::Uncertain(_)
            | SupervisorEvent::Failed(_)
    )
}

enum SupervisorCommand {
    Cancel,
}

fn supervise_device_login(
    spec: CommandSpec,
    commands: mpsc::Receiver<SupervisorCommand>,
    events: mpsc::Sender<SupervisorEvent>,
    instruction_timeout: Duration,
    overall_timeout: Duration,
    termination_grace: Duration,
) {
    let mut child = match spawn_bound_child(&spec) {
        Ok(child) => child,
        Err(error) => {
            let _ = events.send(SupervisorEvent::Failed(error));
            return;
        }
    };
    let process = match read_process_identity(child.id()) {
        Ok(process) => process,
        Err(error) => {
            let _ = terminate_group(&mut child, termination_grace);
            let _ = events.send(SupervisorEvent::Uncertain(error));
            return;
        }
    };
    if events.send(SupervisorEvent::Started(process)).is_err() {
        let _ = terminate_group(&mut child, termination_grace);
        return;
    }

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = terminate_group(&mut child, termination_grace);
            let _ = events.send(SupervisorEvent::Uncertain(LoginBrokerError::Process {
                source: io::Error::other("device-login stdout was not piped"),
            }));
            return;
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = terminate_group(&mut child, termination_grace);
            let _ = events.send(SupervisorEvent::Uncertain(LoginBrokerError::Process {
                source: io::Error::other("device-login stderr was not piped"),
            }));
            return;
        }
    };
    let stdout_capture = SharedCapture::default();
    let stderr_capture = SharedCapture::default();
    let stdout_thread = drain_stream(stdout, stdout_capture.clone());
    let stderr_thread = drain_stream(stderr, stderr_capture.clone());
    let started_at = Instant::now();
    let mut instructions_sent = false;

    loop {
        if matches!(commands.try_recv(), Ok(SupervisorCommand::Cancel)) {
            let status = terminate_group(&mut child, termination_grace);
            let output = finish_capture(
                status,
                &stdout_capture,
                &stderr_capture,
                stdout_thread,
                stderr_thread,
            );
            let _ = events.send(match output {
                Ok(output) => SupervisorEvent::Canceled(output),
                Err(error) => SupervisorEvent::Uncertain(error),
            });
            return;
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let output = finish_capture(
                    Ok(status),
                    &stdout_capture,
                    &stderr_capture,
                    stdout_thread,
                    stderr_thread,
                );
                let _ = events.send(match output {
                    Ok(output) => SupervisorEvent::Exited(output),
                    Err(error) => SupervisorEvent::Uncertain(error),
                });
                return;
            }
            Ok(None) => {}
            Err(source) => {
                let _ = terminate_group(&mut child, termination_grace);
                let _ = events.send(SupervisorEvent::Uncertain(LoginBrokerError::Process {
                    source,
                }));
                return;
            }
        }

        if !instructions_sent {
            let (stdout_bytes, stdout_overflow) = stdout_capture.snapshot();
            let (_, stderr_overflow) = stderr_capture.snapshot();
            if stderr_overflow {
                let _ = terminate_group(&mut child, termination_grace);
                let _ = events.send(SupervisorEvent::Uncertain(
                    LoginBrokerError::OutputLimitExceeded,
                ));
                return;
            }
            match parse_device_prompt(&stdout_bytes, stdout_overflow) {
                Ok(code) => {
                    if events.send(SupervisorEvent::Instructions(code)).is_err() {
                        let _ = terminate_group(&mut child, termination_grace);
                        return;
                    }
                    instructions_sent = true;
                }
                Err(PromptParseError::Incomplete) => {}
                Err(PromptParseError::Terminal(error)) => {
                    let _ = terminate_group(&mut child, termination_grace);
                    let _ = events.send(SupervisorEvent::Uncertain(error));
                    return;
                }
            }
        }

        let elapsed = started_at.elapsed();
        if !instructions_sent && elapsed >= instruction_timeout {
            let status = terminate_group(&mut child, termination_grace);
            let output = finish_capture(
                status,
                &stdout_capture,
                &stderr_capture,
                stdout_thread,
                stderr_thread,
            );
            let _ = events.send(match output {
                Ok(output) => SupervisorEvent::InstructionDeadline(output),
                Err(error) => SupervisorEvent::Uncertain(error),
            });
            return;
        }
        if elapsed >= overall_timeout {
            let status = terminate_group(&mut child, termination_grace);
            let output = finish_capture(
                status,
                &stdout_capture,
                &stderr_capture,
                stdout_thread,
                stderr_thread,
            );
            let _ = events.send(match output {
                Ok(output) => SupervisorEvent::OverallDeadline(output),
                Err(error) => SupervisorEvent::Uncertain(error),
            });
            return;
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn run_short_command(
    spec: CommandSpec,
    timeout: Duration,
    termination_grace: Duration,
) -> Result<CapturedOutput, LoginBrokerError> {
    let mut child = spawn_bound_child(&spec)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| LoginBrokerError::Process {
            source: io::Error::other("short command stdout was not piped"),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| LoginBrokerError::Process {
            source: io::Error::other("short command stderr was not piped"),
        })?;
    let stdout_capture = SharedCapture::default();
    let stderr_capture = SharedCapture::default();
    let stdout_thread = drain_stream(stdout, stdout_capture.clone());
    let stderr_thread = drain_stream(stderr, stderr_capture.clone());
    let started_at = Instant::now();

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started_at.elapsed() < timeout => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                timed_out = true;
                break terminate_group(&mut child, termination_grace);
            }
            Err(source) => break Err(source),
        }
    };

    let output = finish_capture(
        status,
        &stdout_capture,
        &stderr_capture,
        stdout_thread,
        stderr_thread,
    )?;
    if timed_out {
        Err(LoginBrokerError::StatusUnavailable)
    } else {
        Ok(output)
    }
}

fn spawn_bound_child(spec: &CommandSpec) -> Result<Child, LoginBrokerError> {
    spec.command()?
        .spawn()
        .map_err(|source| LoginBrokerError::Process { source })
}

fn terminate_group(child: &mut Child, grace: Duration) -> Result<ExitStatus, io::Error> {
    signal_group(child.id(), libc::SIGTERM)?;
    let started_at = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started_at.elapsed() >= grace {
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    signal_group(child.id(), libc::SIGKILL)?;
    child.wait()
}

fn signal_group(pid: u32, signal: i32) -> Result<(), io::Error> {
    let pid = i32::try_from(pid).map_err(|_| io::Error::other("child PID is out of range"))?;
    // SAFETY: a negative PID targets the child-created process group; no pointers are involved.
    let result = unsafe { libc::kill(-pid, signal) };
    if result == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

#[derive(Clone, Default)]
struct SharedCapture(Arc<Mutex<BoundedCapture>>);

impl SharedCapture {
    fn push(&self, bytes: &[u8]) {
        let mut capture = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        capture.push(bytes);
    }

    fn snapshot(&self) -> (Vec<u8>, bool) {
        let capture = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (capture.bytes.clone(), capture.overflow)
    }
}

#[derive(Default)]
struct BoundedCapture {
    bytes: Vec<u8>,
    overflow: bool,
}

impl BoundedCapture {
    fn push(&mut self, bytes: &[u8]) {
        let remaining = MAX_CAPTURE_BYTES.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        if bytes.len() > remaining {
            self.overflow = true;
        }
    }
}

fn drain_stream<R>(mut stream: R, capture: SharedCapture) -> JoinHandle<Result<(), io::Error>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer)?;
            if read == 0 {
                return Ok(());
            }
            capture.push(&buffer[..read]);
        }
    })
}

fn finish_capture(
    status: Result<ExitStatus, io::Error>,
    stdout: &SharedCapture,
    stderr: &SharedCapture,
    stdout_thread: JoinHandle<Result<(), io::Error>>,
    stderr_thread: JoinHandle<Result<(), io::Error>>,
) -> Result<CapturedOutput, LoginBrokerError> {
    let status = status.map_err(|source| LoginBrokerError::Process { source })?;
    join_drain(stdout_thread)?;
    join_drain(stderr_thread)?;
    let (stdout, stdout_overflow) = stdout.snapshot();
    let (stderr, stderr_overflow) = stderr.snapshot();
    Ok(CapturedOutput {
        stdout,
        stderr,
        stdout_overflow,
        stderr_overflow,
        exit_code: status.code(),
    })
}

fn join_drain(thread: JoinHandle<Result<(), io::Error>>) -> Result<(), LoginBrokerError> {
    thread
        .join()
        .map_err(|_| LoginBrokerError::Process {
            source: io::Error::other("process output drainer panicked"),
        })?
        .map_err(|source| LoginBrokerError::Process { source })
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod test_support {
    use super::*;
    use crate::ledger::process_is_alive;

    pub(crate) fn drain_more_than_limit(
        bytes: Vec<u8>,
    ) -> Result<CapturedOutput, LoginBrokerError> {
        let stdout_capture = SharedCapture::default();
        let stderr_capture = SharedCapture::default();
        let stdout_thread = drain_stream(std::io::Cursor::new(bytes), stdout_capture.clone());
        let stderr_thread = drain_stream(std::io::Cursor::new(Vec::new()), stderr_capture.clone());
        join_drain(stdout_thread)?;
        join_drain(stderr_thread)?;
        let (stdout, stdout_overflow) = stdout_capture.snapshot();
        let (stderr, stderr_overflow) = stderr_capture.snapshot();
        Ok(CapturedOutput {
            stdout,
            stderr,
            stdout_overflow,
            stderr_overflow,
            exit_code: Some(0),
        })
    }

    pub(crate) fn parent_death_kills_child() -> Result<bool, LoginBrokerError> {
        let (sender, receiver) = mpsc::channel();
        let forking_thread = thread::spawn(move || {
            let spec = CommandSpec {
                executable: PathBuf::from("/bin/sleep"),
                working_dir: PathBuf::from("/"),
                codex_home: PathBuf::from("/tmp"),
                args: vec![OsString::from("30")],
            };
            let child = spawn_bound_child(&spec)?;
            let identity = read_process_identity(child.id())?;
            sender
                .send(identity)
                .map_err(|_| LoginBrokerError::Process {
                    source: io::Error::new(io::ErrorKind::BrokenPipe, "test receiver disappeared"),
                })?;
            drop(child);
            Ok::<(), LoginBrokerError>(())
        });
        let identity = receiver.recv().map_err(|_| LoginBrokerError::Process {
            source: io::Error::new(io::ErrorKind::BrokenPipe, "test child was not reported"),
        })?;
        forking_thread
            .join()
            .map_err(|_| LoginBrokerError::Process {
                source: io::Error::other("test forking thread panicked"),
            })??;

        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !process_is_alive(identity)? {
                return Ok(true);
            }
            thread::sleep(POLL_INTERVAL);
        }
        Ok(false)
    }

    pub(crate) fn mismatched_parent_fails_spawn() -> bool {
        let spec = CommandSpec {
            executable: PathBuf::from("/bin/true"),
            working_dir: PathBuf::from("/"),
            codex_home: PathBuf::from("/tmp"),
            args: Vec::new(),
        };
        // SAFETY: `getpid` has no preconditions and uses no pointers.
        let wrong_parent = unsafe { libc::getpid() }.saturating_add(1);
        match spec.command_with_parent_pid(wrong_parent) {
            Ok(mut command) => command.spawn().is_err(),
            Err(_) => true,
        }
    }
}
