---
id: SPEC-T005
title: P0 session API and replayable live stream composition
status: ready
contract_units: [CU-SES-P0-01, CU-API-P0-01, CU-API-P0-02]
module: session-runtime/control-plane
milestone: P0
archetype: E+D+F
atomicity: per-child
invariants: [INV-004, INV-006, INV-007, INV-010, INV-012]
depends_on: [SPEC-T005A, SPEC-T005B, SPEC-T005C]
td_sections: [1.7, 2.3, 4.2, 4.7, 7, 8, 9, 10, 14, 15.0]
adr_refs: [ADR-0002, ADR-0003, ADR-0004]
risk: high
---

# Intent

Compose the three independently specified T005 children into the complete private P0
session/control-plane boundary.

# Responsibility

## Does

- Require accepted T005A, T005B, and T005C evidence.
- Prove the combined login, prompt, status, cancel, recovery, diff, refresh, and reconnect flow.
- Preserve the accepted credential and provider-managed execution boundaries.

## Does Not

- Add a production API beyond child contracts.
- Claim P1 `AgentBackend`, canonical event, artifact, approval, or crash-replay semantics.
- Authorize local repository execution, patch application, or provider retry.

# Public Boundary

The public boundary is exactly the union of SPEC-T005A, SPEC-T005B, and SPEC-T005C. The parent
defines no additional signature or endpoint.

# Inputs and Outputs

Inputs and outputs are owned by the child specifications.

# Preconditions and Disposition

| ID | Condition | Type / Checked / Internal | Trace |
|---|---|---|---|
| P-005-01 | Every child is independently Accepted | Acceptance gate | TD §10 |
| P-005-02 | T004 remains Accepted and unchanged or reverified | Acceptance gate | ADR-0003 |

# Success Postconditions

- A private authenticated operator can complete the ADR-0002 P0 browser flow through one
  process-lifetime session.
- Browser disconnect/reconnect does not cancel work and replays every retained event exactly once
  by sequence.
- Restart refreshes a T004-backed snapshot but does not claim replay of pre-restart P0 events.

# Non-Guarantees

All child non-guarantees apply, especially public/multi-user operation, crash-durable replay,
provider-side cancel, diff safety, and automatic repository mutation.

# Exit Invariants

Every exit preserves INV-004, INV-006, INV-007, INV-010, and INV-012.

# Side Effects

Only the child-owned side effects are allowed.

# Idempotency

Composition does not broaden T005B's process-instance-bound endpoint guarantees.

# Concurrency and Ordering

T005A is the sole session/event ordering authority. T005B/T005C must not synthesize session
sequence values.

# Streaming Semantics

T005C transports the exact T005A replay/snapshot/live handoff.

# Cancellation and Timeout

Only an explicit accepted T005B cancel command reaches T005A/T004B. Transport disconnect and
timeout never imply turn cancel.

# Failure Atomicity

Each child retains its declared atomicity. No parent-level transaction is claimed.

# Failure Modes and Error Contract

Child error contracts remain authoritative and must map without secret/internal leakage.

# Security Contract

The combined surface must pass P14/P15, authentication canaries, prompt/diff/code non-event
checks, stale-instance rejection, and forbidden-authority inspection.

# Observability and Audit Contract

Only safe typed operational categories, fixed actor identity, and bounded identifiers may be
observed. Credentials, bootstrap/cookie values, prompts, verification codes, diffs, raw provider
text, and internal paths are excluded.

# Test Specification

The parent runs every child suite plus these exact composition tests:

1. `p0_composition_operator_flow_replays_without_local_execution`
2. `p0_composition_secret_and_disconnect_boundaries_hold`

The first is a deterministic authenticated HTTP/WebSocket flow over the child ports and proves
initial replay, an explicit turn, disconnect independence, retained reconnect, and no implicit
cancel/recovery/diff call. The second combines forbidden-authority inspection with
bootstrap/cookie/prompt/diff/device-code/path canaries across HTTP and WebSocket outputs.

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Child acceptance reports | Passed | ACCEPT-T005A/B/C |
| Workspace gates | Pending | ACCEPT-T005 |
| Fresh composition review | Pending | ACCEPT-T005 |

# Traceability

TD T005, ADR-0004, T005, and CU-SES-P0-01/CU-API-P0-01/CU-API-P0-02.

# TD Gaps

None. ADR-0004 resolves the P0-only design choices.

# Self-Check

T005A, T005B, and T005C are independently Accepted. T005 is Ready for the two composition tests,
combined workspace gates, and a fresh read-only composition review. It authorizes no new
production behavior.
