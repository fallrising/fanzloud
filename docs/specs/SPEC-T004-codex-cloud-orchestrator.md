---
id: SPEC-T004
subject: T004 Codex Cloud task orchestrator parent
status: proposed
contract_units: [CU-AGT-P0-02, CU-CLOUD-P0-01, CU-CLOUD-P0-02]
atomicity: split
retriable: false
---

# Normative Inputs

- TD §§1.6–1.7, 2.2–2.3, 8.2–8.10, 9.2–9.6, 11, 14, and 15.0
- ADR-0002 and ADR-0003
- T004 parent task
- Accepted T002 and T003

# Contract Boundary

T004 is a coordination parent, not one executable Contract Unit. Its P0 boundary combines two E2
boundaries and one E0 boundary. TD §§9.2–9.3 prohibit implementing that mixed atomicity as one
task. ADR-0003 keeps generic CU-BKD-01 conformance in T180. `[NEW-SPEC]`

The decomposition is:

```text
T002 + T003
      ↓
T004A  CU-CLOUD-P0-01 / E2 trusted submit + inspect runner
      ↓
T004B  CU-AGT-P0-02   / E2 task lifecycle
      ↓
T004C  CU-CLOUD-P0-02 / E0 diff retrieval
      ↓
T004 parent acceptance
```

T004C functionally needs both the accepted trusted runner from T004A and T004B's opaque
`DiffEligibleCloudTask` authority. The dependency order also ensures only one production child is
Ready at a time. `[NEW-SPEC]`

# Composition Rules

- T004A is Accepted; T004B is the sole Ready child.
- A child owns exactly the CU and atomicity listed in its task.
- Shared process code may be private implementation, but its fault tests are claimed by each
  affected CU; sharing code does not merge acceptance.
- T004B became Ready only after T004A was Accepted.
- T004C remains Blocked until its listed dependencies are Accepted.
- T004 remains Blocked until all child acceptance reports are Accepted and the combined P14/P15
  suite passes.

# Cross-Child Security Contract

- Every native CLI process uses the T002A executable, `env_clear`, only `CODEX_HOME`, the private
  non-repository working directory, null stdin, and piped bounded stdout/stderr.
- The only argv come from T003 fixed typed invocations.
- `cloud apply`, local `codex exec`, shells, repository URLs, local paths, hooks, checkout, and push
  are unrepresentable.
- Prompts may reach only the one fixed Cloud exec argv item. They never enter a durable ledger,
  error, debug representation, status/list/diff call, or generic backend identifier.
- A validated private `error.log/` directory sentinel makes the exact pinned cwd-relative
  diagnostic append a no-op; no upstream diagnostic file is returned as an event, artifact, diff,
  log, or download.
- A task ID is not a credential, but its URL and all raw process output retain the T003 redaction
  rules.

# Cross-Child Exit Invariants

- A process is either never spawned, reaped, or represented by a durable uncertain operation whose
  PID/start-time identity is checked during recovery.
- No unknown submit causes an automatic second Cloud exec.
- No successful public submission is returned before its task ID is durable.
- No child applies or executes provider-generated diff content.
- T004/T005 cannot claim a terminal canceled provider task unless an accepted lower boundary proves
  it.

# Machine Acceptance

Parent acceptance requires all child reports, all child named tests, the workspace gates, full P14,
exact P15, and a fresh read-only composition review. A documentation-only decomposition review
does not accept T004 or any production child.

# Non-Guarantees

- The parent does not provide HTTP, browser, persistence beyond its runner ledger, a live provider
  smoke, or multi-user isolation.
- Passing fake-CLI tests does not guarantee the experimental upstream CLI will remain compatible;
  the exact version and fixture pin remain deployment gates.
- The parent does not convert provider-side cancellation or task-correlation limitations into
  guarantees absent from the pinned source.

# Traceability and Gaps

The decomposition and serialized Ready order are `[NEW-SPEC]` applications of TD §9. ADR-0003
resolves the earlier E0 credential-domain and premature generic-backend gaps. No in-scope
`[TD-GAP]` remains in the parent decomposition.
