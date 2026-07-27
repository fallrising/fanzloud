# Codebox Agent Instructions

You are implementing Codebox from `docs/TD.md`.

1. Treat `docs/TD.md`, accepted ADRs, task documents, and specifications as normative.
2. Build or update the task graph before implementing an undecomposed feature.
3. Work on exactly one Ready task at a time.
4. Every production task must reference Contract Units and machine-executable acceptance. The
   infrastructure-only exception is defined by TD §9.3 and ADR-0001.
5. Before production code, create or update the normative specification under `docs/specs`.
6. Answer every archetype question. Mark missing security, atomicity, retry, or recovery semantics as
   `[TD-GAP]`; do not invent them.
7. Generate test skeletons from every contract clause before implementation.
8. Implement the smallest change that satisfies the specification. Do not add unrelated refactors or
   future-phase behavior.
9. Never weaken a system invariant, trust boundary, sandbox restriction, or secret boundary to make
   tests pass.
10. Expected external failures use typed `Result` errors. Do not panic, unwrap, or expose internal
    secrets for external input.
11. Record every side effect through intent, authorization, started, and outcome states. Never retry
    `OutcomeUnknown` automatically.
12. Run the formatting, linting, unit, contract, integration, security, fault, and end-to-end tests
    required by risk and specification.
13. Update acceptance evidence, rustdoc projection, task status, and traceability.
14. Request a fresh-context document-driven acceptance review.
15. A task is complete only when its acceptance report says accepted.
16. If the design conflicts with implementation, repair the documentation through an ADR or
    specification change before continuing.

