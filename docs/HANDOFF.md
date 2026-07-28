# Development Handoff

Date: 2026-07-27

## Current Goal

Deliver the private, single-operator P0 that lets the project owner operate their own ChatGPT/Codex
subscription through a Codebox web control layer. Repository-controlled work remains in
provider-managed Codex Cloud; the local trusted runner must never check out or execute repository
code beside `CODEX_HOME`.

## Repository State

Branch: `main`

`HEAD` and `origin/main` currently point to:

```text
4a06ad2 docs: record T002 CI evidence
```

The working tree contains the coherent, uncommitted T001/T010 acceptance updates, T002A/T002B/T002
implementation and acceptance, and T003 Ready design described below. No commit or push was made in
this session. Preserve these changes and inspect `git status` before editing.

No hosted CI run exists for the current uncommitted implementation. Earlier hosted runs remain
historical evidence only.

## Accepted in This Working Tree

### T001 and T010

- Fresh, read-only Claude Code 2.1.220 acceptance reviews returned `ACCEPTED`.
- Task, acceptance, and traceability records now mark both tasks Accepted.
- Existing T010 local and hosted evidence was retained rather than reimplemented.

### T002A — credential scope and E1 lease

- Added the `codebox-agent-codex` workspace crate.
- Validates absolute canonical Linux paths, ownership/modes, symlinks, repository ancestry, and
  directory overlap.
- Exposes only fixed version/login-status/device-login argv and cleared-environment policy.
- Uses a mode-`0600`, descriptor-validated, nonblocking `flock` lease.
- Never reads `auth.json` or starts Codex.
- One unit test plus 12 integration contract/security tests pass.
- Fresh Claude acceptance returned `ACCEPTED`.

### T002B — E2 device-login lifecycle

- Pinned exact Codex CLI `0.145.0` version, login-status, device-prompt, and completion fixtures.
- Added typed redacted login values/errors, a fail-closed preflight, a versioned durable ledger,
  PID/start-time reconciliation, and no-automatic-retry handling for unknown outcomes.
- Added a dedicated non-pooled child supervisor with bounded background draining, process-group
  cancellation, instruction/overall deadlines, direct-child reap, and Linux
  `PR_SET_PDEATHSIG` race closure.
- Added 22 T002B unit tests; with the retained T002A owner-policy test the crate reports 23 unit
  tests, plus the 12 T002A integration tests.
- Fresh Claude acceptance returned `ACCEPTED` with no blocker.

Non-blocking T002B observations are recorded in `ACCEPT-T002B`: direct ANSI-SGR test coverage,
explicit reconciliation after normally exited malformed output, and fully joining both drainer
handles after the first join error are possible later refinements. They do not weaken fail-closed
behavior or leave a runnable child.

### T002 parent

- T002 is a coordination parent because CU-AUTH-P0-02 is E1 and CU-AUTH-P0-01 is E2.
- Both children and the combined workspace/P14 gates pass.
- A separate fresh composition review returned `ACCEPTED`.

## Current Ready Task

Exactly one task is Ready:

```text
T003 — pinned Codex Cloud contract adapter
CU-AGT-P0-01
archetypes D+F
atomicity E0
```

T003 is intentionally side-effect free. It owns typed values, fixed argv, and decoders for completed
bounded captures. It does not start a process, read credentials, submit/poll a task, reconcile an
unknown submission, implement `AgentBackend`, or apply a diff.

The original T003 seed mixed E0, E2, and E3 CUs. TD §9.3 required correction:

- T003 retains CU-AGT-P0-01 E0.
- CU-AGT-P0-02 E2 and CU-BKD-01 E3 move to the future T004 parent.
- T004 also owns CU-CLOUD-P0-01 E2 and CU-CLOUD-P0-02 E0 and must be decomposed into separate
  atomicity-specific children before any T004 implementation.

## Verified Pinned Cloud Surface

The official `rust-v0.145.0` source and local pinned CLI help establish:

- `cloud exec` succeeds by printing one task URL.
- `cloud status` prints three human-readable lines; only `READY` exits zero.
- `cloud list --json` emits the exact structured page recorded by the synthetic fixture.
- `cloud diff` prints an untrusted raw unified diff.
- `cloud apply` exists and is forbidden because it mutates a local working tree.
- The upstream cloud implementation may append account/diagnostic metadata to cwd-relative
  `error.log`; T004 must use the private trusted T002A working directory and never publish that file.

The source-derived fixtures under `docs/fixtures/codex-0.145.0/cloud/` contain no live credential,
account, repository, task, or user prompt.

Claude reviewed four T003 design revisions. The reviews found and removed:

- a missing list `url` field and URL/row-ID consistency rule;
- the upstream literal `-` stdin sentinel;
- missing Archetype D ownership answers;
- non-TTY status fixture spacing drift;
- missing numeric ceilings;
- missing property-based chunk-partition and missing-exit test skeletons; and
- unclear P14/P15 ownership.

The fourth fresh review returned `DESIGN ACCEPTED` with no blocker.

## Next Work

Work only on T003:

1. Read `docs/tasks/T003.task.md` and `docs/specs/SPEC-T003-codex-cloud-contract.md`.
2. Generate every named T003 test skeleton before production edits.
3. Implement the smallest E0 addition in `codebox-agent-codex`:
   - bounded strong values and redacted errors/debug;
   - non-extensible version/exec/status/list/diff argv;
   - exact completed-capture decoders;
   - no process, credential, retry, apply, or repository-execution surface.
4. Run focused tests/Clippy, then all workspace gates and `cargo deny check`.
5. Update rustdoc, specification evidence, task status, traceability, and request a fresh
   document-first acceptance review.
6. Only after T003 is Accepted, decompose the mixed-atomicity T004 parent before writing T004 code.

Do not re-run T001/T010/T002 acceptance work unless their relevant files or behavior change.

## Validation Evidence

The current working tree passed:

```text
cargo fmt --all -- --check
cargo test -p codebox-agent-codex --all-features
  23 unit tests + 12 integration tests passed
cargo clippy -p codebox-agent-codex --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo build --workspace --bins --all-features
cargo deny check
  advisories ok, bans ok, licenses ok, sources ok
git diff --check
```

`cargo deny check` requires access to the user advisory-cache lock in this environment and was run
with the approved permission. All other listed commands passed locally without a hosted result being
claimed.
