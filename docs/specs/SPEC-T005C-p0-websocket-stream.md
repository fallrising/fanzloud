---
id: SPEC-T005C
title: P0 replay-then-live WebSocket stream
status: ready
contract_unit: CU-API-P0-02
module: codebox-control-plane
milestone: P0
archetype: D+F
atomicity: E0
invariants: [INV-003, INV-007, INV-012]
depends_on: [SPEC-T005A, SPEC-T005B]
td_sections: [1.7, 2.3, 4.4, 7.2, 8, 9, 10, 14, 15.0]
adr_refs: [ADR-0002, ADR-0003, ADR-0004]
risk: high
---

# Intent

Transport one authenticated T005A retained-replay/snapshot/live subscription over a bounded,
versioned WebSocket connection without becoming an ordering authority or mutating session/provider
state.

# Responsibility

## Does

- Authenticate the upgrade with the accepted T005B application cookie and exact Origin.
- Require one bounded version-1 subscribe frame within five seconds.
- Send replay begin, exact retained events, captured snapshot, replay end, then later live events.
- Enforce cursor/session/version/frame/subscriber limits and clean up lagged/disconnected clients.

## Does Not

- Accept mutating commands, prompts, cancellation, recovery, diff requests, auth bootstrap, or
  provider configuration over WebSocket.
- Assign `EventSeq`, retain history, poll, coalesce events, or decide recovery.
- Stream prompt, diff, device code, credential, raw provider output, path, or arbitrary logs.
- Claim pre-restart replay, endless buffering, or generic P1 protocol compatibility.

# Public Boundary

```text
GET /api/p0/v1/session/stream
Cookie: __Host-codebox_p0=<opaque>
Origin: <exact configured HTTPS origin>
Upgrade: websocket
```

The route is installed into the T005B router. There is no independent unauthenticated stream
constructor.

# Inputs and Outputs

The client must send one text frame within five seconds, at most 1024 UTF-8 bytes:

```json
{
  "type": "subscribe",
  "protocol_version": 1,
  "session_id": "<non-nil uuid>",
  "after_seq": 0
}
```

No unknown/duplicate fields are accepted. Binary, continuation beyond the bound, repeated
subscribe, or any other client application frame is a protocol error.

Server version-1 JSON frames are:

- `replay_begin { session_id, after_seq, high_water_seq }`
- `event { envelope }`
- `snapshot { snapshot, high_water_seq }`
- `replay_end { session_id, high_water_seq }`
- `error { code, message, oldest_available?, latest_available? }`

Only `event.envelope` carries a session `seq`. Control frames do not synthesize sequence numbers.
Every frame is at most 64 KiB; session events are designed below that bound.

# Preconditions and Disposition

| ID | Condition | Type / Checked / Internal | Trace |
|---|---|---|---|
| P-005C-01 | Valid unexpired cookie | Checked HTTP 401 | T005B |
| P-005C-02 | Exact configured Origin | Checked HTTP 403 | ADR-0004 |
| P-005C-03 | Valid WebSocket upgrade | Checked HTTP 400/426 | F |
| P-005C-04 | Subscribe arrives within 5 seconds | Typed close/error | Cleanup |
| P-005C-05 | Text/schema/version/session/cursor are exact | Typed close/error | D+F |
| P-005C-06 | T005A admits subscriber | Typed limit/gap error | T005A |

# Success Postconditions

- For captured high-water `H`, sends every retained envelope with `after_seq < seq <= H` in
  strictly increasing order, then the snapshot at `H`, then `ReplayEnd(H)`.
- Live delivery begins only with envelopes `seq > H`.
- A client retaining its last processed `(session_id, seq)` can reconnect without loss or duplicate
  for every retained cursor.
- Connection cleanup removes its one subscriber and no session/provider state.

# Non-Guarantees

- No delivery before the current process instance, after history eviction, or after subscriber
  lag/disconnect.
- No resume of a partially delivered replay; reconnect starts a new subscribe.
- No diff/log/token streaming, compression requirement, or stable P1 WebSocket compatibility.
- Network intermediaries may close idle connections.

# Exit Invariants

On handshake rejection, invalid frame, timeout, history gap, future cursor, lag, serialization/send
failure, peer close, server shutdown, and success:

- no turn cancel/recovery/start/diff/login operation is invoked;
- no session/provider lifecycle or event history changes;
- subscriber registration is removed exactly once;
- emitted event sequences never reorder or cross session IDs;
- forbidden sensitive/untrusted content is absent.

# Side Effects

Only one bounded ephemeral connection task, one admitted T005A subscriber, WebSocket frames, and
periodic protocol ping/pong. No durable side effect.

# Idempotency

Subscription is E0 over durable/provider/session lifecycle. Repeating a subscribe on a new
connection produces the same retained prefix for the same captured state; live timing may differ.
A second subscribe on one connection is rejected.

# Concurrency and Ordering

- T005A atomically returns replay through `H`, snapshot at `H`, and a receiver for `>H`.
- The connection sends those components serially before reading live receiver items.
- At most eight admitted subscribers exist per T005A configuration.
- Network writes have a 10-second bound. A bound breach closes only the connection.
- Client reads and server writes are selected concurrently after subscription so close/pong is
  observed without blocking live send.

# Streaming Semantics

- Ordering is strict ascending `EventSeq` within one session.
- Termination is peer close, typed protocol failure, lag, timeout, server shutdown, or transport
  failure.
- Chunk boundaries are WebSocket message boundaries; one JSON frame is one complete text message.
- Backpressure is the accepted bounded T005A queue plus 10-second network write deadline.
- Ping is sent at most every 30 seconds after subscription; pong carries no application state.
- A retention gap sends `history_gap` with safe bounds and closes; it never sends a partial replay.
- A future cursor sends `future_cursor` with latest safe bound and closes.

# Cancellation and Timeout

- Handshake subscribe deadline: 5 seconds.
- Network write deadline: 10 seconds per frame.
- Server close grace: 2 seconds.
- Transport cancel/disconnect only drops the subscriber/connection.
- No WebSocket input maps to T005A `cancel_turn`.

# Failure Atomicity

E0 covers T004/provider state, T005A session projection/history/sequence, login state, HTTP
idempotency, and every Codebox durable record. Ephemeral connection/subscriber allocation is
cleaned on failure and is not a durable mutation.

# Failure Modes and Error Contract

| Case | Error/close | Retriable | Caller action | Required payload | Trace |
|---|---|---:|---|---|---|
| Auth/Origin/upgrade failure | HTTP 401/403/400/426 | after correction | Authenticate/fix request | safe JSON | T005B |
| Subscribe deadline | `subscribe_timeout` / policy close | yes | Reconnect promptly | none | D |
| Binary/oversize/malformed/repeated | `protocol_error` / policy close | after correction | Fix client | none | F |
| Unsupported version | `unsupported_version` / policy close | no | Upgrade client | supported version 1 | Versioning |
| Wrong session | `wrong_session` / policy close | after refresh | Fetch snapshot | none | Authority |
| History gap | `history_gap` / policy close | after refresh | GET snapshot/reconnect | oldest/latest | ADR-0004 |
| Future cursor | `future_cursor` / policy close | after correction | Reset cursor | latest | ADR-0004 |
| Subscriber cap | `subscriber_limit` / try-again close | bounded | Close/wait/retry | none | Bounds |
| Slow client/queue full | `subscriber_lagged` if writable, then close | after refresh | GET snapshot/reconnect | latest optional | Backpressure |
| Serialization/internal channel | `stream_unavailable`, close | after restart | Refresh/retry | none | Cleanup |

Errors never echo invalid frames, cookies, Origin, or internal sources.

# Security Contract

- Cookie/Origin reuse the accepted T005B validation before upgrade.
- WebSocket URL contains no bearer/cookie/query credential.
- Input uses deny-unknown-fields schema, strong ID parsing, frame/time limits, and no command enum.
- Output allowlists only T005A public event/snapshot fields.
- Canary prompt/diff/code/bootstrap/cookie/credential/provider-output/path values must be absent from
  every frame/error/debug capture.
- Compression is disabled for P0 to avoid cross-secret compression exposure and resource variance.

# Observability and Audit Contract

Allowed metrics: admitted/rejected connection count, stable rejection code, replay event count,
lag/close category, and duration bucket. No cookie, Origin value, subscribe body, event body,
session ID, operation ID, or payload is logged.

# Test Specification

The following exact test names must exist and compile before T005C production code:

1. `p0_ws_requires_cookie_origin_and_valid_upgrade`
2. `p0_ws_requires_one_bounded_subscribe_before_deadline`
3. `p0_ws_replay_snapshot_end_then_live_order_is_exact`
4. `p0_ws_reconnect_after_each_retained_seq_has_no_loss_or_duplicate`
5. `p0_ws_rejects_history_gap_without_partial_replay`
6. `p0_ws_rejects_future_wrong_session_and_unsupported_version`
7. `p0_ws_rejects_binary_unknown_fields_and_repeated_subscribe`
8. `p0_ws_live_handoff_closes_replay_publication_race`
9. `p0_ws_slow_consumer_closes_only_its_connection`
10. `p0_ws_disconnect_never_cancels_or_mutates_session`
11. `p0_ws_shutdown_and_send_failure_remove_subscriber`
12. `p0_ws_frames_and_errors_exclude_sensitive_canaries`
13. `p0_ws_chunk_partition_and_reconnect_model_preserves_order`

# Acceptance Evidence

| Command or check | Result | Evidence URI or hash |
|---|---|---|
| Skeleton compile before production | Pending | T005C commit |
| Focused/repeated WS tests | Pending | ACCEPT-T005C |
| Workspace gates | Pending | ACCEPT-T005C |
| Fresh acceptance review | Pending | ACCEPT-T005C |

# Traceability

CU-API-P0-02 → ADR-0004 → T005C → 13 named tests → `codebox-control-plane`.

# TD Gaps

None. ADR-0004 defines P0 replay retention, gap handling, authentication, and non-durability.

# Self-Check

T005A and T005B are Accepted. T005C is Ready for its required fresh design audit and
skeleton-first implementation; no T005C production implementation is authorized.
