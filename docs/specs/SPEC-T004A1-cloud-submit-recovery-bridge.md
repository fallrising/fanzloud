---
id: SPEC-T004A1
subject: T004A1 Codex Cloud submit recovery bridge
status: accepted
contract_units: [CU-CLOUD-P0-01]
archetypes: [C, F]
atomicity: E2
retriable: false
---

# Normative Inputs

- TD §§2.3 INV-006, 8.2–8.10, 9.2–9.3, 11.3 P15, 14, and 15.0
- ADR-0002 and ADR-0003
- Accepted T004A specification, implementation, and acceptance
- T004A1 task and proposed T004B specification repair

# Contract Boundary

T004A1 is a narrow amendment to the accepted T004A durable submit ledger. It gives the in-crate
T004B composition layer two capabilities that the public Cloud runner intentionally lacks:
prompt-free observation of an existing caller-created operation and durable application of an
already-authorized T004B unknown-resolution decision.

The bridge does not select a candidate, authenticate an operator, create a duplicate-risk
acknowledgement, inspect provider status, expose a public retry, or execute a Cloud command.
Contract: CU-CLOUD-P0-01. `[NEW-SPEC]`

# Private Composition Boundary

The exact private representation may differ, but the behavior is equivalent to:

```rust
pub(crate) enum CloudSubmitObservation {
    Absent,
    FailedBeforeSpawn,
    OutcomeUnknown,
    TaskRecorded(CloudSubmission),
    ExplicitlyAbandoned,
}

pub(crate) enum CloudUnknownResolution {
    AdoptListedTask(CloudTaskId),
    ExplicitlyAbandon,
}

impl CloudTaskRunner {
    pub(crate) fn observe_submit(
        &self,
        operation_id: CloudSubmitOperationId,
    ) -> Result<CloudSubmitObservation, CloudRunnerError>;

    pub(crate) fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        resolution: CloudUnknownResolution,
    ) -> Result<CloudSubmitObservation, CloudRunnerError>;
}
```

Neither enum nor method is exported from the crate. T004B is the only intended production caller.
No prompt, executable, path, environment, argv, or retry callback crosses this boundary.
`[NEW-SPEC]`

# Prompt-free Observation

`observe_submit` holds the accepted credential-scope lease, loads and validates the current T004A
ledger, applies the accepted crash classification, and returns:

| Durable state for requested operation | Observation |
|---|---|
| No ledger | `Absent` |
| A different terminal operation that permits a new operation | `Absent` |
| A different unknown/nonterminal operation | Typed operation conflict naming only that current operation ID |
| `Intent` | First commit `FailedBeforeSpawn`, then return it |
| `Authorized` or `Started` | First commit `OutcomeUnknown`, then return it |
| `FailedBeforeSpawn` | `FailedBeforeSpawn` |
| `TaskRecorded` or `TaskAdopted` | Validated `TaskRecorded(CloudSubmission)` |
| `OutcomeUnknown` or `ReconciliationObserved` | `OutcomeUnknown` |
| `ExplicitlyAbandoned` | `ExplicitlyAbandoned` |

Observation runs no version, login, exec, status, list, or diff command. It never needs the prompt,
which remains absent from every durable record. `[NEW-SPEC]`

# Durable Resolution

The T004A version-1 ledger adds two terminal phases:

```text
ReconciliationObserved → TaskAdopted(task_id)
ReconciliationObserved → ExplicitlyAbandoned
```

Both phases permit a later independent operation ID. They do not imply that the provider task was
canceled or that an abandoned operation created no task.

`AdoptListedTask` is accepted only for the exact current operation, only from
`ReconciliationObserved`, and only when the validated task ID appears in the exact latest durable
candidate set. T004B must successfully inspect provider status before requesting this transition.

`ExplicitlyAbandon` is accepted only for the exact current operation and only after at least one
durable `ReconciliationObserved` phase. T004B owns the authenticated named
`DuplicateRiskAcknowledgement`; T004A stores no authentication payload.

Replaying the exact adopted task or exact abandonment returns the existing terminal observation
without another write or command. A conflicting task, resolution kind, operation ID, missing
reconciliation, or missing candidate fails closed and leaves the ledger unchanged. `[NEW-SPEC]`

# Commit Order and Crash Matrix

```text
T004B validates current operation and decision
→ for adoption, T004B performs one successful status inspection
→ T004A1 reloads and validates exact reconciliation evidence
→ durable TaskAdopted or ExplicitlyAbandoned
→ return terminal observation to T004B
→ T004B durably projects its lifecycle terminal
```

If the process crashes after the T004A1 terminal commit but before the T004B lifecycle commit, a
restart observes `TaskRecorded` or `ExplicitlyAbandoned` without prompt or provider mutation and
repairs the local lifecycle. If the T004A1 commit fails, T004B remains unknown and no later submit
is authorized. `[NEW-SPEC]`

# Ledger Validation and Bounds

- The existing `cloud-submit-ledger.json` path, schema version, 64-KiB size, 32-entry history,
  mode-`0600`, effective-UID owner, single-link, no-follow, synced temporary-file replacement, and
  directory-sync requirements remain unchanged.
- `TaskAdopted` requires exactly one validated task ID and durable prior reconciliation evidence.
- `ExplicitlyAbandoned` has no task ID and retains the bounded latest reconciliation evidence.
- Every loaded history must follow the amended transition graph; malformed or impossible terminal
  fields fail closed.
- Debug and errors contain only safe categories and operation IDs, never candidate lists, provider
  summaries, raw output, prompt, paths, or credentials.

# Archetype C Answers

- Atomicity remains E2: the provider outcome can be unknown, but local evidence and explicit
  resolution are durable.
- Duplicate prevention remains the scope lease plus current-operation ledger; terminal resolution
  permits only a later explicit new operation ID.
- Unknown is presented until T004B supplies a reviewed explicit resolution.
- Crashes before the T004A1 commit leave unknown; crashes after it replay the terminal observation.
- No automatic retry exists; neither bridge method executes Cloud exec.

# Archetype F Answers

- Authority is crate-private and reachable only through T004B composition.
- Errors distinguish missing operation, operation conflict, wrong phase, candidate mismatch,
  resolution conflict, and ledger failure without rejected values.
- Both methods are command-free; retrying observation or the exact resolution is replay-idempotent.
- Existing ledger bounds and redaction apply unchanged.

# Required Test Skeletons

| Clause | Required test |
|---|---|
| Full observation matrix | `cloud_submit_observation_recovers_every_durable_phase` |
| Prompt-free and command-free observation | `cloud_submit_observation_never_executes_cli` |
| Different lower unknown blocks readiness | `cloud_submit_observation_rejects_different_unknown_operation` |
| Adoption requires latest recorded candidate | `cloud_submit_resolution_adopts_only_latest_candidate` |
| Abandonment requires reconciliation | `cloud_submit_resolution_requires_reconciliation` |
| Resolution replay/conflict | `cloud_submit_resolution_is_replay_idempotent_and_conflict_safe` |
| Later explicit submit | `cloud_submit_resolution_terminal_allows_new_operation_without_auto_exec` |
| Crash between lower and lifecycle commits | `cloud_submit_resolution_restarts_from_terminal_observation` |
| Ledger/redaction regression | `cloud_submit_resolution_ledger_fails_closed_and_redacts` |

# Traceability and Gaps

This bridge closes the prompt-free recovery and permanently-blocked-ledger gaps found during the
T004B design review. It preserves accepted T004A public behavior and P15: reconciliation alone
never authorizes retry, while a separate explicit T004B decision can durably terminalize the
unknown operation. No in-scope `[TD-GAP]` remains.

# Design Review

The first review rejected the original T004B design because accepted T004A lacked prompt-free
observation and explicit unknown terminalization. Revised reviews required a lower-readiness gate
before every lifecycle intent and a named rollback fault test. After those corrections, a final
fresh Cursor Agent review returned `DESIGN ACCEPTED` with no blocker.

# Acceptance

All nine required tests, focused/workspace gates, dependency policy, P14/P15 regressions, and a
final fresh Cursor Agent acceptance review passed. Evidence:
[`ACCEPT-T004A1`](../acceptance/T004A1.acceptance.md).
