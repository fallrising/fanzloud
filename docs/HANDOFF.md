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
parent, T005A, T005B, T005C, the T005 coordination parent, and T006 are Accepted. T006 is the
latest completed production task; `ACCEPT-T006` records its design/test-first history,
implementation repair rounds, final fresh-context review, local gates, and hosted evidence.

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

### T005C — authenticated replay-then-live WebSocket

- Added the exact HTTP/1.1 version-13 upgrade route over the accepted cookie, exact Origin,
  application-session lease, and lifecycle admission, with normalized safe 426 and hardened 101
  headers.
- Added one bounded version-1 subscribe frame, prevalidated replay/snapshot/end serialization, and
  direct polling of the accepted T005A live receiver without a second production event queue or
  sequence authority.
- Logout/monotonic expiry and lifecycle shutdown are observed during idle, replay/live sends, and
  even a blocked subscription admission. Current writes remain bounded; shutdown uses 1012 and
  every receiver is dropped exactly once.
- Exact safe error/close mappings cover version/session/cursor/gap/cap/lag/unavailable/protocol
  partitions. Fragmented reassembly, ping/control timing, send timeout, close grace, redaction, and
  E0/no-mutation boundaries are tested directly.
- All 13 required tests are substantive. Tests 8–11 each repeat ten times internally, the complete
  T005C suite passed ten consecutive external runs, and a real loopback exercises the concrete
  accepted T005A adapter.
- The first fresh review rejected four material admission/protocol/evidence gaps. Commit `e965add`
  repaired them; the final fresh-context Codex fallback returned `IMPLEMENTATION ACCEPTED`.

## Accepted T005 Composition

- The parent public boundary is exactly the union of T005A, T005B, and T005C and adds no
  production source or authority.
- Two exact combined regressions cover authenticated HTTP plus real WebSocket initial/live/replay
  flow, disconnect independence, retained cursor resume, forbidden browser-selected execution
  fields/routes, and cross-channel secret containment.
- The control-plane package reports 34 tests and the workspace reports 176 tests. All format,
  Clippy, build, dependency-policy, and diff gates pass.
- Cursor and Grok exceeded the owner's 80-second review windows. The authorized fresh-context
  Codex fallback returned `COMPOSITION ACCEPTED`.

### T006 — private same-origin operator page

- Added compile-time embedded HTML, CSS, and ESM assets with exact MIME, no-store/nosniff,
  frame/referrer/permissions isolation, and an administrator-derived same-origin WSS CSP source.
- Added a dependency-free browser controller for bootstrap, device login, explicit prompt/cancel,
  snapshot refresh, replay/live reconnect, bounded text diff, and logout over only the accepted
  fixed T005 routes.
- Exact response/status/frame validation, snapshot semantics, byte bounds, ambiguous-mutation
  refresh behavior, generation/disposal fences, terminal stream latching, and pre-dispatch
  volatile clearing fail closed without retrying mutations.
- The browser exposes no provider/repository/executable/argv/path/environment/branch/recovery/apply
  authority and never treats untrusted values as HTML.
- All 12 exact tests pass: two Rust route/security tests and ten Node controller/DOM tests. The
  Node suite and three Rust concurrency/reconnect partitions each passed 20 consecutive runs; the
  workspace reports 178 Rust tests.
- Fresh final review returned `T006 ACCEPTED`; hosted CI run 30423184446 passed, including the
  pinned Node 24.18.0 step.

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

1. Materialize T007 spec-first from TD §§1.7 and 15.0.
2. Obtain fresh design acceptance, then complete the exact deterministic fake-Codex P0
   subscription E2E test across the accepted T002–T006 surfaces.
3. Record the operator-authenticated live smoke only when the required credentials/environment are
   genuinely available; otherwise record it explicitly as gated and unavailable, never as passed.

ADR-0004 and the complete T005 decomposition received fresh Cursor Agent design acceptance after
three rejected drafts repaired durability/authentication, state-transition, cancel/shutdown,
startup-observation, and subscription-handoff gaps. T005A is Accepted. A later T005B-specific
review found public-origin, expiry, shutdown, fake-port, route-schema, and error-map blockers.
Three design-review passes repaired those plus two residual contradictions. Implementation reviews
then repaired fixed-work/disconnect/error coverage, admission and shutdown races, UUID
classification, and concurrent logout joining. T005C then repaired exact upgrade, revocation,
subscription-admission, close, and fragmented/reconnect evidence gaps. The final fresh-context
verdict was `IMPLEMENTATION ACCEPTED`. T005B and T005C are Accepted. The two parent composition
regressions and all combined gates then passed; a fresh review returned `COMPOSITION ACCEPTED`.

Do not re-run T001/T010/T002 acceptance work unless their relevant files or behavior change.

## Validation Evidence

The accepted T006 tree passed:

```text
node --test --test-isolation=none apps/control-plane/web/p0-client.test.mjs
  10 dependency-free browser-controller/DOM tests passed
cargo fmt --all -- --check
cargo test -p codebox-control-plane --all-features
  36 HTTP/WebSocket/web/concurrency/security tests passed
  focused T006 Rust route/security tests: 2 passed
cargo clippy -p codebox-control-plane --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
  178 tests passed
cargo build --workspace --bins --all-features
cargo deny check
  advisories ok, bans ok, licenses ok, sources ok
git diff --check
```

The Node suite and the HTTP idempotency, WebSocket reconnect, and WebSocket chunk/reconnect
partitions each passed 20 consecutive runs. All listed commands passed locally. Hosted CI run
30423184446 passed the exact T006 code tree.
