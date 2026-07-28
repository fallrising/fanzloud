---
id: SPEC-T005A
title: P0 in-process session lifecycle and event fanout
status: ready
contract_unit: CU-SES-P0-01
module: codebox-session-runtime
milestone: P0
archetype: E
atomicity: E1
invariants: [INV-003, INV-004, INV-006, INV-007, INV-012]
depends_on: [SPEC-T004, SPEC-T004B, SPEC-T004C]
td_sections: [1.7, 2.3, 4.2, 4.4, 4.7, 8, 9, 10, 14, 15.0]
adr_refs: [ADR-0002, ADR-0003, ADR-0004]
risk: high
---

# Intent

Provide one reduced, process-lifetime P0 session that serializes turn intent, composes the accepted
provider-specific T004 lifecycle, polls normalized provider status at a bounded cadence, and
publishes bounded gap-free session events without exposing transport or authentication policy.

# Responsibility

## Does

- Own one process-lifetime `SessionId`, instance ID, session projection, current `TurnId`, event
  sequence, bounded history, subscriber registry, and one worker.
- Commit a redacted turn intent before the worker calls T004 `start`.
- Publish only already-durable T004 lifecycle projections.
- Monitor pending tasks, apply explicit cancellation, project unknown recovery, and read a bounded
  eligible diff without mutating session/event state.
- Provide an atomic retained-replay/current-snapshot/live subscription handoff.

## Does Not

- Own HTTP/JSON/WebSocket, browser authentication, Origin/CSRF checks, or idempotency keys.
- Decide unknown-submit recovery or mint an acknowledgement without a trusted caller command.
- Define generic `AgentBackend`, P1 events/store/replay, approvals, artifacts, or multi-session
  tenancy.
- Persist prompt, event history, subscriber state, diff, verification code, raw provider output,
  or credentials.
- Apply/publish a diff, cancel a provider task, or retry Cloud submit.

# Public Boundary

```rust
pub struct P0SessionRuntime { /* private */ }
pub struct P0SessionSubscription { /* private */ }
pub struct P0LiveReceiver { /* private */ }
pub struct P0SessionSnapshot {
    pub identity: P0SessionIdentity,
    pub state: P0SessionState,
    pub current_turn: Option<P0TurnSnapshot>,
    pub high_water_seq: EventSeq,
}

impl P0SessionRuntime {
    pub fn new(
        cloud: CloudTaskOrchestrator,
        config: P0SessionConfig,
    ) -> Result<Self, P0SessionError>;

    pub fn identity(&self) -> P0SessionIdentity;
    pub fn snapshot(&self) -> Result<P0SessionSnapshot, P0SessionError>;
    pub fn start_turn(&self, prompt: CloudPrompt) -> Result<P0TurnReceipt, P0SessionError>;
    pub fn cancel_turn(&self, actor: P0Actor) -> Result<P0SessionSnapshot, P0SessionError>;
    pub fn reconcile_unknown(
        &self,
        actor: P0Actor,
    ) -> Result<P0RecoveryCandidates, P0SessionError>;
    pub fn resolve_unknown(
        &self,
        actor: P0Actor,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    ) -> Result<P0SessionSnapshot, P0SessionError>;
    pub fn read_diff(&self) -> Result<CloudDiff, P0SessionError>;
    pub fn subscribe(
        &self,
        session_id: SessionId,
        after_seq: EventSeq,
    ) -> Result<P0SessionSubscription, P0SessionError>;
    pub fn shutdown(&self) -> Result<(), P0SessionError>;
}

impl P0SessionSubscription {
    pub fn replay(&self) -> &[P0SessionEventEnvelope];
    pub fn snapshot(&self) -> &P0SessionSnapshot;
    pub fn high_water_seq(&self) -> EventSeq;
    pub fn into_parts(
        self,
    ) -> (
        Vec<P0SessionEventEnvelope>,
        P0SessionSnapshot,
        P0LiveReceiver,
    );
}

impl P0LiveReceiver {
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<P0SessionEventEnvelope, P0LiveReceiveError>;
    pub fn try_recv(&self) -> Result<P0SessionEventEnvelope, P0LiveReceiveError>;
}
```

`P0SessionConfig`, event/snapshot/projection values, subscription, and `P0SessionError` are typed,
bounded, serializable where transported, and have redacted `Debug`/`Display`.

# Inputs and Outputs

- `P0SessionConfig` accepts:
  - poll interval: 250 milliseconds through 60 seconds; default 2 seconds;
  - history capacity: 64 through 1024; default 256;
  - maximum subscribers: 1 through 32; default 8; and
  - per-subscriber live capacity: 8 through 256; default 32.
- `CloudPrompt` retains the accepted 32-KiB T003 validation and redacted debug contract.
- `P0Actor` has only the fixed `Operator` value; no browser string constructs an actor.
- A recovery resolution must carry the exact current lower operation ID and accepted T004B
  decision type.
- `P0SessionIdentity` contains only the session ID and process instance ID.
- Snapshot contains exactly identity, state, optional current turn/lifecycle, and high-water
  `EventSeq`. A terminal `Cloud` turn remains available while session state is `Ready`; absence is
  normal only before the first/reconstructed operation.
- `P0SessionState` is exactly `Ready`, `Running`, `RecoveryRequired`, `MonitoringDegraded`, or
  `Stopped`.
- `P0CloudLifecycle` is a serializable allowlisted projection with the same variants and strong-ID
  fields as accepted `CloudLifecycle`; provider task IDs become privately constructed
  `P0CloudTaskId` display projections and add no raw-output authority.
- A current `P0TurnProjection` is exactly one of:
  - `Queued`;
  - `Starting { cancel_requested: bool }`;
  - `Cloud { lifecycle: P0CloudLifecycle, cancel_requested: bool }`;
  - `MonitoringDegraded { operation_id, last_known_pending: P0CloudLifecycle,
    cancel_requested: bool }`, where `last_known_pending` must be the previously observed exact
    `Pending` projection;
  - `CanceledBeforeCloudStart`; or
  - `StoppedBeforeCloudStart`; or
  - `StoppedAfterLowerFailure`.
- Events contain schema version 1, session ID, sequence, optional turn ID, and one typed safe event:
  `TurnAccepted`, `TurnCanceledBeforeCloudStart`, `LifecycleChanged`, `CancelRequested`,
  `MonitoringDegraded`, `RecoveryObserved`, `RecoveryResolved`, or
  `RuntimeStopped { reason: Shutdown | LowerFailure }`.
- Events never contain prompt/diff/code text, task URL, raw provider output, error source strings,
  configuration, or paths.

# Preconditions and Disposition

| ID | Condition | Type / Checked / Internal | Trace |
|---|---|---|---|
| P-005A-01 | Config values are in the declared ranges | Checked `InvalidConfig` | F bounds |
| P-005A-02 | Start occurs only from exactly `Ready` | Checked `TurnAlreadyRunning` | INV-004 |
| P-005A-03 | Cancel/recovery targets the current turn | Checked `NoCurrentTurn`/`WrongOperation` | T004B |
| P-005A-04 | Subscriber session ID equals this runtime | Checked `WrongSession` | D trust |
| P-005A-05 | Cursor is retained and not future | Checked `HistoryGap`/`FutureCursor` | ADR-0004 |
| P-005A-06 | Runtime worker has not stopped | Checked `RuntimeStopped` | Cleanup |

# Success Postconditions

## Construction

- Creates fresh non-nil session and instance IDs.
- Starts exactly one named worker before returning.
- Calls T004 `inspect` at most once to build a current startup projection. `NoCurrentOperation`
  produces `Ready` with no current turn; an existing lower lifecycle is projected with a fresh
  local turn ID and no synthetic historical event.
- `Pending` arms the worker's first rate-limited monitor action before construction returns.
  an `inspect` error categorized `OutcomeUnknown` with its required operation ID creates
  `RecoveryRequired`.
- Startup `ProviderRead` cannot reconstruct the deliberately redacted durable task ID and therefore
  fails construction rather than inventing a degraded projection. A later explicit service start
  may retry the one inspection; it never calls Cloud submit.
- Scope, busy, ledger, conflict, `TurnAlreadyRunning`, missing-operation-ID, provider-read, or any
  other non-`NoCurrentOperation`/non-`OutcomeUnknown` construction error is fail-closed:
  construction returns a redacted error, publishes nothing, and joins the not-yet-active worker.
- Construction does not start, cancel, reconcile, resolve, or read a diff.

## Start

- Under the session lock, requires exactly `Ready`, assigns a fresh turn ID, sets `Running`, appends
  one `TurnAccepted`, and queues work before returning the receipt.
- The worker cannot call lower `start` before that commit.
- Each returned receipt corresponds to at most one lower start. An explicit cancel or shutdown
  that removes the still-unclaimed queue item corresponds to zero.
- Lower `Pending` starts monitoring; lower `OutcomeUnknown` enters `RecoveryRequired`; every other
  terminal projection returns the session to `Ready`.

## Monitoring

- Only one worker calls lower `inspect`, with no overlapping calls.
- The next successful pending inspection starts no sooner than one configured interval after the
  previous call completes.
- Checked provider-read failures retain the last known pending lifecycle, enter
  `MonitoringDegraded`, coalesce repeated equal error categories, and back off by powers of two up
  to 60 seconds. A later successful read resets backoff.
- Monitoring stops at lower terminal/canceled/unknown state or local shutdown.

## Cancel and recovery

- If explicit cancel wins while start remains unclaimed, it atomically removes the queue item,
  records `CanceledBeforeCloudStart`, returns to `Ready`, and never calls T004.
- Once the worker claims a start, it records that private fact under the session lock. Cancel
  latches `cancel_requested`, appends `CancelRequested`, and attempts accepted T004 cancel. A
  transient lower `NoCurrentOperation` while the worker is between claim and T004 intent commit is
  not completion: cancel waits/rechecks at a 10-millisecond cadence for at most one second. If the
  lower intent becomes visible, it cancels that exact operation. If admission is still not
  visible, the latched intent remains and the worker must either suppress the not-yet-entered
  lower start or call lower cancel immediately after `start` returns, before monitoring. The
  returned snapshot may remain `Running { cancel_requested: true }` until that projection commits;
  no second start is authorized.
- After lower admission, explicit cancel publishes only the durable returned lower projection.
- Cancel never records or returns provider cancellation. A known task canceled locally projects
  `provider_may_continue=true`.
- `RecoveryRequired` rejects `cancel_turn` as `WrongState` without calling lower cancel; recovery
  must be reconciled/resolved explicitly.
- Reconciliation publishes only operation ID, completeness, and bounded candidate task IDs already
  returned by T004.
- Resolution requires the exact current operation, passes the caller-created T004B decision
  unchanged, records the fixed actor and decision kind, and resumes monitoring only if the durable
  result is pending.

## Diff

- `read_diff` mints the current T004C opaque authority, creates one private fresh
  `CloudCancellation`, and calls the bound reader exactly once.
- Success returns the accepted bounded untrusted `CloudDiff`.
- Success or failure leaves session snapshot, event sequence/history, subscribers, and every T004
  managed record unchanged except provider-owned/OS state excluded by ADR-0003.
- Session shutdown does not signal the private diff cancellation. The accepted T004C timeout and
  process-group policy terminate/reap the call before `read_diff` returns.

## Subscription

- Under one lock, validates the cursor, copies retained events above it through high-water,
  captures the snapshot at that high-water, and registers a live sender for only later events.
- No event can be lost or duplicated between replay and live for a retained cursor.
- `P0SessionSubscription` exposes the immutable replay, snapshot, and high-water for inspection or
  consumes itself into owned replay/snapshot/live parts. `P0LiveReceiver` yields only envelopes
  above high-water and distinguishes `Empty`, `Lagged`, `RuntimeStopped`, and `Closed` without raw
  channel errors.
- `recv_timeout` returns `Empty` only when its caller-supplied bounded wait elapses while the
  subscription remains live. `try_recv` returns `Empty` only when no frame is currently queued.
  Sender removal reads the recorded close reason and returns `Lagged`, `RuntimeStopped`, or
  `Closed`; none is converted into an empty poll.

# Legal Transition Table

Accepted lower projections map exactly as follows:

| T004 observation | P0 session state |
|---|---|
| no current operation | `Ready` with no current turn |
| `Pending` | `Running(Cloud Pending)` with monitoring armed |
| `Err(OutcomeUnknown)` with operation ID | `RecoveryRequired` |
| `FailedBeforeSubmit` | `Ready(Cloud FailedBeforeSubmit)` |
| `Ready` | `Ready(Cloud Ready)` |
| `Applied` | `Ready(Cloud Applied)` |
| `ProviderError` | `Ready(Cloud ProviderError)` |
| `CanceledLocally` | `Ready(Cloud CanceledLocally)` |
| `AbandonedUnknown` | `Ready(Cloud AbandonedUnknown)` |

No other lower error is interpreted as a lifecycle projection.

| Current session state | Input / accepted lower result | Next state | Lower call |
|---|---|---|---|
| `Ready` | `start_turn` intent commit | `Running(Queued)` | queued, not yet called |
| `Running(Queued)` | explicit cancel wins | `Ready(CanceledBeforeCloudStart)` | none |
| `Running(Queued)` | shutdown wins | `Stopped(StoppedBeforeCloudStart)` | none |
| `Running(Queued)` | worker claim | `Running(Starting { cancel_requested: false })` | at most one `start` |
| `Running(Starting)` | explicit cancel latch | `Running(Starting { cancel_requested: true })` | cancel admission protocol |
| `Running(Starting)` | lower `Pending` | `Running(Cloud Pending)` | monitor unless cancel latched |
| `Running(Starting)` | lower `OutcomeUnknown` | `RecoveryRequired` | no retry |
| `Running(Starting)` | lower terminal | `Ready(Cloud terminal)` | none |
| `Running(Starting)` | checked non-projectable lower error | `Stopped(StoppedAfterLowerFailure)` | no retry |
| `Running(Cloud Pending)` | inspect pending | `Running(Cloud Pending)` | next scheduled inspect |
| `Running(Cloud Pending)` | checked inspect failure | `MonitoringDegraded(last known Pending)` | backoff inspect |
| `MonitoringDegraded` | inspect pending | `Running(Cloud Pending)` | reset cadence |
| `Running`/`MonitoringDegraded` | checked non-provider-read lower error | `Stopped(StoppedAfterLowerFailure)` | no retry |
| `Running`/`MonitoringDegraded` | lower terminal or explicit cancel result | `Ready` | none |
| `RecoveryRequired` | reconciliation | `RecoveryRequired` | one reconcile |
| `RecoveryRequired` | adopt to pending | `Running(Cloud Pending)` | monitoring armed |
| `RecoveryRequired` | adopt to terminal or abandon | `Ready` | none |
| Any non-stopped state | shutdown | `Stopped` | no new call; join admitted bounded call |
| `Stopped` | any command | `Stopped` plus `RuntimeStopped` error | none |

Every unlisted transition is rejected without event publication or lower invocation.
`CloudLifecycle::Submitting` is never synthesized: accepted T004 `start` returns only its committed
post-submit projection, while accepted `inspect` reports `TurnAlreadyRunning` rather than returning
that variant.

# Non-Guarantees

- No pre-restart event replay, prompt recovery, app session recovery, or HTTP response recovery.
- No provider-side cancellation, Cloud completion deadline, or guarantee that a diff is safe.
- No perpetual delivery to a slow/dropped subscriber.
- No durable audit store; P1 owns durable canonical events.

# Exit Invariants

After success, checked failure, cancellation, lower timeout, concurrent calls, subscriber lag, or
shutdown:

- event sequences are unique, strictly increasing, and gap-free within retained history;
- at most one mutating turn and one worker-owned lower start/inspect action exist; an explicit
  caller-thread cancel may overlap accepted T004 submit exactly as required by T004B;
- no emitted lower lifecycle precedes its T004 durable commit;
- unknown submit is never automatically retried or cleared;
- a subscriber failure changes no session/provider lifecycle;
- prompts, diffs, codes, credentials, raw provider text, and paths are absent from events/errors;
- every started runtime worker exits and is joined by `shutdown`/drop, while shutdown never calls
  lower cancel.

# Side Effects

- Process-lifetime memory, one worker thread, mutex/condition variable, and bounded subscriber
  channels.
- Calls only accepted T004 `start`, `inspect`, `cancel`, `reconcile_unknown`, `resolve_unknown`, and
  T004C diff methods.
- No filesystem, network, executable, environment, repository, artifact, or logging authority is
  added by this crate.

# Idempotency

- `snapshot` and retained `subscribe` are observation-only.
- `read_diff` retains T004C E0 and performs no internal retry.
- `cancel` and matching recovery resolution inherit T004B replay behavior.
- `start_turn` is not caller-replay-idempotent; T005B owns an instance-bound idempotency gate and
  invokes it at most once per accepted key.

# Concurrency and Ordering

- One mutex is the single writer for session projection, event sequence/history, work queue, and
  subscriber registry.
- Lower blocking calls occur without the session mutex.
- A persistent worker consumes queued start/monitor commands after the intent commit.
- Cancel/recovery serialize at the accepted T004 mutex and re-read session state before publishing.
- The first terminal projection wins; later equal projections are no-ops, and a backward/conflicting
  projection is `LowerConflict` without publication.
- Full subscriber queues are removed using nonblocking send; publication never waits on a client.

# Streaming Semantics

- Event schema version is exactly 1.
- History eviction removes complete oldest envelopes only.
- If current high-water is `N` and oldest retained is `O`, valid `after_seq` is `O-1..=N`.
- With no events, only `after_seq=0` is valid.
- Future and gap cursors return bounds without a partial subscription.
- Repeated equivalent `MonitoringDegraded` events may coalesce; lifecycle and explicit command
  events never coalesce.

# Cancellation and Timeout

- Only `cancel_turn(P0Actor::Operator)` calls T004B cancel.
- Subscriber drop, history eviction, HTTP/WS disconnect, worker shutdown, and diff-reader drop do
  not call turn cancel.
- Lower commands retain their accepted deadlines/process-group cleanup.
- Poll waiting is interruptible by explicit cancel, recovery transition, or shutdown.
- `shutdown` atomically removes an unclaimed start as `StoppedBeforeCloudStart`. If the worker has
  claimed but not entered T004, the shutdown latch makes it suppress that call. If T004 was
  admitted, shutdown waits for the bounded call, retains its durable projection, performs no
  cancel, starts no monitoring/read, and then joins the worker.

# Failure Atomicity

CU-SES-P0-01 is E1 for each in-memory session/event transition. T004 submit remains lower E2.

- Config/construction failure publishes nothing and leaves no worker.
- Start intent/queue commits atomically before lower E2 work.
- Lower success/failure is projected in one later atomic transition.
- An event sequence overflow stops the runtime fail-closed before another event or lower start.
- Diff reads are E0 over ADR-0003 managed state.

# Failure Modes and Error Contract

| Case | Error | Retriable | Caller action | Required payload | Trace |
|---|---|---:|---|---|---|
| Invalid config | `InvalidConfig` | no | Fix admin config | safe field enum | Bounds |
| Existing active turn | `TurnAlreadyRunning` | no | Inspect/cancel current | current turn ID | INV-004 |
| No current turn | `NoCurrentTurn` | no | Refresh | none | State |
| Cancel while recovery required | `WrongState` | no | Reconcile/resolve | operation ID | INV-006 |
| Wrong session/operation | `WrongSession`/`WrongOperation` | no | Refresh | none | Authority |
| Runtime stopped/poisoned | `RuntimeStopped` | no | Restart service | none | Cleanup |
| Lower lifecycle failure | `CloudLifecycle` | category-dependent | Follow safe category | safe lower category/op ID | T004B |
| Lower diff failure | `CloudDiff` | no internal retry | Refresh/explicit retry | safe lower category | T004C |
| Cursor evicted | `HistoryGap` | yes after refresh | Fetch snapshot | oldest/latest seq | ADR-0004 |
| Cursor in future | `FutureCursor` | no | Correct client state | latest seq | ADR-0004 |
| Subscriber cap | `SubscriberLimit` | yes bounded | Close/retry | none | Backpressure |
| Sequence overflow/conflict | `SequenceExhausted`/`LowerConflict` | no | Restart/operator repair | none | INV-003/006 |

All error `Debug` and `Display` output is typed and redacted.

# Security Contract

- The crate has no credential, executable, environment ID, branch, host path, repository, shell,
  network, serialization-to-disk, diff-apply, or artifact API.
- Public event/snapshot serde contains only allowlisted fields.
- Prompt and diff canaries must be absent from event JSON, errors, and debug.
- Recovery accepts only accepted strong IDs and T004B decision types.

# Observability and Audit Contract

The event stream is the only session observability. It records safe state categories, bounded IDs,
fixed actor on explicit cancel/recovery, and sequence/version. It is not a durable audit log.

# Test Specification

The following exact test names must exist and compile before production code:

1. `p0_session_start_commits_intent_before_cloud_call`
2. `p0_session_allows_only_one_mutating_turn`
3. `p0_session_projects_monotonic_cloud_lifecycle`
4. `p0_session_polling_is_rate_limited_and_stops_at_terminal`
5. `p0_session_subscriber_drop_does_not_cancel_turn`
6. `p0_session_explicit_cancel_targets_current_turn`
7. `p0_session_unknown_requires_operation_bound_decision`
8. `p0_session_recovery_resumes_pending_monitoring`
9. `p0_session_diff_read_preserves_managed_state`
10. `p0_session_events_are_gap_free_and_bounded`
11. `p0_session_slow_subscriber_isolated_from_publication`
12. `p0_session_shutdown_joins_worker_without_provider_cancel`
13. `p0_session_redacts_prompt_diff_and_lower_errors`
14. `p0_session_model_preserves_single_turn_and_sequence_invariants`
15. `p0_session_cancel_or_shutdown_before_worker_claim_never_starts_cloud`
16. `p0_session_subscription_handoff_is_atomic`

## Unit

Config bounds, projections, error redaction, history boundaries, and state transition tables.

## Contract

All 16 named tests against a crate-private deterministic fake of the exact accepted T004 calls.

## Property / Model

Random command/lower-result/subscriber sequences preserve one active turn, monotonic projection,
gap-free sequence, no publish-before-commit, and no automatic second start.

## Integration

Use the accepted concrete orchestrator fixtures to prove constructor and public compatibility
without live provider/network access.

## Fault Injection

Worker-start failure, lower checked errors, poisoned/closed subscriber, sequence edge, shutdown
during wait, cancel during submit, and recovery races.

## Security

Canary prompt/diff/raw-error/path values never appear in event JSON or debug; architectural
inspection proves no forbidden authority.

## Regression

Named tests 5, 7, 9, 11, 12, 13, 15, and 16 cover INV-012, P15, E0, backpressure, cleanup,
queue races, handoff ordering, and redaction.

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Skeleton compile before production | Pending | T005A commit |
| Focused/repeated tests | Pending | ACCEPT-T005A |
| Workspace gates | Pending | ACCEPT-T005A |
| Fresh acceptance review | Pending | ACCEPT-T005A |

# Traceability

CU-SES-P0-01 → ADR-0004 → T005A → the 16 named tests → `codebox-session-runtime`.

# TD Gaps

None. ADR-0004 resolves process-lifetime durability, recovery ownership, polling, and retention.

# Self-Check

Ready after fresh Cursor Agent design acceptance. All 16 named skeletons must compile before
production implementation.
