---
id: ADR-0004
title: Process-lifetime P0 session, operator authentication, and replay
status: accepted
date: 2026-07-28
deciders: [project]
---

# Context

T005 joins three P0 Contract Units whose atomicity and trust boundaries differ:

- CU-SES-P0-01 is an E1 in-process session lifecycle;
- CU-API-P0-01 is a per-endpoint HTTP trust boundary; and
- CU-API-P0-02 is an E0 replay/live stream.

TD §1.7 explicitly excludes crash-durable replay from P0, while the accepted T004B specification
said T005 must add "durable session events" before exposing its lifecycle over HTTP. The intended
requirement was that an HTTP event must never announce lower state before T004B durably commits
that state. It cannot require the P1 event store, canonical events, or restart replay without
pulling T020/T030/T040/T200 into the P0 fast path.

The TD also requires a privately authenticated browser but deliberately leaves the private
deployment's application authentication, P0 endpoint inventory, replay-retention gaps, and HTTP
idempotency scope open. Those choices affect security and retry behavior and therefore cannot be
invented only in production code.

# Decision

## Decompose T005 by Contract Unit

T005 is a coordination parent with three independently accepted children:

- T005A — CU-SES-P0-01, E1 process-lifetime session lifecycle and event fanout;
- T005B — CU-API-P0-01, authenticated P0 login/session HTTP API; and
- T005C — CU-API-P0-02, E0 replay-then-live WebSocket transport.

The serialized Ready order is T005A, T005B, T005C, then the T005 composition parent. T005 consumes
the accepted provider-specific T004 orchestrator directly and does not define or claim generic
`AgentBackend` conformance.

## Process-lifetime session and replay

P0 has one session per control-plane process. Its session ID and instance ID are fresh non-nil
identifiers. A bounded in-memory history assigns gap-free, strictly increasing `EventSeq` values
while that process remains alive.

The lower T004 submit ledger and lifecycle projection remain durable and authoritative for
provider-side progress. On startup, T005A reconstructs only a current snapshot from the accepted
T004 lifecycle. It does not reconstruct prior P0 events, prompts, HTTP responses, or subscribers.
An event that projects a lower lifecycle is published only after T004 returns its already-durable
projection.

This decision supersedes the T004B sentence requiring "durable session events." For P0 it means
ordered process-lifetime session events backed by an already-durable lower lifecycle. P1
crash-durable canonical events and replay remain owned by T020/T030/T040/T200.

## Reduced P0 session lifecycle and lower E2 boundary

The P0 session projection has:

- `Ready` — no mutating turn is running;
- `Running` — submit or provider monitoring is active;
- `RecoveryRequired` — the accepted lower lifecycle is `OutcomeUnknown`;
- `MonitoringDegraded` — a pending provider task remains known but its latest status read failed;
  and
- `Stopped` — local workers have shut down without claiming provider cancellation.

One current turn projection retains its typed turn ID and normalized T004 lifecycle. Prompts,
verification codes, raw provider output, diffs, and credentials are never event payloads.

`Ready` is the only state from which a new turn may start. `Running`,
`MonitoringDegraded`, and `RecoveryRequired` remain active or blocked for INV-004; they return to
`Ready` only through an accepted lower terminal projection or the explicit recovery/cancellation
rules. `Stopped` never transitions.

CU-SES-P0-01 E1 covers only the atomic in-memory session transition and ordered event publication.
T004 Cloud submit remains E2. Start commits a redacted turn intent before calling T004, then emits
only the durable lower projection returned by T004. An unknown result enters `RecoveryRequired`
and is never submitted again automatically.

T005A may call T004B `reconcile_unknown` and apply an operation-bound recovery command, but it does
not authenticate the operator, create recovery policy, or infer a decision. T005B alone validates
the authenticated request and creates either the exact adopt decision or the explicit
duplicate-risk acknowledgement bound to the current operation. The session event records the
fixed P0 operator actor and safe decision kind, never the acknowledgement payload.

Pending tasks, including a pending lifecycle reconstructed during startup, are observed by one
worker. A successful pending inspection schedules the next read
no sooner than the configured interval. Consecutive checked read failures use bounded exponential
backoff up to 60 seconds and coalesce repeated equal error events. Polling stops on a terminal
local lifecycle, explicit local cancel, recovery-required state, or runtime shutdown. Shutdown
does not call cancel or claim provider termination.

## Bounded history, replay gaps, and subscribers

The P0 baseline retains the latest 256 session events, supports at most eight subscribers, and
gives each subscriber a 32-frame live queue. These are administrator configuration values bounded
to safe ranges in SPEC-T005A.

Subscription setup is atomic with respect to event publication:

1. validate `after_seq`;
2. copy retained events with `seq > after_seq`;
3. capture the current snapshot and replay high-water mark;
4. register the subscriber for events above that high-water mark; and
5. release the session lock.

If `after_seq` is greater than the current sequence, the request fails as a future cursor. If it
is less than one before the oldest retained sequence, it fails with `HistoryGap` and reports only
the oldest and latest available sequence numbers. The client must fetch the current HTTP snapshot
and reconnect from its returned high-water mark. No partial replay is presented as complete.

A full subscriber queue removes only that subscriber. It never blocks event publication, cancels
a turn, or changes provider/session lifecycle. Repeated equivalent monitoring errors may be
coalesced; lifecycle transitions and explicit operator commands are never coalesced.

## Single-operator application authentication

P0 does not trust an unverified reverse-proxy identity header. The administrator configures one
opaque bootstrap bearer secret of 32 through 128 bytes outside repository and credential storage.
It is not a provider credential and is never logged, serialized, placed in an event, or returned
after exchange.

`POST /api/p0/v1/operator/session` accepts that bearer secret over HTTPS, compares it without
content-dependent early exit, and creates a random process-lifetime application session. The
response returns the runtime `p0_session_id` and `instance_id` and sets a separate opaque
application-authentication cookie:

```text
__Host-codebox_p0=<opaque>; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=<bounded>
```

The application session expires after at most 12 hours and is lost on restart. All other P0 HTTP
and WebSocket routes require the cookie. Mutating cookie-authenticated requests and the WebSocket
handshake must also match the administrator-configured HTTPS origin. Logout invalidates only the
application session. Tailscale, Cloudflare Access, or another private edge remains required
defense in depth; P0 is not a public or multi-user authentication design.

The authenticated actor is the fixed `P0Actor::Operator`; there is no browser-selected actor or
tenant.

SPEC-T005B owns the exact mechanics: the normalized administrator origin is compared as one
canonical HTTPS origin, bootstrap and cookie checks process their full bounded representations,
cookie values use 32 bytes from the OS CSPRNG, and monotonic time owns expiry. Crate-private
clock/entropy and lower-port seams make these policies deterministic under contract tests without
adding a browser-selectable production surface.

## P0 HTTP surface and process-lifetime idempotency

T005B defines only this P0 surface:

```text
POST /api/p0/v1/operator/session
DELETE /api/p0/v1/operator/session
GET /api/p0/v1/login
POST /api/p0/v1/login/device
POST /api/p0/v1/login/cancel
GET /api/p0/v1/session
POST /api/p0/v1/session/turns
POST /api/p0/v1/session/cancel
POST /api/p0/v1/session/reconcile
POST /api/p0/v1/session/resolve
GET /api/p0/v1/session/diff
GET /api/p0/v1/session/stream
```

This is not the P1 REST surface in TD §7.1.

Every mutating route except bootstrap exchange requires:

- a valid non-nil UUID `Idempotency-Key`; and
- the current `Codebox-Instance-Id` header.

Within one process, the latest 128 completed/in-flight keys are retained. Repeating the same key,
method, route, and bounded body joins or replays the first response. Reusing a key for a different
request is an idempotency conflict. The cache and bodies are never logged.

After restart, the instance ID changes. A stale instance header fails before invoking a handler,
forcing the browser to refresh the T004-backed snapshot. T006 must not automatically regenerate an
instance header and resubmit a prompt or recovery decision. This gives P0 explicit
process-lifetime idempotency without falsely claiming crash-durable HTTP replay. The accepted T004
ledger remains the protection against duplicate submission within an ambiguous lower operation.

Request bodies, headers, and responses have explicit limits in SPEC-T005B. The device verification
code is returned only in the authenticated start-login response, is never placed in an event or
idempotency diagnostic, and is discarded when its bounded cache entry expires.

Control-plane shutdown first rejects new admissions, drains already-admitted detached handlers,
then stops T005A and invokes the accepted T002B cancel/reconcile cleanup off the async reactor. It
does not infer provider cancellation or discard an in-flight idempotency result before the lower
call finishes.

## WebSocket replay/live protocol

An authenticated client connects to `/api/p0/v1/session/stream`, then sends exactly one bounded
version-1 subscribe frame within five seconds:

```json
{
  "type": "subscribe",
  "protocol_version": 1,
  "session_id": "<uuid>",
  "after_seq": 0
}
```

For a valid retained cursor the server sends:

1. `ReplayBegin`;
2. retained versioned public P0 events with `seq > after_seq`;
3. one current snapshot at the captured high-water mark;
4. `ReplayEnd`; and
5. live events above that high-water mark.

Web frames are separate transport types from P0 session events. Future cursors, retention gaps,
wrong sessions, unsupported versions, binary frames, oversized frames, repeated subscriptions,
and subscriber lag receive typed safe errors and close. WebSocket disconnect never calls session
cancel. Diff bytes, prompts, device codes, credentials, raw provider text, and internal paths are
never stream frames.

# Consequences

- T005 can satisfy P0 reconnect and refresh without pulling the P1 event store into the fast path.
- A process restart loses P0 event history, app sessions, and HTTP response replay, but keeps and
  safely reprojects accepted T004 provider lifecycle state.
- The instance precondition prevents a browser from blindly resubmitting a stale mutating request
  after restart.
- The bootstrap bearer is a new deployment secret and must be provisioned outside the repository.
- T006 must escape diff text at render time and must keep the bootstrap secret out of browser
  persistence and logs.
- T007 deterministic CI may use fakes; its live smoke requires an operator-authenticated private
  deployment and must not claim crash-durable replay.

# Alternatives Considered

## Pull the P1 event store and session actor into P0

Rejected because it defeats ADR-0002's fast path and contradicts the explicit P0 non-guarantee.

## Trust a caller-supplied reverse-proxy identity header

Rejected because direct access or proxy misconfiguration could forge the operator identity.

## Put the bootstrap bearer in the WebSocket URL

Rejected because URLs and subprotocol diagnostics are commonly logged. The HttpOnly cookie allows
browser WebSockets without exposing the bootstrap secret to JavaScript after exchange.

## Silently replay only the retained suffix

Rejected because it would present an incomplete history as complete and break ordering semantics.

## Persist prompts and HTTP responses for restart idempotency

Rejected for P0 because it creates a new sensitive durable store and migration/recovery contract.
The instance precondition makes the narrower process-lifetime guarantee explicit.

# Review Evidence

The first fresh read-only Cursor Agent decomposition review rejected the undeclared durability,
authentication, recovery-ownership, retention-gap, and lower-E2 semantics. Two subsequent reviews
rejected incomplete session transition and startup-observation details. The repaired ADR/task/spec
set closed those gaps with an exact state table, queue/cancel/shutdown protocol, fail-closed startup
read behavior, and atomic subscription handoff. A fourth fresh read-only review returned
`DESIGN ACCEPTED` with no blocker.

After T005A acceptance, three T005B-specific fresh reviews further closed the public-origin,
session-expiry/entropy, shutdown, deterministic-port, exact-route-schema, and exhaustive-error-map
details. The final T005B design review returned `DESIGN ACCEPTED`.
