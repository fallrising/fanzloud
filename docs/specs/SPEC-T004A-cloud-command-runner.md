---
id: SPEC-T004A
subject: T004A trusted Codex Cloud command runner
status: accepted
contract_units: [CU-CLOUD-P0-01]
archetypes: [C, F]
atomicity: E2
retriable: false
---

# Normative Inputs

- TD §§1.6–1.7, 2.2–2.3, 4.8, 8.2–8.10, 11.1–11.3 P14–P15, 14, and 15.0
- ADR-0002 §§Credential boundary, First execution interface, and Initial repository boundary
- ADR-0003
- T004A task and SPEC-T004
- Accepted SPEC-T002, SPEC-T002A, SPEC-T002B, and SPEC-T003
- Official Codex CLI `rust-v0.145.0` Cloud source used by SPEC-T003

# Contract Boundary

`CloudTaskRunner` is trusted platform infrastructure for one administrator-configured
`CredentialScope`. It starts only accepted T003 version, exec, status, and list invocations. It
combines the accepted credential lease, cleared environment, fixed argv, private working
directory, bounded supervision, exact completed-capture decoders, and a durable submit ledger.

The runner does not decide the provider task lifecycle after a task ID is recorded, retrieve a
diff, implement `AgentBackend`, or expose a general command builder. Contract:
CU-CLOUD-P0-01. `[NEW-SPEC]`

# Public Boundary

The public behavior is represented by these provider-specific types; exact private fields and
threading types may differ without changing the contract:

```rust
pub struct CloudTaskRunner { /* one validated credential scope */ }
pub struct CloudRunnerConfig {
    /* CredentialScope + CloudEnvironmentId + CloudBranch */
}
pub struct CloudCancellation { /* cloneable one-way cancellation signal */ }
pub struct CloudSubmitRequest {
    /* CloudSubmitOperationId + CloudPrompt */
}
pub struct CloudSubmission {
    pub operation_id: CloudSubmitOperationId,
    pub task_id: CloudTaskId,
}
pub struct CloudReconciliation {
    pub operation_id: CloudSubmitOperationId,
    pub tasks: Vec<CloudTaskSummary>,
    pub complete: bool,
}

impl CloudRunnerConfig {
    pub fn new(
        scope: CredentialScope,
        environment: CloudEnvironmentId,
        branch: CloudBranch,
    ) -> Self;
}

impl CloudSubmitOperationId {
    pub fn new() -> Self;
}

impl CloudSubmitRequest {
    pub fn new(operation_id: CloudSubmitOperationId, prompt: CloudPrompt) -> Self;
}

impl CloudTaskRunner {
    pub fn new(config: CloudRunnerConfig) -> Result<Self, CloudRunnerError>;
    pub fn submit(
        &self,
        request: CloudSubmitRequest,
        cancel: CloudCancellation,
    ) -> Result<CloudSubmission, CloudRunnerError>;
    pub fn status(&self, task_id: &CloudTaskId)
        -> Result<CloudTaskStatus, CloudRunnerError>;
    pub fn reconcile_unknown(&self)
        -> Result<CloudReconciliation, CloudRunnerError>;
}
```

`CloudSubmitOperationId` is a non-nil UUID newtype created by the trusted T004B caller before it
invokes the runner; it is never interchangeable with a task or turn ID. `CloudSubmitRequest` owns
that ID and one accepted prompt. The operation ID is the replay key: reusing it can only observe the
same durable submit outcome and can never execute a second Cloud exec. A caller must use a new ID
for a deliberate later independent submission. `[NEW-SPEC]`

`CloudRunnerConfig::new` requires one already validated `CredentialScope`,
`CloudEnvironmentId`, and `CloudBranch`; it has no string/path fallback, environment-variable
lookup, setter, or deserialization surface. Only the trusted application composition root may
construct it. T005 browser input cannot select or replace any field. `CloudCancellation` is created
by the trusted caller and can only transition from live to cancel-requested. `Debug` for config,
request, runner, cancellation, submission, reconciliation, and all errors omits environment,
branch, prompt, URL, title, cursor, raw output, configured paths, and credential content.
`[NEW-SPEC]`

# Preconditions and Authorization

| Condition | Enforcement | Failure disposition |
|---|---|---|
| Linux P0 host and accepted scope paths | T002A revalidation on lease and invocation | Typed scope error, no Cloud process |
| Exact `codex-cli 0.145.0` | Fixed version command under the same lease | Version/provider-drift error, no submit |
| Exact accepted ChatGPT login status | Fixed accepted T002B login-status command under the same lease | Not-authenticated/status error, no Cloud exec |
| One operation in the credential scope | Existing T002A kernel lease | Busy error, no process |
| No unresolved submit ledger | Durable current-operation check | `OutcomeUnknown`, no process |
| Pinned diagnostic append is disabled | Revalidated private `error.log/` directory sentinel | Diagnostic-boundary error, no Cloud process |
| Environment and branch are administrator values | T003 strong values stored in runner configuration | Typed configuration error |
| Prompt is one accepted T003 value | `CloudPrompt` constructor and fixed argv | Typed prompt error |
| Cancellation is not already requested | Checked before durable authorization and spawn | Canceled-before-start, no process |

Authorization for P0 is the accepted fixed environment/branch configuration plus ownership of the
exclusive credential lease after scope, version, and exact ChatGPT login-status validation.
`LoggedOut`, an active/unknown login ledger, malformed status, and status timeout fail before Cloud
exec. There is no browser-selected repository, environment, branch, executable, path, or
capability. Intent is recorded before the login/authorization decision; checked authorization
failure is then durably recorded as `FailedBeforeSpawn`. `[NEW-SPEC]`

The login-ledger preflight reuses the accepted T002B loader/validator for the distinct
`<state_dir>/login-ledger.json`; T004A does not parse, replace, or fork that schema. A nonterminal or
unknown T002B record blocks Cloud authorization until the login broker reconciles it. `[NEW-SPEC]`

# Fixed Process Policy

For every child:

- executable, `CODEX_HOME`, state directory, and cwd come only from the revalidated T002A scope;
- cwd is the private mode-`0700` non-repository working directory;
- the environment is cleared and rebuilt with only `CODEX_HOME`;
- stdin is null and stdout/stderr are pipes, never a TTY;
- argv is copied only from one non-extensible T003 `CloudInvocation`;
- the child creates a new process group and installs the accepted T002B Linux
  `PR_SET_PDEATHSIG=SIGKILL` race closure before exec;
- the dedicated supervisor owns the direct child until it is reaped;
- both streams continue draining after their retained T003 bound so truncation cannot deadlock the
  child; and
- cancellation or deadline sends termination to the group, waits two seconds, then sends
  `SIGKILL` and reaps the direct child.

Before the first Cloud command, the runner creates `<working_dir>/error.log` as a mode-`0700`
directory if it is absent. Before every later Cloud command it opens that exact entry without
following symlinks and verifies that it remains an effective-UID-owned directory with mode `0700`.
An existing file, symlink, wrong owner/mode, replacement, or unavailable entry fails closed before
spawn and is never deleted or repaired automatically. The pinned
[`append_error_log`](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/cloud-tasks/src/util.rs)
uses `OpenOptions::create(true).append(true).open("error.log")` and ignores open failure, so a
directory at that path makes the append a no-op. The reviewed
[`init_backend` path](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/cloud-tasks/src/lib.rs)
calls that helper before exec/status/list/diff provider operations. A CLI version change must review
both Cloud crates for additional cwd-relative writes before retaining this gate. `[NEW-SPEC]`

The runner must refactor or reuse the accepted T002B supervisor mechanism rather than introduce a
second weaker fork/exec policy. The public surface still has no shell, arbitrary environment,
arbitrary argv, repository path, `cloud apply`, local `codex exec`, or extension callback.
`[NEW-SPEC]`

# Bounds

- Version uses the accepted five-second T002B deadline and 64-KiB T003 capture contract.
- Submit has a 60-second wall-clock deadline.
- Status has a 30-second wall-clock deadline.
- Reconciliation has one 60-second overall deadline, visits at most five list pages, and retains at
  most 100 unique tasks because each pinned page contains at most 20.
- Exec, status, and list retain at most 64 KiB per stream and continue draining thereafter.
- Process-group termination uses the accepted two-second grace.
- The operation ledger is at most 64 KiB, has at most 32 history entries, and records at most 100
  reconciliation candidate task IDs.

These are local defensive bounds below the TD 600-second P1 maximum. They are configuration
constants for P0, not browser input. `[NEW-SPEC]`

# Submit Commit Order

The runner holds the credential-scope lease for the whole submit call:

```text
scope revalidation
→ load/recover/replay current operation
→ exact version check for a new operation
→ durable Intent(operation_id)
→ exact login-status check
→ durable Authorized
→ spawn fixed exec
→ durable Started(pid,start_time)
→ bounded drain + terminal wait
→ exact T003 exec decode
→ durable TaskRecorded(task_id)
→ return CloudSubmission
```

Each ledger transition is persisted before the next side-effect boundary. A successful task ID is
never returned until `TaskRecorded` and its directory entry are durable. The prompt, raw task URL,
stdout, stderr, cursor, title, diff, environment credentials, and `CODEX_HOME` path are never
stored. The accepted administrator environment/branch identifiers, operation ID, process identity,
task ID, redacted phase, and bounded reconciliation evidence may be stored. `[NEW-SPEC]`

Before every submit dispatch, while holding the scope lease, the runner loads and classifies the
current ledger. A bare `Intent` proves authorization/spawn did not occur, so recovery first commits
terminal `FailedBeforeSpawn`. `Authorized` or `Started` is durably changed to `OutcomeUnknown`;
existing `OutcomeUnknown` or `ReconciliationObserved` remains unknown. No recovery classification
executes a Cloud command. `[NEW-SPEC]`

If the resulting current ledger is `TaskRecorded` for the request's operation ID, `submit` returns
that durable task without executing version, login status, or Cloud exec. If it is
`FailedBeforeSpawn` for the same ID, `submit` returns the typed prior pre-start failure without
executing; an explicit retry requires a new operation ID. A different operation ID may begin only
when the current record is terminal `TaskRecorded` or `FailedBeforeSpawn`. Any request against
`Authorized`, `Started`, `OutcomeUnknown`, or `ReconciliationObserved` returns unknown without
Cloud exec. The prompt is not persisted; operation identity, not prompt equality, defines replay.
`[NEW-SPEC]`

# Durable Ledger

The runner uses a distinct version-1 current-operation record at
`<state_dir>/cloud-submit-ledger.json`. The record is a mode-`0600`, effective-UID-owned, regular
single-link file opened with no symlink following. Replacement uses
`.cloud-submit-ledger.<operation-id>.tmp`, mode `0600`, file sync, atomic rename, and
state-directory sync. Implementation may extract the accepted T002B atomic-file mechanism, but
login and Cloud schemas/files remain separate. Malformed, oversized, unknown-version, wrong-owner,
wrong-mode, symlinked, or hard-linked state fails closed and is never discarded automatically.

Allowed phases are:

```text
Intent → Authorized
Intent → FailedBeforeSpawn
Authorized → Started
Authorized → FailedBeforeSpawn
Authorized → OutcomeUnknown
Started → TaskRecorded
Started → OutcomeUnknown
OutcomeUnknown → ReconciliationObserved
ReconciliationObserved → ReconciliationObserved
```

`Started` contains the Linux PID/start-time identity. `TaskRecorded` contains the validated task
ID. `ReconciliationObserved` retains `OutcomeUnknown` as the operation disposition and records only
bounded validated candidate IDs plus whether pagination completed. Terminal `TaskRecorded` and
`FailedBeforeSpawn` permit a later independent submission; `OutcomeUnknown` and
`ReconciliationObserved` do not. `[NEW-SPEC]`

# Crash, Cancellation, and Failure Matrix

| Crash or failure point | Durable/result classification | Next action |
|---|---|---|
| Before `Intent` commit | Proven no submit attempt | Caller may explicitly call again |
| After `Intent`, before `Authorized` | `FailedBeforeSpawn` on recovery after scope revalidation | Caller may explicitly call again |
| Login/diagnostic/cancellation failure after `Intent` and before `Authorized` | Durable `FailedBeforeSpawn` | Repair the precondition or explicitly call again |
| After `Authorized`, before durable `Started` | `OutcomeUnknown` | Bounded list reconciliation; no automatic retry |
| Spawn syscall proves no child was created | Durable `FailedBeforeSpawn` | Explicit caller retry is allowed |
| Any child exists but `Started` cannot be committed | Existing durable state is treated as `OutcomeUnknown` | Repair storage, reap/verify child, reconcile |
| After `Started`, before durable `TaskRecorded` | `OutcomeUnknown` | Bounded list reconciliation; no automatic retry |
| Timeout, cancellation, parent/process kill, missing exit, overflow, nonzero exec, malformed exec output, or post-spawn ledger failure | `OutcomeUnknown` | Bounded list reconciliation; no automatic retry |
| After durable `TaskRecorded`, before return | Replay with the same operation ID returns the recorded submission; no second exec | Caller observes existing task |
| Status/list timeout, drift, or failure | Typed read error; submit ledger unchanged | Caller may issue a later read |

If durable state cannot be updated after a child may exist, the public error is
`RecoveryRequired`, never a pre-start failure. The in-memory runner still terminates/reaps any child
it owns, but process cleanup is not evidence that the provider submit did not occur.
`[NEW-SPEC]`

# Reconciliation

`reconcile_unknown` is valid only for the current unknown operation. It executes fixed
`cloud list --env=<configured> --limit=20 --json` pages through the T003 cursor policy until the
cursor ends, a duplicate cursor/task ID appears, five pages/100 tasks are retained, the overall
deadline expires, or a checked failure occurs.

A duplicate task ID across pages or repeated cursor is provider drift and fails closed. A limit or
deadline returns the validated retained tasks with `complete: false`; normal cursor exhaustion
returns `complete: true`. The observed candidate IDs and completeness flag are recorded durably
without changing the unknown disposition.

The pinned list rows contain no stable client submission key or exact prompt/branch correlation.
Therefore neither zero, one, nor many candidates proves which task belongs to the interrupted
submit. T004A does not select, adopt, abandon, or retry a candidate and exposes no retry method.
T004B must require an explicit operator recovery action if it resolves the lifecycle. This is the
conservative implementation of ADR-0002 and INV-006, not an inferred provider guarantee.
`[NEW-SPEC]`

# Archetype C Answers

- Atomicity: E2. A provider task may exist without a durable task ID, but the local operation and a
  bounded provider task list are observable.
- Duplicate prevention: one caller-supplied strong operation ID, one scope lease, one durable
  current-operation record, intent and authorization before spawn, task ID before success, and no
  submit API while the record is unknown.
- `OutcomeUnknown`: returned as a typed redacted error carrying only the operation ID. It is also
  durable and queryable through `reconcile_unknown`.
- Crash points: every boundary is classified by the failure matrix; any ambiguity after durable
  authorization is unknown.
- Retry: there is no automatic retry and no runner retry method. Replaying the same operation ID is
  observation-only once a task is recorded; reconciliation alone does not make an unknown action
  retryable.

# Process Stream and Trust-Boundary Answers

- Ordering: bytes within each pipe retain source order; no stdout/stderr cross-order is inferred.
- Termination: every command ends with a concrete captured exit, a timeout/cancel capture, or a
  typed uncertain supervisor failure; the direct child is reaped.
- Cancellation: a pre-authorization request is no-effect; after authorization it is conservatively
  unknown. Cancellation targets the local process group only.
- Chunk boundaries: the existing byte-preserving T003 property applies to completed capture; the
  live runner has property tests over arbitrary read chunking.
- Backpressure: both drains continue after the retained bound until EOF/termination.
- Framing: T003 is the only successful decoder. Partial, overflowed, missing-exit, or malformed
  frames never produce a public task/status/page.
- Error mapping: scope, busy, version, ledger, process, timeout, canceled, output-limit,
  provider-drift, and unknown/recovery-required categories are typed and redacted.
- Idempotency: status/list are provider reads; submit is non-idempotent and never automatically
  repeated.
- Redaction: no error or `Debug` includes prompt, raw bytes, URL, title, cursor, path, environment
  content, account metadata, or credential material.

# Security and P14

The pinned upstream implementation attempts to append account/diagnostic data to cwd-relative
`error.log`. T004A prevents that write with the validated directory sentinel above. Codebox never
reads, parses, copies, logs, serves, archives, or publishes content below that sentinel. Deployment
backup/artifact collection must exclude the private working directory. `[NEW-SPEC]`

Full P14 tests use a credential canary, a repository tree containing executable hooks/scripts with
sentinels, a fake pinned CLI, and an argv/environment/cwd observation channel owned by the test.
They prove:

- cwd and every configured path are outside the repository;
- no repository path or executable enters argv/environment;
- only `CODEX_HOME` survives `env_clear`;
- no shell, hook, repository script, checkout, `codex exec`, or `cloud apply` runs;
- prompt metacharacters remain one argv item; and
- the canary and fake raw output never enter returned values, errors, ledger, logs, events,
  artifacts, or diff projections.

The trusted Codex CLI is allowed to read its own `CODEX_HOME`; P14 proves that credential state is
not forwarded into a repository-controlled process or public projection. `[NEW-SPEC]`

# Exit Invariants

After success, checked failure, cancellation, timeout, or recovery:

- no owned direct child remains unreaped;
- any provider submit that may have occurred is `TaskRecorded` or `OutcomeUnknown`;
- success implies exactly one durable validated task ID;
- an unknown current record prevents another submit;
- no automatic path invoked Cloud exec more than once for one operation;
- cwd diagnostics and raw captures remain private; and
- the credential-scope lease is released only after child ownership and ledger disposition are
  settled as far as the available storage permits.

# Non-Guarantees

- List reconciliation does not prove task identity, absence, or provider consistency.
- Local process cancellation does not cancel a provider task that may already exist.
- A `TaskRecorded` result does not mean the provider task is complete or successful.
- Reusing an operation ID with a different prompt never creates a new task and does not prove the
  persisted prompt matched, because raw prompts are intentionally not stored.
- The runner does not protect against a compromised administrator, host, or configured Codex
  binary.
- The directory sentinel prevents only the exact reviewed `0.145.0` append implementation; it does
  not guarantee that a future CLI version has no other filesystem writes.
- Status/list calls do not rate-limit future T005 polling; T005 must define its own poll schedule.

# Required Test Skeletons

| Clause | Required test |
|---|---|
| Fixed submit/status/list execution | `cloud_runner_executes_only_pinned_invocations` |
| Scope, version, login, and authorization preflight | `cloud_submit_fails_closed_before_spawn` |
| Diagnostic append prevention for exec/status/list | `cloud_runner_blocks_pinned_error_log_write` |
| Intent/authorization/started/task commit order | `cloud_submit_ledger_precedes_each_effect_boundary` |
| Success requires durable task ID | `cloud_submit_returns_only_durable_task` |
| Same-ID replay after response loss | `cloud_submit_replay_returns_recorded_task_without_exec` |
| Every submit crash point | `cloud_submit_crash_matrix_is_reconcilable` |
| Cancellation/timeout classification and reap | `cloud_submit_cancel_and_timeout_reap_as_unknown` |
| Parent death and install race | `cloud_submit_parent_death_does_not_leave_runnable_child` |
| Drain after output limit | `regression_cloud_output_drains_after_truncation` |
| Live chunk invariance | `cloud_runner_capture_is_chunk_partition_invariant` (property-based) |
| Status exit mapping through real runner | `cloud_runner_status_uses_pinned_exit_mapping` |
| Bounded pagination | `cloud_reconciliation_is_bounded_and_detects_cycles` |
| Candidate ambiguity | `cloud_reconciliation_never_infers_task_identity` |
| Existing unknown blocks submit | `cloud_unknown_submit_blocks_another_exec` |
| Malformed/unsafe ledger | `cloud_submit_ledger_fails_closed` |
| Redacted errors/debug/ledger | `cloud_runner_outputs_and_state_are_redacted` |
| Full TD P14 launcher regression | `regression_cloud_runner_never_executes_repository_code` |
| Exact TD P15 regression | `regression_unknown_cloud_submit_reconciles_before_retry` |

P15 must inject interruption after the fake provider records a submit but before Codebox persists
the task ID, restart the runner, invoke reconciliation, and prove the fake Cloud exec count remains
exactly one. Zero, one, and multiple returned list candidates all remain non-retry-authorizing.

The durable-response-loss test injects a crash after `TaskRecorded` but before response delivery,
replays the same operation ID, and proves the same task is returned with a Cloud exec count of one.

# Common Test Partitions

- Empty/one/20/21 and one/five/six list pages; no cursor, cursor, repeated cursor, duplicate task.
- Cancel before intent, after intent, after authorization, after spawn, after provider record, after
  task-ledger commit, and after return.
- Spawn failure, timeout, SIGTERM success, SIGKILL escalation, parent death, network loss, output
  overflow, malformed output, disk full/rename failure, and corrupt ledger.
- Prompt spaces/metacharacters/newlines, canary credential, hostile repository hooks, and hostile
  replacement of the cwd `error.log/` sentinel.
- Repeated calls, concurrent lease contention, restart with intent/authorized/started/recorded/
  unknown records.

# Traceability and Gaps

All bounds, ledger shapes, process reuse, and conservative reconciliation behavior are
`[NEW-SPEC]` local derivations of TD §§4.8, 8, 11 and ADR-0002. They do not authorize a retry or
weaken the credential boundary. No in-scope `[TD-GAP]` remains; T004B owns explicit provider task
lifecycle and recovery action after T004A returns candidates.

# Design Review

The first fresh Cursor Agent review rejected the draft because `CloudTaskRunner::new` did not carry
the mandatory administrator environment/branch and the P0 graph contradicted its own generic
backend recommendation. The revision added the typed config, ADR-0003, exact login preflight,
pinned-source diagnostic sentinel trace, and distinct T004A/T004B retry tests.

The second fresh, read-only review returned `DESIGN ACCEPTED`. It found the E2 ledger/commit matrix,
CU boundary, bounds, redaction, P14/P15 ownership, cancellation non-guarantee, and test mapping
complete enough to make T004A the sole Ready task.

A final focused fresh review accepted the caller-created operation ID and observe-only same-ID
replay correction. It required pre-authorization `Intent` recovery to commit
`FailedBeforeSpawn` rather than unknown; that correction is normative above. The review confirmed
that all 19 required test skeletons exist and production implementation may begin.

The first implementation review found and closed two blockers: ambiguous post-spawn decoder
failure now returns `OutcomeUnknown`, and the exact P15 test now proves zero/one/many candidates do
not authorize another exec after restart. The final implementation, including explicit
fork-inherited lease release, passed a fresh Cursor Agent acceptance review with no blocker.
