---
id: SPEC-T002
subject: T002 Codex login broker
status: blocked
contract_units: [CU-AUTH-P0-01, CU-AUTH-P0-02]
archetypes: [B, C, E, F]
atomicity: E2
retriable: false
---

# Normative Inputs

- TD §§1.6–1.7, 2.2, 2.3, 8.2–8.10, 11.3 P14, and 15.0
- ADR-0002 §§Credential boundary and First execution interface
- T002 task document
- T010 domain values
- Codex manual [Configuration, Authentication, and Models](https://learn.chatgpt.com/docs/config-file/config-reference)
  and [Authentication](https://learn.chatgpt.com/docs/auth)

# Contract Boundary

The broker is trusted platform infrastructure. It owns one administrator-configured Codex
executable, one operator-scoped `CODEX_HOME`, a single active login lease, and typed projections
for login state.

The broker may invoke only fixed commands equivalent to:

```text
codex login --device-auth
codex login status
codex logout
```

The executable path, exact version, `CODEX_HOME`, working directory, and environment allowlist are
administrator configuration. Browser input cannot select or override them.

# Known Official Surface

The current Codex manual documents that `CODEX_HOME` is the root for config, auth, logs, sessions,
skills, and package metadata; it must already exist. It documents device authentication through
`codex login --device-auth`, status inspection through `codex login status`, and credential stores
including file, keyring, and auto. It also warns that file-based `auth.json` contains access tokens.

# Public Operations

## `start_device_login`

### Preconditions

- The operator scope is configured and not leased by another login operation.
- `CODEX_HOME` exists, is a directory, is owned by the runner identity, and has no group/other
  permissions.
- The configured executable version matches the administrator pin.
- No repository path, prompt, token, or browser-provided command argument is accepted.

### Success

- Starts exactly one fixed `codex login --device-auth` process in a non-repository trusted working
  directory.
- Returns only a bounded `LoginInteraction` containing provider verification instructions after
  redaction and validation.
- Persists intent and started state without credential material.

### Failure and caller action

- Unsafe scope or executable configuration: typed configuration error; operator must repair it.
- Existing lease: `LoginAlreadyRunning`; caller must observe the active operation.
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
- Returns a redacted, bounded status projection only.

### Failure and caller action

- Unknown output is `ProviderOutputInvalid`; do not infer `LoggedIn`.
- CLI timeout or process failure is `StatusUnavailable`; caller may poll later, but must not start a
  new login while an operation is uncertain.

## `reconcile`

An interrupted login is reconciled by a bounded `status` operation and the durable login ledger.
If the status cannot prove `LoggedIn` or `LoggedOut`, the operation remains `OutcomeUnknown` and
automatic retry is forbidden.

# [TD-GAP: T002 / device-code output and exit semantics]

Question: What exact machine-readable fields and process termination semantics does the pinned
Codex CLI provide for `codex login --device-auth` and `codex login status`?

Known design: The public manual documents the commands and human-facing purpose, but not a stable
JSON schema or complete exit-state contract for the experimental headless interaction.

Candidate A: Capture and version fixtures from the pinned CLI, then define a bounded parser for the
observed output and exit states.

Candidate B: Require an official machine-readable output flag or stable API before implementation.

Recommendation: Candidate A for the private pinned-CLI P0, provided fixtures are captured without
real credentials and unknown output remains a typed failure.

Impact: Blocks T002 implementation and T004's login precondition. It does not block T010 or the
documentation work that identifies the gap.

# Security Boundary

- `CODEX_HOME` is never a repository, workspace, artifact root, or browser-selected path.
- The runner passes no secret-bearing environment variable except the process's configured
  credential-store settings; it must not pass `CODEX_ACCESS_TOKEN`, `CODEX_API_KEY`, or token text
  from the browser.
- Captured stdout/stderr is treated as untrusted process output, bounded before parsing, and
  redacted before any response or durable record.
- The runner never launches shell commands, repository hooks, repository scripts, or `codex exec`.
- Permission checks and canary tests prove that a fake CLI cannot read or emit the credential store
  through a repository-controlled command.

# State and Failure Semantics

```text
Idle → Starting → DeviceInstructions → Completing → LoggedIn
                    ├→ LoggedOut
                    ├→ Failed
                    └→ OutcomeUnknown → Reconciling
```

The login side effect is E2: credentials may be written by the provider CLI, but status can query
the result. No automatic retry is allowed while the result is unknown. Every operation records
intent, started, and one terminal or unknown outcome without recording credential bytes.

# Concurrency and Cleanup

- A per-operator lease serializes login and logout operations.
- A second caller observes the existing operation rather than spawning another CLI.
- Cancellation terminates the trusted CLI process and reaps it; if completion cannot be proven,
  the operation becomes `OutcomeUnknown` and requires reconciliation.
- Temporary process output and working directories are removed or marked for recovery without
  touching `CODEX_HOME` contents outside the provider CLI's ownership.

# Non-guarantees

- The broker does not validate the provider account's entitlement beyond the official CLI status.
- The broker does not expose access tokens, refresh tokens, or `auth.json` content.
- The broker does not protect against a compromised trusted host or a malicious provider binary
  configured by an administrator.
- Device-code instructions are not a general OAuth API contract; they are bounded projections of
  the pinned CLI fixture.

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
| No secret persistence or response | `login_credentials_never_reach_events_or_artifacts` |
| Unknown outcome retry rule | `login_unknown_outcome_is_not_retried` |
| Crash/recovery exit invariant | `login_crash_leaves_reconcilable_ledger` |

# Acceptance

T002 is blocked by the `[TD-GAP]` above and by T001/T010 acceptance. No production implementation
or live subscription login is authorized by this draft.

The documentation commit passed hosted CI in [run 30263626003](https://github.com/fallrising/fanzloud/actions/runs/30263626003)
on `9f1bbcc`; this validates repository gates only and does not resolve the T002 contract gap.
