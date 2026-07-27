# Development Handoff

Date: 2026-07-27

## Current Goal

Deliver a private, single-operator P0 that lets the project owner operate their own ChatGPT/Codex
subscription through a Codebox web control layer.

The first slice uses provider-managed Codex Cloud tasks rather than local `codex exec`. This keeps
the local subscription credential runner separate from repository-controlled execution.

## Repository State

Branch: `main`

Local commits:

```text
f9f3e2d docs: save development handoff
9d45850 docs: prioritize personal Codex cloud BYOS slice
d605028 docs: record T000 verification evidence
76f72b3 chore: bootstrap codebox workspace
```

The pushed remote state is `f9f3e2d` (`main -> origin/main`). The current T000/T001/T010
documentation and T010 implementation changes are uncommitted and have not been pushed.

The configured remote is `git@github.com:fallrising/fanzloud.git`; `origin/main` points to
`f9f3e2d`.

## Completed

- T000 Rust workspace bootstrap implementation.
- Four inert binary packages: control plane, node agent, boxd, and CLI.
- Rust `1.97.1` and `cargo-deny 0.19.4` pins.
- Formatting, Clippy, test, build, dependency-policy, and CI configuration.
- All five T000 commands passed against clean commit `76f72b3`.
- Hosted GitHub Actions run `30260756940` passed all five T000 commands; T000 is accepted.
- ADR-0001 infrastructure-only task exception.
- Accepted ADR-0002 personal BYOS Codex P0.
- P0 Contract Unit inventory and T001–T007 task graph.
- Claude reviewed ADR-0002 twice. The initial local-exec design was rejected because Codex's local
  sandbox must not be relied on to prevent credential reads. The revised Codex Cloud design passed
  content review.
- T010 domain implementation: strong UUID IDs, validated `WorkspacePath`, `EventSeq`, typed
  errors, serde validation, and compile-fail coverage. Local executable acceptance passed.

## Verified Official Codex Surface

- Current stable npm package inspected: `@openai/codex@0.145.0`.
- ChatGPT sign-in provides subscription-backed Codex access.
- Headless login supports `codex login --device-auth`.
- The pinned CLI exposes experimental:
  - `codex cloud exec`
  - `codex cloud status`
  - `codex cloud list --json`
  - `codex cloud diff`
- P0 must pin the CLI version and contract-test captured output because the cloud commands are
  experimental.

## Decided P0 Flow

```text
Private operator browser
→ Codebox control plane
→ trusted credential runner with operator-owned CODEX_HOME
→ pinned Codex Cloud CLI
→ OpenAI-managed Codex Cloud environment and repository
→ normalized status and final diff
```

The credential runner must never check out repositories or run repository-controlled commands.
Codex Cloud environment ID and branch are administrator configuration, not browser-controlled input.

## Current Blockers

1. T001 remains `verifying` until its fresh document-first acceptance review returns a report.
2. T010 remains `verifying` for the same acceptance-process reason; its executable checks pass.
3. The current T010/docs changes are uncommitted and unpushed.

## Next Session

1. Obtain fresh read-only Claude acceptance reports for T001 and T010, then mark accepted if clear.
2. Ask for authorization before pushing the current uncommitted changes.
3. Write T002/T003 specifications and tests after T001 and T010 are accepted.
4. Implement:
   - T002 Codex login broker
   - T003 pinned Codex Cloud CLI adapter
   - T004 cloud task orchestrator
   - T005 session API and stream
   - T006 minimal private web UI
   - T007 deterministic and live subscription E2E

Do not revert to local `codex exec` beside `CODEX_HOME` unless a new ADR provides a proven
credential-read isolation mechanism.

## Validation Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo build --workspace --bins --all-features
cargo deny check
```
