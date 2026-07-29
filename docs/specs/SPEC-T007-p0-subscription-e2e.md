---
id: SPEC-T007
title: P0 subscription end-to-end acceptance
status: accepted
task: T007
contract_units:
  [CU-AUTH-P0-01, CU-AUTH-P0-02, CU-AGT-P0-01, CU-CLOUD-P0-01, CU-AGT-P0-02,
   CU-CLOUD-P0-02, CU-SES-P0-01, CU-API-P0-01, CU-API-P0-02, CU-WEB-P0-01]
failure_atomicity: inherited per accepted operation; deterministic fixture cleanup E1
---

# Outcome

Prove the TD §1.7 private personal-BYOS scenario across the exact accepted P0 composition with a
deterministic fake Codex executable in CI. Separately define the credential/environment-gated
operator live smoke without weakening the deterministic acceptance gate or manufacturing a live
success claim.

# Normative Dependencies

- TD §§1.6–1.7, 14, and 15.0
- ADR-0002 personal-BYOS and credential/repository boundary
- ADR-0003 provider-specific Cloud orchestration
- ADR-0004 process-lifetime session, HTTP, and WebSocket boundary
- Accepted T001–T006 tasks, specifications, code, tests, and acceptance reports
- Pinned Codex CLI contract version `0.145.0`

T007 is acceptance composition only. It owns no production API, event, lifecycle, persistence,
credential, process, filesystem, provider, repository, or browser authority.

# Acceptance Composition

The deterministic proof is the conjunction of:

1. the accepted T006 controller/DOM suite, which proves the dependency-free browser emits and
   validates the exact fixed HTTP/WebSocket contract without implicit mutation or selectable
   execution authority; and
2. one wire-equivalent browser flow against the concrete accepted T002–T005 stack, static T006
   assets, a real loopback WebSocket, and a real supervised fake CLI process.

No browser binary or second production client is added. The T007 Rust harness performs only the
same allowlisted requests and subscribe frame already proven for the actual T006 controller. Any
drift in either side remains caught by the exact T005/T006 contract suites and this composition
test.

# Deterministic Fixture

The test lives at `apps/control-plane/tests/p0_subscription_e2e.rs`, is Linux-only, and creates one
unique mode-`0700` root outside the repository with:

- a mode-`0700` fake `codex` executable;
- separate private `CODEX_HOME`, state, and working directories;
- a mode-`0600` bounded invocation ledger; and
- a credential canary in the private credential directory.

Every path is validated by the production `CredentialScope`; the same administrator-owned paths
configure the concrete `LoginBroker` and `CloudTaskOrchestrator`. Cleanup targets only the resolved
unique fixture root after the control plane and lower runtimes have shut down.

The fixture implements only the pinned invocations:

| Invocation | Exact deterministic result |
|---|---|
| `--version` | pinned `codex-cli 0.145.0` fixture |
| `login status` | pinned logged-in ChatGPT status on stderr |
| `cloud exec` | matches the exact prompt as the final argv item and returns one fixed valid task URL |
| `cloud status` | returns the pinned `READY` status for that exact task |
| `cloud diff` | returns one fixed valid bounded unified diff for attempt 1 |

All other argv exit with a fixed failure. The exact fixture script contains only shell builtins
for argument matching, bounded classification-ledger append, and fixed output; it contains no
network or child-process command and receives no repository path or repository-controlled working
directory. It matches but never persists the final prompt argv item, and production stdin remains
null exactly as required by T004A. Its ledger records an allowlisted command classification for
each invocation, never argv values, environment, credentials, stdin, or raw output. The test fails
unless the ledger contains exactly one `cloud exec`, at least one bounded `cloud status`, exactly
one `cloud diff`, and no unknown invocation, `cloud apply`, or local/repository command.

# Exact Deterministic Flow

The test constructs the production objects directly: two validated credential-scope handles, the
concrete login broker, concrete Cloud orchestrator with fixed administrator environment/branch,
the concrete P0 session runtime, and the concrete control plane. It uses an exact fixed HTTPS
Origin string for policy while Axum listens only on an ephemeral plaintext loopback port owned by
the test.

The flow is:

1. `GET /` returns the exact accepted embedded page, HTML MIME, CSP, and no-store/nosniff headers.
2. `POST /api/p0/v1/operator/session` with the exact opaque bearer and Origin returns `201`, one
   secure `__Host-codebox_p0` cookie, current instance/session identity, and no secret.
3. Authenticated `GET /api/p0/v1/login` returns exactly `{"state":"logged_in"}` through the concrete
   login broker and fake CLI credential scope.
4. Authenticated `GET /api/p0/v1/session` returns Ready, no current turn, and high-water zero.
5. A real loopback WebSocket with the exact cookie and Origin sends the version-1 subscribe frame
   at cursor zero and receives exactly replay-begin, snapshot, replay-end before live events.
6. One explicit `POST /api/p0/v1/session/turns` supplies current instance, one fresh UUIDv4
   idempotency key, JSON media type, and one fixed prompt. It returns the accepted receipt once;
   no retry occurs.
7. The same socket receives contiguous, duplicate-free events through `turn_accepted`, pending
   `lifecycle_changed`, and Ready `lifecycle_changed`. Every envelope has the current session and
   turn identity. No prompt, credential, private path, raw provider output, or diff appears.
8. Authenticated session refresh returns Ready with the same exact operation/task and high-water as
   the last event. Reconnecting from that high-water returns only replay-begin, snapshot,
   replay-end with no duplicate event.
9. `GET /api/p0/v1/session/diff` returns `200`, exact `text/plain; charset=utf-8`, the fixed bounded
   untrusted diff, and no cache. Snapshot/event high-water before and after the read is identical.
10. WebSocket disconnect causes no cancel, submit, reconcile, resolve, diff, or login operation.
    Explicit logout returns `204`; the same cookie is then rejected.
11. Shutdown completes and joins the concrete session/login resources before exact fixture cleanup.

All HTTP and WebSocket reads have a five-second test deadline. The test records response/frame
bytes in memory only up to the accepted T005 bounds. It inspects exact JSON fields and fixed
nonsecret lifecycle values, never prints sensitive bodies on failure, and scans all non-diff
responses/frames plus the invocation ledger for the bootstrap, cookie, credential, prompt, diff,
and private-path canaries.

# Failure Atomicity

The test adds no new production operation. Bootstrap/logout/idempotency inherit T005B per-endpoint
semantics; turn intent inherits T005A E1 and lower Cloud E2; reads, replay, and diff remain E0 over
their accepted managed-state boundaries. The deterministic fake may create a process and private
fixture files; bounded shutdown plus exact-root cleanup is E1. A test failure must not be masked by
cleanup failure, and cleanup never targets a repository, credential directory outside the unique
fixture, workspace root, home directory, or unresolved variable.

# Secret and Authority Boundaries

The credential canary is placed inside the private fake `CODEX_HOME`, but neither the fixture nor
Codebox reads or projects it; response scans prove its absence. The bootstrap bearer is used once
on its fixed route, the application cookie remains an HTTP/WebSocket header, and the prompt reaches
only the accepted turn body and the single final `cloud exec` argv item; the classification ledger
does not persist it. The diff appears only in the explicit diff response/assertion.

The flow contains no request or field for provider, environment, branch, repository, path,
executable, argv, credential, reconciliation, recovery decision, apply, artifact, or push
authority. Environment and branch are fixed in administrator test construction. The test fails on
any extra mutation or fake-CLI invocation.

# Live-Smoke Gate

The live smoke is not a CI test and must never use fake credentials. It is permitted only when an
operator explicitly provides all of:

- the accepted pinned Codex executable and an already authenticated private `CODEX_HOME`;
- separate safe private state and working directories;
- an administrator-selected accessible Codex Cloud environment and branch;
- a private HTTPS Origin and TLS termination forwarding to the control-plane listener;
- a fresh bootstrap token and a low-risk operator-authored prompt; and
- explicit authorization to create one real provider-managed Codex Cloud task.

The operator starts the production control plane with the documented `CODEBOX_*` administrator
configuration, opens the exact private HTTPS page in a supported browser, exchanges the bootstrap
token, confirms logged-in status, submits exactly one prompt, observes replay/live Ready status,
loads the final diff, refreshes without a second submit, and explicitly logs out.

The evidence record may contain only date, pinned CLI version, browser family/version, fixed pass
or failure stage, and the hosted/local revision. It must not contain tokens, cookies, account data,
environment/branch values, task IDs/URLs, prompt, verification code, diff, filesystem paths,
provider output, or credentials. If any prerequisite is absent, the record must say
`not run — credential/environment gate unavailable`; T007 may rely on the deterministic fake gate
but may not claim live success.

# Test Specification

The exact machine test is:

`p0_subscription_e2e_fake_codex_reaches_final_diff`

It must be committed first as a compiling skeleton that fails with one fixed “not implemented”
assertion. That skeleton is run alone before the fixture/flow is implemented. Final acceptance
requires:

```text
cargo test -p codebox-control-plane --test p0_subscription_e2e --all-features \
  -- --exact p0_subscription_e2e_fake_codex_reaches_final_diff
```

The test must then pass ten consecutive runs and remain part of the ordinary workspace suite.
The accepted T006 Node command also runs unchanged. Final gates are:

```text
node --test --test-isolation=none apps/control-plane/web/p0-client.test.mjs
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo build --workspace --bins --all-features
cargo deny check
git diff --check
```

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Fresh design review | `DESIGN ACCEPTED` | `58d644f`; fresh-context Codex subagent |
| Failing skeleton before implementation | Passed | `adf5d19`; compiled, then fixed failure |
| Focused deterministic E2E | Passed | `2e54567`; ACCEPT-T007 |
| Ten consecutive deterministic E2E runs | Passed | `2e54567`; ACCEPT-T007 |
| Workspace gates | Passed: 179 Rust, 10 Node | `2e54567`; ACCEPT-T007 |
| Live smoke | `not run — credential/environment gate unavailable` | ACCEPT-T007 |
| Fresh final acceptance review | `T007 ACCEPTED` | `2e54567`; fresh-context Codex subagent |
| Hosted CI | Passed | [run 30425447197](https://github.com/fallrising/fanzloud/actions/runs/30425447197) |

# Traceability

All accepted P0 CUs → TD §1.7 / ADR-0002–0004 → T007 →
`p0_subscription_e2e_fake_codex_reaches_final_diff`.

# TD Gaps

None. The live-smoke gate follows the owner's explicit acceptance instruction: credentials and a
private provider environment are external prerequisites, so their absence is recorded rather than
replaced with synthetic live-success evidence.
