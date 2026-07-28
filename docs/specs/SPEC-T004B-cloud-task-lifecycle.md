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
- Accepted T004A before implementation

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
pub fn start(&self, prompt: CloudPrompt)
        -> Result<CloudLifecycle, CloudLifecycleError>;
    pub fn inspect(&self) -> Result<CloudLifecycle, CloudLifecycleError>;
    pub fn reconcile_unknown(&self)
        -> Result<CloudReconciliation, CloudLifecycleError>;
    pub fn resolve_unknown(&self, decision: UnknownSubmitDecision)
        -> Result<CloudLifecycle, CloudLifecycleError>;
    pub fn cancel(&self) -> Result<CloudLifecycle, CloudLifecycleError>;
}
```

The exact concurrency wrapper may be synchronous or async, but one orchestrator has exactly one
current operation. `DuplicateRiskAcknowledgement` is a named authenticated-operator command value,
not a boolean option or a value the backend creates automatically. T005 owns its HTTP
authorization/audit projection. Debug and errors remain redacted under T003/T004A rules.
`[NEW-SPEC]`

Before invoking T004A, `start` creates the strong `CloudSubmitOperationId`,
`CloudSubmitRequest`, and `CloudCancellation`, then stores the operation ID and cancellation
signaling half in its serialized current-operation state. Concurrent `cancel` targets that exact
signal. Recovery reuses the same operation ID; a deliberate later independent start creates a new
one. The signal is cleared only after the submit call and its ledger disposition are observed; it
is never reused for another operation. `[NEW-SPEC]`

# Lifecycle

```text
Idle → Submitting → Pending → Ready
          │            ├────→ Applied
          │            ├────→ ProviderError
          │            └────→ CanceledLocally(provider may continue)
          └→ OutcomeUnknown → ReconciliationObserved
                                  ├→ Pending|Ready|Applied|ProviderError (explicit adopt)
                                  └→ AbandonedUnknown (explicit risk acknowledgement)
```

`inspect` performs at most one fixed status read and maps the exact T003 status:

| Provider status | Lifecycle |
|---|---|
| `Pending` | `Pending` |
| `Ready` | `Ready` |
| `Applied` | `Applied` |
| `Error` | `ProviderError` |

`Ready`, `Applied`, `ProviderError`, `CanceledLocally`, and `AbandonedUnknown` are terminal for the
local operation. A caller may still inspect a known provider task through a separate read, but no
terminal operation resumes automatically. `[NEW-SPEC]`

# Commit Order and Concurrency

- `start` delegates exactly one non-idempotent submit to T004A and projects its durable operation
  ID before emitting any lifecycle state.
- A second `start` while the current operation is submitting, unknown, pending, or awaiting
  recovery fails with `TurnAlreadyRunning`; it never invokes Cloud exec.
- A task status is committed to the local lifecycle projection before it is emitted to a caller.
- Concurrent inspect/cancel/recovery commands serialize through one operation mutex or actor. The
  first committed terminal transition wins; a loser re-reads and returns the committed state.
- Browser connection lifetime is not an owner lease. Disconnect, reconnect, or a dropped response
  does not call `cancel`, mutate lifecycle state, or start another submit.
- T005 must add durable session events before presenting this lifecycle over HTTP; T004B does not
  claim session-event durability. `[NEW-SPEC]`

# Unknown-Submit Recovery

`reconcile_unknown` delegates to the accepted bounded T004A list reconciliation. It does not change
the unknown disposition.

`AdoptListedTask` is accepted only when:

1. the authenticated operator names the current operation ID through the enclosing T005 command;
2. a successful reconciliation was durably recorded for that operation;
3. the validated task ID appears in that exact latest recorded candidate set; and
4. a one-shot status inspection of that task succeeds.

The resulting provider status determines the adopted lifecycle. A task absent from the latest set,
a stale operation, failed status read, incomplete/malformed evidence, or concurrent terminal
decision fails closed and leaves the operation unknown.

`AbandonAfterReconciliation` is accepted only after at least one durable bounded reconciliation,
whether complete or incomplete, and an authenticated named duplicate-risk acknowledgement. It
records `AbandonedUnknown` before permitting a later independent submit. It does not claim that no
provider task exists. A later submit is explicit operator action with a new operation ID, never an
automatic retry of the unknown operation. `[NEW-SPEC]`

# Cancellation

The pinned `0.145.0` Cloud CLI has no provider-task cancel command.

- Before T004A authorization, explicit cancel produces local `CanceledLocally` with no task ID and
  no provider side effect.
- During submit after authorization but before a durable task ID, it signals the T004A process
  group, waits for reap, and returns `OutcomeUnknown`; cleanup does not prove the provider did not
  create a task.
- After a durable task ID, it stops local monitoring and records
  `CanceledLocally { provider_may_continue: true }`. It does not issue status as a substitute for
  cancel and does not claim provider termination.
- Repeated cancel is replay-idempotent and returns the committed local terminal state.
- Browser disconnect never invokes this operation.

This satisfies INV-012 at the Codebox turn boundary: only an explicit cancel command terminates the
local turn. The required non-guarantee is that an already submitted provider task may continue and
remain inspectable. T005/T006 must label and explain this behavior rather than imply remote
termination. `[NEW-SPEC]`

# Archetype C Answers

- Atomicity: E2. Submit progress is durable through T004A and provider state is inspectable after a
  task ID or bounded-list recovery evidence exists.
- Duplicate ledger: the T004A current-operation record plus one serialized lifecycle state prevents
  a second automatic submit.
- Unknown presentation: `OutcomeUnknown` carries only the strong operation ID and exposes explicit
  reconciliation; it never contains prompt/raw output.
- Crash points: pre-authorization cancel is no-effect; every ambiguity after authorization is
  unknown; durable task IDs resume through status; terminal local decisions replay.
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
- The orchestrator stores at most one current operation and the bounded T004A evidence; it does not
  persist prompts, titles, URLs, diffs, or raw captures.
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

The launcher-level TD P15 test
`regression_unknown_cloud_submit_reconciles_before_retry` is owned and executed by T004A. The
distinct T004B test above drives `start`, reconciliation, adopt/abandon decisions, and repeated
inspection through the orchestrator while asserting that none invokes a second Cloud exec.

# Common Test Partitions

- Pending/ready/applied/error; no operation, submitting, known task, unknown, every terminal state.
- Disconnect/reconnect before and after each state.
- Zero/one/many candidates; stale and current candidate; complete/incomplete reconciliation.
- Cancel before authorization, during submit, after task ID, concurrent with status, repeated after
  terminal state.
- Crash before/after each lifecycle commit and conflict loser re-read.

# Traceability and Gaps

The lifecycle, explicit recovery decisions, and local-only cancellation semantics are
`[NEW-SPEC]` derivations of TD INV-006/INV-012, the TD cancellation non-guarantee, and ADR-0002's
pinned surface. They do not claim an absent provider API. No in-scope `[TD-GAP]` remains.
