---
id: SPEC-T002
subject: T002 Codex login broker parent
status: decomposed
contract_units: [CU-AUTH-P0-01, CU-AUTH-P0-02]
archetypes: per-child
atomicity: per-child
retriable: false
---

# Decomposition

T002 is a coordination parent with no direct production implementation. TD §9.3 requires separate
tasks when independently testable boundaries use different failure-atomicity models:

- [`SPEC-T002A`](SPEC-T002A-credential-scope.md) owns CU-AUTH-P0-02, archetypes B+E, atomicity E1.
- [`SPEC-T002B`](SPEC-T002B-codex-device-login.md) owns CU-AUTH-P0-01, archetypes D+F, atomicity E2.

T002 is Accepted only after T002A and T002B are independently Accepted and their combined
workspace/security gates pass. T004 depends on the accepted T002 parent.

# Design Review

Claude Code 2.1.220 reviewed the T002 design three times with fresh, read-only context. The first
review required CU/atomicity alignment, fail-closed status preflight, and broker-crash child
handling. The second review identified Linux `PR_SET_PDEATHSIG` thread affinity. The final revision
uses separate T002A E1 and T002B E2 tasks plus a dedicated non-pooled child-supervisor thread, and
the third review returned `DESIGN ACCEPTED` with no blocker.
