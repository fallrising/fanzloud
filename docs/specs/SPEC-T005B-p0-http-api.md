---
id: SPEC-T005B
title: Authenticated P0 login and session HTTP API
status: accepted
contract_unit: CU-API-P0-01
module: codebox-control-plane
milestone: P0
archetype: F
atomicity: per-endpoint
invariants: [INV-004, INV-006, INV-007, INV-010, INV-012]
depends_on: [SPEC-T002, SPEC-T005A]
td_sections: [1.6, 1.7, 2.2, 2.3, 7, 8, 9, 10, 14, 15.0]
adr_refs: [ADR-0002, ADR-0003, ADR-0004]
risk: high
---

# Intent

Expose the accepted P0 login broker and session runtime through the exact private
single-operator HTTP surface in ADR-0004, with application authentication, bounded parsing,
process-instance idempotency, safe recovery authority, and typed redacted responses.

# Responsibility

## Does

- Exchange one administrator-configured bootstrap bearer for a process-lifetime HttpOnly cookie.
- Authenticate every other P0 route and enforce the configured HTTPS Origin on cookie-authenticated
  mutations.
- Expose normalized login status/device flow and the T005A session snapshot/start/cancel/recovery/
  diff operations.
- Require valid idempotency and current-instance headers before every mutation.
- Map accepted lower errors to stable JSON codes without raw sources.

## Does Not

- Serve WebSocket replay frames (T005C) or browser assets (T006).
- Trust proxy identity headers, support multiple operators, or claim public authentication.
- Accept browser paths, executable, Codex home, environment ID, branch, repository URL, CLI argv,
  provider token, or recovery acknowledgement text.
- Log/persist bearer/cookie values, verification codes, prompts, diffs, provider output, or
  idempotency bodies/responses.
- Add automatic submit/recovery retry, provider cancel, diff application, artifact publication, or
  repository commands.

# Public Boundary

```rust
pub struct OperatorBootstrapToken { /* secret, redacted */ }
pub struct P0PublicOrigin { /* canonical public HTTPS origin */ }
pub struct P0HttpConfig { /* private fields */ }
pub enum P0HttpConfigField { PublicOrigin, BootstrapToken, SessionLifetime }
pub struct P0HttpConfigError { /* safe field only */ }
pub struct P0HttpShutdownError { /* safe category only */ }
pub struct P0ControlPlane { /* private */ }

impl OperatorBootstrapToken {
    pub fn try_new(value: impl AsRef<[u8]>) -> Result<Self, P0HttpConfigError>;
}

impl P0PublicOrigin {
    pub fn try_new(value: &str) -> Result<Self, P0HttpConfigError>;
    pub fn as_str(&self) -> &str;
}

impl P0HttpConfig {
    pub fn new(
        public_origin: P0PublicOrigin,
        bootstrap: OperatorBootstrapToken,
    ) -> Self;
    pub fn try_with_session_lifetime(
        self,
        lifetime: Duration,
    ) -> Result<Self, P0HttpConfigError>;
}

impl P0ControlPlane {
    pub fn new(
        config: P0HttpConfig,
        login: LoginBroker,
        session: Arc<P0SessionRuntime>,
    ) -> Result<Self, P0HttpConfigError>;

    pub fn router(&self) -> axum::Router;
    pub async fn shutdown(&self) -> Result<(), P0HttpShutdownError>;
}
```

The binary constructs all credential paths, provider configuration, public origin, listener, and
bootstrap token from administrator process configuration before it constructs the router. None are
HTTP inputs. `P0HttpConfigError` stores only `P0HttpConfigField`; its `Debug`/`Display` never stores
or echoes the rejected origin, secret, or lifetime. `P0HttpShutdownError` stores only
`Session`, `Login`, or `Worker` and no lower source.

Production `new` binds crate-private `LoginPort` and `SessionPort` adapters to the accepted concrete
types. The crate also has a crate-private constructor accepting fake ports, a monotonic clock, and
an entropy source. Those ports expose only the exact accepted methods named in this specification;
they are not public, do not add arbitrary process/configuration authority, and exist so all 19
contract tests are deterministic. Tests 18 and 19 explicitly use the crate-private clock, entropy,
and lower-port constructor. The production clock uses monotonic elapsed time and the production
entropy source is the operating-system CSPRNG.

# Inputs and Outputs

## Administrator values

- `P0PublicOrigin::try_new` accepts UTF-8 of at most 256 bytes that parses as one absolute URL with
  scheme exactly `https`, a nonempty host, no username/password, no query/fragment, and no path
  other than the parser's single root slash. It rejects opaque URLs and invalid ports.
- The URL parser applies standard IDNA ASCII host conversion and lowercase normalization. The
  stored canonical serialization is exactly `https://<ascii-host>[:port]`, with IPv6 brackets, no
  trailing slash, and explicit port 443 removed. A non-443 port is retained.
- A request `Origin` is accepted only when its entire header value, after HTTP optional whitespace
  removal by the header parser, is byte-for-byte equal to that stored canonical serialization.
  `null`, multiple origins, alternate host spelling/case, an explicit default port, a trailing
  slash, userinfo, query, fragment, or suffix match is rejected. The rejected bytes are not stored.
- `OperatorBootstrapToken::try_new` accepts 32 through 128 bytes with no ASCII control byte. It
  stores a fixed 128-byte zero-padded buffer plus the length. Authentication compares all 128
  buffer bytes and the length accumulator on every attempt; malformed/missing bearer syntax still
  receives the same generic authentication response.
- `P0HttpConfig::new` uses the 12-hour session lifetime. `try_with_session_lifetime` accepts whole
  seconds from 300 through 43,200 inclusive and otherwise returns only
  `P0HttpConfigField::SessionLifetime`.

## Common transport contract

- HTTPS is required at the private edge. The control-plane listener is not a public TLS
  termination claim.
- JSON request bodies are UTF-8, `application/json`, at most 40 KiB unless the endpoint has no
  body. Prompt validation retains the lower exact 32-KiB bound.
- Header values are at most 256 bytes. Unsupported content type is 415; oversized input is 413;
  malformed JSON/UUID/value is 400 or 422 as classified below.
- JSON request types deny unknown and duplicate fields. No-body routes accept exactly zero body
  octets with either no content type or `application/json`; a nonempty body is 422
  `body_not_empty`. JSON routes require exactly `application/json` with optional UTF-8 charset.
- Every response uses `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`.
- Every JSON response has `Content-Type: application/json`; the empty logout and plain-text diff
  exceptions use the exact endpoint contracts below.
- JSON errors have only:

```json
{
  "error": {
    "code": "stable_snake_case",
    "message": "fixed safe text",
    "operation_id": "<optional safe uuid>"
  }
}
```

- Error responses never echo rejected body/header values.

## Authentication endpoints

`POST /api/p0/v1/operator/session`

- Requires `Authorization: Bearer <bootstrap>` and exact configured Origin.
- The bearer is 32–128 opaque bytes and compared across the full maximum length without
  content-dependent early return.
- Success is 201 JSON with fixed actor, expiry seconds, `p0_session_id` (the T005A runtime session
  used by WebSocket subscribe), and `instance_id`, plus
  `Set-Cookie: __Host-codebox_p0=<opaque>; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=<bounded>`.
- The opaque application-authentication session ID exists only as the cookie value and is never
  returned in JSON.
- At most four application sessions exist; creating another removes the oldest expired session or
  returns 429. Lifetime is configurable from 5 minutes through 12 hours, default 12 hours.
- The exact 201 body has `expires_in_seconds` equal to the configured whole lifetime seconds and
  therefore equal to cookie `Max-Age`. For the default configuration it is:

```json
{
  "actor": "operator",
  "expires_in_seconds": 43200,
  "p0_session_id": "<uuid>",
  "instance_id": "<uuid>"
}
```

- A cookie token is exactly 32 bytes from the OS CSPRNG encoded as 43 unpadded base64url ASCII
  bytes. Generation failure is 503 before registry mutation. Cookie lookup compares all 43 bytes
  plus validity without a content-dependent early return.
- Bootstrap accepts exactly one `Authorization` field with the ASCII scheme/prefix `Bearer ` and no
  surrounding whitespace. Cookie authentication accepts exactly one
  `__Host-codebox_p0=<43-byte token>` pair; duplicate target cookies are invalid, while unrelated
  cookie pairs are ignored. Missing, malformed, wrong-length, and unknown values all execute the
  fixed-size comparison path against a dummy/current slot and return the same 401.
- A generated cookie collision draws again before mutation, for at most eight total draws. Eight
  collisions or an entropy error returns 503; no existing session is replaced.
- `Max-Age` is exactly the configured whole lifetime seconds. The monotonic expiry instant is
  captured from the same admission clock reading. On every authentication and new-session
  admission, entries with `now >= expires_at` are invalidated before capacity is checked; if
  several exist, the earliest issuance sequence is removed first. Wall-clock changes cannot extend
  a session.
- Tests 1 and 18 use a non-default lifetime and prove JSON `expires_in_seconds`, cookie `Max-Age`,
  and the crate-private monotonic expiration boundary are the same configured value.

`DELETE /api/p0/v1/operator/session`

- Requires the valid cookie, Origin, instance, and idempotency key.
- Invalidates that cookie and returns 204 with an expiring `Set-Cookie`.
- The empty 204 response sets exactly
  `__Host-codebox_p0=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0` and has no response
  body. No `Domain` attribute is ever emitted.

All other routes require a valid unexpired cookie. A browser-supplied bearer is not accepted as a
substitute.

## Login endpoints

`GET /api/p0/v1/login`

- Returns 200 with one normalized `logged_out`, `device_login_pending`, `logged_in`, or
  `outcome_unknown` state and optional operation ID.
- The exact tagged body is one of:

```json
{ "state": "logged_out" }
{ "state": "device_login_pending", "operation_id": "<uuid>" }
{ "state": "logged_in" }
{ "state": "outcome_unknown", "operation_id": "<uuid>" }
```

`POST /api/p0/v1/login/device`

- Empty body. On success returns 202 with operation ID, exact pinned HTTPS verification URL,
  bounded verification code, and expiry seconds.
- The verification code is the only intentionally revealed provider-issued value. It appears only
  in this authenticated response and an exact same-instance idempotency replay.
- The exact body is:

```json
{
  "operation_id": "<uuid>",
  "verification_url": "https://auth.openai.com/codex/device",
  "verification_code": "<bounded code>",
  "expires_in_seconds": 900
}
```

  The idempotency entry expires at `admission_monotonic_time + expires_in_seconds`, not response
  completion, so a replay is never retained later than the provider instruction lifetime.

`POST /api/p0/v1/login/cancel`

- Empty body. Returns 200 with the exact normalized login-status schema above. It does not claim
  that provider
  authorization was prevented.

## Session endpoints

`GET /api/p0/v1/session`

- Returns 200 with the exact T005A identity/snapshot projection:

```json
{
  "identity": {
    "session_id": "<uuid>",
    "instance_id": "<uuid>"
  },
  "state": "ready|running|recovery_required|monitoring_degraded|stopped",
  "current_turn": null,
  "high_water_seq": 0
}
```

  When present, `current_turn` is
  `{"turn_id":"<uuid>","projection":<P0TurnProjection>}`. The projection is exactly the tagged
  `phase` schema and nested `P0CloudLifecycle` `state` schema defined by SPEC-T005A and its accepted
  serde types; no HTTP-only lifecycle state or field is added.

`POST /api/p0/v1/session/turns`

```json
{ "prompt": "<validated CloudPrompt>" }
```

- Success is 202 with exactly
  `{"turn_id":"<uuid>","high_water_seq":<u64>}`.
- Prompt is passed once to T005A and is absent from response/events/logs.

`POST /api/p0/v1/session/cancel`

- Empty body. Passes fixed `P0Actor::Operator`; success returns 200 with the exact snapshot schema
  above.
- Response wording is `canceled_locally` and preserves `provider_may_continue`.

`POST /api/p0/v1/session/reconcile`

- Empty body. Passes fixed actor; success returns 200 with exactly
  `{"operation_id":"<uuid>","task_ids":["task_..."],"complete":<bool>}` and at most 100
  bounded task IDs from accepted T004 reconciliation.
- It does not resolve unknown state or authorize a new submit.

`POST /api/p0/v1/session/resolve`

Exactly one tagged body is accepted:

```json
{
  "operation_id": "<uuid>",
  "decision": { "type": "adopt", "task_id": "task_..." }
}
```

or:

```json
{
  "operation_id": "<uuid>",
  "decision": {
    "type": "abandon",
    "acknowledge_duplicate_task_risk": true
  }
}
```

- False/missing acknowledgement is 422 `acknowledgement_required`.
- The handler constructs `DuplicateRiskAcknowledgement` only after cookie, Origin, instance,
  idempotency, body, current-operation, and exact `true` checks.
- Success returns 200 with the exact snapshot schema above. It never automatically calls start.

`GET /api/p0/v1/session/diff`

- Delegates once to T005A/T004C and returns 200 with at most 2 MiB as
  `text/plain; charset=utf-8`, `Content-Security-Policy: default-src 'none'; sandbox`, and
  `Content-Disposition: inline`.
- The body is untrusted display text. The API does not parse, escape, summarize, persist, stream,
  apply, or publish it.

T005C adds `GET /api/p0/v1/session/stream` without changing this CU's mutation surface.

# Preconditions and Disposition

| ID | Condition | Type / Checked / Internal | Trace |
|---|---|---|---|
| P-005B-01 | Admin origin is absolute HTTPS with no userinfo/query/fragment | Checked config error | ADR-0004 |
| P-005B-02 | Bootstrap length 32–128 and no control bytes | Secret type/check | Auth |
| P-005B-03 | Protected route has valid cookie | Checked 401 | Auth |
| P-005B-04 | Cookie mutation has exact Origin | Checked 403 | CSRF |
| P-005B-05 | Mutation has valid key and current instance | Checked 400/409 | Idempotency |
| P-005B-06 | Body/content type/size/schema is exact | Checked 400/413/415/422 | F bounds |
| P-005B-07 | Strong IDs and prompt/task values validate | Checked 422 | T010/T003 |
| P-005B-08 | Login/session lower state permits operation | Typed lower mapping | T002/T005A |

# Success Postconditions

- Authentication creates only a bounded in-memory application session and one secure cookie.
- An accepted mutating key invokes its endpoint handler at most once in the process.
- Login routes invoke only accepted `LoginBroker` methods.
- Session routes invoke only accepted T005A methods.
- Recovery abandonment authority is created only on the fully authenticated/validated exact route.
- HTTP transport never changes event order or synthesizes lifecycle state.

# Non-Guarantees

- No public/multi-user auth, OIDC, refresh token, durable app session, durable HTTP idempotency, or
  operation across an instance change.
- Edge/TLS/private-network configuration is deployment responsibility.
- Device login cancel does not prove the operator did not finish authorization.
- Local turn cancel does not cancel the provider task.
- Diff is untrusted and is not HTML-safe without T006 text rendering.

# Exit Invariants

On every status, parse/auth failure, handler error, disconnect, timeout, and task shutdown:

- unauthenticated/untrusted inputs invoke no lower operation;
- stale instance and idempotency conflict invoke no handler;
- one accepted key invokes at most one mutation;
- no credential/bootstrap/cookie/code/prompt/diff/raw output/internal path appears in errors/logs;
- browser/HTTP disconnect never invokes turn cancel;
- recovery acknowledgement is never inferred or reused for another operation.

# Side Effects

- Bounded process-memory app sessions and idempotency records.
- Secure cookie issue/invalidation.
- Exact accepted login/session calls.
- No new filesystem, repository, arbitrary process, provider configuration, or artifact effects.

# Idempotency

- GET routes are observation-only but may perform the single accepted provider read for login/diff.
- The required header names are exactly `Idempotency-Key` and `Codebox-Instance-Id` (HTTP field
  names are case-insensitive). Mutating requests are keyed globally within the process by non-nil
  `CommandId`.
- Cache identity is exact method + normalized route + bounded body bytes + current instance.
- The first request installs an in-flight entry before handler invocation. An equal concurrent
  request waits and receives the exact first status/headers/body; a different request is 409.
- At most 128 entries and 8 MiB total response/body storage are retained; completed oldest entries
  are evicted first. If only in-flight entries prevent admission, return 503 before invocation.
- Size accounting conservatively includes the key, method/normalized route discriminants, exact
  bounded request body, response status, replayed response headers, and response body. Admission
  evicts expired entries first, then completed entries by completion sequence until both bounds
  fit. It never evicts or replaces an in-flight entry.
- Verification-code responses expire from the cache no later than their provider instruction
  expiry. Logout removes its cached response after replay waiters complete.
- Logout is the sole replay exception after its first response completes: because it invalidates
  the authenticating cookie and removes its completed cache entry, a later duplicate fails
  authentication rather than replaying 204.
- Instance mismatch is checked before key lookup; restart never treats an old key as replayable.

# Concurrency and Ordering

- App-session and idempotency registries are synchronized and never held across blocking lower
  operations.
- Blocking login methods serialize through one broker mutex and execute off async reactor threads.
- T005A remains the session ordering authority.
- Logout racing a protected request uses authentication state captured at request admission;
  requests admitted after invalidation fail.
- A crate-private RAII admission count covers every authenticated request after authentication and
  every detached blocking handler until its response is committed to the idempotency entry.
  Disconnect drops only the HTTP waiter, not this count or worker.
- Bootstrap validation order is shutdown/method/body bounds, exact Origin, then the fixed-work
  bearer comparison. Protected mutations validate shutdown, cookie, Origin, current instance,
  idempotency-key syntax, body bounds/media/schema, then perform idempotency lookup/admission.
  Only the first owner performs route-specific current-state validation and lower invocation;
  an exact replay returns the recorded response even if lower state later changes.
- Logout invalidates its captured cookie only inside the admitted owner operation. Concurrent exact
  requests already authenticated before invalidation join the in-flight entry; a later request
  fails cookie authentication before key lookup.

# Streaming Semantics

CU-API-P0-01 returns completed bounded HTTP responses. It does not chunk provider output or expose
the session event stream. The diff response is one bounded text body.

# Cancellation and Timeout

- Request disconnect does not cancel an admitted mutation or turn.
- HTTP handler deadlines may stop waiting for a response but do not infer lower outcome or invoke a
  second call. The idempotency entry remains in flight until the worker records its result.
- Only the explicit login/turn cancel endpoints call lower cancel methods.
- `P0ControlPlane::shutdown` is idempotent. The first call atomically latches shutdown so later
  admissions return 503, waits for every already-admitted detached handler to commit its response,
  then calls T005A `shutdown` once, calls T002B `cancel` once (which reconciles when no active child
  exists under the accepted broker contract), and finally clears application-session and
  idempotency registries. Blocking waits/lower calls run off async reactor threads.
- Concurrent shutdown callers join that same shutdown result. A session failure does not skip the
  login cleanup; a login failure does not undo the session stop. The returned error exposes only
  `Session`, `Login`, or `Worker`, preferring the first failure in that ordering.
- Shutdown never claims provider authorization/task cancellation, starts login, starts a turn,
  resolves recovery, or reads a diff.

# Failure Atomicity

| Endpoint class | Atomicity |
|---|---|
| Auth exchange/logout | E1 process-memory session update |
| Login status | Lower E0/E2 reconciliation contract |
| Device login/cancel | Lower T002B E2 |
| Session snapshot/reconcile/cancel/resolve | Lower T005A/T004 declared contract |
| Start turn | T005A E1 intent plus lower T004 E2 |
| Diff | T004C E0 managed state |

Transport errors before handler admission are E0. The API never upgrades lower atomicity.

# Failure Modes and Error Contract

| Case | HTTP / code | Retriable | Caller action | Required payload | Trace |
|---|---|---:|---|---|---|
| Missing/invalid bootstrap or cookie | 401 `authentication_required` | no | Authenticate | none | Auth |
| Origin mismatch | 403 `origin_forbidden` | no | Use configured origin | none | CSRF |
| App-session cap | 429 `session_limit` | later | Expire/logout | none | Bounds |
| Missing/invalid key | 400 `idempotency_key_invalid` | yes | Send UUID | none | Idempotency |
| Stale instance | 409 `instance_changed` | no auto retry | Refresh snapshot | none | ADR-0004 |
| Key conflict | 409 `idempotency_conflict` | no | New explicit command | none | Idempotency |
| Cache saturated | 503 `idempotency_unavailable` | bounded | Wait/retry same key | none | Bounds |
| Wrong media/body/size | 415/400/413/422 | after correction | Correct request | safe field code | F |
| Login lower error | mapped 409/422/503 | category-dependent | Follow safe code | optional op ID | T002 |
| Session lower error | mapped 409/422/503/504 | category-dependent | Refresh/explicit action | optional op ID | T005A |
| Diff unavailable/failure | mapped 409/503/504 | no internal retry | Refresh/explicit retry | safe code | T004C |
| Internal join/poison | 503 `service_unavailable` | after restart | Retry safely | none | Cleanup |

The transport codes/messages are exact:

| Condition | HTTP / code | Fixed message |
|---|---|---|
| Authentication | 401 `authentication_required` | `operator authentication is required` |
| Origin | 403 `origin_forbidden` | `request origin is not allowed` |
| App-session capacity | 429 `session_limit` | `operator session limit reached` |
| Invalid idempotency key | 400 `idempotency_key_invalid` | `idempotency key must be a non-nil UUID` |
| Missing/invalid/stale instance | 409 `instance_changed` | `control-plane instance changed; refresh before retry` |
| Key conflict | 409 `idempotency_conflict` | `idempotency key was used for another request` |
| Cache unavailable | 503 `idempotency_unavailable` | `idempotency registry is temporarily unavailable` |
| Unsupported media type | 415 `unsupported_media_type` | `application/json is required` |
| Body too large | 413 `request_too_large` | `request body exceeds its limit` |
| Empty-route body present | 422 `body_not_empty` | `request body must be empty` |
| Malformed JSON | 400 `malformed_json` | `request body is not valid JSON` |
| Unknown/duplicate/missing field | 422 `invalid_request` | `request schema is invalid` |
| Invalid prompt/task/UUID value | 422 `invalid_value` | `request value is invalid` |
| Missing exact abandon acknowledgement | 422 `acknowledgement_required` | `duplicate-task-risk acknowledgement is required` |
| Shutdown/join/poison | 503 `service_unavailable` | `control-plane service is unavailable` |

Every `LoginBrokerError` maps exactly:

| Lower variant | HTTP / code | Fixed message |
|---|---|---|
| `CredentialScope(_)` | 503 `login_unavailable` | `login credential scope is unavailable` |
| `VersionMismatch` | 503 `login_version_mismatch` | `accepted login provider version is unavailable` |
| `LoginAlreadyRunning` | 409 `login_already_running` | `a device login is already running` |
| `AlreadyLoggedIn` | 409 `already_logged_in` | `operator is already logged in` |
| `ProviderOutputInvalid` | 503 `login_provider_drift` | `login provider response is unavailable` |
| `OutputLimitExceeded` | 503 `login_output_limit` | `login provider response exceeded its limit` |
| `StatusUnavailable` | 503 `login_status_unavailable` | `login status is unavailable` |
| `LoginFailed` | 409 `login_failed` | `device login did not complete` |
| `OutcomeUnknown` | 409 `login_outcome_unknown` | `device login outcome requires reconciliation` |
| `Process { .. }` | 503 `login_process_unavailable` | `login process is unavailable` |
| `LedgerUnavailable { .. }` | 503 `login_state_unavailable` | `login state is unavailable` |
| `LedgerInvalid` | 503 `login_state_invalid` | `login state requires operator repair` |

Every top-level `P0SessionErrorCategory` maps exactly. `CloudLifecycle` and `CloudDiff` delegate to
the exhaustive lower tables below.

| Session category | HTTP / code | Fixed message |
|---|---|---|
| `InvalidConfig` | 503 `session_config_invalid` | `session configuration is unavailable` |
| `TurnAlreadyRunning` | 409 `turn_already_running` | `a turn is already active` |
| `NoCurrentTurn` | 409 `no_current_turn` | `no current turn is available` |
| `WrongState` | 409 `session_wrong_state` | `session state does not allow this operation` |
| `WrongSession` | 409 `session_changed` | `session identity changed; refresh before retry` |
| `WrongOperation` | 409 `operation_changed` | `current operation changed; refresh before retry` |
| `RuntimeStopped` | 503 `session_stopped` | `session runtime is stopped` |
| `CloudLifecycle` | lower table | lower table |
| `CloudDiff` | lower table | lower table |
| `HistoryGap` | 409 `history_gap` | `requested session history is no longer retained` |
| `FutureCursor` | 409 `future_cursor` | `requested session cursor is in the future` |
| `SubscriberLimit` | 503 `subscriber_limit` | `session subscriber limit reached` |
| `SequenceExhausted` | 503 `session_sequence_exhausted` | `session event sequence is exhausted` |
| `LowerConflict` | 409 `provider_state_conflict` | `provider state changed incompatibly` |

Every nested `CloudLifecycleErrorCategory` maps exactly:

| Lifecycle category | HTTP / code | Fixed message |
|---|---|---|
| `Scope` | 503 `provider_scope_unavailable` | `provider credential scope is unavailable` |
| `Busy` | 503 `provider_busy` | `provider operation is busy` |
| `TurnAlreadyRunning` | 409 `provider_turn_running` | `a provider turn is already active` |
| `NoCurrentOperation` | 409 `no_current_operation` | `no provider operation is available` |
| `WrongState` | 409 `provider_wrong_state` | `provider state does not allow this operation` |
| `StaleDecision` | 409 `recovery_decision_stale` | `recovery decision is stale` |
| `TaskNotListed` | 422 `task_not_listed` | `task is not in the complete recovery set` |
| `AcknowledgementRequired` | 422 `acknowledgement_required` | `duplicate-task-risk acknowledgement is required` |
| `LowerRunner` | 503 `provider_runner_unavailable` | `provider runner is unavailable` |
| `ProviderRead` | 503 `provider_read_unavailable` | `provider state cannot be read` |
| `OperationConflict` | 409 `provider_operation_conflict` | `another provider operation owns current state` |
| `OutcomeUnknown` | 409 `provider_outcome_unknown` | `provider outcome requires explicit recovery` |
| `LedgerInvalid` | 503 `provider_state_invalid` | `provider state requires operator repair` |
| `LedgerUnavailable` | 503 `provider_state_unavailable` | `provider state is unavailable` |
| `RecoveryRequired` | 409 `provider_recovery_required` | `provider recovery requires operator action` |

Every nested `CloudDiffReadErrorCategory` maps exactly:

| Diff category | HTTP / code | Fixed message |
|---|---|---|
| `IneligibleLifecycle` | 409 `diff_not_ready` | `current task is not eligible for diff retrieval` |
| `AuthorityMismatch` | 409 `diff_authority_changed` | `current task changed; refresh before retry` |
| `Scope` | 503 `diff_scope_unavailable` | `diff credential scope is unavailable` |
| `Busy` | 503 `diff_busy` | `diff provider operation is busy` |
| `Version` | 503 `diff_version_mismatch` | `accepted diff provider version is unavailable` |
| `DiagnosticBoundary` | 503 `diff_boundary_unavailable` | `diff diagnostic boundary is unavailable` |
| `Process` | 503 `diff_process_unavailable` | `diff process is unavailable` |
| `Timeout` | 504 `diff_timeout` | `diff retrieval timed out` |
| `Canceled` | 409 `diff_canceled` | `diff retrieval was canceled` |
| `OutputLimit` | 503 `diff_output_limit` | `diff exceeded its output limit` |
| `ProviderDrift` | 503 `diff_provider_drift` | `diff provider response is unavailable` |
| `InvalidDiff` | 503 `diff_invalid` | `diff display data is invalid` |

`error.operation_id` is omitted for transport/value errors and caller-supplied wrong-operation
errors. It is included only when an accepted lower lifecycle error carries a durable current
operation ID. No other JSON fields are permitted.

# Security Contract

- Bootstrap and cookie types redact debug/display and are excluded from serde.
- Authentication comparison processes the full bounded lengths.
- `__Host-` cookie has no Domain and has Secure/HttpOnly/SameSite=Strict/Path attributes.
- Origin is exact normalized HTTPS scheme/host/effective-port equality; no suffix matching.
- Responses use no-store/nosniff; diff adds restrictive CSP and plain-text type.
- Request logging, panic text, tracing fields, and idempotency diagnostics contain no headers/body.
- Canary tests cover bootstrap, cookie, device code, prompt, diff, credential marker, provider raw
  text, and internal paths.
- Browser inputs cannot select configuration or construct arbitrary lower commands.

# Observability and Audit Contract

Allowed metrics are route template, method, status class, latency bucket, and stable error code.
Recovery events record fixed actor and safe decision kind through T005A. Raw headers, bodies,
values, IDs beyond allowed operation correlation, and response content are excluded.

# Test Specification

The following exact test names must exist and compile before T005B production code:

1. `p0_http_bootstrap_sets_secure_host_cookie_and_redacts_secret`
2. `p0_http_rejects_missing_cookie_and_wrong_origin_before_handler`
3. `p0_http_login_status_and_device_code_are_exact_and_bounded`
4. `p0_http_device_code_never_enters_events_errors_or_logs`
5. `p0_http_start_turn_validates_prompt_and_returns_accepted_receipt`
6. `p0_http_mutations_require_current_instance_and_idempotency_key`
7. `p0_http_same_key_replays_once_and_different_request_conflicts`
8. `p0_http_concurrent_same_key_joins_in_flight_response`
9. `p0_http_cancel_is_explicit_and_disconnect_is_not_cancel`
10. `p0_http_reconcile_does_not_resolve_or_retry`
11. `p0_http_abandon_requires_exact_true_ack_and_current_operation`
12. `p0_http_adopt_rejects_unlisted_or_stale_task`
13. `p0_http_diff_is_plain_bounded_untrusted_and_not_cached`
14. `p0_http_bounds_content_type_and_error_schema_fail_closed`
15. `p0_http_logout_invalidates_only_current_app_session`
16. `p0_http_forbids_browser_provider_and_host_configuration`
17. `p0_http_canaries_are_absent_from_debug_and_nonsecret_responses`
18. `p0_http_session_expiry_capacity_and_cookie_comparison_are_bounded`
19. `p0_http_shutdown_drains_handlers_and_cleans_lower_runtime`

Test 3 also exercises the exact `POST /login/cancel` status projection. Test 5 first asserts the
standalone `GET /session` schema before starting the turn.

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Skeleton compile before production | Passed | `5852fa7` |
| Focused/concurrent/security tests | Passed: 19 tests, 10 repeated package runs | ACCEPT-T005B |
| Workspace gates | Passed: fmt, Clippy, 161 tests, bins, deny, diff | ACCEPT-T005B |
| Fresh acceptance review | `IMPLEMENTATION ACCEPTED` | ACCEPT-T005B |

# Traceability

CU-API-P0-01 → ADR-0004 → T005B → 19 named tests → `codebox-control-plane`.

# TD Gaps

None. ADR-0004 defines the P0 app-auth, routes, idempotency lifetime, and instance behavior.

# Self-Check

T005A is Accepted. Three T005B-specific design reviews repaired seven public-origin,
configuration/expiry, shutdown, fake-port, route-schema, and error-mapping blockers plus two
residual documentation contradictions. Implementation reviews then repaired authentication work
coverage, disconnect/replay fidelity, exact lower-error coverage, bootstrap/shutdown admission,
UUID classification, and a concurrent logout race. All 19 tests and workspace gates pass; the
final fresh-context read-only review returned `IMPLEMENTATION ACCEPTED`. T005B is Accepted and
T005C is the sole Ready task.
