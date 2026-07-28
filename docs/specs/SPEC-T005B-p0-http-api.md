---
id: SPEC-T005B
title: Authenticated P0 login and session HTTP API
status: draft
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
pub struct P0HttpConfig { /* private fields */ }
pub struct P0ControlPlane { /* private */ }

impl OperatorBootstrapToken {
    pub fn try_new(value: impl AsRef<[u8]>) -> Result<Self, P0HttpConfigError>;
}

impl P0HttpConfig {
    pub fn new(
        public_origin: P0PublicOrigin,
        bootstrap: OperatorBootstrapToken,
    ) -> Self;
}

impl P0ControlPlane {
    pub fn new(
        config: P0HttpConfig,
        login: LoginBroker,
        session: Arc<P0SessionRuntime>,
    ) -> Result<Self, P0HttpConfigError>;

    pub fn router(&self) -> axum::Router;
}
```

The binary constructs all credential paths, provider configuration, public origin, listener, and
bootstrap token from administrator process configuration before it constructs the router. None are
HTTP inputs.

# Inputs and Outputs

## Common transport contract

- HTTPS is required at the private edge. The control-plane listener is not a public TLS
  termination claim.
- JSON request bodies are UTF-8, `application/json`, at most 40 KiB unless the endpoint has no
  body. Prompt validation retains the lower exact 32-KiB bound.
- Header values are at most 256 bytes. Unsupported content type is 415; oversized input is 413;
  malformed JSON/UUID/value is 400 or 422 as classified below.
- Every response uses `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`.
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

`DELETE /api/p0/v1/operator/session`

- Requires the valid cookie, Origin, instance, and idempotency key.
- Invalidates that cookie and returns 204 with an expiring `Set-Cookie`.

All other routes require a valid unexpired cookie. A browser-supplied bearer is not accepted as a
substitute.

## Login endpoints

`GET /api/p0/v1/login`

- Returns 200 with one normalized `logged_out`, `device_login_pending`, `logged_in`, or
  `outcome_unknown` state and optional operation ID.

`POST /api/p0/v1/login/device`

- Empty body. On success returns 202 with operation ID, exact pinned HTTPS verification URL,
  bounded verification code, and expiry seconds.
- The verification code is the only intentionally revealed provider-issued value. It appears only
  in this authenticated response and an exact same-instance idempotency replay.

`POST /api/p0/v1/login/cancel`

- Empty body. Returns the normalized reconciled login status. It does not claim that provider
  authorization was prevented.

## Session endpoints

`GET /api/p0/v1/session`

- Returns the exact T005A identity/snapshot projection.

`POST /api/p0/v1/session/turns`

```json
{ "prompt": "<validated CloudPrompt>" }
```

- Success is 202 with turn receipt and current high-water sequence.
- Prompt is passed once to T005A and is absent from response/events/logs.

`POST /api/p0/v1/session/cancel`

- Empty body. Passes fixed `P0Actor::Operator`; success returns the new snapshot.
- Response wording is `canceled_locally` and preserves `provider_may_continue`.

`POST /api/p0/v1/session/reconcile`

- Empty body. Passes fixed actor; success returns operation ID, completeness, and at most 100
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
- Success returns the new snapshot. It never automatically calls start.

`GET /api/p0/v1/session/diff`

- Delegates once to T005A/T004C and returns at most 2 MiB as
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

# Streaming Semantics

CU-API-P0-01 returns completed bounded HTTP responses. It does not chunk provider output or expose
the session event stream. The diff response is one bounded text body.

# Cancellation and Timeout

- Request disconnect does not cancel an admitted mutation or turn.
- HTTP handler deadlines may stop waiting for a response but do not infer lower outcome or invoke a
  second call. The idempotency entry remains in flight until the worker records its result.
- Only the explicit login/turn cancel endpoints call lower cancel methods.
- Control-plane shutdown calls T005A shutdown and T002B cancel/reconcile according to accepted
  lower behavior before process exit; it never claims provider cancellation.

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
| Stale instance | 409 `instance_changed` | no auto retry | Refresh snapshot | current instance ID | ADR-0004 |
| Key conflict | 409 `idempotency_conflict` | no | New explicit command | none | Idempotency |
| Cache saturated | 503 `idempotency_unavailable` | bounded | Wait/retry same key | none | Bounds |
| Wrong media/body/size | 415/400/413/422 | after correction | Correct request | safe field code | F |
| Login lower error | mapped 409/422/503 | category-dependent | Follow safe code | optional op ID | T002 |
| Session lower error | mapped 409/422/503/504 | category-dependent | Refresh/explicit action | optional op ID | T005A |
| Diff unavailable/failure | mapped 409/503/504 | no internal retry | Refresh/explicit retry | safe code | T004C |
| Internal join/poison | 503 `service_unavailable` | after restart | Retry safely | none | Cleanup |

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

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Skeleton compile before production | Pending | T005B commit |
| Focused/concurrent/security tests | Pending | ACCEPT-T005B |
| Workspace gates | Pending | ACCEPT-T005B |
| Fresh acceptance review | Pending | ACCEPT-T005B |

# Traceability

CU-API-P0-01 → ADR-0004 → T005B → 17 named tests → `codebox-control-plane`.

# TD Gaps

None. ADR-0004 defines the P0 app-auth, routes, idempotency lifetime, and instance behavior.

# Self-Check

Draft pending T005A acceptance and fresh design acceptance. No T005B production implementation is
authorized.
