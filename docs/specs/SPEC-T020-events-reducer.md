---
id: SPEC-T020
title: Versioned domain events and deterministic session reducer
status: verified
contract_units: [CU-PROTO-01, CU-PROTO-03]
module: codebox-domain
milestone: P1
archetypes: [A]
atomicity: E0
invariants: [INV-003, INV-004]
depends_on: [SPEC-T010]
td_sections: [4.1, 4.2, 4.4, 4.5, 8.2-8.10, 11.1-11.4, 14, 15.1, 15.2]
adr_refs: []
risk: medium
---

# Intent

Make the TD §4.2 session state machine replayable through one explicit version-1 durable event
schema and one deterministic pure reducer. Keep semantic durable events separate from
high-frequency runtime deltas before T030 introduces a persistence adapter.

# Responsibility

## Does

- Defines the version-1 semantic event envelope and payload taxonomy consumed by later event-store
  and actor tasks.
- Reconstructs one session projection from a unique, contiguous, same-stream event sequence.
- Enforces legal session, active-turn, and pending-approval transitions.
- Projects status, active turn, pending approval, sandbox, last activity, and high-water sequence.
- Classifies semantic domain events as durable and the four TD §4.4 delta classes as ephemeral.

## Does Not

- Persist, append, load, snapshot, upcast, publish, coalesce, authorize, execute, log, or emit an
  event.
- Enforce event-ID uniqueness, transactional append, concurrent-writer exclusion, restart
  durability, or replay paging.
- Build conversation or web projections.
- Interpret prompts, provider output, tool output, filesystem paths, credentials, or secrets.
- Perform sandbox cleanup when a session archives.

# Public Boundary

```rust
pub const DOMAIN_EVENT_SCHEMA_V1: u16 = 1;

pub enum DomainEvent { /* version-1 semantic transition payloads */ }

pub struct NewDomainEvent {
    pub schema_version: u16,
    pub occurred_at: DateTime<Utc>,
    pub causation_id: Option<Uuid>,
    pub correlation_id: Uuid,
    pub payload: DomainEvent,
}

pub struct DomainEventEnvelope {
    pub event_id: Uuid,
    pub stream_id: SessionId,
    pub seq: EventSeq,
    pub schema_version: u16,
    pub occurred_at: DateTime<Utc>,
    pub causation_id: Option<Uuid>,
    pub correlation_id: Uuid,
    pub payload: DomainEvent,
}

pub struct SessionReducer { /* immutable reducer state */ }

impl SessionReducer {
    pub fn new(stream_id: SessionId) -> Self;
    pub fn apply(&self, event: &DomainEventEnvelope)
        -> Result<Self, SessionReducerError>;
    pub fn projection(&self) -> Option<&SessionProjection>;
}

pub enum RuntimeEventKind {
    Domain(DomainEventKind),
    TextDelta,
    ReasoningSummaryDelta,
    TerminalDelta,
    ToolOutputDelta,
}

impl RuntimeEventKind {
    pub const fn persistence(self) -> EventPersistence;
}
```

# Inputs and Outputs

`SessionReducer::new` accepts one validated non-nil `SessionId` from T010 and represents an empty
stream at `EventSeq::initial()`. `apply` borrows the current reducer and one fully decoded envelope,
then returns a new reducer on success. It never mutates the input reducer.

`projection()` is `None` before the first accepted `SessionCreated` and `Some` afterward. The
projection contains:

- the reducer's session ID;
- `SessionStatus`;
- the exact active `TurnId`, if any;
- the exact pending `ApprovalId`, if any;
- the provisioned `SandboxId`, if any;
- the `occurred_at` value of the highest accepted sequence; and
- that contiguous high-water `EventSeq`.

`[NEW-SPEC]` An explicit `SessionCreated` first event distinguishes an absent stream from a
persisted session in `Provisioning`. This makes empty replay and session creation unambiguous
without assigning persistence behavior to T020.

# Version-1 Domain Events

| Event | Required payload | Legal source | Result |
|---|---|---|---|
| `SessionCreated` | none | empty reducer | `Provisioning` |
| `SandboxProvisioned` | `sandbox_id` | `Provisioning` | `Ready`, sandbox set |
| `ProvisioningFailed` | none | `Provisioning` | `Failed` |
| `TurnStarted` | `turn_id` | `Ready` | `Running`, active turn set |
| `ApprovalRequested` | `turn_id`, `approval_id` | `Running` | `WaitingApproval`, approval set |
| `ApprovalResolved` | `turn_id`, `approval_id`, `decision` | `WaitingApproval` | `Running`, approval cleared |
| `TurnCancellationRequested` | `turn_id` | `Running` | `Cancelling` |
| `TurnCancelled` | `turn_id` | `Cancelling` | `Ready`, active turn cleared |
| `TurnCompleted` | `turn_id` | `Running` | `Ready`, active turn cleared |
| `TurnFailed` | `turn_id` | `Running` | `Failed`, active turn cleared |
| `SessionIdled` | none | `Ready` | `Idle` |
| `SessionResumed` | none | `Idle` | `Ready` |
| `SessionArchivingStarted` | none | `Ready`, `Idle`, or `Failed` | `Archiving` |
| `SessionArchived` | none | `Archiving` | `Archived` |

`ApprovalDecision` is exactly `Approved` or `Denied`. Both return the session to `Running`; later
tool and policy tasks own the resulting tool transition.

`[NEW-SPEC]` `SessionCreated` and the exact payload fields above are the smallest semantic event
set that makes every TD §4.2 transition deterministic without freezing tool-ledger, provider,
conversation, web, or persistence semantics.

# Preconditions and Disposition

| ID | Condition | Type / Checked / Internal | Trace |
|---|---|---|---|
| P1 | Reducer stream ID is non-nil | T010 `SessionId` invariant | TD §§4.1, 16.1 |
| P2 | Envelope belongs to reducer stream | Checked `WrongStream` | TD §§4.4, 11.2 |
| P3 | Next sequence is representable | Checked `SequenceOverflow` | TD §§2.3 INV-003, 8.2 |
| P4 | Envelope sequence is exactly current plus one | Checked `UnexpectedSequence` | TD §§2.3 INV-003, 4.4 |
| P5 | Schema version is exactly one | Checked `UnsupportedSchemaVersion` | TD §§4.4, 12.10 |
| P6 | First event is `SessionCreated` | Checked `SessionNotCreated` | `[NEW-SPEC]` |
| P7 | Current state allows the event | Checked `InvalidTransition` | TD §4.2 |
| P8 | Turn-bound event names current active turn | Checked `ActiveTurnMismatch` | TD §§2.3 INV-004, 4.2 |
| P9 | Approval resolution names pending approval | Checked `PendingApprovalMismatch` | TD §4.2 |

Checks run in table order. A wrong-state event returns `InvalidTransition` before turn or approval
identity checks. This deterministic precedence is `[NEW-SPEC]`.

# Success Postconditions

- The returned reducer has accepted exactly one additional event.
- Its high-water is the envelope sequence.
- The projection's `last_activity` is the envelope `occurred_at`, even when wall-clock values are
  non-monotonic; sequence order remains authoritative. `[NEW-SPEC]`
- State, active turn, pending approval, and sandbox match the transition table.
- The input reducer is byte-for-byte logically equal to its entry snapshot.
- Applying the same valid event sequence to equal empty reducers produces equal reducers.
- Serde round trip preserves every envelope field and typed event payload.

# Non-Guarantees

- T020 does not guarantee that an envelope was durably stored, uniquely identified, authorized,
  or produced by a trusted actor.
- It does not compare timestamps or guarantee wall-clock monotonicity.
- It does not remove, stop, or reconcile a sandbox when a session archives.
- It does not persist or coalesce ephemeral deltas; classification only tells a downstream
  boundary which category it received.
- It does not accept unknown schema versions or provide an upcaster.
- It does not make a mutable reducer call idempotent; duplicate sequence application is rejected.

# Exit Invariants

After success or every checked failure:

- The input reducer and input envelope are unchanged.
- Accepted high-water sequences are unique, contiguous, and gap-free.
- At most one turn and one approval are active.
- A projection exists if and only if `SessionCreated` has been accepted.
- No external state, log, metric, event store, clock, network, process, or filesystem is touched.

# Side Effects

None. Construction, application, classification, and serde conversion operate only on caller-owned
memory.

# Idempotency

The boundary is replay-deterministic: equal reducer and envelope inputs return equal results.
Applying an already accepted envelope to the returned reducer is a duplicate and returns
`UnexpectedSequence`; callers must not treat a duplicate as a second successful transition.

# Concurrency and Ordering

Calls on separate immutable reducer values are independent and thread-safe when their value types
are shared safely by Rust. T020 does not select a concurrent writer. T030 must use expected-sequence
append to choose one winner; T040 must serialize session commands.

Ordering is exclusively the contiguous `EventSeq`. `occurred_at`, event UUIDs, causation UUIDs, and
correlation UUIDs do not reorder events.

# Streaming Semantics

`SessionReducer::apply` accepts one complete decoded envelope and returns one complete result; it
does not consume a byte or provider stream.

`RuntimeEventKind::persistence` is a total classifier:

| Kind | Result |
|---|---|
| `Domain(_)` | `Durable` |
| `TextDelta` | `Ephemeral` |
| `ReasoningSummaryDelta` | `Ephemeral` |
| `TerminalDelta` | `Ephemeral` |
| `ToolOutputDelta` | `Ephemeral` |

Chunk boundaries, buffering, coalescing, backpressure, and transport termination remain downstream
responsibilities.

# Cancellation and Timeout

Not applicable. Every operation is synchronous, pure, and bounded by one event plus its fixed-size
identifier payloads. T020 starts no task, thread, process, or timer.

# Failure Atomicity

E0. Every failure returns a typed error and an unchanged input reducer. There is no commit point,
partial projection, rollback, reconciliation, or retry side effect.

# Failure Modes and Error Contract

| Case | Error | Retriable | Caller action | Required payload | Trace |
|---|---|---:|---|---|---|
| Envelope stream differs | `WrongStream` | No | Route to correct reducer | expected, actual IDs | TD §4.4 |
| Current sequence is maximum | `SequenceOverflow` | No | Stop stream and repair/migrate representation | current seq | INV-003 |
| Duplicate, gap, or out-of-order seq | `UnexpectedSequence` | No blind retry | Re-read authoritative stream and re-decide | expected, actual seq | INV-003 |
| Schema is not version 1 | `UnsupportedSchemaVersion` | No | Select supported upcaster/version | supported, actual | TD §§4.4, 12.10 |
| First event is not creation | `SessionNotCreated` | No | Load stream from its beginning or repair it | actual event kind | `[NEW-SPEC]` |
| Event is illegal for state | `InvalidTransition` | No | Reject command or repair event history | current state, event kind | TD §4.2 |
| Turn ID differs | `ActiveTurnMismatch` | No | Route/re-read exact active turn | expected, actual IDs | INV-004 |
| Approval ID differs | `PendingApprovalMismatch` | No | Route/re-read exact pending approval | expected, actual IDs | TD §4.2 |

Errors contain identifiers, numeric sequence/version values, state, and event kind only. They never
contain prompt, provider, tool-output, credential, path, or arbitrary serialized payload text.

# Security Contract

The reducer treats the envelope as already decoded but untrusted value input. It rejects
cross-session routing, sequence confusion, unsupported schemas, illegal transitions, and
turn/approval confusion before returning a new projection. It grants no authorization and exposes
no executable, provider, repository, environment, credential, path, or arbitrary text field.

Unknown serde event variants and unknown envelope fields are rejected rather than ignored.

# Observability and Audit Contract

No logs, metrics, traces, events, or audit records are emitted. A caller may classify the typed
error and record boundary-specific telemetry without arbitrary event content. Causation and
correlation UUIDs are preserved in the envelope but have no reducer authority.

# Test Specification

## Unit

- `domain_event_envelope_v1_round_trips_without_field_drift`
- `domain_event_v1_variant_schema_is_locked`
- `domain_event_serde_rejects_unknown_fields_and_variants`
- `session_reducer_applies_every_legal_transition`
- `session_projection_tracks_status_turn_approval_sandbox_activity_and_high_water`
- `session_reducer_rejects_wrong_stream`
- `session_reducer_rejects_duplicate_gap_and_out_of_order_sequences`
- `session_reducer_rejects_sequence_overflow`
- `session_reducer_rejects_unsupported_schema_version`
- `session_reducer_requires_creation_first`
- `session_reducer_rejects_every_illegal_transition`
- `session_reducer_rejects_wrong_turn_and_approval`
- `session_reducer_errors_have_bounded_safe_debug`
- `runtime_event_kind_classification_is_total`

## Contract

Not applicable beyond the public crate tests because T020 has no adapter. T030 will reuse these
values in event-store contract tests without weakening this reducer contract.

## Property / Model

- `session_reducer_replay_is_deterministic`
- `session_reducer_checked_failures_are_e0`

Generate bounded legal turn/approval/cancel cycles and bounded invalid envelopes. Equal input
sequences must produce equal projections; every rejected apply must leave the source reducer equal
to its snapshot.

## Integration

The existing `codebox-domain` integration-test target exercises the public re-exported API.
Workspace gates prove no P0 regression. Database and restart integration are T030.

## Fault Injection

External fault injection is not applicable to an E0 memory-only boundary. Sequence overflow,
malformed serde, and every checked transition failure are injected directly.

## Security

- Cross-stream envelope rejection
- Unsupported-version and unknown-field/variant serde rejection
- Turn and approval identity confusion rejection
- Error/debug inspection proving absence of arbitrary content fields

## Regression

- `regression_ephemeral_not_persisted` — TD §11.3 P7 / `CU-PROTO-03`
- All T010 identifier, path, and sequence regressions remain green.

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Design review | Focused Cursor review found two missing evidence cases; both repaired | Explicit error-precedence cases and exact JSON fixtures for all 14 version-1 variants |
| Failing test skeleton | Failed before production implementation with the fixed `T020 skeleton: not implemented` panic | `cargo test -p codebox-domain --test events_reducer --all-features -- --exact domain_event_envelope_v1_round_trips_without_field_drift` |
| Focused domain suite | Passed: 1 reducer unit, 10 retained T010 integration tests, 16 T020 integration tests, and 1 compile-fail doctest | Local 2026-07-30 run |
| Workspace gates | Passed: 196 Rust tests, 10 Node tests, formatting, Clippy, build, dependency policy, and diff check | Local 2026-07-30 run |
| Hosted CI | Passed on implementation commit `375c3b6` | [GitHub Actions run 30523996895](https://github.com/fallrising/fanzloud/actions/runs/30523996895) |
| Final fresh-context review | `T020 IMPLEMENTATION ACCEPTED` | [`ACCEPT-T020`](../acceptance/T020.acceptance.md) |

# Traceability

- `CU-PROTO-01` → TD §§2.3 INV-003/INV-004, 4.2, 4.4–4.5 → this specification → reducer tests
- `CU-PROTO-03` → TD §4.4 and §11.3 P7 → this specification →
  `regression_ephemeral_not_persisted`
- T020 → TD §§15.1–15.2 → this specification → `codebox-domain`

# TD Gaps

None.

The explicit creation event, version-1 payload set, immutable reducer API, error precedence,
highest-sequence activity timestamp, and persistence classifier enum are `[NEW-SPEC]` local
derivations. They make existing TD requirements executable without changing trust boundaries,
failure atomicity, retry, recovery, or persistence ownership.

# Self-Check

- Archetype A: total over typed inputs, deterministic, replay-deterministic, and property-tested.
- E0: no external or shared mutable state; checked failures preserve the source reducer.
- Concurrency: T020 does not select writers; T030/T040 ownership is explicit.
- Streaming: complete values only; ephemeral categories are classified, not buffered or persisted.
- Cancellation/timeout/crash: no owned resource or side effect.
- Security: no arbitrary text or execution authority; routing/schema/identity confusion fails
  closed.
- Non-guarantees: persistence, authorization, upcasting, cleanup, transport, and coalescing are
  explicit.
- Every normative assertion traces to TD, T010, or `[NEW-SPEC]`.
