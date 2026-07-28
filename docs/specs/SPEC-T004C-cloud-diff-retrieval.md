---
id: SPEC-T004C
subject: T004C provider-managed task diff retrieval
status: proposed
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
arbitrary task ID. It contains the validated task ID privately and proves only local eligibility,
not patch safety. `[NEW-SPEC]`

# Preconditions

- T004A revalidates the accepted Linux credential scope, exact CLI version, fixed environment, and
  diagnostic-write sentinel before spawn.
- The task reference came from the current administrator-configured environment through accepted
  T004A/T004B state.
- The cancellation signal is not already requested.
- No browser value selects executable, path, environment, branch, attempt, argv extension, or
  output destination.

A failed precondition starts no diff child and returns a typed redacted error. `[NEW-SPEC]`

# Success

- Executes exactly `codex cloud diff --attempt=1 <TASK_ID>` from the T003 invocation.
- Uses null stdin, non-TTY pipes, cleared environment with only `CODEX_HOME`, the private
  non-repository cwd, and the shared T004A bounded supervisor.
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
- Backpressure: stdout/stderr continue draining after the retained bound until EOF or termination.
- Framing: the entire stdout capture is one raw diff frame; no partial diff is returned.

# Archetype F Answers

- Task authority: only the opaque accepted task reference crosses into the reader.
- Error mapping: ineligible task, scope/version/sentinel, process, timeout, canceled, output-limit,
  provider drift, and invalid diff are typed; no raw lower error is exposed.
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
- T004A's validated `error.log/` directory sentinel prevents the exact pinned Cloud startup
  diagnostic append before the diff child runs.
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

No partial diff escapes, the child is reaped, the diagnostic sentinel is unchanged, and the reader
performs no internal retry. `[NEW-SPEC]`

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
| Opaque eligible task authority | `cloud_diff_requires_accepted_task_reference` |
| Exact fixed diff invocation | `cloud_diff_runner_executes_only_pinned_attempt` |
| Ready/applied eligibility | `cloud_diff_rejects_ineligible_lifecycle` |
| E0 success snapshot | `cloud_diff_success_leaves_managed_state_identical` |
| E0 injected-failure matrix | `cloud_diff_failure_leaves_managed_state_identical` |
| Diagnostic sentinel | `cloud_diff_cannot_append_pinned_error_log` |
| Bounds and continued drain | `cloud_diff_runner_drains_after_limit` |
| Cancellation and reap | `cloud_diff_cancel_reaps_without_partial_result` |
| Chunk partition invariance | `cloud_diff_runner_is_chunk_partition_invariant` (property-based) |
| Redacted error/debug | `cloud_diff_runner_errors_and_debug_are_redacted` |
| No apply or execution | `cloud_diff_has_no_local_application_surface` |

# Common Test Partitions

- Empty/one byte/2 MiB/2 MiB plus one, LF/TAB/control, valid/invalid UTF-8.
- Ready/applied versus pending/error/canceled/unknown/stale task references.
- Exit zero/nonzero/missing/signal, timeout, cancellation, network loss, stdout/stderr overflow.
- Failure before spawn, after spawn, mid-stream, after exit, and during projection.
- Sentinel directory, file, symlink, wrong owner/mode, and replacement race.

# Traceability and Gaps

The opaque task authority, exact bounds, and no-apply surface are `[NEW-SPEC]` derivations of
INV-007/INV-011 and accepted lower contracts. ADR-0003 resolves the E0 state scope. No in-scope
`[TD-GAP]` remains.
