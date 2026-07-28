# Development Handoff

Date: 2026-07-28

## Current Goal

Deliver the private, single-operator P0 that lets the project owner operate their own ChatGPT/Codex
subscription through a Codebox web control layer. Repository-controlled work remains in
provider-managed Codex Cloud; the local trusted runner must never check out or execute repository
code beside `CODEX_HOME`.

## Repository State

Branch: `main`

T001, T010, T002A, T002B, T002, T003, and T004A are Accepted. T004A is the latest completed
production task. Its local acceptance is recorded in `ACCEPT-T004A`; inspect the latest commit and
hosted CI before continuing.

## Accepted Baseline

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
- Explicitly unlocks on lease drop so a concurrently forked child's pre-exec descriptor copy cannot
  extend the intended lease lifetime.
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

### T003 — pinned Codex Cloud contract adapter

- Added bounded environment, branch, prompt, task ID/URL, cursor, list-page, status, and raw-diff
  values with typed redacted errors and debug output.
- Added non-extensible version, Cloud exec/status/list/diff argv; there is no executable, process,
  credential, repository, retry, `cloud apply`, or diff-application surface.
- Added exact completed-capture decoders for the source-derived `0.145.0` fixtures, including strict
  schema/URL/exit mapping, RFC3339 and numeric bounds, missing-exit handling, and output limits.
- All 17 named T003 tests pass, including property-based chunk partitioning and narrow P14/P15
  regressions.
- A fresh, read-only Cursor Agent acceptance review returned `ACCEPTED` with no blocker. Claude was
  unavailable at its usage limit; the project owner explicitly authorized the replacement reviewer.

### T004A — trusted Codex Cloud command runner

- Added typed administrator configuration and caller-created non-nil submit operation/request
  values.
- Executes only the pinned version, login-status, Cloud exec, status, and bounded list invocations
  under the accepted credential lease and private process policy.
- Added the mode-`0700` `error.log/` directory sentinel, bounded dual-pipe supervision, deadlines,
  process-group termination, parent-death binding, and direct-child reap.
- Added the synced versioned submit ledger with intent/authorization/start/task commits,
  observation-only same-ID replay, fail-closed crash recovery, and no automatic retry.
- Added bounded reconciliation that records zero/one/many candidates without inferring task identity.
- All 19 named T004A tests pass; the crate reports 59 unit/property tests plus 12 integration tests.
- The package suite passed ten consecutive parallel runs after fixing the fork-inherited lease
  release race.
- The final fresh Cursor Agent acceptance review returned `IMPLEMENTATION ACCEPTED` with no blocker.

## Current T004 Decomposition

T004A is Accepted and T004B is the sole Ready production task. The parent is decomposed into:

- T004A — CU-CLOUD-P0-01 E2 trusted submit/status/list runner.
- T004B — CU-AGT-P0-02 E2 provider task lifecycle.
- T004C — CU-CLOUD-P0-02 E0 provider-managed diff retrieval.

ADR-0003 keeps generic CU-BKD-01 conformance in its existing T180 task after T020 rather than
freezing incomplete backend/event types in the provider-specific P0. It also defines T004C E0 over
provider-task and Codebox-managed state while excluding byte comparisons of provider-owned
credential storage.

T004C remains dependency-blocked on T004B; the T004 parent remains Blocked until T004B and T004C
are separately Accepted.

## Verified Pinned Cloud Surface

The official `rust-v0.145.0` source and local pinned CLI help establish:

- `cloud exec` succeeds by printing one task URL.
- `cloud status` prints three human-readable lines; only `READY` exits zero.
- `cloud list --json` emits the exact structured page recorded by the synthetic fixture.
- `cloud diff` prints an untrusted raw unified diff.
- `cloud apply` exists and is forbidden because it mutates a local working tree.
- The upstream cloud implementation attempts to append account/diagnostic metadata to cwd-relative
  `error.log`. T004A installs and revalidates a private `error.log/` directory sentinel, making the
  exact pinned append a no-op; the private working directory is never published.

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

The fourth fresh design review returned `DESIGN ACCEPTED` with no blocker. The implemented contract
subsequently passed a fresh Cursor Agent acceptance review with no blocker.

## Next Work

1. Generate every named SPEC-T004B lifecycle test skeleton before production edits.
2. Implement the serialized Cloud task lifecycle, explicit unknown-submit recovery decisions,
   browser-disconnect independence, and local-only cancellation semantics.
3. Run focused/workspace gates and request a fresh Cursor Agent acceptance review before making
   T004C Ready.

Do not re-run T001/T010/T002 acceptance work unless their relevant files or behavior change.

## Validation Evidence

The accepted T004A tree passed:

```text
cargo fmt --all -- --check
cargo test -p codebox-agent-codex --all-features
  59 unit/property tests + 12 integration tests passed
  repeated 10 consecutive parallel package runs passed
cargo clippy -p codebox-agent-codex --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo build --workspace --bins --all-features
cargo deny check
  advisories ok, bans ok, licenses ok, sources ok
git diff --check
```

`cargo deny check` requires access to the user advisory-cache lock in this environment and was run
with the approved permission. All listed commands passed locally. Hosted evidence must be checked
against the pushed T004A commit; this handoff does not claim a run that had not completed when the
acceptance record was written.
