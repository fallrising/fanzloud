---
id: SPEC-T002B
subject: T002B Codex device-login lifecycle
status: verified
contract_units: [CU-AUTH-P0-01]
archetypes: [D, F]
atomicity: E2
retriable: false
---

# Normative Inputs

- TD §§1.6–1.7, 2.2, 2.3, 8.2–8.10, 11.3 P14, and 15.0
- ADR-0002 §§Credential boundary and First execution interface
- T002B task document
- SPEC-T002A credential-scope lease and isolation
- T010 domain values
- Codex manual [Configuration, Authentication, and Models](https://learn.chatgpt.com/docs/config-file/config-reference)
  and [Authentication](https://learn.chatgpt.com/docs/auth)

# Contract Boundary

The broker is trusted platform infrastructure. It consumes one validated and exclusively leased
credential scope from `SPEC-T002A`, owns one pinned Codex device-login child at a time, and returns
typed projections for login state.

The broker may invoke only fixed commands equivalent to:

```text
codex -c forced_login_method="chatgpt" -c cli_auth_credentials_store="file" login --device-auth
codex -c forced_login_method="chatgpt" -c cli_auth_credentials_store="file" login status
```

The executable path, exact version, `CODEX_HOME`, working directory, state directory, and
environment allowlist come only from the validated `CredentialScope`. Browser input cannot select
or override them.

# Known Official Surface

The current Codex manual documents that `CODEX_HOME` is the root for config, auth, logs, sessions,
skills, and package metadata; it must already exist. It documents device authentication through
`codex login --device-auth`, status inspection through `codex login status`, and credential stores
including file, keyring, and auto. It also warns that file-based `auth.json` contains access tokens.

# Pinned CLI `0.145.0` Contract

The gap identified in the first draft is resolved for the private P0 by captured, redacted fixtures
under `docs/fixtures/codex-0.145.0/login/` and inspection of the official
[`rust-v0.145.0` login source](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/cli/src/login.rs)
and
[`device-code source`](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/login/src/device_code_auth.rs).
These facts are part of this specification:

- `codex --version` must report exactly `codex-cli 0.145.0`.
- Device login clears existing CLI authentication before requesting a device code. The broker must
  therefore run the fixed status command while holding the login lease. It may spawn device login
  only when the exact pinned fixture proves `LoggedOut`; `LoggedIn`, timeout, process failure,
  malformed output, output overflow, and every other inconclusive result fail closed.
- Device instructions are human-readable ANSI-decorated stdout, not JSON. After stripping only ANSI
  SGR color codes, the versioned template contains the exact verification URL
  `https://auth.openai.com/codex/device` and a ten-character code: nine uppercase ASCII
  alphanumeric characters plus one separator, with shape `[A-Z0-9]{4}-[A-Z0-9]{5}`.
- The prompt says the code expires in 15 minutes. The process remains running while it polls for
  completion; printing instructions is not a successful terminal exit.
- Successful credential persistence is followed by `Successfully logged in` on stderr and exit
  code `0`. Device-login errors are written to stderr and exit `1`; arbitrary error text is never
  forwarded or durably stored.
- `codex login status` writes its result to stderr. Exact `Logged in using ChatGPT` plus exit `0`
  maps to `LoggedIn`; exact `Not logged in` plus exit `1` maps to `LoggedOut`.
- Exit `1` with any other status text is `StatusUnavailable`, not `LoggedOut`. Exit `0` with any
  other authentication mode is `ProviderOutputInvalid`; P0 does not accept API-key, access-token,
  personal-access-token, Bedrock, or unknown authentication.

The live device fixture was captured with an empty mode-`0700` `CODEX_HOME`; the short-lived code
was never authorized and is replaced by a shape-preserving placeholder. No real credential,
account identifier, or token is present in the fixture.

Changing the CLI version, verification origin, output template, code shape, stream assignment, or
exit mapping requires a specification and fixture update before deployment. Unknown output is not
accepted for forward compatibility. `[NEW-SPEC]`

# Public Types and Operations

The public Rust boundary is:

```rust
pub struct LoginBroker { /* private fields */ }

impl LoginBroker {
    pub fn new(scope: CredentialScope) -> Result<Self, LoginBrokerError>;
    pub fn start_device_login(&mut self) -> Result<LoginInteraction, LoginBrokerError>;
    pub fn status(&mut self) -> Result<LoginStatus, LoginBrokerError>;
    pub fn reconcile(&mut self) -> Result<LoginStatus, LoginBrokerError>;
    pub fn cancel(&mut self) -> Result<LoginStatus, LoginBrokerError>;
}
```

`LoginInteraction` contains a strong operation ID, the exact pinned verification URL, a
non-debug-printing verification-code value, and the fixed 900-second expiry. `LoginStatus` is one
of `LoggedOut`, `DeviceLoginPending`, `LoggedIn`, or `OutcomeUnknown`; pending and unknown states
carry the operation ID. No public type contains raw stdout, stderr, a token, or an auth-cache path.
`[NEW-SPEC]`

# Public Operations

## `start_device_login`

### Preconditions

- The operator scope is configured and not leased by another login operation.
- `SPEC-T002A` validated `CODEX_HOME`, the state directory, trusted working directory, executable,
  fixed environment, and non-repository boundary.
- The configured executable version matches the administrator pin.
- No repository path, prompt, token, or browser-provided command argument is accepted.

### Success

- Starts exactly one fixed `codex login --device-auth` process in a non-repository trusted working
  directory.
- Returns only a bounded `LoginInteraction` containing provider verification instructions after
  redaction and validation.
- Persists intent before spawn and started state immediately after spawn without credential
  material.
- Keeps draining bounded stdout and stderr while the process runs; output after the retained bound
  is discarded but still drained so the child cannot deadlock on a full pipe.

### Failure and caller action

- Unsafe scope or executable configuration: typed configuration error; operator must repair it.
- Existing lease: `LoginAlreadyRunning`; caller must observe the active operation.
- Existing ChatGPT authentication: `AlreadyLoggedIn`; caller must use `status`, and the destructive
  device-login command is not spawned.
- Inconclusive preflight status: `StatusUnavailable`, `ProviderOutputInvalid`, or
  `OutputLimitExceeded`; the caller may inspect operator-only diagnostics, but device login is not
  spawned and no automatic retry occurs.
- Malformed or oversized CLI output: typed `ProviderOutputInvalid` or `OutputLimitExceeded`; caller
  must inspect bounded diagnostics, never retry blindly.
- Process failure before authentication: typed `LoginFailed`; caller may start a new operation only
  after the prior operation is terminal.

## `status`

### Preconditions

- The configured executable and `CODEX_HOME` pass the same trust checks.

### Success

- Executes fixed `codex login status` and maps recognized output to `LoggedOut`, `DeviceLoginPending`,
  or `LoggedIn`.
- While an in-process login child is still running, returns `DeviceLoginPending` from the broker
  state without spawning another login.
- Returns a redacted, bounded status projection only.

### Failure and caller action

- Unknown output is `ProviderOutputInvalid`; do not infer `LoggedIn`.
- CLI timeout or process failure is `StatusUnavailable`; caller may poll later, but must not start a
  new login while an operation is uncertain.

## `reconcile`

An interrupted login is reconciled by a bounded `status` operation and the durable login ledger.
If the status cannot prove `LoggedIn` or `LoggedOut`, the operation remains `OutcomeUnknown` and
automatic retry is forbidden.

# Bounds and Process Semantics

- Version and status commands have a five-second wall-clock deadline.
- Device instructions must become complete within 30 seconds.
- A device-login child has a 16-minute broker deadline, allowing the CLI's documented 15-minute
  polling window plus bounded process cleanup.
- Captured stdout and stderr are each limited to 16 KiB. Readers continue draining after the bound
  and report `OutputLimitExceeded`.
- Cancellation sends termination, waits up to two seconds, then force-kills and reaps the child.
  Because provider authorization or local persistence may have raced with cancellation, the ledger
  becomes `OutcomeUnknown` until `reconcile` proves the state. `[NEW-SPEC]`
- Output parsing is invariant to process chunk boundaries and CRLF versus LF. Only ANSI SGR color
  sequences are stripped. Any other control sequence, extra non-empty line, different origin,
  malformed code, invalid UTF-8, or template drift is rejected. `[NEW-SPEC]`
- The public methods are synchronous. `start_device_login` creates one dedicated, non-pooled
  `std::thread`; that exact thread performs spawn/fork, installs the parent-death binding, owns the
  `Child`, and remains alive until the child is reaped, canceled, or reaches its deadline. It never
  returns to a reusable or idle-timeout thread pool while the child exists. `[NEW-SPEC]`
- The dedicated supervisor starts subordinate stdout/stderr drainer threads and communicates with
  the broker through bounded channels. `start_device_login` rendezvous with it only until spawn is
  confirmed and complete instructions arrive or the 30-second bound expires. Later status/cancel
  commands are sent to the same supervisor. An async caller may bridge the short synchronous public
  call, but the bridging thread never owns the fork or parent-death binding. `[NEW-SPEC]`

# Security Boundary

- `CODEX_HOME` is never a repository, workspace, artifact root, or browser-selected path.
- The runner passes no secret-bearing environment variable except the process's configured
  credential-store settings; it must not pass `CODEX_ACCESS_TOKEN`, `CODEX_API_KEY`, or token text
  from the browser.
- Captured stdout/stderr is treated as untrusted process output, bounded before parsing, and
  redacted before any response or durable record.
- The child environment is cleared and rebuilt from a fixed administrator-owned allowlist. P0 passes
  `CODEX_HOME` and no browser, repository, prompt, token, API-key, or access-token environment data.
- The runner never launches shell commands, repository hooks, repository scripts, or `codex exec`.
- Permission checks and canary tests prove that a fake CLI cannot read or emit the credential store
  through a repository-controlled command.
- T002B supports the Linux P0 deployment in TD §7.4. Other targets return
  `UnsupportedPlatform` before a process or ledger side effect. `[NEW-SPEC]`

# State and Failure Semantics

```text
Idle → Starting → DeviceInstructions → Completing → LoggedIn
                    ├→ LoggedOut
                    ├→ Failed
                    └→ OutcomeUnknown → Reconciling
```

`Starting`, `Completing`, and `Reconciling` are transient broker activities. Durable history uses
`Intent`, `Started`, `DeviceInstructions`, and the terminal/unknown states below.
`DeviceLoginPending` projects either durable `Started` or `DeviceInstructions` while the supervised
child remains active. `[NEW-SPEC]`

The login side effect is E2: credentials may be written by the provider CLI, but status can query
the result. No automatic retry is allowed while the result is unknown. Every operation records
intent, started, and one terminal or unknown outcome without recording credential bytes.

The ledger is a version-1 JSON document in the trusted state directory. It contains an operation ID,
the supervised child's Linux PID/start-time identity after spawn, and an ordered state history drawn
from `Intent`, `Started`, `DeviceInstructions`, `LoggedIn`, `LoggedOut`, `Failed`, and
`OutcomeUnknown`. Each replacement is written to a
mode-`0600` temporary file, synced, atomically renamed, and followed by a state-directory sync.
A crash after durable intent but before durable started is reconciled as an uncertain attempted
spawn. A malformed, unsupported-version, or permission-unsafe ledger is a typed recovery error and
is never discarded automatically. `[NEW-SPEC]`

# Concurrency and Cleanup

- A per-operator lease serializes login start, cancellation, status transition, and reconciliation
  operations.
- A second caller observes the existing operation rather than spawning another CLI.
- Cancellation terminates the trusted CLI process and reaps it; if completion cannot be proven,
  the operation becomes `OutcomeUnknown` and requires reconciliation.
- Temporary process output and working directories are removed or marked for recovery without
  touching `CODEX_HOME` contents outside the provider CLI's ownership.
- The lease is a mode-`0600` file lock in the trusted state directory. Kernel lock release after a
  process crash does not make the prior operation retryable; the durable ledger still requires
  reconciliation. `[NEW-SPEC]`
- Before exec, the Linux child creates a new process group and installs `PR_SET_PDEATHSIG=SIGKILL`;
  it verifies that its parent PID still matches the spawning broker after installing the signal.
  Failure of any binding step fails the spawn. The configured executable is the pinned native
  binary, not a shell or npm wrapper, and the reviewed device-auth path does not launch a browser
  or helper process. `[NEW-SPEC]`
- The binding order is exact: (1) capture the dedicated supervisor thread's process PID before
  spawn, (2) `setpgid(0, 0)`, (3) `prctl(PR_SET_PDEATHSIG, SIGKILL)`, and (4) compare `getppid()`
  with the captured PID. Any mismatch or syscall failure returns a non-allocating spawn error before
  exec. `[NEW-SPEC]`
- The `pre_exec` closure performs only async-signal-safe raw libc calls (`setpgid`, `prctl`,
  `getppid`, and a non-allocating failure return). It does not allocate, lock, format, log, panic,
  or unwind after fork. `[NEW-SPEC]`
- Cancellation and deadlines signal the entire process group, wait up to two seconds, then send
  `SIGKILL` to the group and reap the direct child. A restarted broker inspects any recorded
  PID/start-time identity and refuses a new operation while that exact process is alive. An
  intent-only crash record waits through the two-second death grace and reconciles status before
  allowing a new attempt. `[NEW-SPEC]`

# Non-guarantees

- The broker does not validate the provider account's entitlement beyond the official CLI status.
- The broker does not expose access tokens, refresh tokens, or `auth.json` content.
- The broker does not protect against a compromised trusted host or a malicious provider binary
  configured by an administrator.
- Device-code instructions are not a general OAuth API contract; they are bounded projections of
  the pinned CLI fixture.
- Repository ancestry checks reject configured directories with a `.git` file or directory in
  their ancestry; they do not discover repositories that deliberately remove or disguise that
  marker. Administrator configuration remains trusted. `[NEW-SPEC]`
- P0 does not preserve proxy, custom-CA, keyring, or arbitrary inherited environment configuration.
  Adding one requires an explicit administrator-owned allowlist and security review. `[NEW-SPEC]`

# Required Tests

| Clause | Required test |
|---|---|
| Fixed command and no browser argv | `login_command_is_not_user_controlled` |
| Login lifecycle | `fake_cli_login_lifecycle` |
| Status reconciliation | `login_status_reconciles_after_interruption` |
| Single writer | `login_scope_is_single_writer` |
| `CODEX_HOME` safety | `login_home_permissions_are_rejected_when_unsafe` |
| Bounded provider output | `login_output_is_bounded_and_redacted` |
| No repository execution | `login_runner_never_executes_repository_commands` |
| TD §11.3 P14 regression | `regression_cloud_runner_never_executes_repository_code` |
| No secret persistence or response | `login_credentials_never_reach_events_or_artifacts` |
| Unknown outcome retry rule | `login_unknown_outcome_is_not_retried` |
| Crash/recovery exit invariant | `login_crash_leaves_reconcilable_ledger` |
| Pinned status fixtures and exit mapping | `pinned_login_status_fixtures_are_exact` |
| Pinned version and terminal fixture drift | `pinned_cli_version_and_completion_fixtures_are_exact` |
| Pinned device prompt parser | `pinned_device_prompt_fixture_is_parsed` |
| Logged-in preflight prevents destructive relogin | `logged_in_status_does_not_spawn_device_login` |
| Output chunk-boundary invariance | `device_prompt_parser_is_chunk_boundary_invariant` |
| Cancellation cleanup | `login_cancellation_reaps_and_reconciles` |
| Drain after truncation | `regression_login_output_drains_after_truncation` |
| Parent crash kills bound child | `login_parent_crash_does_not_leave_runnable_orphan` |
| Parent-death install race | `login_parent_death_binding_race_fails_closed` |
| Instruction deadline terminates group | `login_instruction_deadline_kills_group_and_reconciles` |
| Overall deadline terminates group | `login_overall_deadline_kills_group_and_reconciles` |
| Post-spawn failure remains typed after reconciliation | `post_spawn_output_failure_is_reconciled_without_losing_its_type` |
| Malformed or permission-unsafe ledger fails closed | `login_ledger_rejects_malformed_and_permission_unsafe_files` |

# Acceptance

T001, T010, and T002A are Accepted. The pinned fixture decision resolves the device-output and
exit-semantics gap without a real credential. The implementation and every local executable gate
pass, and [`ACCEPT-T002B`](../acceptance/T002B.acceptance.md) records a fresh accepted review.

The earlier documentation commit passed hosted CI in
[run 30263626003](https://github.com/fallrising/fanzloud/actions/runs/30263626003) on `9f1bbcc`; it
predates this implementation and is not presented as hosted implementation evidence.

# Verification Evidence

Local validation on 2026-07-27:

```text
cargo fmt --all -- --check
  passed
cargo test -p codebox-agent-codex --all-features
  passed: 23 unit tests + 12 integration tests
cargo clippy -p codebox-agent-codex --all-targets --all-features -- -D warnings
  passed
cargo clippy --workspace --all-targets --all-features -- -D warnings
  passed
cargo test --workspace --all-targets --all-features
  passed
cargo build --workspace --bins --all-features
  passed
cargo deny check
  passed: advisories, bans, licenses, sources
git diff --check
  passed
```

Of the 23 crate unit tests, one is the retained T002A owner-policy unit test and 22 exercise T002B.
