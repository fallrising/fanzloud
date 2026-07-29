---
id: SPEC-T006
title: Private single-page operator flow
status: ready
contract_unit: CU-WEB-P0-01
module: codebox-control-plane/web
milestone: P0
archetype: F
atomicity: per_action
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

The exact successful response media types are:

| Route | `Content-Type` |
|---|---|
| `/` | `text/html; charset=utf-8` |
| `/assets/p0-app.js` | `text/javascript; charset=utf-8` |
| `/assets/p0-client.js` | `text/javascript; charset=utf-8` |
| `/assets/p0.css` | `text/css; charset=utf-8` |

HTML imports `/assets/p0-app.js` as an external module. `p0-app.js` owns only DOM bindings and
constructs the controller from browser-native `fetch`, `WebSocket`, `crypto.randomUUID`,
`sessionStorage`, `AbortController`, `TextEncoder`, `TextDecoder`, timers, and `location`.
`p0-client.js` exports one controller factory for the DOM adapter and dependency-free tests; it
exposes no provider or server-side API. `p0-app.js` exports one DOM-mount function, auto-mounts only
when a browser document exists, and is imported and mounted against a fake DOM by the Node suite.
`apps/control-plane/web/package.json` declares only `"private": true` and `"type": "module"` so the
same `.js` modules are loaded by browsers and Node.

# Inputs and Outputs

Operator inputs:

- bootstrap bearer: 32–128 non-control UTF-8 bytes, used once from one password field; the
  controller UTF-8 encodes the entered scalar-value string and converts each resulting byte to the
  same-valued Web IDL `ByteString` code unit before constructing `Authorization`, so the browser
  sends the exact bytes accepted by T005B rather than UTF-16 code units;
- prompt: nonempty, not all whitespace or the reserved single `-`, free of ASCII controls other
  than LF/TAB, and at most 32 KiB after UTF-8 encoding; its exact serialized `{"prompt":...}` body
  must also be at most the accepted 40-KiB HTTP-body limit;
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
| P-006-07 | Stream frame matches exact T005C version-1 allowlist and current session | Close; on gap/future/wrong identity, clear the record and adopt the refreshed snapshot high-water |
| P-006-08 | Stored cursor record has exact version/UUID/nonnegative-safe-integer schema | Delete it and adopt the authenticated snapshot high-water before subscribe |
| P-006-09 | Fetch remains on one fixed route with redirect mode `error` | Redirect/network failure; no follow or retry |
| P-006-10 | Response status, media type, decoded byte count, UTF-8, JSON, and exact schema validate | Cancel body stream; fixed safe error; no untrusted display |

The UTF-8-to-`ByteString` conversion rejects encoded ASCII controls and any length outside the
accepted byte bound before constructing `Headers`. The bootstrap input is copied only long enough
to build its request header, and the DOM field is cleared before awaiting network completion. It is
never written to storage, page URL, DOM output, exception text, or diagnostic output. A definitive
bootstrap rejection requires re-entry. A timeout, abort, redirect, or network loss may have set a
cookie; the controller performs at most one read-only session observation to discover that state
and never repeats the bootstrap request automatically.

# Success Postconditions

- After bootstrap, the page obtains identity from accepted bootstrap/snapshot responses, fetches
  login/session status, and connects the accepted stream with the current session and retained
  cursor.
- An explicit submit sends exactly `{prompt}` once with one fresh idempotency key/current instance.
- Public events are displayed in sequence; the last fully accepted sequence is stored with exact
  instance/session identity and used on reconnect/refresh.
- A missing, malformed, stale-identity, future, or history-gap cursor adopts the exact current
  snapshot high-water mark before reconnecting, so resynchronization cannot loop from zero.
- Explicit cancel sends exactly one accepted cancellation request. Closing/reconnecting the socket
  invokes no cancel.
- Explicit diff retrieval displays the exact response as inert text.

# Non-Guarantees

- No background synchronization while the page is closed, offline cache, cross-tab coordination, or
  retained cursor after tab/session storage ends.
- No recovery of evicted event detail and no automatic mutation/recovery decision after history
  gaps, process restart, provider ambiguity, or login outcome unknown. The page automatically
  resynchronizes only the accepted current snapshot/high-water and requires explicit operator
  action for every subsequent mutation.
- No client abort, timeout, redirect rejection, or lost response proves that an admitted bootstrap,
  logout, login, turn, or cancellation mutation rolled back. The browser never labels such an
  outcome successful and never retries it automatically.
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

The bootstrap field is cleared before dispatch. The prompt field and controller copy are cleared
before dispatch. Verification instructions are cleared on login cancel, definitive login-state
change, refresh, logout, authentication loss, and disposal. Diff text is cleared before retrieval,
on any refresh or mutation, logout, authentication loss, and disposal. Event summaries are fixed
allowlisted labels, retain at most 256 entries, and are cleared on identity change, logout,
authentication loss, and disposal.

# Side Effects

Static GET/HEAD responses, accepted T005 fetches/WebSocket connection, ephemeral DOM state, and one
fixed-key `sessionStorage` cursor record. There is no server-side state beyond effects already owned
by explicitly invoked T005 endpoints.

# Idempotency

Every explicit cookie-authenticated mutation creates one UUID v4-compatible key with
`crypto.randomUUID()` immediately before its single fetch and attaches the current
`Codebox-Instance-Id`. The controller does not retry a mutation after rejection, timeout, abort,
redirect, refresh, or ambiguous network outcome. After an ambiguous response it may perform one
read-only login/session refresh, and another mutation remains disabled until that refresh finishes
and the operator acts again. A new operator action creates a new key. Session/diff observations
and stream reconnects are E0 and may be repeated. Login status retains the accepted T002B/T005B
E0/E2 reconciliation behavior and is never treated as mutation retry authority.

# Concurrency and Ordering

- At most one HTTP operation is current. Bootstrap, mutation, diff read, and snapshot/login refresh
  do not overlap; controls are disabled while it is in flight. At most one WebSocket generation is
  current.
- A monotonically increasing controller generation invalidates late observation/stream callbacks
  after refresh, logout, or disposal. The controller rechecks both disposal and the captured
  generation after each awaited session/login observation before applying state or connecting a
  socket.
- A later refresh cannot overwrite a newer controller generation.
- Stream events are accepted only for the current session and when `seq` is exactly the next
  sequence after the last displayed/stored sequence; replay duplicates at or below the cursor are
  ignored, while a gap closes the socket and triggers one read-only refresh.
- Reconnect uses bounded exponential delay of 250, 500, 1,000, 2,000, 4,000, then 5,000
  milliseconds and is canceled by logout/disposal. Before reconnect it performs one read-only
  session refresh; it never triggers an HTTP mutation.

Snapshot/cursor resynchronization is exact:

| Stored/browser state | Accepted snapshot | Cursor used for next subscribe |
|---|---|---:|
| Exact instance and session, stored seq ≤ snapshot high-water | Same identity | stored seq |
| No record, malformed record, storage failure, identity mismatch, stored future seq, or stream `history_gap`/`future_cursor`/`wrong_session` | Any authenticated current identity | snapshot high-water |
| Same identity after a normal manual refresh | Same identity and stored seq remains valid | stored seq |
| Application cookie lost/expired, including restart | HTTP 401 | none; clear identity and require bootstrap |
| Recovery-required snapshot | Same identity | validated cursor; show fixed trusted-recovery instruction and expose no recovery mutation |

Adopting snapshot high-water means prior event detail is intentionally unavailable; it does not
synthesize or display events. A new authenticated tab without a valid record therefore begins live
delivery at the snapshot high-water instead of requesting an already-evicted zero cursor.

# Streaming Semantics

The controller sends exactly the accepted T005C subscribe frame after socket open. One connection
accepts only `replay_begin`, zero or more `event`, exactly one `snapshot`, exactly one `replay_end`,
then zero or more live `event` frames. It validates exact object keys, protocol/session identity,
safe-integer sequence/high-water values, replay bounds, the exact T005A snapshot shape, envelope
schema version 1, and the allowlisted T005A event/lifecycle tags. Any out-of-order,
unknown-field, binary, oversized, malformed, wrong-session, or impossible frame closes the
connection without reflecting input. Replay duplicates at or below the requested cursor are
ignored; only a validated next version-1 envelope updates the cursor. The UI shows only fixed
allowlisted lifecycle/event summaries, never raw prompt/diff/code/provider output.

# Cancellation and Timeout

Client HTTP operations use a 15-second abort timer. A timeout is displayed as a fixed local error;
for a dispatched mutation it is also treated as outcome-ambiguous and never retried. Controller
disposal aborts fetches, cancels reconnect timers, and closes the socket without calling turn/login
cancel. Turn/login cancel buttons are separate explicit accepted mutations.

# Failure Atomicity

| T006 action | Effective atomicity and browser disposition |
|---|---|
| Static GET/HEAD, session snapshot, diff read, stream/reconnect | E0 over accepted server/provider state |
| Login-status observation | Accepted T002B/T005B E0/E2 reconciliation; it may repair only the lower login ledger and never starts login |
| Local validation or storage failure before dispatch | E0; no request |
| Bootstrap/logout | Accepted T005B E1 server effect; lost response is browser-ambiguous and is never retried automatically |
| Device-login start/cancel | Accepted T002B/T005B E2; timeout/lost response requires observation before another explicit action |
| Turn start | Accepted T005A E1 intent plus lower T004 E2; timeout/lost response requires snapshot observation and never resubmission |
| Turn cancel | Accepted T005A E1 local transition plus lower T004B E2 cancellation semantics; provider may continue and the browser never claims provider cancellation |

T006 itself adds no server-side mutation, ledger, or retry authority. Its owned state is ephemeral
DOM/controller state and the safe cursor record. Storage write failure does not change server state
and makes the next reconnect adopt the current snapshot high-water mark.

# Failure Modes and Error Contract

The controller ignores every server `message` and recognizes these exact accepted HTTP code groups:

| Disposition | Exact codes | Fixed local result |
|---|---|---|
| Reauthenticate | `authentication_required` | Clear identity/volatile state and show `Authentication is required.` |
| Resynchronize once, never replay a mutation | `instance_changed`, `session_changed`, `history_gap`, `future_cursor`, `operation_changed`, `diff_authority_changed` | Clear/adopt cursor per the table and show `Current state changed; status was refreshed.` |
| Trusted recovery required | `login_outcome_unknown`, `provider_outcome_unknown`, `provider_recovery_required` | Show `Outcome requires the trusted operator recovery procedure.` |
| Correct explicit input/action | `origin_forbidden`, `session_limit`, `idempotency_key_invalid`, `idempotency_conflict`, `unsupported_media_type`, `request_too_large`, `body_not_empty`, `malformed_json`, `invalid_request`, `invalid_value`, `acknowledgement_required`, `task_not_listed`, `login_already_running`, `already_logged_in`, `login_failed`, `turn_already_running`, `no_current_turn`, `session_wrong_state`, `recovery_decision_stale`, `provider_turn_running`, `no_current_operation`, `provider_wrong_state`, `diff_not_ready`, `diff_canceled` | Show `The explicit action was rejected; refresh before deciding whether to act again.` |
| Service/operator repair, no automatic mutation retry | `idempotency_unavailable`, `service_unavailable`, `login_unavailable`, `login_version_mismatch`, `login_provider_drift`, `login_output_limit`, `login_status_unavailable`, `login_process_unavailable`, `login_state_unavailable`, `login_state_invalid`, `session_config_invalid`, `session_stopped`, `subscriber_limit`, `session_sequence_exhausted`, `provider_state_conflict`, `provider_scope_unavailable`, `provider_busy`, `provider_runner_unavailable`, `provider_read_unavailable`, `provider_operation_conflict`, `provider_state_invalid`, `provider_state_unavailable`, `diff_scope_unavailable`, `diff_busy`, `diff_version_mismatch`, `diff_boundary_unavailable`, `diff_process_unavailable`, `diff_timeout`, `diff_output_limit`, `diff_provider_drift`, `diff_invalid` | Show `The service could not complete the request; no command was retried.` |

Each accepted HTTP code is valid only with its exact inherited T005B status:

| Status | Exact accepted codes |
|---:|---|
| 400 | `idempotency_key_invalid`, `malformed_json` |
| 401 | `authentication_required` |
| 403 | `origin_forbidden` |
| 409 | `instance_changed`, `idempotency_conflict`, `login_already_running`, `already_logged_in`, `login_failed`, `login_outcome_unknown`, `turn_already_running`, `no_current_turn`, `session_wrong_state`, `session_changed`, `operation_changed`, `history_gap`, `future_cursor`, `provider_state_conflict`, `provider_turn_running`, `no_current_operation`, `provider_wrong_state`, `recovery_decision_stale`, `provider_operation_conflict`, `provider_outcome_unknown`, `provider_recovery_required`, `diff_not_ready`, `diff_authority_changed`, `diff_canceled` |
| 413 | `request_too_large` |
| 415 | `unsupported_media_type` |
| 422 | `body_not_empty`, `invalid_request`, `invalid_value`, `acknowledgement_required`, `task_not_listed` |
| 429 | `session_limit` |
| 503 | `idempotency_unavailable`, `service_unavailable`, `login_unavailable`, `login_version_mismatch`, `login_provider_drift`, `login_output_limit`, `login_status_unavailable`, `login_process_unavailable`, `login_state_unavailable`, `login_state_invalid`, `session_config_invalid`, `session_stopped`, `subscriber_limit`, `session_sequence_exhausted`, `provider_scope_unavailable`, `provider_busy`, `provider_runner_unavailable`, `provider_read_unavailable`, `provider_state_invalid`, `provider_state_unavailable`, `diff_scope_unavailable`, `diff_busy`, `diff_version_mismatch`, `diff_boundary_unavailable`, `diff_process_unavailable`, `diff_output_limit`, `diff_provider_drift`, `diff_invalid` |
| 504 | `diff_timeout` |

No other status/code pairing is accepted. The optional `operation_id` field is accepted only on a
code inherited from `CloudLifecycleErrorCategory`, and must be a non-nil UUID; all other codes
require exactly `code` and `message`.

The exact accepted WebSocket codes are `authentication_expired`, `subscribe_timeout`,
`protocol_error`, `unsupported_version`, `wrong_session`, `history_gap`, `future_cursor`,
`subscriber_limit`, `subscriber_lagged`, and `stream_unavailable`. Authentication expiry clears
identity. Wrong session/gap/future cursor perform the snapshot-high-water resynchronization once.
Timeout/subscriber-limit/lag/unavailable and close 1012 use bounded read-refresh/reconnect.
Protocol error and unsupported version stop automatic reconnect until manual refresh. Peer/network
close uses bounded read-refresh/reconnect. `subscriber_lagged` accepts exactly either the base
`type`/`code`/`message` shape or that shape plus a safe-integer `latest_available`; no other error
shape is accepted. No close reason is inspected or displayed.

Unknown codes, malformed/non-JSON errors, redirect responses, invalid media types, invalid UTF-8,
schema drift, body-read failure, and raw exceptions become the one fixed `request_failed` result.
The controller never renders a response body, header, URL, invalid frame, exception string, stack,
or close reason.

For JSON, the controller requires exact `application/json`, reads `response.body` incrementally,
counts decoded-stream bytes through 64 KiB inclusive, cancels the reader on byte 65,537, decodes
UTF-8 with a fatal decoder only after EOF, then parses and validates the exact endpoint schema.
For diff it requires exact `text/plain; charset=utf-8`, applies the same algorithm through 2 MiB,
and assigns the decoded result only after EOF. A canonical decimal `Content-Length` over the
applicable limit rejects before reading, but absence or compression never bypasses streaming
counting. WebSocket text is UTF-8-counted through 64 KiB before `JSON.parse`; non-string data is
rejected. Excess becomes fixed `response_too_large` or `diff_too_large` and no partial text is
rendered.

Successful HTTP responses are accepted only with this exact status/body contract:

| Action | Status | Exact body validator |
|---|---:|---|
| Bootstrap | 201 | `{actor:"operator", expires_in_seconds, p0_session_id, instance_id}` with exact keys, accepted lifetime integer, and non-nil UUID identities |
| Logout | 204 | zero body bytes |
| Login status / cancel login | 200 | one exact T005B tagged login-status shape |
| Start device login | 202 | exact operation UUID, pinned verification URL, bounded code, and `expires_in_seconds:900` |
| Session snapshot / cancel turn | 200 | exact accepted T005A snapshot/turn/lifecycle shape |
| Start turn | 202 | exact non-nil turn UUID and safe-integer `high_water_seq` |
| Diff | 200 | bounded UTF-8 plain text under the media-type rule above |

Any other 2xx/3xx/4xx/5xx status is handled only through its matching exact success validator or
the bounded accepted error object. A success body under an error status, an error body under a
success status, a 1xx/3xx response, or status/schema drift becomes fixed `request_failed`.

# Security Contract

Every successful static GET/HEAD response carries its exact media type plus:

```text
Cache-Control: no-store
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
X-Frame-Options: DENY
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Resource-Policy: same-origin
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self';
  connect-src 'self' wss://<exact-canonical-public-host[:non-default-port]>;
  img-src 'self'; base-uri 'none'; form-action 'none';
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
credentials, `mode: "same-origin"`, `cache: "no-store"`, and `redirect: "error"`. The WSS source is
derived server-side only from the already-canonical administrator `https` origin by replacing that
scheme with `wss`; it is not taken from a request header. This explicit source preserves
same-origin scope while covering browsers in which CSP `'self'` does not match WebSocket schemes.

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
`p0-client.js`; test 11 additionally imports and mounts the actual `p0-app.js`. They require no
network, browser binary, package manager, or third-party module.
Test 6 partitions valid retained cursors, duplicates, gaps, current-session mismatch, reconnect
delays, snapshot-high-water adoption, restart authentication loss, and storage failure. Test 7
covers peer close, disposal, ambiguous cancellation response, and explicit cancel separately.
Test 8 includes HTML/script/diff-size, invalid UTF-8, chunk, content-length, EOF, and media-type
partitions. Across tests 3–10, every HTTP success status/schema is accepted exactly and rejected at
the wrong status. JSON, diff, and WebSocket bounds cover limit/limit-plus-one, one/many chunks,
UTF-8 byte length versus JavaScript length, and reader cancellation on overflow. Stream tests cover
every legal phase plus out-of-order, binary, oversized, malformed, unknown-field, and impossible
frames. Test 9 proves load/refresh make only GET plus WebSocket subscribe operations and covers
same/mismatched identity and future stored cursors. Test 10 covers every allowlisted HTTP/WS
disposition at its exact status, every wrong HTTP status/code pairing, both legal
`subscriber_lagged` shapes, plus 301/302/303/307/308 rejection and unknown/error canaries. Test 11 inspects the
actual DOM adapter/controller sources for forbidden authority and sinks, imports both production
modules, and mounts the adapter against a fake DOM. Test 12 runs a
deterministic action/response/frame model across partitioned schedules and asserts the mutation log
equals the explicit action log, including timeout/abort/late-response schedules and inherited
per-action atomicity/ambiguity dispositions. Its schedule matrix covers every explicit mutation
kind; accepted success, fixed error, redirect/network/timeout, concurrent-gesture, refresh,
disposal, and late-success outcomes; legal and invalid frame ordering; and stale generation
callbacks. Each schedule records the admitted explicit action and requires exactly zero or one
matching mutation dispatch with no automatic duplicate or cross-action mutation.

The official dependency-free command is:

```text
node --test --test-isolation=none apps/control-plane/web/p0-client.test.mjs
```

Development and hosted CI use Node 24.18.0. CI installs that exact version with the pinned official
setup action and runs the command before Rust gates. No npm install, lockfile, registry, browser
binary, transpiler, linter package, or third-party module exists. JavaScript syntax/import checking
is part of the same Node run. All 12 test names must be visible in Rust/Node test output;
placeholder/skipped/todo tests do not satisfy final acceptance.

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Fresh repaired-tree design review | `DESIGN ACCEPTED` | fresh-context Codex subagent |
| Skeleton compile/run before production | Pending | T006 skeleton commit |
| Focused Rust/Node tests | Pending | ACCEPT-T006 |
| Workspace gates | Pending | ACCEPT-T006 |
| Fresh acceptance review | Pending | ACCEPT-T006 |

# Traceability

CU-WEB-P0-01 → ADR-0002/ADR-0004 → T006 → 12 named tests →
`apps/control-plane/{src,web}/**`.

# TD Gaps

None. The first fresh design review's atomicity, resynchronization, browser bearer, MIME/WSS CSP,
redirect/bounds, error-map, and Node/ESM blockers are resolved here and in TD §14 without claiming
P1 UI scope. A second rejected pass identified five residual precision gaps; the final
fresh-context read-only verdict confirmed their repair and returned `DESIGN ACCEPTED`.

# Self-Check

T005 is Accepted. T006 is the sole Ready production task. Its public boundary is static same-origin
presentation plus the exact accepted T005 routes, with no new mutation or execution authority.
The first fresh read-only design review returned `DESIGN REJECTED`; all seven blockers were
repaired normatively before test skeletons or production edits. After one residual rejected pass,
a final fresh-context read-only review returned `DESIGN ACCEPTED`.
