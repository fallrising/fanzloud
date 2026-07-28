---
id: SPEC-T006
title: Private single-page operator flow
status: ready
contract_unit: CU-WEB-P0-01
module: codebox-control-plane/web
milestone: P0
archetype: F
atomicity: E0
invariants: [INV-007, INV-010, INV-012]
depends_on: [SPEC-T005]
td_sections: [1.7, 2.3, 7, 8, 9, 10, 14, 15.0]
adr_refs: [ADR-0002, ADR-0004]
risk: high
---

# Intent

Provide the smallest private single-operator browser flow over the accepted T005 same-origin
HTTP/WebSocket surface while ensuring presentation code cannot manufacture execution authority,
persist sensitive values, reinterpret untrusted text as markup, or replay mutations on refresh.

# Responsibility

## Does

- Serve one dependency-free HTML page, one DOM adapter module, one controller module, and one CSS
  asset from compile-time bytes on the existing control-plane origin.
- Exchange an operator-entered bootstrap bearer for the accepted HttpOnly cookie.
- Observe login/session state, explicitly start/cancel device login and turns, replay/stream status,
  explicitly retrieve a final diff, and logout.
- Retain only validated process/session identity and last event sequence in `sessionStorage` so a
  same-tab refresh can reconnect to retained process-lifetime history.

## Does Not

- Add or weaken any HTTP/WebSocket API, authenticate independently, read the HttpOnly cookie, or
  expose bootstrap configuration to JavaScript.
- Accept environment ID, branch, repository URL, executable, argv, environment variable, local
  path, task-apply, artifact, approval, push, or arbitrary route authority.
- Automatically start/cancel/reconcile/resolve/login/logout, automatically retry mutations, or
  implement recovery decisions.
- Persist or log bootstrap token, prompt, verification code, diff, event body, cookie, provider
  output, or error source.
- Claim offline operation, pre-restart replay, public/multi-user safety, or P1 UI compatibility.

# Public Boundary

The existing `P0ControlPlane::router()` additionally serves exactly:

```text
GET /
GET /assets/p0-app.js
GET /assets/p0-client.js
GET /assets/p0.css
```

No wildcard file route, directory listing, path parameter, filesystem read, template input, or
public constructor is added. Bytes are embedded at compile time. Only `GET` and `HEAD` are accepted;
`HEAD` returns the same headers and an empty body. All other paths continue through T005's safe JSON
404, and unsupported methods use its safe 405 response.

HTML imports `/assets/p0-app.js` as an external module. `p0-app.js` owns only DOM bindings and
constructs the controller from browser-native `fetch`, `WebSocket`, `crypto.randomUUID`,
`sessionStorage`, timers, and `location`. `p0-client.js` exports one controller factory for the DOM
adapter and dependency-free tests; it exposes no provider or server-side API.

# Inputs and Outputs

Operator inputs:

- bootstrap bearer: 32–128 non-control UTF-8 bytes, used once from one password field;
- prompt: nonempty and at most 32 KiB after UTF-8 encoding;
- explicit buttons: authenticate, start/cancel device login, submit/cancel turn, show diff, logout,
  retry read-only refresh.

The page displays only:

- fixed local labels and stable safe error text selected by allowlisted error code;
- accepted login status and device verification URL/code;
- accepted session snapshot and version-1 public replay/live event frames;
- accepted diff bytes in a text-only `<pre>`.

The controller invokes only these accepted routes:

```text
POST   /api/p0/v1/operator/session
DELETE /api/p0/v1/operator/session
GET    /api/p0/v1/login
POST   /api/p0/v1/login/device
POST   /api/p0/v1/login/cancel
GET    /api/p0/v1/session
POST   /api/p0/v1/session/turns
POST   /api/p0/v1/session/cancel
GET    /api/p0/v1/session/diff
WS     /api/p0/v1/session/stream
```

T005 recovery routes are deliberately not browser-exposed by T006. A recovery-required snapshot is
shown with a fixed instruction to use the trusted operator recovery procedure and refresh.

# Preconditions and Disposition

| ID | Condition | Disposition |
|---|---|---|
| P-006-01 | Static path and method are exact | Safe 404/405; no filesystem lookup |
| P-006-02 | Browser primitives required by controller exist | Fixed unsupported-browser state; no request |
| P-006-03 | Bootstrap is bounded before request | Fixed validation state; no request/storage |
| P-006-04 | Authenticated observations return accepted bounded JSON | Allowlisted safe error; clear volatile display |
| P-006-05 | Mutation has current non-nil instance and direct gesture | Otherwise no mutation |
| P-006-06 | Prompt satisfies accepted UTF-8 bound | Fixed validation state; no request |
| P-006-07 | Stream frame matches exact T005C version-1 allowlist and current session | Close, clear cursor if identity changed, read-only refresh |
| P-006-08 | Stored cursor record has exact version/UUID/nonnegative-safe-integer schema | Delete it and start from zero |

The bootstrap input is copied only long enough to build its request header, and the DOM field is
cleared before awaiting network completion. It is never written to storage, page URL, DOM output,
exception text, or diagnostic output. Failed bootstrap requires re-entry.

# Success Postconditions

- After bootstrap, the page obtains identity from accepted bootstrap/snapshot responses, fetches
  login/session status, and connects the accepted stream with the current session and retained
  cursor.
- An explicit submit sends exactly `{prompt}` once with one fresh idempotency key/current instance.
- Public events are displayed in sequence; the last fully accepted sequence is stored with exact
  instance/session identity and used on reconnect/refresh.
- Explicit cancel sends exactly one accepted cancellation request. Closing/reconnecting the socket
  invokes no cancel.
- Explicit diff retrieval displays the exact response as inert text.

# Non-Guarantees

- No background synchronization while the page is closed, offline cache, cross-tab coordination, or
  retained cursor after tab/session storage ends.
- No automatic recovery from T005 history gaps, process restart, provider ambiguity, or login
  outcome unknown; the page performs a read-only snapshot refresh and requires explicit action.
- No rendering of source files, Markdown, ANSI/terminal control, HTML diffs, or provider raw output.
- No browser automation compatibility beyond current standards used by the dependency-free page.

# Exit Invariants

On load, success, validation failure, HTTP error, malformed response/frame, WebSocket close,
history gap, refresh, logout, and controller disposal:

- no implicit mutation is issued;
- no browser-selected execution/provider/repository authority exists;
- volatile bootstrap/prompt/code/diff displays are cleared at the specified transition;
- stored state contains only schema version, instance ID, session ID, and event sequence;
- untrusted text is assigned through `textContent` or form `.value`, never HTML/script sinks;
- no sensitive value or raw exception is logged or placed in a URL.

# Side Effects

Static GET/HEAD responses, accepted T005 fetches/WebSocket connection, ephemeral DOM state, and one
fixed-key `sessionStorage` cursor record. There is no server-side state beyond effects already owned
by explicitly invoked T005 endpoints.

# Idempotency

Every explicit cookie-authenticated mutation creates one UUID v4-compatible key with
`crypto.randomUUID()` immediately before its single fetch and attaches the current
`Codebox-Instance-Id`. The controller does not retry a mutation after rejection, timeout, abort,
refresh, or ambiguous network outcome. A new operator action creates a new key. Observations and
stream reconnects are E0 and may be repeated.

# Concurrency and Ordering

- At most one bootstrap, mutation, read refresh, and WebSocket generation is current; controls are
  disabled while their corresponding operation is in flight.
- A monotonically increasing controller generation invalidates late observation/stream callbacks
  after refresh, logout, or disposal.
- A later refresh cannot overwrite a newer controller generation.
- Stream events are accepted only for the current session and when `seq` is exactly the next
  sequence after the last displayed/stored sequence; replay duplicates at or below the cursor are
  ignored, while a gap closes the socket and triggers one read-only refresh.
- Reconnect uses bounded exponential delay from 250 milliseconds through 5 seconds and is canceled
  by logout/disposal. It never triggers an HTTP mutation.

# Streaming Semantics

The controller sends exactly the accepted T005C subscribe frame after socket open. It processes
`replay_begin`, `event`, `snapshot`, `replay_end`, and `error`; any other/oversized/malformed frame
closes the connection without reflecting input. Only version-1 envelope events update the cursor.
The UI may show safe normalized lifecycle/status summaries but not raw prompt/diff/code/provider
output in the event timeline.

# Cancellation and Timeout

The browser relies on T005 server bounds. Client observation/mutation fetches use a 15-second abort
timer; timeout is displayed as a fixed local error. An abort never starts a second mutation.
Controller disposal aborts fetches, cancels reconnect timers, and closes the socket without calling
turn/login cancel. Turn/login cancel buttons are separate explicit accepted mutations.

# Failure Atomicity

E0 over provider, credential, T005 session, idempotency, and durable state. The only T006-owned
mutable state is ephemeral DOM/controller state and the safe cursor record. Storage write failure
degrades to reconnect from zero without changing server state.

# Failure Modes and Error Contract

The controller recognizes only an allowlist of accepted T005 error `code` values and maps each to a
fixed local message. It never renders a server `message`, response body, header, URL, invalid frame,
exception string, stack, or close reason. Unknown/malformed/non-JSON errors become one fixed
`request_failed`; authentication errors return to the bootstrap view; `instance_changed`,
`session_changed`, `history_gap`, and `future_cursor` clear the cursor and perform at most one
read-only refresh.

HTTP bodies are read with browser-native bounded server responses. A diff response is capped again
in the controller at 2 MiB before display; excess becomes a fixed `diff_too_large` state and is not
rendered. JSON responses and WebSocket messages are capped at 64 KiB before parsing.

# Security Contract

Every static response carries:

```text
Cache-Control: no-store
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
X-Frame-Options: DENY
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Resource-Policy: same-origin
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self';
  connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none';
  frame-ancestors 'none'; object-src 'none'
```

HTML contains no inline script/style/event handler, external origin, form action, base tag, iframe,
object, embed, service worker, manifest, preload, or dynamic code sink. JavaScript contains no
`innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, `eval`, `Function`, dynamic import,
worker, service worker, `postMessage`, `console`, analytics, beacon, clipboard, URL query/fragment
write, arbitrary fetch URL, or arbitrary WebSocket URL.

All untrusted content uses `textContent`. The verification URL is displayed as text, not installed
as an attacker-controlled link. Bootstrap uses `Authorization: Bearer` only for the exact
same-origin bootstrap path and `referrerPolicy: "no-referrer"`; all requests use same-origin
credentials and never redirect across origin.

# Observability and Audit Contract

T006 adds no client telemetry or console output. Server observation remains the accepted T005 stable
route/status metrics and must not include asset bodies or sensitive browser values. Static failures
may expose only fixed path-independent status codes.

# Test Specification

These 12 exact tests must exist and compile/run as skeletons before T006 production code:

1. `p0_web_serves_exact_embedded_assets_with_security_headers`
2. `p0_web_rejects_unknown_paths_and_methods_without_filesystem_lookup`
3. `p0_web_bootstrap_token_is_ephemeral_and_never_persisted`
4. `p0_web_login_status_and_device_actions_use_exact_api_contract`
5. `p0_web_prompt_submission_requires_one_explicit_operator_action`
6. `p0_web_stream_replays_and_reconnects_from_validated_cursor`
7. `p0_web_cancel_is_explicit_and_disconnect_never_cancels`
8. `p0_web_diff_is_bounded_text_and_never_html`
9. `p0_web_refresh_rehydrates_identity_without_replaying_mutations`
10. `p0_web_errors_and_diagnostics_exclude_sensitive_canaries`
11. `p0_web_exposes_no_execution_provider_or_arbitrary_route_authority`
12. `p0_web_controller_model_preserves_generation_sequence_and_e0_boundaries`

Tests 1–2 are Rust route tests against the concrete Axum router and exact embedded bytes. Tests 3–12
use Node's built-in `node:test`, fake browser primitives, and the actual production
`p0-client.js`; they require no network, browser binary, package manager, or third-party module.
Test 6 partitions valid retained cursors, duplicates, gaps, current-session mismatch, reconnect
delays, and storage failure. Test 7 covers peer close, disposal, and explicit cancel separately.
Test 8 includes HTML/script/diff-size canaries. Test 9 proves load/refresh make only GET plus
WebSocket subscribe operations. Test 12 runs a deterministic action/response/frame model across
partitioned schedules and asserts the mutation log equals the explicit action log.

The repository provides one official `node --test` command and hosted CI runs it. All 12 test names
must be visible in test output; placeholder/skipped/todo tests do not satisfy acceptance.

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Skeleton compile/run before production | Pending | T006 skeleton commit |
| Focused Rust/Node tests | Pending | ACCEPT-T006 |
| Workspace gates | Pending | ACCEPT-T006 |
| Fresh acceptance review | Pending | ACCEPT-T006 |

# Traceability

CU-WEB-P0-01 → ADR-0002/ADR-0004 → T006 → 12 named tests →
`apps/control-plane/{src,web}/**`.

# TD Gaps

None. This specification fixes the minimal P0 web route, storage, rendering, reconnect, timeout,
and browser-test mechanics without claiming P1 UI scope.

# Self-Check

T005 is Accepted. T006 is the sole Ready production task. Its public boundary is static same-origin
presentation plus the exact accepted T005 routes, with no new mutation or execution authority. A
fresh read-only design review remains required before production edits.
