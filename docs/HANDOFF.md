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
9d45850 docs: prioritize personal Codex cloud BYOS slice
d605028 docs: record T000 verification evidence
76f72b3 chore: bootstrap codebox workspace
```

The configured remote is `git@github.com:fallrising/fanzloud.git`. Nothing has been pushed. The local
branch reports `origin/main [gone]`.

## Completed

- T000 Rust workspace bootstrap implementation.
- Four inert binary packages: control plane, node agent, boxd, and CLI.
- Rust `1.97.1` and `cargo-deny 0.19.4` pins.
- Formatting, Clippy, test, build, dependency-policy, and CI configuration.
- All five T000 commands passed against clean commit `76f72b3`.
- Conditional T000 acceptance report; hosted GitHub CI remains pending.
- ADR-0001 infrastructure-only task exception.
- Proposed ADR-0002 personal BYOS Codex P0.
- P0 Contract Unit inventory and T001–T007 task graph.
- Claude reviewed ADR-0002 twice. The initial local-exec design was rejected because Codex's local
  sandbox must not be relied on to prevent credential reads. The revised Codex Cloud design passed
  content review.

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

1. T000 is still `verifying`, not `accepted`, because no hosted GitHub Actions run exists.
2. T001 depends on T000 and therefore remains `blocked`.
3. ADR-0002 remains `proposed` until T001 can be accepted.
4. Push is an irreversible external action under TD §0.4 and has not been authorized.

## Next Session

1. Ask for explicit authorization to push `main` to `origin/main`.
2. Push and monitor GitHub Actions.
3. Record the CI run evidence; finalize T000 acceptance.
4. Finalize ADR-0002 and produce fresh T001 acceptance.
5. Implement and accept T010 strong IDs and errors.
6. Write T002/T003 specifications and tests.
7. Implement:
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

