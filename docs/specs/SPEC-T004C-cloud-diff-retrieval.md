---
id: SPEC-T004C
subject: T004C provider-managed task diff retrieval
status: specifying
contract_units: [CU-CLOUD-P0-02]
archetypes: [D, F]
atomicity: E0
retriable: false
---

# Normative Inputs

- TD §§2.3 INV-007 and INV-011, 8.2–8.10, 11, 14, and 15.0
- ADR-0002 and ADR-0003
- T004C task and SPEC-T004
- Accepted T003 and, before implementation, T004A/T004B
- Official Codex CLI `rust-v0.145.0` Cloud source used by SPEC-T003

# Contract Boundary

`CloudDiffReader` executes only the pinned T003 diff invocation through the accepted T004A process
boundary and returns one bounded untrusted `CloudDiff`. It does not parse, normalize, store,
summarize, apply, execute, or publish the diff. Contract: CU-CLOUD-P0-02.

# Public Boundary

```rust
pub struct DiffEligibleCloudTask { /* opaque accepted task reference */ }
pub struct CloudDiffReader { /* same trusted runner as the orchestrator */ }

impl CloudTaskOrchestrator {
    pub fn diff_reader(&self) -> CloudDiffReader;
    pub fn diff_eligible_task(
        &self,
    ) -> Result<DiffEligibleCloudTask, CloudDiffReadError>;
}

impl CloudDiffReader {
    pub fn retrieve(
        &self,
        task: &DiffEligibleCloudTask,
        cancel: CloudCancellation,
    ) -> Result<CloudDiff, CloudDiffReadError>;
}
```

`DiffEligibleCloudTask` is minted only by T004B from a durably recorded or explicitly adopted task
whose latest lifecycle is `Ready` or `Applied`. Browser input cannot construct it directly from an
arbitrary task ID. It contains the exact operation ID and validated task ID privately and proves
only local eligibility, not patch safety. It has no public constructor, deserializer, raw-ID
conversion, or mutable field. Its `Debug` projection is redacted. `[NEW-SPEC]`

`CloudDiffReader` has no public constructor from `CloudRunnerConfig`, `CredentialScope`,
`CloudTaskRunner`, executable, path, or environment values. `CloudTaskOrchestrator::diff_reader`
clones its already accepted private runner handle, so the lifecycle, reader, and lower ledgers share
one trusted configuration and scope. This is a T004C composition surface and does not add diff to
the public T004A `CloudTaskRunner` API. `[NEW-SPEC]`

`diff_eligible_task` locks the current orchestrator state and succeeds only when the exact durable
phase is `Ready` or `Applied`; every other or absent phase returns a typed redacted ineligible
error. At retrieval, the reader acquires its runner's scope lease and, before any diff spawn,
re-loads both the T004B lifecycle record and T004A submit record without recovery or mutation. The
lifecycle must still be `Ready` or `Applied` with the token's exact operation and task IDs. The
submit record must use the reader's exact configured environment and branch, name the same
operation and recorded/adopted task, and end in exactly `TaskRecorded` or `TaskAdopted`. The reader
holds that same scope lease until the diff child is reaped. This makes stale/replaced and
wrong-scope authority fail closed without a browser-visible scope fingerprint or a time-of-check
window against another accepted runner operation. `[NEW-SPEC]`

The redacted public failure boundary is:

```rust
pub enum CloudDiffReadErrorCategory {
    IneligibleLifecycle,
    AuthorityMismatch,
    Scope,
    Busy,
    Version,
    DiagnosticBoundary,
    Process,
    Timeout,
    Canceled,
    OutputLimit,
    ProviderDrift,
    InvalidDiff,
}

pub struct CloudDiffReadError { /* safe category only */ }
```

The exact private error representation may differ, but public errors and `Debug` expose only a safe
category. They never retain operation/task IDs, raw lower errors, task URLs, diff bytes, streams,
paths, environment/branch values, account data, or credentials. `[NEW-SPEC]`

# Preconditions

- The cancellation signal is checked before acquiring execution authority and is not already
  requested.
- The reader revalidates the accepted Linux credential scope, exact CLI version, fixed
  environment/branch provenance, lifecycle/submit authority, and existing diagnostic-write
  sentinel before spawn.
- The task reference came from the current administrator-configured environment through accepted
  T004A/T004B state and is still the current durable `Ready` or `Applied` task.
- No browser value selects executable, path, environment, branch, attempt, argv extension, or
  output destination.

A failed precondition starts no diff child, creates or repairs no sentinel or ledger, and returns a
typed redacted error. `[NEW-SPEC]`

# Success

- Executes exactly `codex cloud diff --attempt=1 <TASK_ID>` from the T003 invocation.
- Uses null stdin, non-TTY pipes, cleared environment with only `CODEX_HOME`, the private
  non-repository cwd, and the shared T004A bounded supervisor with diff-specific capture limits.
- Requires one concrete zero exit, empty stderr, no overflow, and a successful T003 diff decode.
- Returns at most 2 MiB of valid bounded untrusted UTF-8, including an allowed empty diff.
- Leaves the provider task and every Codebox-managed durable record unchanged.

# Archetype D Answers

- Ordering: stdout bytes retain source order; stderr is separate and no cross-stream order is
  inferred.
- Termination: success requires a terminal zero exit. Timeout, cancel, missing exit, signal,
  nonzero exit, overflow, malformed capture, or supervisor uncertainty is a typed failure.
- Cancellation: the shared supervisor terminates and reaps the local process group. The operation
  is a provider read and does not claim provider task cancellation.
- Chunk-boundary invariance: every byte partition of the same retained streams produces the same
  T003 result; live drain partitioning receives a property test.
- Backpressure: the diff invocation retains at most 2 MiB stdout and 64 KiB stderr. Both streams
  continue draining after their individual retained bound until EOF or termination. The shared
  supervisor's other accepted invocations retain their existing 64-KiB-per-stream limits.
- Framing: the entire stdout capture is one raw diff frame; no partial diff is returned.

# Archetype F Answers

- Task authority: only the opaque accepted task reference crosses into the reader; the current
  lifecycle and lower submit provenance are revalidated under the held scope lease.
- Error mapping: ineligible lifecycle or authority, scope/busy/version/sentinel, process, timeout,
  canceled, output-limit, provider drift, and invalid diff are typed; no raw lower error is
  exposed.
- Idempotency: retrieval is specified as an E0 read and the reader performs no internal retry. A
  caller may make a later explicit read under the ADR-0003 managed-state boundary.
- Bounds: one 60-second command deadline, two-second termination grace, 2 MiB retained per stdout
  and 64 KiB retained stderr, with continued drain.
- Redaction: typed errors and `Debug` omit task URL, diff bytes, raw streams, paths, account data,
  credentials, and provider text.

# Security and No-Apply Contract

- There is no `cloud apply`, local patch parser, filesystem writer, shell, repository checkout,
  hook, subprocess callback, or artifact write in this CU.
- Diff bytes are data only. They are not interpreted as paths, ANSI, Markdown trust, commands, or
  configuration.
- T004A's existing validated `error.log/` directory sentinel prevents the exact pinned Cloud startup
  diagnostic append before the diff child runs. T004C validates it as an owned mode-`0700`
  no-follow directory immediately before spawn and fails if it is absent or unsafe; it never
  creates, replaces, repairs, chmods, or removes the sentinel.
- The private working directory and `CODEX_HOME` are never served or collected as artifacts.
- T005/T006 may render the bounded diff as escaped text; that browser/content security contract is
  not owned here. `[NEW-SPEC]`

# Atomicity Scope and Exit Invariant

ADR-0003 resolves the E0 scope. On success or injected failure, the snapshot includes and leaves
identical:

- provider task/repository state;
- T004A submit ledger and T004B lifecycle projection;
- lease metadata, task eligibility, and configuration;
- the trusted working directory and diagnostic sentinel; and
- every Codebox-managed event, artifact, log, and durable record.

The snapshot excludes byte identity of provider-owned `CODEX_HOME`, remote access/audit logs,
network telemetry, and host access timestamps. ADR-0002 authorizes the official CLI to operate its
credential store, and Codebox must not read credential bytes to compare them. This exclusion does
not permit credential data to cross into Codebox-managed state.

No partial diff escapes, the child is reaped, the lifecycle and submit records remain byte
identical, the diagnostic sentinel is unchanged, and the reader performs no internal retry.
`DiffEligibleCloudTask` itself is immutable; retrieval neither consumes nor refreshes its authority.
Any later explicit retrieval repeats the full durable eligibility check. `[NEW-SPEC]`

# Non-Guarantees

- A retrieved diff is not safe, applicable, complete, or from a semantically verified repository
  revision.
- `Ready`/`Applied` eligibility does not prove the patch is free of secrets or malicious content.
- The reader does not provide syntax highlighting, artifacts, pagination, local application, or
  repository path validation.
- Local cancellation does not alter the provider task.
- E0 does not promise byte-identical provider-owned credential cache, remote audit/telemetry, or
  host access timestamps.

# Required Test Skeletons

| Clause | Required test |
|---|---|
| Opaque eligible task authority, same-scope reader | `cloud_diff_requires_accepted_task_reference` |
| Exact fixed diff invocation | `cloud_diff_runner_executes_only_pinned_attempt` |
| Ready/applied eligibility and stale/replaced rejection | `cloud_diff_rejects_ineligible_lifecycle` |
| E0 success snapshot | `cloud_diff_success_leaves_managed_state_identical` |
| E0 injected-failure matrix | `cloud_diff_failure_leaves_managed_state_identical` |
| Diagnostic sentinel | `cloud_diff_cannot_append_pinned_error_log` |
| 2-MiB/64-KiB bounds and continued drain | `cloud_diff_runner_drains_after_limit` |
| Cancellation and reap | `cloud_diff_cancel_reaps_without_partial_result` |
| Chunk partition invariance | `cloud_diff_runner_is_chunk_partition_invariant` (property-based) |
| Redacted error/debug | `cloud_diff_runner_errors_and_debug_are_redacted` |
| No apply or execution | `cloud_diff_has_no_local_application_surface` |

# Common Test Partitions

- Empty/one byte/2 MiB/2 MiB plus one, LF/TAB/control, valid/invalid UTF-8.
- Ready/applied versus pending/error/canceled/unknown/stale/replaced task references and a reader
  from a different trusted scope.
- Exit zero/nonzero/missing/signal, timeout, cancellation, network loss, stdout/stderr overflow.
- Failure before spawn, after spawn, mid-stream, after exit, and during projection.
- Existing sentinel directory versus missing, file, symlink, wrong owner/mode, and replacement
  before the final validation.

# Traceability and Gaps

The opaque task authority, exact bounds, validate-only sentinel, and no-apply surface are
`[NEW-SPEC]` derivations of INV-007/INV-011 and accepted lower contracts. The first fresh Cursor
Agent design review rejected the earlier draft because it omitted the authority mint API,
same-scope/retrieval-time binding, reader construction rule, and projection of diff-specific
capture limits onto the shared supervisor. This revision closes those implementation-blocking
gaps while rejecting a public `CloudDiffReader::new(&CloudTaskRunner)` constructor in favor of
orchestrator-only composition. ADR-0003 resolves the E0 state scope. No in-scope `[TD-GAP]`
remains.
