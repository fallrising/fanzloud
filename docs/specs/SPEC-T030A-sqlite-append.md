---
id: SPEC-T030A
title: SQLite initialization and atomic event append
status: verified
contract_units: [CU-EVT-01]
module: codebox-event-store
milestone: P1
archetypes: [E, B]
atomicity: E1
invariants: [INV-003, INV-004]
depends_on: [SPEC-T020]
td_sections: [2.3, 3.2, 4.4-4.6, 7.4, 8.1-8.10, 11.1-11.4, 14, 15.1-15.2, 16.2]
adr_refs: []
risk: high
---

# Intent

Provide the first durable P1 storage transition: initialize one private version-1 SQLite event
database and atomically append a non-empty batch at one exact session-stream high-water. The
adapter assigns event identity and sequence; it does not decide whether the semantic transitions
are legal.

# Responsibility

## Does

- Opens or initializes one private SQLite database with a fixed Codebox application ID, user
  version, event table, and unique event/stream-sequence constraints.
- Appends one bounded batch in one write transaction after comparing the committed stream
  high-water with `expected_seq`.
- Assigns non-nil UUIDv4 event IDs and contiguous sequence values.
- Stores every accepted T020 envelope field in a version-1 row codec that T030B can replay.
- Returns committed envelopes only after the SQLite commit succeeds.

## Does Not

- Expose public replay, snapshot, actor, command, WebSocket, reducer, migration, upcaster, backup,
  encryption, audit, or side-effect-ledger behavior.
- Validate TD §4.2 lifecycle transitions; the accepted T020 reducer and later actor own semantics.
- Make append idempotent by payload equality or permit a conflict loser to advance and retry
  blindly.
- Accept an administrator database path from an HTTP/browser/model/repository boundary.

# Public Boundary

```rust
pub const MAX_APPEND_EVENTS: usize = 256;
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 65_536;
pub const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SqliteEventStore { /* private validated database path and ID source */ }

impl SqliteEventStore {
    pub async fn open(database_path: PathBuf) -> Result<Self, EventStoreError>;

    pub async fn append(
        &self,
        stream: SessionId,
        expected_seq: EventSeq,
        events: Vec<NewDomainEvent>,
    ) -> Result<Vec<DomainEventEnvelope>, EventStoreError>;
}
```

`append` is the exact `CU-EVT-01` signature from TD §16.2. `open` is inseparable adapter
construction, not a second state-machine CU. T030B–D add the remaining TD §4.6 methods without
changing this signature.

# Inputs and Outputs

- `database_path` is an administrator-owned absolute UTF-8-or-platform path to one SQLite file.
  The existing canonical parent directory is the containment boundary.
- `stream` is one validated non-nil T010 `SessionId`.
- `expected_seq` is the caller's last observed committed sequence, including zero for a new stream.
- `events` contains 1–256 complete T020 `NewDomainEvent` values. Every payload's serialized JSON is
  at most 65,536 bytes and every `schema_version` is exactly `DOMAIN_EVENT_SCHEMA_V1`.
- Success returns one envelope per input in input order with copied metadata/payload, the named
  stream, unique generated IDs, and sequences `expected_seq + 1..=expected_seq + len`.

The count and payload bounds are `[NEW-SPEC]` P1 resource limits. Fixed-width big-endian 8-byte
sequence storage is `[NEW-SPEC]` so the full T010 `u64` domain retains SQLite lexical ordering
without signed-integer narrowing.

# Preconditions and Disposition

| ID | Condition | Type / Checked / Internal | Trace |
|---|---|---|---|
| P1 | Stream ID is non-nil | T010 type invariant | TD §§4.1, 8.3 |
| P2 | Database path is absolute and its parent is existing, canonical, private, and owned by the process user | Checked `InvalidDatabasePath` | TD §§2.2, 7.4; `[NEW-SPEC]` |
| P3 | Existing target is a regular non-symlink private file owned by the process user | Checked `InvalidDatabasePath` | TD §§7.4, 8.5; `[NEW-SPEC]` |
| P4 | Batch is non-empty | Checked `EmptyBatch` before database access | TD §16.2 |
| P5 | Batch count is within 256 | Checked `BatchTooLarge` before database access | TD §§8.5, 11.2; `[NEW-SPEC]` |
| P6 | Every schema version is exactly version 1 | Checked `UnsupportedEventSchema` before database access | TD §4.4; SPEC-T020 |
| P7 | Every version-1 payload is within 65,536 bytes | T020 closed enum/schema invariant plus defensive checked `EventPayloadTooLarge` on internal codec drift | TD §§8.5, 11.2; SPEC-T020; `[NEW-SPEC]` |
| P8 | `expected_seq + batch length` is representable | Checked `SequenceOverflow` before transaction | INV-003; TD §16.2 |
| P9 | Committed stream high-water equals `expected_seq` | Checked under the write transaction; `SequenceConflict` returns actual | TD §16.2 |
| P10 | Generated event IDs are non-nil and globally unique | UUID source plus checked SQLite key constraint | TD §§4.4, 16.2 |
| P11 | Database application/schema identity is version 1 | Checked `UnsupportedDatabaseSchema` / `CorruptStore` | TD §§4.4, 12; `[NEW-SPEC]` |

The process that configures the database path is an administrator boundary. Rejecting malicious
same-UID concurrent replacement of a validated parent/path would require descriptor-relative
deployment integration and is explicitly not claimed by this child.

# Success Postconditions

1. Exactly one transaction committed all input rows.
2. The committed stream high-water is `expected_seq + events.len()`.
3. `(stream_id, seq)` is unique and sequences assigned by this call are contiguous.
4. Every event ID is non-nil and globally unique.
5. Returned envelopes exactly match committed rows and preserve input order and all input metadata.
6. Dropping and reopening the store preserves the committed batch.
7. The version-1 row codec is immediately readable by the same internal decoder and the future
   T030B replay boundary.

# Non-Guarantees

- T030A does not expose `load_after`; callers receive public replay only after T030B.
- Append does not prove lifecycle legality, first-event creation, or reducer replay validity.
- Payload equality is not an idempotency key.
- SQLite/database-file confidentiality depends on the private deployment directory and backups;
  T030A does not encrypt at rest.
- A cancelled Rust future does not stop a running blocking SQLite transaction.
- SQLite durability assumes the filesystem and storage device honor SQLite sync semantics.
- Schema migration from any non-version-1 database is not attempted.

# Exit Invariants

After success, checked failure, busy timeout, caller cancellation, process crash, or restart:

- the event table contains either every row in the batch or none of them;
- every committed stream retains one unique, gap-free sequence at each high-water position;
- a checked conflict or constraint failure does not change the stream;
- no returned success precedes the transaction commit; and
- database-path, event-payload, and SQLite diagnostic text are absent from public error/debug
  values.

# Side Effects

`open` may create one mode-`0600` database plus SQLite-owned journal/WAL sidecars inside the
validated private parent. It sets a fixed application ID, user version 1, `journal_mode=WAL`,
`synchronous=FULL`, and a five-second busy timeout. Version-1 DDL is transactional. If initial
opening is interrupted, the next `open` either completes an empty version-0 initialization or
fails closed on incompatible/non-empty schema state.

`append` opens a bounded connection, starts one immediate write transaction, reads the current
high-water under that writer lock, inserts the complete batch, and commits once.

# Idempotency

Append is not idempotent by content. Repeating a successful call with the original
`expected_seq` returns `SequenceConflict { actual }` and appends nothing. After cancellation or a
lost response, a caller may issue the exact same expected-sequence attempt; it either commits if
the first attempt did not, or conflicts if it did. A caller must then use T030B to re-read and
re-decide. It must never replace `expected_seq` with the conflict's actual value and blindly retry.

# Concurrency and Ordering

SQLite `BEGIN IMMEDIATE` selects one writer before the actual-sequence read. For concurrent calls
with the same stream and expected sequence, exactly one may commit; every later writer observes the
winner's committed high-water and returns conflict. Different streams remain serialized by
SQLite's single-file writer boundary in P1 but retain independent sequence values.

Input order determines assigned sequence order. Event occurrence timestamps do not affect ordering.

# Streaming Semantics

Not applicable. Append accepts and returns complete bounded vectors; it emits no chunks or live
frames. T030B owns bounded replay and later session tasks own live streaming/backpressure.

# Cancellation and Timeout

Each blocking SQLite operation runs off the async executor. SQLite lock acquisition is bounded by
`SQLITE_BUSY_TIMEOUT`; expiration returns `Busy` with no partial batch. Dropping/cancelling the
async future does not kill its blocking transaction, so the caller must treat completion as
unobserved and retry only with the same `expected_seq`, then re-read/re-decide on conflict.

There is no broader operation deadline in T030A. A caller may impose one but inherits the same
unobserved-completion rule. A process crash before commit rolls back; a crash after commit leaves
the complete batch replayable after restart.

# Failure Atomicity

E1 for `CU-EVT-01`. Validation failures occur before a transaction. Conflict is detected under the
writer transaction. Any serialization, generated-ID, SQL statement, constraint, busy, or commit
failure returns without a partially visible batch. SQLite commit is the only durable boundary.

Adapter initialization is an inseparable prerequisite rather than an append transition. It never
creates event rows and fails closed on incompatible schema/application identity.

# Failure Modes and Error Contract

| Case | Error | Retriable | Caller action | Required payload | Trace |
|---|---|---:|---|---|---|
| Empty input | `EmptyBatch` | No | Supply at least one event | None | TD §16.2 |
| More than 256 events | `BatchTooLarge` | No | Split at a semantic boundary and re-decide expected sequence | max, actual | `[NEW-SPEC]` |
| Unsupported event version | `UnsupportedEventSchema` | No | Use supported schema/upcaster task | index, supported, actual | TD §4.4 |
| Oversized payload | `EventPayloadTooLarge` | No | Store large data as artifact / reduce semantic event | index, max, actual | TD §§4.4, 8.5 |
| Sequence arithmetic overflow | `SequenceOverflow` | No | Stop stream and investigate | expected, count | INV-003 |
| Expected sequence differs | `SequenceConflict` | No blind retry | Re-read and re-decide | expected, actual | TD §16.2 |
| Generated ID collision | `DuplicateEventId` | Bounded same-expected retry allowed | Retry exact call; investigate repeated source failure | None | TD §16.2 |
| SQLite lock timeout | `Busy` | Yes, bounded | Back off and retry same expected sequence | None | TD §§8.6, 16.2; `[NEW-SPEC]` |
| Invalid/private-path check | `InvalidDatabasePath` | After operator repair | Repair administrator configuration | bounded reason enum | TD §§7.4, 8.2 |
| Unknown application/user schema | `UnsupportedDatabaseSchema` | No | Run an explicit migration or select correct DB | expected, actual | TD §§4.4, 12 |
| Malformed/inconsistent stored structure | `CorruptStore` | No | Stop writes, restore/investigate | bounded stage enum | TD §§8.2, 11.2 |
| Open/transaction/commit failure | `Storage` | Depends on bounded kind | Repair storage; same-expected retry only when classified transient | bounded operation/kind | TD §§8.2, 8.6 |
| Blocking worker cannot complete/join | `WorkerUnavailable` | Same expected only | Reconcile/reopen before continuing | None | TD §§8.2, 8.7 |

No error includes a filesystem path, SQL text, serialized payload, SQLite message, prompt, secret,
or provider output.

# Security Contract

- The target is an administrator-configured local path, never browser/model/repository-selected.
- The parent must already exist, resolve canonically, be owned by the process user, and deny
  group/other access. A new database is created mode `0600`; an existing symlink, non-regular file,
  foreign owner, or group/other-accessible file is rejected.
- The store never logs or embeds event JSON or database paths in errors/debug output.
- Every serialized payload and batch is bounded before a write transaction.
- SQL uses fixed statements and bound parameters only.
- Application/user version checks fail closed; no loadable extensions, arbitrary pragmas, or SQL
  input are exposed.
- T020's durable schema contains no prompt, diff, tool output, credentials, or paths. T030A does
  not weaken that boundary or accept ephemeral runtime events.

# Observability and Audit Contract

T030A emits no log or metric by itself. Success returns typed envelopes with session, sequence,
event, causation, and correlation identifiers for caller-owned structured observability. Errors
expose only bounded classifications and safe numeric/enum data. Audit storage is outside this CU.

# Test Specification

The following exact tests must first exist as compiling fixed-failure skeletons.

## Unit

- `sqlite_sequence_codec_preserves_full_u64_order`
- `sqlite_row_codec_preserves_every_envelope_field`
- `duplicate_event_id_rolls_back_entire_batch`
- `event_store_errors_have_bounded_safe_debug`
- `event_payload_limit_rejects_oversized_bytes`
- `database_owner_policy_rejects_foreign_parent_and_target`

## Contract

- `append_rejects_empty_batch_without_database_access`
- `new_stream_append_assigns_contiguous_unique_envelopes`
- `append_rejects_oversized_batch_and_all_v1_payloads_fit_bound`
- `append_rejects_unsupported_event_schema`
- `append_rejects_sequence_overflow_without_database_change`
- `sequence_conflict_returns_actual_without_change`
- `append_does_not_interpret_reducer_transitions`

## Property / Model

- `append_model_keeps_sequences_contiguous_and_metadata_exact`

## Integration

- `two_writers_same_expected_sequence_exactly_one_succeeds`
- `committed_batch_persists_across_store_restart`
- `initialized_database_has_fixed_identity_and_private_mode`
- `database_schema_drift_fails_closed`

## Fault Injection

- `mid_transaction_failure_rolls_back_entire_batch`
- `busy_timeout_leaves_stream_unchanged`
- `append_future_cancellation_requires_same_expected_reconciliation`

The duplicate-ID unit test injects a private deterministic ID source; production callers cannot
choose persisted IDs. The mid-transaction test installs a test-only abort trigger after schema
initialization. Both exercise the production transaction and constraints.

## Security

- `database_path_rejects_relative_symlink_and_open_permissions`
- `store_debug_and_errors_do_not_expose_path_or_payload`

## Regression

- All accepted T010/T020 tests, including `regression_ephemeral_not_persisted`, remain green.

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Fresh decomposition/design review | `T030A DESIGN ACCEPTED`; T030A first Ready child | Fresh read-only Cursor Agent, 2026-07-30 |
| Failing test skeleton | Failed before production implementation with fixed `T030A skeleton: not implemented` panic | Exact `append_rejects_empty_batch_without_database_access` run |
| Focused suite | Passed: 6 unit + 17 integration/property/concurrency/fault/security tests | Local 2026-07-30 run |
| Workspace/security/fault gates | Passed: 219 Rust, 10 Node, fmt, Clippy, build, dependency policy, diff check | Local 2026-07-30 run |
| Fresh implementation review | `CONTRACT ACCEPTED` and `SECURITY ACCEPTED` after two evidence blockers were repaired | Fresh read-only Cursor Agent and Grok reviews, 2026-07-30 |
| Hosted CI | Passed: `Rust` job in 3m41s on implementation commit `16b468b` | [GitHub Actions run 30554757181](https://github.com/fallrising/fanzloud/actions/runs/30554757181) |
| Acceptance | Accepted | [`ACCEPT-T030A`](../acceptance/T030A.acceptance.md) |

# Traceability

- `CU-EVT-01` → TD §§2.3 INV-003/INV-004, 4.4–4.6, 16.2 → this specification →
  `codebox-event-store` append/fault/concurrency/restart tests.
- T030A → TD §§9.3, 15.1–15.2 → T030 parent decomposition.
- Accepted T020 schema → this version-1 row codec; schema changes require an explicit later
  migration/upcaster note.

# TD Gaps

None for T030A.

Snapshot save atomicity is a separate `CU-EVT-04` gap recorded in `T030D.task.md`; it does not
block append. Bounds, private path checks, UUID generation, SQLite application/user version,
big-endian sequence storage, WAL/FULL sync, and busy timeout are `[NEW-SPEC]` reversible adapter
details that preserve the fixed E1 and retry contract.

# Self-Check

- Archetype E: legal transition is exact expected-sequence append; the transaction-lock winner
  commits, loser re-reads/re-decides; commit order and all exit invariants are explicit.
- Archetype B: containment, bounds, malicious path shapes, ownership/mode checks, cleanup
  limitations, and same-UID replacement non-guarantee are explicit.
- E1: validation/conflict/constraint/busy/crash partitions never expose a partial batch.
- Retry: only bounded same-expected retry is allowed; conflict never permits blind advancement.
- Streaming: complete bounded vectors only.
- Cancellation/timeout: unobserved blocking completion and same-expected reconciliation are
  explicit.
- Security/observability: fixed SQL, private path, bounded payload, redacted errors, no logs.
- Non-guarantees and downstream T030B–D ownership are explicit.
- Every normative assertion traces to TD, accepted T020, or `[NEW-SPEC]`.
