# Development Handoff

Date: 2026-07-28

## Current Goal

Deliver the private, single-operator P0 that lets the project owner operate their own ChatGPT/Codex
subscription through a Codebox web control layer. Repository-controlled work remains in
provider-managed Codex Cloud; the local trusted runner must never check out or execute repository
code beside `CODEX_HOME`.

## Repository State

Branch: `main`

T001, T010, T002A, T002B, T002, T003, T004A, T004A1, T004B, T004C, the T004 coordination
parent, T005A, and T005B are Accepted. T005B is the latest completed production task;
`ACCEPT-T005B` records its implementation and review evidence. T005C is the sole Ready production
task; the T005 coordination parent remains Proposed until T005C is independently Accepted.

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

### T004A1 — submit recovery bridge

- Added command-free observation for caller-created operation IDs and durable adopted/abandoned
  terminal phases.
- Resolution requires exact durable reconciliation evidence, is replay-idempotent, and never
  executes another Cloud command.
- All nine named tests, workspace gates, and final fresh Cursor Agent acceptance passed.

### T004B — provider task lifecycle

- Added the public provider-specific lifecycle, operation-bound unknown decisions, and named
  duplicate-risk acknowledgement.
- Added the bounded synced `cloud-lifecycle.json` projection, full restart repair, lower-readiness
  gate, lower-before-upper resolution, and conflict rollback/removal.
- Explicit cancellation targets the exact in-memory submit signal, waits without holding the state
  mutex, and never claims provider termination; a real CLI test proves process-group reap.
- All 18 named tests pass; the crate reports 86 unit/property tests plus 12 integration tests, and
  ten consecutive package runs passed.
- Two fresh Cursor Agent implementation reviews returned accepted with no blocker.

### T004C — provider-managed diff retrieval

- Added an opaque task authority minted only from the current durable Ready/Applied lifecycle and a
  reader constructible only from the same orchestrator.
- Retrieval revalidates exact lifecycle and submit-ledger operation/task/configuration provenance
  under one held scope lease before executing the exact pinned first-attempt diff argv.
- Added validate-only diagnostic sentinel enforcement, 2-MiB stdout / 64-KiB stderr retained
  capture with continued draining, typed redacted failures, cancellation/timeout/reap handling, and
  no local application or artifact surface.
- All 11 named tests pass; the crate reports 97 unit/property tests plus 12 integration tests.
- The real cancellation/reap and stdout/stderr drain tests each passed ten consecutive runs.
- The final fresh Cursor Agent implementation review returned `IMPLEMENTATION ACCEPTED` with no
  blocker.

## Accepted T004 Composition

T004A, its T004A1 recovery amendment, T004B, and T004C are independently Accepted. The parent
composition is also Accepted over:

- T004A — CU-CLOUD-P0-01 E2 trusted submit/status/list runner.
- T004A1 — CU-CLOUD-P0-01 E2 prompt-free observation and explicit unknown terminalization.
- T004B — CU-AGT-P0-02 E2 provider task lifecycle.
- T004C — CU-CLOUD-P0-02 E0 provider-managed diff retrieval.

ADR-0003 keeps generic CU-BKD-01 conformance in its existing T180 task after T020 rather than
freezing incomplete backend/event types in the provider-specific P0. It also defines T004C E0 over
provider-task and Codebox-managed state while excluding byte comparisons of provider-owned
credential storage.

The combined local workspace, P14, and P15 gates pass. A separate fresh Cursor Agent composition
review returned `COMPOSITION ACCEPTED`, and the parent is Accepted.

### T005A — process-lifetime P0 session lifecycle

- Added one single-writer session runtime with one worker, one active turn, bounded ordered event
  history, atomic replay/snapshot/live subscription handoff, and nonblocking subscriber isolation.
- Turn intent commits before the accepted T004 start call; pending lifecycle monitoring is
  rate-limited, checked provider-read failures retain the exact pending projection with bounded
  backoff, and unknown outcomes require explicit operation-bound recovery.
- Explicit cancel and shutdown suppress queued or claimed-but-not-admitted starts. A stale monitor
  result cannot overwrite an already-terminal cancellation, and shutdown joins without claiming
  provider cancellation.
- Diff reads use only the accepted T004C authority and preserve session/event state. Public
  projections and typed errors are serializable and redacted without exposing prompts, diffs,
  provider output, configuration values, or paths.
- All 16 required contract names are substantive; the crate reports 22 unit/property tests plus one
  concrete accepted-orchestrator integration, and the package suite passed ten consecutive runs.
- The final fresh Cursor Agent review returned `IMPLEMENTATION ACCEPTED` after independently
  rechecking all prior concurrency, composition, and configuration-field blockers.

### T005B — authenticated private P0 HTTP API

- Added the exact private bootstrap/login/session route set with one canonical HTTPS Origin,
  fixed-work bootstrap/cookie comparison, secure bounded `__Host-` application sessions, exact
  no-store/nosniff responses, and no browser-selected provider/process/repository authority.
- Added process-instance-global idempotency with exact raw-body identity, in-flight joins, bounded
  completed storage, request-disconnect independence, and explicit-only cancel/recovery.
- Every authenticated request owns a lifecycle and application-session RAII admission before body
  reading. Concurrent preauthenticated same-key logout requests join one owner; the session and
  tombstone are removed only after all captured-auth requests drain.
- Added exact exhaustive login/session/lifecycle/diff error mappings, structural-versus-value JSON
  classification, bounded bodies/headers, secret canaries, and plain untrusted diff responses.
- The runnable binary constructs all listener, credential, provider, origin, and bootstrap
  configuration only from administrator process state and coordinates lower shutdown/cleanup.
- All 19 required tests are substantive; the package suite passed ten consecutive final runs and
  the workspace reports 161 passing tests.
- Cursor and Grok exceeded the owner's bounded review windows. The explicitly authorized
  fresh-context Codex fallback returned `IMPLEMENTATION ACCEPTED` after all review findings were
  repaired.

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

1. Compile all 13 sole-Ready SPEC-T005C test skeletons before production, then implement and
   independently accept the replay/live WebSocket stream under its repaired accepted design.
2. Run the T005 composition acceptance after all three children are Accepted.
3. Continue with T006 minimal private operator web flow and T007 deterministic/live subscription
   end-to-end acceptance after their dependencies are Accepted.

ADR-0004 and the complete T005 decomposition received fresh Cursor Agent design acceptance after
three rejected drafts repaired durability/authentication, state-transition, cancel/shutdown,
startup-observation, and subscription-handoff gaps. T005A is Accepted. A later T005B-specific
review found public-origin, expiry, shutdown, fake-port, route-schema, and error-map blockers.
Three design-review passes repaired those plus two residual contradictions. Implementation reviews
then repaired fixed-work/disconnect/error coverage, admission and shutdown races, UUID
classification, and concurrent logout joining. The final fresh-context verdict was
`IMPLEMENTATION ACCEPTED`. T005B is Accepted, T005C is the sole Ready production task, and T005
remains Proposed.

Do not re-run T001/T010/T002 acceptance work unless their relevant files or behavior change.

## Validation Evidence

The accepted T005B tree passed:

```text
cargo fmt --all -- --check
cargo test -p codebox-control-plane --all-features
  19 HTTP/concurrency/security tests passed
  complete package suite repeated 10 consecutive runs
cargo clippy -p codebox-control-plane --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
  161 tests passed
cargo build --workspace --bins --all-features
cargo deny check
  advisories ok, bans ok, licenses ok, sources ok
git diff --check
```

`cargo deny check` requires access to the user advisory-cache lock in this environment and was run
with the approved permission. All listed commands passed locally. Hosted evidence must be checked
against the pushed T005B acceptance commit.
