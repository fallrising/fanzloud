---
id: SPEC-T004B
subject: T004B Codex Cloud task lifecycle policy
status: ready
contract_units: [CU-AGT-P0-02]
archetypes: [C, D]
atomicity: E2
retriable: false
---

# Normative Inputs

- TD §§2.3 INV-005, INV-006, INV-012, 4.3, 4.8, 5.1, 8.2–8.10, 11, 14, and 15.0
- ADR-0002 and ADR-0003
- T004B task and SPEC-T004
- Accepted T004A and, before implementation, accepted T004A1

# Contract Boundary

`CloudTaskOrchestrator` maps one T004A submission operation and its provider task into a normalized
P0 lifecycle. It owns local lifecycle state, explicit inspection, unknown-submit recovery, and
local cancellation. It does not execute arbitrary commands, schedule background polling, retrieve
diff text, implement the generic `AgentBackend`, or claim a provider-side cancel operation.
Contract: CU-AGT-P0-02. `[NEW-SPEC]`

# Public Boundary

The behavior is represented by:

```rust
pub enum CloudLifecycle {
    Submitting { operation_id: CloudSubmitOperationId },
    FailedBeforeSubmit { operation_id: CloudSubmitOperationId },
    OutcomeUnknown { operation_id: CloudSubmitOperationId },
    Pending { operation_id: CloudSubmitOperationId, task_id: CloudTaskId },
    Ready { operation_id: CloudSubmitOperationId, task_id: CloudTaskId },
    Applied { operation_id: CloudSubmitOperationId, task_id: CloudTaskId },
    ProviderError { operation_id: CloudSubmitOperationId, task_id: CloudTaskId },
    CanceledLocally {
        operation_id: CloudSubmitOperationId,
        task_id: Option<CloudTaskId>,
        provider_may_continue: bool,
    },
    AbandonedUnknown { operation_id: CloudSubmitOperationId },
}

pub enum UnknownSubmitDecision {
    AdoptListedTask(CloudTaskId),
    AbandonAfterReconciliation(DuplicateRiskAcknowledgement),
}

impl CloudTaskOrchestrator {
    pub fn new(config: CloudRunnerConfig)
        -> Result<Self, CloudLifecycleError>;
    pub fn start(&self, prompt: CloudPrompt)
        -> Result<CloudLifecycle, CloudLifecycleError>;
    pub fn inspect(&self) -> Result<CloudLifecycle, CloudLifecycleError>;
    pub fn reconcile_unknown(&self)
        -> Result<CloudReconciliation, CloudLifecycleError>;
    pub fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    )
        -> Result<CloudLifecycle, CloudLifecycleError>;
    pub fn cancel(&self) -> Result<CloudLifecycle, CloudLifecycleError>;
}
```

The exact concurrency wrapper may be synchronous or async, but one orchestrator has exactly one
current operation. `DuplicateRiskAcknowledgement::for_operation(operation_id)` is a named value
created only after the authenticated operator accepts duplicate risk; it is not a boolean option
or a value the orchestrator creates automatically. `resolve_unknown` rejects an operation ID or
acknowledgement bound to anything other than the exact durable current operation. T005 owns its
HTTP authorization/audit projection. Debug and errors remain redacted under T003/T004A rules.
`[NEW-SPEC]`

Before invoking T004A, `start` creates the strong `CloudSubmitOperationId`,
`CloudSubmitRequest`, and `CloudCancellation`, then durably stores the operation ID and retains the
cancellation signaling half only in memory. Concurrent `cancel` targets that exact signal.
Recovery uses the prompt-free T004A1 observation for the same operation ID; a deliberate later
independent start creates a new one. The signal is cleared only after the submit call and its
ledger disposition are observed; it is never persisted or reused. `[NEW-SPEC]`

# Lifecycle

```text
Idle → Submitting → Pending → Ready
          ├→ FailedBeforeSubmit
          ├→ OutcomeUnknown → ReconciliationObserved
          │                       ├→ Pending|Ready|Applied|ProviderError (explicit adopt)
          │                       └→ AbandonedUnknown (explicit risk acknowledgement)
          └→ CanceledLocally(no provider side effect)
                       Pending ──→ CanceledLocally(provider may continue)
                       Pending ──→ Applied|ProviderError
```

`inspect` performs at most one fixed status read and maps the exact T003 status:

| Provider status | Lifecycle |
|---|---|
| `Pending` | `Pending` |
| `Ready` | `Ready` |
| `Applied` | `Applied` |
| `Error` | `ProviderError` |

`FailedBeforeSubmit`, `Ready`, `Applied`, `ProviderError`, `CanceledLocally`, and
`AbandonedUnknown` are terminal for the local operation. `OutcomeUnknown` is nonterminal but
blocked pending explicit recovery. A caller may still inspect a known provider task through a
separate read, but no terminal operation resumes automatically. A later `start` after a terminal
state is an explicit independent operation with a new operation ID. `[NEW-SPEC]`

# Durable Lifecycle Projection

The orchestrator stores one version-1 current-operation record at
`<state_dir>/cloud-lifecycle.json`, distinct from the T002B login ledger and T004A submit ledger.
The record contains only:

- schema version and current `CloudSubmitOperationId`;
- current lifecycle phase;
- optional validated task ID;
- latest reconciliation generation, bounded candidate task IDs, and completeness flag; and
- a history of at most 32 valid lifecycle phases.

It never contains a prompt, cancellation signal, acknowledgement payload, task title/URL, raw
provider output, diff, cursor, environment/branch, path, or credential data. The file is at most
64 KiB and uses the accepted state-directory policy: mode `0600`, effective-UID owner, regular
single-link file, no symlink following, create-new temporary file, file sync, atomic rename, and
state-directory sync. Malformed, oversized, unknown-version, permission-unsafe, linked, or
impossible state fails closed and is never discarded automatically. `[NEW-SPEC]`

An absent lifecycle file means local `Idle`; it does not prove that the lower T004A submit ledger
is absent or terminal. Construction returns Idle without a provider command, and every later
`start` must still pass the lower-readiness gate below. `[NEW-SPEC]`

Allowed transitions within one operation are:

```text
Submitting → FailedBeforeSubmit
Submitting → Pending
Submitting → OutcomeUnknown
Submitting → CanceledLocally(no task, provider_may_continue=false)
Pending → Pending|Ready|Applied|ProviderError
Pending → CanceledLocally(task, provider_may_continue=true)
OutcomeUnknown → OutcomeUnknown
OutcomeUnknown → Pending|Ready|Applied|ProviderError
OutcomeUnknown → AbandonedUnknown
```

Starting a new operation after a terminal state replaces the current record with a fresh
`Submitting` history and new operation ID. Submitting, Pending, and OutcomeUnknown block a new
operation. Reconciliation evidence updates only OutcomeUnknown metadata and does not itself change
the phase or permit a new start. `[NEW-SPEC]`

# Restart Reconciliation

`CloudTaskOrchestrator::new` revalidates the trusted scope, loads the lifecycle record, and repairs
a durable `Submitting` or `OutcomeUnknown` phase through the command-free T004A1 observation:

| Local phase | T004A1 observation | Repaired lifecycle |
|---|---|---|
| `Submitting` | `Absent` or `FailedBeforeSpawn` | `FailedBeforeSubmit` |
| `Submitting` | `OutcomeUnknown` | `OutcomeUnknown` |
| `Submitting` | `TaskRecorded(task)` | `Pending(task)` |
| `Submitting` | `ExplicitlyAbandoned` | `AbandonedUnknown` |
| `OutcomeUnknown` | `OutcomeUnknown` | unchanged |
| `OutcomeUnknown` | `TaskRecorded(task)` | `Pending(task)` |
| `OutcomeUnknown` | `ExplicitlyAbandoned` | `AbandonedUnknown` |
| `OutcomeUnknown` | `Absent` or `FailedBeforeSpawn` | typed lower-state conflict; unchanged |

The repair is durable before construction succeeds. Observation conflict, unsafe/malformed lower
state, or a lifecycle/lower operation-ID mismatch fails construction without deleting either
record. Other lifecycle phases are already committed and load without a provider command. Before
replacing any local terminal with a new operation, `start` uses command-free observation to ensure
that no different lower unknown operation would be orphaned. Prompts are never reconstructed or
persisted. `[NEW-SPEC]`

# Commit Order and Concurrency

- Under the lifecycle mutex, `start` creates the operation/request/cancellation values and retains
  the prior Idle/terminal lifecycle snapshot. Before any `Submitting` commit, it invokes
  command-free T004A1 observation for the new operation ID and requires exactly `Absent`. A
  different lower unknown/nonterminal operation, malformed lower ledger, or any other observation
  fails closed without changing the lifecycle record.
- Only after that gate succeeds does `start` commit `Submitting`, install the in-memory signal, and
  release the mutex.
- `start` delegates exactly one non-idempotent submit to T004A without holding the lifecycle mutex.
  It reacquires the mutex, verifies the same operation, commits the mapped lifecycle, clears the
  signal, notifies waiters, and only then emits a result.
- A proven pre-submit lower failure commits `FailedBeforeSubmit`. Pre-authorization explicit
  cancellation commits `CanceledLocally` without a task. Any ambiguous lower result commits
  `OutcomeUnknown` only when the lower error names the same operation. A successful durable task
  commits `Pending`; a lower error naming another operation is a typed conflict and never becomes
  the new operation's unknown state.
- If delegation nevertheless reports a different-operation lower conflict, `start` durably
  restores the retained prior terminal record, or durably removes the just-created lifecycle file
  and syncs the state directory when the prior state was Idle, before returning the conflict. A
  restoration storage failure is `RecoveryRequired`; the orphan `Submitting` remains fail-closed
  for operator repair and never authorizes another submit.
- A second `start` while the current operation is submitting, unknown, pending, or awaiting
  recovery fails with `TurnAlreadyRunning`; it never invokes Cloud exec.
- A task status is committed to the local lifecycle projection before it is emitted to a caller.
- Inspect and recovery serialize through the operation mutex. `cancel` can acquire the mutex while
  submit is outside it, clone/signal the exact active cancellation value, and wait on a condition
  or actor response until the submit owner commits its lower disposition. It then re-reads that
  disposition; if it became `Pending`, cancel commits local cancellation before returning. No
  state mutex is held while waiting for T004A submit.
- The first committed terminal transition wins; a loser re-reads and returns the committed state.
- Browser connection lifetime is not an owner lease. Disconnect, reconnect, or a dropped response
  does not call `cancel`, mutate lifecycle state, or start another submit.
- T005 must add durable session events before presenting this lifecycle over HTTP; T004B does not
  claim session-event durability. `[NEW-SPEC]`

# Unknown-Submit Recovery

`reconcile_unknown` delegates to the accepted bounded T004A list reconciliation. While still
holding the lifecycle operation authority, it durably copies only the returned operation ID,
candidate task IDs, completeness flag, and a monotonically increasing reconciliation generation
into the lifecycle record before returning the lower redacted projection. It does not change the
unknown disposition or authorize a submit.

`AdoptListedTask` is accepted only when:

1. the supplied operation ID exactly names the durable current operation;
2. a successful reconciliation was durably recorded for that operation;
3. the validated task ID appears in that exact latest recorded candidate set; and
4. that latest reconciliation is complete; and
5. a one-shot status inspection of that task succeeds.

The resulting provider status determines the adopted lifecycle. Before committing that lifecycle,
the orchestrator invokes the T004A1 trusted bridge to durably record the exact task as
`TaskAdopted`; this terminalizes the lower unknown operation without Cloud exec. A task absent from
the latest set, a stale operation, failed status read, incomplete/malformed evidence, lower
resolution failure, or concurrent terminal decision fails closed and leaves the lifecycle unknown.

`AbandonAfterReconciliation` is accepted only after at least one durable bounded reconciliation,
whether complete or incomplete, and an authenticated named duplicate-risk acknowledgement bound to
the same operation ID. The orchestrator first invokes the T004A1 bridge to durably record
`ExplicitlyAbandoned`, then records `AbandonedUnknown`. Only those two terminal records permit a
later independent submit. They do not claim that no provider task exists. A later submit is
explicit operator action with a new operation ID, never an automatic retry of the unknown
operation. `[NEW-SPEC]`

If the process crashes after T004A1 terminalizes the lower ledger but before the lifecycle commit,
restart reconciliation maps the lower task/abandonment observation to `Pending` or
`AbandonedUnknown`. Repeating the exact decision is idempotent; a conflicting resolution fails
closed.

# Cancellation

The pinned `0.145.0` Cloud CLI has no provider-task cancel command.

- Before T004A authorization, explicit cancel produces local `CanceledLocally` with no task ID and
  no provider side effect.
- During submit after authorization but before a durable task ID, it signals the T004A process
  group, waits for reap, and returns `OutcomeUnknown`; cleanup does not prove the provider did not
  create a task.
- While `Pending` after a durable task ID, it stops local monitoring and records
  `CanceledLocally { provider_may_continue: true }`. It does not issue status as a substitute for
  cancel and does not claim provider termination.
- Cancel on `OutcomeUnknown` is an idempotent no-op that returns the same blocked unknown state.
- Repeated cancel after `CanceledLocally` is replay-idempotent. Cancel on
  `FailedBeforeSubmit`, `Ready`, `Applied`, `ProviderError`, or `AbandonedUnknown` returns that
  already committed terminal state without mutation or provider access.
- Browser disconnect never invokes this operation.

This satisfies INV-012 at the Codebox turn boundary: only an explicit cancel command terminates the
local turn. The required non-guarantee is that an already submitted provider task may continue and
remain inspectable. T005/T006 must label and explain this behavior rather than imply remote
termination. `[NEW-SPEC]`

# Archetype C Answers

- Atomicity: E2. Submit progress is durable through T004A and provider state is inspectable after a
  task ID or bounded-list recovery evidence exists.
- Duplicate ledger: the T004A current-operation record plus the versioned lifecycle record prevents
  a second automatic submit; both must be terminal before a later explicit operation.
- Unknown presentation: `OutcomeUnknown` carries only the strong operation ID and exposes explicit
  reconciliation; it never contains prompt/raw output.
- Crash points: restart repairs `Submitting` through prompt-free T004A1 observation;
  pre-authorization cancel is no-effect; every ambiguity after authorization is unknown; durable
  task IDs resume through status; lower resolution precedes and repairs terminal local decisions.
- Retry: automatic retry does not exist. Explicit abandon requires a prior reconciliation and named
  duplicate-risk acknowledgement, then a later action uses a new operation ID.

# Archetype D Answers

- Ordering: lifecycle projections for one operation are strictly monotonic according to the state
  graph. Concurrent observations may repeat a state but never move backward.
- Termination: a local operation reaches one provider terminal state, `CanceledLocally`,
  `AbandonedUnknown`, or remains queryable `OutcomeUnknown`; no fake success is synthesized.
- Cancellation: only the explicit operation above changes local state. Provider-side cancellation
  is a stated non-guarantee.
- Chunk-boundary invariance: T004B consumes typed T004A/T003 values, not byte chunks; the lower
  contracts own byte partition tests.
- Backpressure: T004B emits bounded state snapshots rather than token deltas. T005 owns subscriber
  buffering/coalescing.
- Framing/partial data: a state is emitted only from a complete typed lower-boundary result.

# Errors, Bounds, and Redaction

- Each `inspect` performs at most one 30-second T004A status command.
- Each recovery attempt inherits T004A's five-page/100-task/60-second bound.
- The orchestrator stores at most one current operation, 32 phases, and 100 bounded candidate IDs
  in a 64-KiB lifecycle record; it does not persist prompts, titles, URLs, diffs, raw captures,
  cancellation signals, or acknowledgement payloads.
- Errors distinguish busy, no-current-operation, wrong-state, stale decision, task-not-listed,
  acknowledgement-required, lower runner, provider read, conflict, and unknown outcome without
  rejected values or lower raw text.
- There is no internal polling loop, retry timer, or provider request after browser disconnect.
  T005 owns rate-limited polling cadence. `[NEW-SPEC]`

# Exit Invariants

After success, checked failure, cancellation, timeout, concurrent loss, or recovery:

- one operation has at most one local terminal state;
- no code path automatically calls Cloud exec twice;
- unknown state remains recoverable or explicitly abandoned, never silently cleared;
- T004A and T004B unknown dispositions are terminalized in lower-then-upper order before a later
  independent operation is permitted;
- known task ID/status transitions never become an unrelated task ID;
- local cancel is explicit and never presented as proof of provider cancellation; and
- lower owned processes satisfy T004A reap/ledger invariants.

# Non-Guarantees

- Local cancellation does not cancel or stop the provider task.
- Reconciliation candidates do not prove which task belongs to an unknown submit.
- Explicit abandonment does not prove no task exists and can be followed by a duplicate provider
  task if the operator chooses to submit again.
- T004B does not define HTTP idempotency, durable session events, stream reconnect, polling rate,
  diff retrieval, or artifact storage.
- Provider `Ready` means the pinned provider status, not that a patch is safe or applicable.

# Required Test Skeletons

| Clause | Required test |
|---|---|
| Exact lifecycle mapping | `cloud_lifecycle_maps_all_pinned_statuses` |
| Monotonic state transitions | `cloud_lifecycle_never_moves_backward` (property-based) |
| One mutating operation | `cloud_lifecycle_rejects_concurrent_start` |
| Browser disconnect independence | `cloud_disconnect_does_not_cancel_or_resubmit` |
| Unknown remains blocked | `cloud_unknown_requires_explicit_recovery` |
| Adopt only latest listed candidate | `cloud_recovery_adopts_only_recorded_candidate` |
| Incomplete reconciliation stays safe | `cloud_incomplete_reconciliation_does_not_infer_absence` |
| Explicit abandon acknowledgement | `cloud_abandon_requires_reconciliation_and_duplicate_risk_ack` |
| Submit-stage cancel is unknown | `cloud_cancel_during_submit_reaps_and_reconciles` |
| Known-task cancel is local only | `cloud_cancel_does_not_claim_provider_termination` |
| Repeated cancel | `cloud_cancel_is_replay_idempotent` |
| Typed redacted errors/state | `cloud_lifecycle_errors_and_debug_are_redacted` |
| Orchestrator never auto-resubmits | `cloud_orchestrator_never_auto_resubmits_after_unknown` |
| Durable lifecycle ledger | `cloud_lifecycle_ledger_fails_closed` |
| Restart reconciliation | `cloud_submitting_restart_observes_lower_disposition` |
| Lower resolution commit order | `cloud_resolution_terminalizes_lower_submit_before_lifecycle` |
| Lower readiness before local intent | `cloud_start_checks_lower_readiness_before_submitting` |
| Post-gate lower conflict rollback | `cloud_start_restores_prior_lifecycle_on_lower_conflict` |

The launcher-level TD P15 test
`regression_unknown_cloud_submit_reconciles_before_retry` is owned and executed by T004A. The
distinct T004B test above drives `start`, reconciliation, adopt/abandon decisions, and repeated
inspection through the orchestrator while asserting that none invokes a second Cloud exec.

The lower-readiness test partitions absent-file Idle, terminal replacement, and a different lower
unknown while proving lifecycle bytes remain unchanged on gate failure. The post-gate conflict
test proves prior-terminal restoration, Idle file removal plus directory sync, `RecoveryRequired`
on injected restoration failure, and no second Cloud exec.

# Common Test Partitions

- Pending/ready/applied/error; no operation, submitting, known task, unknown, every terminal state.
- Disconnect/reconnect before and after each state.
- Zero/one/many candidates; stale and current candidate; complete/incomplete reconciliation.
- Cancel before authorization, during submit, after task ID, concurrent with status, repeated after
  terminal state.
- Crash before/after each lifecycle and lower-resolution commit; malformed/missing/mismatched lower
  ledger; conflict loser re-read.

# Traceability and Gaps

The lifecycle, explicit recovery decisions, durable projection, and local-only cancellation
semantics are `[NEW-SPEC]` derivations of TD INV-006/INV-012, the TD cancellation non-guarantee,
and ADR-0002's pinned surface. SPEC-T004A1 closes the prompt-free observation and lower unknown
terminalization gaps. They do not claim an absent provider API. No in-scope `[TD-GAP]` remains.

# Design Review

The first implementation-preparation review returned `DESIGN REJECTED`. It found that the accepted
T004A surface could neither recover an orphaned durable `Submitting` state without a prompt nor
terminalize an adopted/abandoned lower unknown operation; the recovery decision lacked operation
identity, the lifecycle ledger was unspecified, and the mutex/cancellation protocol could block
explicit cancel. This revision adds SPEC-T004A1, the versioned lifecycle projection, exact restart
mapping, operation-bound decisions, lower-then-upper resolution commits, and out-of-band cancel
coordination.

Two follow-up reviews required the lower-readiness gate to precede every local `Submitting` commit
and a named fault test for post-gate conflict rollback. The final fresh review returned
`DESIGN ACCEPTED`. T004A1 is now Accepted, so T004B is the sole Ready production task.
