# Development Handoff

Date: 2026-07-28

## Current Goal

Deliver the private, single-operator P0 that lets the project owner operate their own ChatGPT/Codex
subscription through a Codebox web control layer. Repository-controlled work remains in
provider-managed Codex Cloud; the local trusted runner must never check out or execute repository
code beside `CODEX_HOME`.

## Repository State

Branch: `main`

T001, T010, T002A, T002B, T002, and T003 are Accepted. T003 is the latest completed production task.
Inspect `git status`, the latest commit, and hosted CI before continuing; this handoff does not claim
a hosted result for T003.

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

## Current Ready Task

No production implementation task is Ready. The next documentation task is to decompose the T004
integration parent into separate atomicity-specific children before writing T004 code.

The original T003 seed mixed E0, E2, and E3 CUs. TD §9.3 corrected the boundary:

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

The fourth fresh design review returned `DESIGN ACCEPTED` with no blocker. The implemented contract
subsequently passed a fresh Cursor Agent acceptance review with no blocker.

## Next Work

Decompose T004 before production code:

1. Re-read TD §15.0, ADR-0002, accepted T002/T003 contracts, and the P0 CU inventory.
2. Create the T004 parent task and split CU-AGT-P0-02 E2, CU-CLOUD-P0-01 E2,
   CU-CLOUD-P0-02 E0, and CU-BKD-01 E3 into independently testable children.
3. Write specifications and machine acceptance for each child, including full P14 launcher coverage,
   durable submit intent/outcome recording, bounded list reconciliation, and the TD-exact P15
   regression.
4. Request the required fresh design review before marking any T004 child Ready.
5. Select exactly one Ready child and generate its test skeletons before production edits.

Do not re-run T001/T010/T002 acceptance work unless their relevant files or behavior change.

## Validation Evidence

The current working tree passed:

```text
cargo fmt --all -- --check
cargo test -p codebox-agent-codex --all-features
  40 unit/property tests + 12 integration tests passed
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
