import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { mountP0App } from "./p0-app.js";
import { createP0Controller } from "./p0-client.js";

const SESSION_ID = "00000000-0000-0000-0000-000000000001";
const INSTANCE_ID = "00000000-0000-0000-0000-000000000002";
const OPERATION_ID = "00000000-0000-0000-0000-000000000003";
const TURN_ID = "00000000-0000-0000-0000-000000000004";
const OTHER_SESSION_ID = "00000000-0000-0000-0000-000000000005";
const KEY = "11111111-1111-4111-8111-111111111111";
const CURSOR_KEY = "codebox.p0.cursor.v1";
const JSON_TYPE = "application/json";
const DIFF_TYPE = "text/plain; charset=utf-8";
const JSON_LIMIT = 64 * 1024;
const DIFF_LIMIT = 2 * 1024 * 1024;

const ROUTES = Object.freeze({
  bootstrap: "/api/p0/v1/operator/session",
  login: "/api/p0/v1/login",
  loginDevice: "/api/p0/v1/login/device",
  loginCancel: "/api/p0/v1/login/cancel",
  session: "/api/p0/v1/session",
  turns: "/api/p0/v1/session/turns",
  turnCancel: "/api/p0/v1/session/cancel",
  diff: "/api/p0/v1/session/diff",
  stream: "/api/p0/v1/session/stream",
});

const FIXED = Object.freeze({
  authentication: "Authentication is required.",
  resync: "Current state changed; status was refreshed.",
  recovery: "Outcome requires the trusted operator recovery procedure.",
  rejected: "The explicit action was rejected; refresh before deciding whether to act again.",
  service: "The service could not complete the request; no command was retried.",
  request: "The request could not be completed safely.",
  responseLimit: "The response exceeded its safe display limit.",
  diffLimit: "The diff exceeded its safe display limit.",
  protocol: "The session stream returned an invalid frame.",
});

const HTTP_GROUPS = Object.freeze({
  authentication: ["authentication_required"],
  resync: [
    "instance_changed",
    "session_changed",
    "history_gap",
    "future_cursor",
    "operation_changed",
    "diff_authority_changed",
  ],
  recovery: [
    "login_outcome_unknown",
    "provider_outcome_unknown",
    "provider_recovery_required",
  ],
  rejected: [
    "origin_forbidden",
    "session_limit",
    "idempotency_key_invalid",
    "idempotency_conflict",
    "unsupported_media_type",
    "request_too_large",
    "body_not_empty",
    "malformed_json",
    "invalid_request",
    "invalid_value",
    "acknowledgement_required",
    "task_not_listed",
    "login_already_running",
    "already_logged_in",
    "login_failed",
    "turn_already_running",
    "no_current_turn",
    "session_wrong_state",
    "recovery_decision_stale",
    "provider_turn_running",
    "no_current_operation",
    "provider_wrong_state",
    "diff_not_ready",
    "diff_canceled",
  ],
  service: [
    "idempotency_unavailable",
    "service_unavailable",
    "login_unavailable",
    "login_version_mismatch",
    "login_provider_drift",
    "login_output_limit",
    "login_status_unavailable",
    "login_process_unavailable",
    "login_state_unavailable",
    "login_state_invalid",
    "session_config_invalid",
    "session_stopped",
    "subscriber_limit",
    "session_sequence_exhausted",
    "provider_state_conflict",
    "provider_scope_unavailable",
    "provider_busy",
    "provider_runner_unavailable",
    "provider_read_unavailable",
    "provider_operation_conflict",
    "provider_state_invalid",
    "provider_state_unavailable",
    "diff_scope_unavailable",
    "diff_busy",
    "diff_version_mismatch",
    "diff_boundary_unavailable",
    "diff_process_unavailable",
    "diff_timeout",
    "diff_output_limit",
    "diff_provider_drift",
    "diff_invalid",
  ],
});
const HTTP_STATUS = new Map([
  ...["idempotency_key_invalid", "malformed_json"].map((code) => [code, 400]),
  ["authentication_required", 401],
  ["origin_forbidden", 403],
  ...[
    "instance_changed",
    "idempotency_conflict",
    "login_already_running",
    "already_logged_in",
    "login_failed",
    "login_outcome_unknown",
    "turn_already_running",
    "no_current_turn",
    "session_wrong_state",
    "session_changed",
    "operation_changed",
    "history_gap",
    "future_cursor",
    "provider_state_conflict",
    "provider_turn_running",
    "no_current_operation",
    "provider_wrong_state",
    "recovery_decision_stale",
    "provider_operation_conflict",
    "provider_outcome_unknown",
    "provider_recovery_required",
    "diff_not_ready",
    "diff_authority_changed",
    "diff_canceled",
  ].map((code) => [code, 409]),
  ["request_too_large", 413],
  ["unsupported_media_type", 415],
  ...[
    "body_not_empty",
    "invalid_request",
    "invalid_value",
    "acknowledgement_required",
    "task_not_listed",
  ].map((code) => [code, 422]),
  ["session_limit", 429],
  ...[
    "idempotency_unavailable",
    "service_unavailable",
    "login_unavailable",
    "login_version_mismatch",
    "login_provider_drift",
    "login_output_limit",
    "login_status_unavailable",
    "login_process_unavailable",
    "login_state_unavailable",
    "login_state_invalid",
    "session_config_invalid",
    "session_stopped",
    "subscriber_limit",
    "session_sequence_exhausted",
    "provider_scope_unavailable",
    "provider_busy",
    "provider_runner_unavailable",
    "provider_read_unavailable",
    "provider_state_invalid",
    "provider_state_unavailable",
    "diff_scope_unavailable",
    "diff_busy",
    "diff_version_mismatch",
    "diff_boundary_unavailable",
    "diff_process_unavailable",
    "diff_output_limit",
    "diff_provider_drift",
    "diff_invalid",
  ].map((code) => [code, 503]),
  ["diff_timeout", 504],
]);
const OPERATION_ERROR_CODES = new Set([
  "provider_scope_unavailable",
  "provider_busy",
  "provider_turn_running",
  "no_current_operation",
  "provider_wrong_state",
  "recovery_decision_stale",
  "task_not_listed",
  "acknowledgement_required",
  "provider_runner_unavailable",
  "provider_read_unavailable",
  "provider_operation_conflict",
  "provider_outcome_unknown",
  "provider_state_invalid",
  "provider_state_unavailable",
  "provider_recovery_required",
]);

class MemoryStorage {
  constructor(initial = {}, options = {}) {
    this.values = new Map(Object.entries(initial));
    this.failGet = options.failGet ?? false;
    this.failSet = options.failSet ?? false;
    this.failRemove = options.failRemove ?? false;
  }

  getItem(key) {
    if (this.failGet) {
      throw new Error("storage canary");
    }
    return this.values.has(key) ? this.values.get(key) : null;
  }

  setItem(key, value) {
    if (this.failSet) {
      throw new Error("storage canary");
    }
    this.values.set(key, value);
  }

  removeItem(key) {
    if (this.failRemove) {
      throw new Error("storage canary");
    }
    this.values.delete(key);
  }
}

class FakeTimers {
  constructor() {
    this.nextId = 1;
    this.tasks = new Map();
  }

  setTimeout(callback, delay) {
    const id = this.nextId;
    this.nextId += 1;
    this.tasks.set(id, { callback, delay });
    return id;
  }

  clearTimeout(id) {
    this.tasks.delete(id);
  }

  delays() {
    return [...this.tasks.values()].map(({ delay }) => delay);
  }

  async runNext() {
    const next = [...this.tasks.entries()].sort(
      ([leftId, left], [rightId, right]) => left.delay - right.delay || leftId - rightId,
    )[0];
    assert.ok(next, "expected a pending timer");
    const [id, task] = next;
    this.tasks.delete(id);
    await task.callback();
  }

  async runDelay(delay) {
    const next = [...this.tasks.entries()].find(([, task]) => task.delay === delay);
    assert.ok(next, `expected ${delay}ms timer`);
    const [id, task] = next;
    this.tasks.delete(id);
    await task.callback();
  }
}

class FakeSocket {
  constructor(url) {
    this.url = url;
    this.sent = [];
    this.closeCalls = [];
    this.onopen = null;
    this.onmessage = null;
    this.onerror = null;
    this.onclose = null;
  }

  open() {
    this.onopen?.();
  }

  send(value) {
    this.sent.push(value);
  }

  message(value) {
    this.onmessage?.({ data: value });
  }

  close(code, reason) {
    this.closeCalls.push({ code, reason });
  }

  peerClose(code = 1006, reason = "SECRET CLOSE CANARY") {
    this.onclose?.({ code, reason });
  }
}

function jsonResponse(value, status = 200, extraHeaders = {}) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": JSON_TYPE, ...extraHeaders },
  });
}

function bytesResponse(chunks, options = {}) {
  let canceled = false;
  let index = 0;
  const stream = new ReadableStream(
    {
      pull(controller) {
        if (index === chunks.length) {
          controller.close();
          return;
        }
        const chunk = chunks[index];
        index += 1;
        if (chunk instanceof Error) {
          controller.error(chunk);
        } else {
          controller.enqueue(chunk);
        }
      },
      cancel() {
        canceled = true;
      },
    },
    { highWaterMark: 0 },
  );
  const response = new Response(stream, {
    status: options.status ?? 200,
    headers: {
      "content-type": options.contentType ?? DIFF_TYPE,
      ...(options.contentLength === undefined
        ? {}
        : { "content-length": String(options.contentLength) }),
    },
  });
  return { response, wasCanceled: () => canceled };
}

function errorResponse(
  code,
  message = "UNTRUSTED ERROR CANARY",
  status = 400,
  operationId,
) {
  return jsonResponse(
    {
      error: {
        code,
        message,
        ...(operationId === undefined ? {} : { operation_id: operationId }),
      },
    },
    status,
  );
}

function snapshot(highWater = 0, options = {}) {
  return {
    identity: {
      session_id: options.sessionId ?? SESSION_ID,
      instance_id: options.instanceId ?? INSTANCE_ID,
    },
    state: options.state ?? "ready",
    current_turn: options.currentTurn ?? null,
    high_water_seq: highWater,
  };
}

function cloudLifecycle(state, operationId = OPERATION_ID) {
  if (
    state === "submitting" ||
    state === "failed_before_submit" ||
    state === "outcome_unknown" ||
    state === "abandoned_unknown"
  ) {
    return { state, operation_id: operationId };
  }
  if (state === "canceled_locally") {
    return {
      state,
      operation_id: operationId,
      task_id: null,
      provider_may_continue: true,
    };
  }
  return { state, operation_id: operationId, task_id: "task_1" };
}

function currentTurn(projection) {
  return { turn_id: TURN_ID, projection };
}

function bootstrap() {
  return {
    actor: "operator",
    expires_in_seconds: 300,
    p0_session_id: SESSION_ID,
    instance_id: INSTANCE_ID,
  };
}

function deviceLogin() {
  return {
    operation_id: OPERATION_ID,
    verification_url: "https://auth.openai.com/codex/device",
    verification_code: "AB12-CDE34",
    expires_in_seconds: 900,
  };
}

function turnReceipt(highWater = 0) {
  return { turn_id: TURN_ID, high_water_seq: highWater };
}

function envelope(seq, type = "turn_accepted", options = {}) {
  return {
    schema_version: 1,
    session_id: options.sessionId ?? SESSION_ID,
    seq,
    turn_id: options.turnId ?? TURN_ID,
    payload: options.payload ?? { type },
  };
}

function frame(value) {
  return JSON.stringify(value);
}

function cursor(sessionId = SESSION_ID, eventSeq = 0, instanceId = INSTANCE_ID) {
  return JSON.stringify({
    schema_version: 1,
    instance_id: instanceId,
    session_id: sessionId,
    event_seq: eventSeq,
  });
}

function makeHarness(options = {}) {
  const queue = [];
  const requests = [];
  const sockets = [];
  const states = [];
  const timers = options.timers ?? new FakeTimers();
  const storage = options.storage ?? new MemoryStorage();
  let uuidCounter = 0;

  async function fetchRequest(path, init) {
    requests.push({ path, init });
    assert.ok(queue.length > 0, `unexpected request ${init.method} ${path}`);
    const action = queue.shift();
    if (action instanceof Error) {
      throw action;
    }
    return typeof action === "function" ? action(path, init) : action;
  }

  let controller;
  controller = createP0Controller({
    fetch: fetchRequest,
    createWebSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    randomUUID: () => {
      uuidCounter += 1;
      return uuidCounter === 1
        ? KEY
        : `22222222-2222-4222-8222-${String(uuidCounter).padStart(12, "0")}`;
    },
    storage,
    setTimeout: timers.setTimeout.bind(timers),
    clearTimeout: timers.clearTimeout.bind(timers),
    location: { origin: "https://operator.example" },
    AbortController,
    TextEncoder,
    TextDecoder,
    publish: (state) => {
      states.push(state);
      if (controller) {
        options.onPublish?.(state, controller);
      }
    },
  });

  return {
    controller,
    queue,
    requests,
    sockets,
    states,
    timers,
    storage,
    state: () => states.at(-1),
  };
}

async function load(harness, acceptedSnapshot = snapshot(), login = { state: "logged_out" }) {
  harness.queue.push(jsonResponse(acceptedSnapshot), jsonResponse(login));
  assert.equal(await harness.controller.load(), true);
  assert.equal(harness.queue.length, 0);
  return harness.sockets.at(-1);
}

function requestHeaders(request) {
  return new Headers(request.init.headers);
}

function assertMutationRequest(request, path, method = "POST") {
  assert.equal(request.path, path);
  assert.equal(request.init.method, method);
  assert.equal(request.init.cache, "no-store");
  assert.equal(request.init.credentials, "same-origin");
  assert.equal(request.init.mode, "same-origin");
  assert.equal(request.init.redirect, "error");
  assert.equal(request.init.referrerPolicy, "no-referrer");
  assert.equal(requestHeaders(request).get("codebox-instance-id"), INSTANCE_ID);
  assert.match(
    requestHeaders(request).get("idempotency-key"),
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i,
  );
}

function openStream(socket, afterSeq, acceptedSnapshot = snapshot(afterSeq)) {
  assert.equal(socket.url, `wss://operator.example${ROUTES.stream}`);
  socket.open();
  assert.deepEqual(JSON.parse(socket.sent[0]), {
    type: "subscribe",
    protocol_version: 1,
    session_id: SESSION_ID,
    after_seq: afterSeq,
  });
  socket.message(
    frame({
      type: "replay_begin",
      session_id: SESSION_ID,
      after_seq: afterSeq,
      high_water_seq: acceptedSnapshot.high_water_seq,
    }),
  );
  socket.message(
    frame({
      type: "snapshot",
      snapshot: acceptedSnapshot,
      high_water_seq: acceptedSnapshot.high_water_seq,
    }),
  );
  socket.message(
    frame({
      type: "replay_end",
      session_id: SESSION_ID,
      high_water_seq: acceptedSnapshot.high_water_seq,
    }),
  );
}

test("p0_web_bootstrap_token_is_ephemeral_and_never_persisted", async () => {
  const harness = makeHarness();
  const token = "é".repeat(16);
  harness.queue.push(
    jsonResponse(bootstrap(), 201),
    jsonResponse(snapshot()),
    jsonResponse({ state: "logged_out" }),
  );

  assert.equal(await harness.controller.authenticate(token), true);
  assert.equal(harness.requests.filter(({ path }) => path === ROUTES.bootstrap).length, 1);
  const authorization = requestHeaders(harness.requests[0]).get("authorization");
  assert.ok(authorization.startsWith("Bearer "));
  assert.deepEqual(
    [...authorization.slice(7)].map((character) => character.charCodeAt(0)),
    [...new TextEncoder().encode(token)],
  );
  assert.equal(harness.requests[0].path, ROUTES.bootstrap);
  assert.equal(harness.requests[0].init.method, "POST");
  assert.equal(harness.requests[0].init.redirect, "error");
  assert.ok(!JSON.stringify([...harness.storage.values]).includes(token));
  assert.ok(!JSON.stringify(harness.states).includes(token));
  assert.ok(!harness.sockets[0].url.includes(token));

  for (const invalid of [
    "short",
    "x".repeat(31),
    "x".repeat(129),
    `${"x".repeat(31)}\u007f`,
    `${"x".repeat(31)}\ud800`,
  ]) {
    assert.equal(await harness.controller.authenticate(invalid), false);
  }
  assert.equal(harness.requests.filter(({ path }) => path === ROUTES.bootstrap).length, 1);

  const wrongStatus = makeHarness();
  wrongStatus.queue.push(
    jsonResponse(bootstrap(), 200),
    errorResponse("authentication_required", "SECRET"),
  );
  assert.equal(await wrongStatus.controller.authenticate("x".repeat(32)), false);
  assert.equal(
    wrongStatus.requests.filter(({ path }) => path === ROUTES.bootstrap).length,
    1,
  );
  assert.ok(!JSON.stringify(wrongStatus.states).includes("SECRET"));
});

test("p0_web_login_status_and_device_actions_use_exact_api_contract", async () => {
  const harness = makeHarness();
  await load(harness, snapshot(), { state: "logged_out" });
  assert.deepEqual(
    harness.requests.slice(0, 2).map(({ path, init }) => [init.method, path]),
    [
      ["GET", ROUTES.session],
      ["GET", ROUTES.login],
    ],
  );

  harness.queue.push(jsonResponse(deviceLogin(), 202));
  assert.equal(await harness.controller.startDeviceLogin(), true);
  const start = harness.requests.at(-1);
  assertMutationRequest(start, ROUTES.loginDevice);
  assert.equal(start.init.body, undefined);
  assert.equal(requestHeaders(start).has("content-type"), false);
  assert.equal(harness.state().device.verificationCode, "AB12-CDE34");

  harness.queue.push(jsonResponse({ state: "logged_out" }));
  assert.equal(await harness.controller.cancelDeviceLogin(), true);
  const cancel = harness.requests.at(-1);
  assertMutationRequest(cancel, ROUTES.loginCancel);
  assert.equal(harness.state().device, null);

  harness.queue.push(
    jsonResponse(deviceLogin(), 200),
    jsonResponse(snapshot()),
    jsonResponse({ state: "logged_out" }),
  );
  assert.equal(await harness.controller.startDeviceLogin(), false);
  assert.equal(
    harness.requests.filter(({ path }) => path === ROUTES.loginDevice).length,
    2,
  );
});

test("p0_web_prompt_submission_requires_one_explicit_operator_action", async () => {
  const harness = makeHarness();
  await load(harness);
  const before = harness.requests.length;
  for (const invalid of [
    "",
    "-",
    " \n\t ",
    "unsafe\u0000prompt",
    "unsafe\u007fprompt",
    "x".repeat(32 * 1024 + 1),
  ]) {
    assert.equal(await harness.controller.submitPrompt(invalid), false);
  }
  assert.equal(harness.requests.length, before);

  const prompt = "One explicit operation ✅";
  harness.queue.push(jsonResponse(turnReceipt(), 202));
  assert.equal(await harness.controller.submitPrompt(prompt), true);
  const accepted = harness.requests.at(-1);
  assertMutationRequest(accepted, ROUTES.turns);
  assert.equal(requestHeaders(accepted).get("content-type"), JSON_TYPE);
  assert.equal(accepted.init.body, JSON.stringify({ prompt }));
  assert.equal(harness.requests.filter(({ path }) => path === ROUTES.turns).length, 1);

  harness.queue.push(
    jsonResponse(turnReceipt(), 200),
    jsonResponse(snapshot()),
    jsonResponse({ state: "logged_out" }),
  );
  assert.equal(await harness.controller.submitPrompt("second action"), false);
  assert.equal(harness.requests.filter(({ path }) => path === ROUTES.turns).length, 2);
});

test("p0_web_stream_replays_and_reconnects_from_validated_cursor", async () => {
  const storage = new MemoryStorage({ [CURSOR_KEY]: cursor(SESSION_ID, 2) });
  const harness = makeHarness({ storage });
  const first = await load(harness, snapshot(4));
  first.open();
  assert.equal(JSON.parse(first.sent[0]).after_seq, 2);
  first.message(
    frame({
      type: "replay_begin",
      session_id: SESSION_ID,
      after_seq: 2,
      high_water_seq: 4,
    }),
  );
  first.message(frame({ type: "event", envelope: envelope(2) }));
  first.message(frame({ type: "event", envelope: envelope(3) }));
  first.message(frame({ type: "event", envelope: envelope(4, "lifecycle_changed", {
    payload: {
      type: "lifecycle_changed",
      lifecycle: { state: "pending", operation_id: OPERATION_ID, task_id: "task_1" },
    },
  }) }));
  first.message(frame({ type: "snapshot", snapshot: snapshot(4), high_water_seq: 4 }));
  first.message(
    frame({ type: "replay_end", session_id: SESSION_ID, high_water_seq: 4 }),
  );
  first.message(frame({ type: "event", envelope: envelope(5) }));
  assert.equal(harness.state().connection, "connected");
  assert.deepEqual(harness.state().events.map(({ seq }) => seq), [3, 4, 5]);
  assert.equal(JSON.parse(storage.getItem(CURSOR_KEY)).event_seq, 5);

  first.message(frame({ type: "event", envelope: envelope(7) }));
  assert.deepEqual(first.closeCalls.at(-1), { code: 1008, reason: "" });
  first.peerClose(1008);
  assert.deepEqual(harness.timers.delays(), [250]);
  harness.queue.push(jsonResponse(snapshot(7)), jsonResponse({ state: "logged_out" }));
  await harness.timers.runNext();
  const second = harness.sockets.at(-1);
  second.open();
  assert.equal(JSON.parse(second.sent[0]).after_seq, 7);
  second.message(
    frame({
      type: "error",
      code: "history_gap",
      message: "SECRET HISTORY CANARY",
      oldest_available: 9,
      latest_available: 10,
    }),
  );
  assert.deepEqual(second.closeCalls.at(-1), { code: 1008, reason: "" });
  second.peerClose(1008);
  assert.deepEqual(harness.timers.delays(), [500]);
  harness.queue.push(jsonResponse(snapshot(10)), jsonResponse({ state: "logged_out" }));
  await harness.timers.runNext();
  const third = harness.sockets.at(-1);
  third.open();
  assert.equal(JSON.parse(third.sent[0]).after_seq, 10);
  assert.ok(!JSON.stringify(harness.states).includes("SECRET HISTORY CANARY"));

  const exactFrameHarness = makeHarness();
  const exactFrameSocket = await load(exactFrameHarness);
  exactFrameSocket.open();
  const emptyError = frame({
    type: "error",
    code: "stream_unavailable",
    message: "",
  });
  const exactFrame = frame({
    type: "error",
    code: "stream_unavailable",
    message: "x".repeat(JSON_LIMIT - emptyError.length),
  });
  assert.equal(new TextEncoder().encode(exactFrame).length, JSON_LIMIT);
  exactFrameSocket.message(exactFrame);
  assert.equal(exactFrameHarness.state().error, "Session stream is reconnecting.");

  const failingStorage = new MemoryStorage({}, { failSet: true });
  const storageHarness = makeHarness({ storage: failingStorage });
  const storageSocket = await load(storageHarness, snapshot(3));
  openStream(storageSocket, 3, snapshot(3));
  storageSocket.peerClose();
  storageHarness.queue.push(
    jsonResponse(snapshot(4)),
    jsonResponse({ state: "logged_out" }),
  );
  await storageHarness.timers.runNext();
  storageHarness.sockets.at(-1).open();
  assert.equal(JSON.parse(storageHarness.sockets.at(-1).sent[0]).after_seq, 4);

  const cloudStates = [
    "submitting",
    "failed_before_submit",
    "outcome_unknown",
    "pending",
    "ready",
    "applied",
    "provider_error",
    "canceled_locally",
    "abandoned_unknown",
  ];
  const projectionCases = [
    ["queued", { phase: "queued" }],
    ["starting", { phase: "starting", cancel_requested: false }],
    ["canceled", { phase: "canceled_before_cloud_start" }],
    ["stopped_before", { phase: "stopped_before_cloud_start" }],
    ["stopped_after", { phase: "stopped_after_lower_failure" }],
    [
      "degraded",
      {
        phase: "monitoring_degraded",
        operation_id: OPERATION_ID,
        last_known_pending: cloudLifecycle("pending"),
        cancel_requested: false,
      },
    ],
    [
      "degraded_mismatch",
      {
        phase: "monitoring_degraded",
        operation_id: OTHER_SESSION_ID,
        last_known_pending: cloudLifecycle("pending"),
        cancel_requested: false,
      },
    ],
    ...cloudStates.map((state) => [
      `cloud_${state}`,
      {
        phase: "cloud",
        lifecycle: cloudLifecycle(state),
        cancel_requested: false,
      },
    ]),
  ];
  const readyCloud = new Set([
    "failed_before_submit",
    "ready",
    "applied",
    "provider_error",
    "canceled_locally",
    "abandoned_unknown",
  ]);
  function combinationIsReachable(state, name) {
    if (name === "absent") {
      return state === "ready" || state === "stopped";
    }
    if (state === "ready") {
      return (
        name === "canceled" ||
        (name.startsWith("cloud_") && readyCloud.has(name.slice(6)))
      );
    }
    if (state === "running") {
      return (
        name === "queued" ||
        name === "starting" ||
        name === "cloud_submitting" ||
        name === "cloud_pending"
      );
    }
    if (state === "recovery_required") {
      return name === "cloud_outcome_unknown";
    }
    if (state === "monitoring_degraded") {
      return name === "degraded";
    }
    return (
      state === "stopped" &&
      (name === "canceled" ||
        name === "stopped_before" ||
        name === "stopped_after" ||
        name === "degraded" ||
        name.startsWith("cloud_"))
    );
  }

  const semanticMatrix = makeHarness();
  await load(semanticMatrix);
  for (const state of [
    "ready",
    "running",
    "recovery_required",
    "monitoring_degraded",
    "stopped",
  ]) {
    for (const [name, projection] of [
      ["absent", null],
      ...projectionCases,
    ]) {
      const reachable = combinationIsReachable(state, name);
      semanticMatrix.queue.push(
        jsonResponse(
          snapshot(0, {
            state,
            currentTurn: projection === null ? null : currentTurn(projection),
          }),
        ),
      );
      if (reachable) {
        semanticMatrix.queue.push(jsonResponse({ state: "logged_out" }));
      }
      assert.equal(
        await semanticMatrix.controller.refresh(),
        reachable,
        `${state}/${name}`,
      );
    }
  }

  const impossibleSnapshot = makeHarness();
  const impossibleSocket = await load(impossibleSnapshot);
  impossibleSocket.open();
  impossibleSocket.message(
    frame({
      type: "replay_begin",
      session_id: SESSION_ID,
      after_seq: 0,
      high_water_seq: 0,
    }),
  );
  impossibleSocket.message(
    frame({
      type: "snapshot",
      snapshot: snapshot(0, {
        state: "ready",
        currentTurn: currentTurn({ phase: "queued" }),
      }),
      high_water_seq: 0,
    }),
  );
  assert.deepEqual(impossibleSocket.closeCalls.at(-1), { code: 1008, reason: "" });
  assert.equal(impossibleSnapshot.state().error, FIXED.protocol);

  for (const invalid of [
    new Uint8Array([1]),
    "{",
    frame({ type: "unknown" }),
    frame({ type: "event", envelope: envelope(1) }),
    frame({ type: "snapshot", snapshot: snapshot(), high_water_seq: 0 }),
    frame({
      type: "replay_begin",
      session_id: SESSION_ID,
      after_seq: 0,
      high_water_seq: 0,
      unknown: true,
    }),
    "x".repeat(JSON_LIMIT + 1),
  ]) {
    const invalidHarness = makeHarness();
    const invalidSocket = await load(invalidHarness);
    invalidSocket.open();
    invalidSocket.message(invalid);
    assert.deepEqual(invalidSocket.closeCalls.at(-1), { code: 1008, reason: "" });
    assert.equal(invalidHarness.state().error, FIXED.protocol);
  }
});

test("p0_web_cancel_is_explicit_and_disconnect_never_cancels", async () => {
  const harness = makeHarness();
  const socket = await load(harness);
  socket.open();
  socket.peerClose();
  assert.equal(harness.requests.filter(({ path }) => path === ROUTES.turnCancel).length, 0);

  harness.queue.push(jsonResponse(snapshot(), 200));
  assert.equal(await harness.controller.cancelTurn(), true);
  assertMutationRequest(harness.requests.at(-1), ROUTES.turnCancel);
  assert.equal(harness.requests.filter(({ path }) => path === ROUTES.turnCancel).length, 1);
  harness.controller.dispose();
  assert.equal(harness.requests.filter(({ path }) => path === ROUTES.turnCancel).length, 1);

  const ambiguous = makeHarness();
  await load(ambiguous);
  ambiguous.queue.push(
    new Error("SECRET NETWORK CANARY"),
    jsonResponse(snapshot()),
    jsonResponse({ state: "logged_out" }),
  );
  assert.equal(await ambiguous.controller.cancelTurn(), false);
  assert.equal(
    ambiguous.requests.filter(({ path }) => path === ROUTES.turnCancel).length,
    1,
  );
  assert.ok(!JSON.stringify(ambiguous.states).includes("SECRET NETWORK CANARY"));
});

test("p0_web_diff_is_bounded_text_and_never_html", async () => {
  const harness = makeHarness();
  await load(harness);
  const canary = "diff --git a/x b/x\n+<script>DIFF SECRET</script>\n";
  harness.queue.push(new Response(canary, { headers: { "content-type": DIFF_TYPE } }));
  assert.equal(await harness.controller.showDiff(), true);
  assert.equal(harness.state().diff, canary);

  const exact = bytesResponse([
    new Uint8Array(DIFF_LIMIT / 2).fill(97),
    new Uint8Array(DIFF_LIMIT / 2).fill(98),
  ]);
  harness.queue.push(exact.response);
  assert.equal(await harness.controller.showDiff(), true);
  assert.equal(harness.state().diff.length, DIFF_LIMIT);

  const overflow = bytesResponse([
    new Uint8Array(DIFF_LIMIT).fill(97),
    new Uint8Array([98]),
  ]);
  harness.queue.push(overflow.response);
  assert.equal(await harness.controller.showDiff(), false);
  assert.equal(harness.state().diff, "");
  assert.equal(harness.state().error, FIXED.diffLimit);
  assert.equal(overflow.wasCanceled(), true);

  const contentLength = bytesResponse([new Uint8Array([97])], {
    contentLength: DIFF_LIMIT + 1,
  });
  harness.queue.push(contentLength.response);
  assert.equal(await harness.controller.showDiff(), false);
  assert.equal(harness.state().error, FIXED.diffLimit);

  const invalidUtf8 = bytesResponse([new Uint8Array([0xff])]);
  harness.queue.push(invalidUtf8.response);
  assert.equal(await harness.controller.showDiff(), false);
  assert.equal(harness.state().error, FIXED.request);

  const failedChunk = bytesResponse([
    new TextEncoder().encode("partial secret"),
    new Error("SECRET BODY CANARY"),
  ]);
  harness.queue.push(failedChunk.response);
  assert.equal(await harness.controller.showDiff(), false);
  assert.equal(harness.state().diff, "");

  harness.queue.push(
    new Response("plain", { headers: { "content-type": "text/plain" } }),
    new Response("plain", { status: 201, headers: { "content-type": DIFF_TYPE } }),
  );
  assert.equal(await harness.controller.showDiff(), false);
  assert.equal(await harness.controller.showDiff(), false);
  assert.ok(!JSON.stringify(harness.states).includes("SECRET BODY CANARY"));
});

test("p0_web_refresh_rehydrates_identity_without_replaying_mutations", async () => {
  const harness = makeHarness();
  const first = await load(harness, snapshot(5));
  first.open();
  assert.equal(JSON.parse(first.sent[0]).after_seq, 5);
  assert.deepEqual(
    harness.requests.map(({ path, init }) => [init.method, path]),
    [
      ["GET", ROUTES.session],
      ["GET", ROUTES.login],
    ],
  );

  harness.queue.push(jsonResponse(snapshot(5)), jsonResponse({ state: "logged_out" }));
  assert.equal(await harness.controller.refresh(), true);
  harness.sockets.at(-1).open();
  assert.equal(JSON.parse(harness.sockets.at(-1).sent[0]).after_seq, 5);
  assert.equal(
    harness.requests.filter(({ init }) => init.method !== "GET").length,
    0,
  );

  for (const stored of [
    cursor(OTHER_SESSION_ID, 2),
    cursor(SESSION_ID, 99),
    "{",
  ]) {
    const partition = makeHarness({
      storage: new MemoryStorage({ [CURSOR_KEY]: stored }),
    });
    const socket = await load(partition, snapshot(8));
    socket.open();
    assert.equal(JSON.parse(socket.sent[0]).after_seq, 8);
  }

  harness.queue.push(errorResponse("authentication_required", "COOKIE SECRET", 401));
  assert.equal(await harness.controller.refresh(), false);
  assert.equal(harness.state().authenticated, false);
  assert.equal(harness.storage.getItem(CURSOR_KEY), null);
  assert.equal(harness.state().error, FIXED.authentication);
  assert.ok(!JSON.stringify(harness.states).includes("COOKIE SECRET"));
});

test("p0_web_errors_and_diagnostics_exclude_sensitive_canaries", async () => {
  const expectedMessages = {
    authentication: FIXED.authentication,
    resync: FIXED.resync,
    recovery: FIXED.recovery,
    rejected: FIXED.rejected,
    service: FIXED.service,
  };
  const groupedCodes = new Set(Object.values(HTTP_GROUPS).flat());
  assert.deepEqual(groupedCodes, new Set(HTTP_STATUS.keys()));
  for (const [group, codes] of Object.entries(HTTP_GROUPS)) {
    for (const code of codes) {
      const status = HTTP_STATUS.get(code);
      const harness = makeHarness();
      await load(harness);
      harness.queue.push(errorResponse(code, "UNTRUSTED ERROR CANARY", status));
      if (group === "resync") {
        harness.queue.push(jsonResponse(snapshot()), jsonResponse({ state: "logged_out" }));
      }
      assert.equal(await harness.controller.showDiff(), false, code);
      assert.equal(harness.state().error, expectedMessages[group], code);
      assert.ok(!JSON.stringify(harness.states).includes("UNTRUSTED ERROR CANARY"), code);

      const wrongStatus = makeHarness();
      await load(wrongStatus);
      wrongStatus.queue.push(
        errorResponse(
          code,
          "WRONG STATUS SECRET",
          status === 400 ? 409 : 400,
        ),
      );
      assert.equal(await wrongStatus.controller.showDiff(), false, code);
      assert.equal(wrongStatus.state().error, FIXED.request, code);
      assert.ok(!JSON.stringify(wrongStatus.states).includes("WRONG STATUS SECRET"), code);

      const withOperation = makeHarness();
      await load(withOperation);
      withOperation.queue.push(
        errorResponse(code, "OPERATION FIELD SECRET", status, OPERATION_ID),
      );
      assert.equal(await withOperation.controller.showDiff(), false, code);
      assert.equal(
        withOperation.state().error,
        OPERATION_ERROR_CODES.has(code) ? expectedMessages[group] : FIXED.request,
        code,
      );
      assert.ok(!JSON.stringify(withOperation.states).includes("OPERATION FIELD SECRET"), code);
    }
  }

  const invalidOperation = makeHarness();
  await load(invalidOperation);
  invalidOperation.queue.push(
    errorResponse(
      "provider_busy",
      "INVALID OPERATION SECRET",
      503,
      "00000000-0000-0000-0000-000000000000",
    ),
  );
  assert.equal(await invalidOperation.controller.showDiff(), false);
  assert.equal(invalidOperation.state().error, FIXED.request);
  assert.ok(!JSON.stringify(invalidOperation.states).includes("INVALID OPERATION SECRET"));

  const jsonPrefix = '{"error":{"code":"diff_not_ready","message":"';
  const jsonSuffix = '"}}';
  const exactJsonBytes = new TextEncoder().encode(
    `${jsonPrefix}${"x".repeat(JSON_LIMIT - jsonPrefix.length - jsonSuffix.length)}${jsonSuffix}`,
  );
  assert.equal(exactJsonBytes.byteLength, JSON_LIMIT);
  const jsonBounds = makeHarness();
  await load(jsonBounds);
  const exactJson = bytesResponse(
    [exactJsonBytes.slice(0, 123), exactJsonBytes.slice(123)],
    { status: 409, contentType: JSON_TYPE },
  );
  jsonBounds.queue.push(exactJson.response);
  assert.equal(await jsonBounds.controller.showDiff(), false);
  assert.equal(jsonBounds.state().error, FIXED.rejected);

  const overJsonBytes = new TextEncoder().encode(
    `${jsonPrefix}${"x".repeat(JSON_LIMIT + 1 - jsonPrefix.length - jsonSuffix.length)}${jsonSuffix}`,
  );
  const overJson = bytesResponse(
    [overJsonBytes.slice(0, JSON_LIMIT), overJsonBytes.slice(JSON_LIMIT)],
    { status: 409, contentType: JSON_TYPE },
  );
  jsonBounds.queue.push(overJson.response);
  assert.equal(await jsonBounds.controller.showDiff(), false);
  assert.equal(jsonBounds.state().error, FIXED.responseLimit);
  assert.equal(overJson.wasCanceled(), true);

  const unicodeJsonBytes = new TextEncoder().encode(
    JSON.stringify({
      error: { code: "diff_not_ready", message: "é".repeat(33_000) },
    }),
  );
  assert.ok(new TextDecoder().decode(unicodeJsonBytes).length < JSON_LIMIT);
  assert.ok(unicodeJsonBytes.byteLength > JSON_LIMIT);
  const unicodeJson = bytesResponse([unicodeJsonBytes], {
    status: 409,
    contentType: JSON_TYPE,
  });
  jsonBounds.queue.push(unicodeJson.response);
  assert.equal(await jsonBounds.controller.showDiff(), false);
  assert.equal(jsonBounds.state().error, FIXED.responseLimit);

  const wsCases = [
    ["authentication_expired", {}, FIXED.authentication],
    ["subscribe_timeout", {}, "Session stream is reconnecting."],
    ["protocol_error", {}, FIXED.protocol],
    ["unsupported_version", { supported_version: 1 }, FIXED.protocol],
    ["wrong_session", {}, FIXED.resync],
    [
      "history_gap",
      { oldest_available: 2, latest_available: 3 },
      FIXED.resync,
    ],
    ["future_cursor", { latest_available: 3 }, FIXED.resync],
    ["subscriber_limit", {}, "Session stream is reconnecting."],
    ["subscriber_lagged", {}, "Session stream is reconnecting."],
    ["subscriber_lagged", { latest_available: 3 }, "Session stream is reconnecting."],
    ["stream_unavailable", {}, "Session stream is reconnecting."],
  ];
  for (const [code, details, expected] of wsCases) {
    const harness = makeHarness();
    const socket = await load(harness);
    socket.open();
    socket.message(
      frame({ type: "error", code, message: "WS SECRET CANARY", ...details }),
    );
    assert.equal(harness.state().error, expected, code);
    assert.ok(!JSON.stringify(harness.states).includes("WS SECRET CANARY"), code);
  }

  for (const status of [301, 302, 303, 307, 308]) {
    const harness = makeHarness();
    await load(harness);
    harness.queue.push(new Response(null, { status }));
    assert.equal(await harness.controller.showDiff(), false);
    assert.equal(harness.requests.at(-1).init.redirect, "error");
    assert.equal(harness.state().error, FIXED.request);
  }

  const unknown = makeHarness();
  await load(unknown);
  unknown.queue.push(errorResponse("SECRET_UNKNOWN_CODE", "SECRET UNKNOWN BODY"));
  assert.equal(await unknown.controller.showDiff(), false);
  assert.equal(unknown.state().error, FIXED.request);
  assert.ok(!JSON.stringify(unknown.states).includes("SECRET"));
});

class FakeElement {
  constructor() {
    this.textContent = "";
    this.value = "";
    this.hidden = false;
    this.disabled = false;
    this.listeners = new Map();
  }

  addEventListener(type, listener) {
    this.listeners.set(type, listener);
  }

  async dispatch(type) {
    const listener = this.listeners.get(type);
    assert.ok(listener, `missing ${type} listener`);
    await listener();
  }
}

test("p0_web_exposes_no_execution_provider_or_arbitrary_route_authority", async () => {
  const [clientSource, appSource, html] = await Promise.all([
    readFile(new URL("./p0-client.js", import.meta.url), "utf8"),
    readFile(new URL("./p0-app.js", import.meta.url), "utf8"),
    readFile(new URL("./index.html", import.meta.url), "utf8"),
  ]);
  const source = `${clientSource}\n${appSource}`;
  for (const forbidden of [
    "innerHTML",
    "outerHTML",
    "insertAdjacentHTML",
    "document.write",
    "eval(",
    "Function(",
    "import(",
    "Worker(",
    "postMessage",
    "console.",
    "sendBeacon",
    "clipboard",
    "/api/p0/v1/session/reconcile",
    "/api/p0/v1/session/resolve",
    "window.location.search",
    "window.location.hash",
  ]) {
    assert.equal(source.includes(forbidden), false, forbidden);
  }
  for (const forbiddenInput of ["repository", "provider", "execution", "path", "task-id"]) {
    assert.equal(html.includes(`id="${forbiddenInput}"`), false, forbiddenInput);
  }
  const routeLiterals = new Set(
    [...clientSource.matchAll(/"\/api\/p0\/v1\/[^"]+"/g)].map((match) =>
      match[0].slice(1, -1),
    ),
  );
  assert.deepEqual(routeLiterals, new Set(Object.values(ROUTES)));

  const ids = [
    "connection-status",
    "bootstrap-panel",
    "bootstrap-token",
    "authenticate",
    "operator-panel",
    "session-status",
    "refresh",
    "login-status",
    "start-login",
    "cancel-login",
    "verification-url",
    "verification-code",
    "prompt",
    "submit-turn",
    "cancel-turn",
    "stream-status",
    "event-list",
    "show-diff",
    "diff-output",
    "logout",
    "error-message",
  ];
  const elements = new Map(ids.map((id) => [id, new FakeElement()]));
  const storage = new MemoryStorage();
  const timers = new FakeTimers();
  const queue = [errorResponse("authentication_required", "INITIAL SECRET", 401)];
  const requests = [];
  const sockets = [];
  class BrowserSocket {
    constructor(url) {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    }
  }
  const environment = {
    document: { getElementById: (id) => elements.get(id) ?? null },
    async fetch(path, init) {
      requests.push({ path, init });
      return queue.shift();
    },
    WebSocket: BrowserSocket,
    crypto: { randomUUID: () => KEY },
    sessionStorage: storage,
    setTimeout: timers.setTimeout.bind(timers),
    clearTimeout: timers.clearTimeout.bind(timers),
    location: { origin: "https://operator.example" },
    AbortController,
    TextEncoder,
    TextDecoder,
    addEventListener() {},
  };
  const controller = mountP0App(environment);
  assert.ok(controller);
  await new Promise((resolve) => setImmediate(resolve));

  const token = "B".repeat(32);
  elements.get("bootstrap-token").value = token;
  queue.push(
    jsonResponse(bootstrap(), 201),
    jsonResponse(snapshot()),
    jsonResponse({ state: "logged_out" }),
  );
  await elements.get("authenticate").dispatch("click");
  assert.equal(elements.get("bootstrap-token").value, "");
  assert.ok(!JSON.stringify([...storage.values]).includes(token));

  const prompt = "PROMPT DOM SECRET";
  elements.get("prompt").value = prompt;
  queue.push(jsonResponse(turnReceipt(), 202));
  await elements.get("submit-turn").dispatch("click");
  assert.equal(elements.get("prompt").value, "");
  assert.equal(requests.at(-1).init.body, JSON.stringify({ prompt }));
  assert.equal(elements.get("diff-output").textContent, "No diff loaded.");
  assert.equal(elements.get("verification-url").textContent, "");
  controller.dispose();
});

test("p0_web_controller_model_preserves_generation_sequence_and_e0_boundaries", async () => {
  const actions = [
    {
      name: "authenticate",
      authenticated: false,
      method: "POST",
      path: ROUTES.bootstrap,
      success: () => [
        jsonResponse(bootstrap(), 201),
        jsonResponse(snapshot()),
        jsonResponse({ state: "logged_out" }),
      ],
      wrongSuccess: () => jsonResponse(bootstrap(), 200),
      invoke: (controller) => controller.authenticate("A".repeat(32)),
    },
    {
      name: "start-login",
      authenticated: true,
      method: "POST",
      path: ROUTES.loginDevice,
      success: () => [jsonResponse(deviceLogin(), 202)],
      wrongSuccess: () => jsonResponse(deviceLogin(), 200),
      invoke: (controller) => controller.startDeviceLogin(),
    },
    {
      name: "cancel-login",
      authenticated: true,
      method: "POST",
      path: ROUTES.loginCancel,
      success: () => [jsonResponse({ state: "logged_out" })],
      wrongSuccess: () => jsonResponse({ state: "logged_out" }, 202),
      invoke: (controller) => controller.cancelDeviceLogin(),
    },
    {
      name: "submit-turn",
      authenticated: true,
      method: "POST",
      path: ROUTES.turns,
      success: () => [jsonResponse(turnReceipt(), 202)],
      wrongSuccess: () => jsonResponse(turnReceipt(), 200),
      invoke: (controller) => controller.submitPrompt("explicit model prompt"),
    },
    {
      name: "cancel-turn",
      authenticated: true,
      method: "POST",
      path: ROUTES.turnCancel,
      success: () => [jsonResponse(snapshot(), 200)],
      wrongSuccess: () => jsonResponse(snapshot(), 202),
      invoke: (controller) => controller.cancelTurn(),
    },
    {
      name: "logout",
      authenticated: true,
      method: "DELETE",
      path: ROUTES.bootstrap,
      success: () => [new Response(null, { status: 204 })],
      wrongSuccess: () => new Response(null, { status: 205 }),
      invoke: (controller) => controller.logout(),
    },
  ];

  async function prepare(action) {
    const harness = makeHarness();
    if (action.authenticated) {
      await load(harness);
    }
    return harness;
  }

  function mutationLog(harness) {
    return harness.requests
      .filter(({ init }) => init.method === "POST" || init.method === "DELETE")
      .map(({ path, init }) => [init.method, path]);
  }

  for (const action of actions) {
    const success = await prepare(action);
    success.queue.push(...action.success());
    assert.equal(await action.invoke(success.controller), true, `${action.name}: success`);
    assert.deepEqual(mutationLog(success), [[action.method, action.path]]);

    const fixedError = await prepare(action);
    fixedError.queue.push(errorResponse("service_unavailable", "MODEL ERROR SECRET", 503));
    assert.equal(await action.invoke(fixedError.controller), false, `${action.name}: error`);
    assert.deepEqual(mutationLog(fixedError), [[action.method, action.path]]);
    assert.equal(fixedError.state().error, FIXED.service);

    const wrongSuccess = await prepare(action);
    wrongSuccess.queue.push(
      action.wrongSuccess(),
      jsonResponse(snapshot()),
      jsonResponse({ state: "logged_out" }),
    );
    assert.equal(
      await action.invoke(wrongSuccess.controller),
      false,
      `${action.name}: wrong success status`,
    );
    assert.deepEqual(mutationLog(wrongSuccess), [[action.method, action.path]]);
    assert.equal(wrongSuccess.state().error, FIXED.request);

    for (const outcome of ["network", "redirect"]) {
      const ambiguous = await prepare(action);
      ambiguous.queue.push(
        outcome === "network"
          ? new Error("MODEL NETWORK SECRET")
          : new Response(null, { status: 307 }),
        jsonResponse(snapshot()),
        jsonResponse({ state: "logged_out" }),
      );
      assert.equal(
        await action.invoke(ambiguous.controller),
        false,
        `${action.name}: ${outcome}`,
      );
      assert.deepEqual(mutationLog(ambiguous), [[action.method, action.path]]);
      assert.equal(ambiguous.state().error, FIXED.request);
    }

    for (const outcome of ["ambiguous-refresh", "resync-refresh"]) {
      let armed = false;
      let disposedDuringRefresh = false;
      let statesAfterRefreshDispose = 0;
      let refreshDispose;
      refreshDispose = makeHarness({
        onPublish(state, controller) {
          if (
            armed &&
            !disposedDuringRefresh &&
            !state.busy &&
            refreshDispose.requests.at(-1)?.path === ROUTES.login
          ) {
            disposedDuringRefresh = true;
            controller.dispose();
            statesAfterRefreshDispose = refreshDispose.states.length;
          }
        },
      });
      if (action.authenticated) {
        await load(refreshDispose);
      }
      refreshDispose.queue.push(
        outcome === "ambiguous-refresh"
          ? new Error("AUTO REFRESH SECRET")
          : errorResponse("instance_changed", "AUTO RESYNC SECRET", 409),
        jsonResponse(snapshot()),
        jsonResponse({ state: "logged_out" }),
      );
      armed = true;
      assert.equal(
        await action.invoke(refreshDispose.controller),
        false,
        `${action.name}: ${outcome}`,
      );
      assert.equal(disposedDuringRefresh, true, `${action.name}: ${outcome}`);
      assert.equal(refreshDispose.states.length, statesAfterRefreshDispose);
      assert.deepEqual(mutationLog(refreshDispose), [[action.method, action.path]]);
      assert.ok(!JSON.stringify(refreshDispose.states).includes("SECRET"));
    }

    const timeout = await prepare(action);
    timeout.queue.push(
      (_path, init) =>
        new Promise((_resolve, reject) => {
          init.signal.addEventListener("abort", () =>
            reject(new Error("MODEL TIMEOUT SECRET")),
          );
        }),
      jsonResponse(snapshot()),
      jsonResponse({ state: "logged_out" }),
    );
    const timedAction = action.invoke(timeout.controller);
    const overlapping =
      action.name === "authenticate"
        ? timeout.controller.submitPrompt("no identity")
        : timeout.controller.authenticate("B".repeat(32));
    assert.equal(await overlapping, false, `${action.name}: concurrent gesture`);
    await timeout.timers.runDelay(15_000);
    assert.equal(await timedAction, false, `${action.name}: timeout`);
    assert.deepEqual(mutationLog(timeout), [[action.method, action.path]]);

    const late = await prepare(action);
    let resolveLate;
    late.queue.push(
      () =>
        new Promise((resolve) => {
          resolveLate = resolve;
        }),
    );
    const lateAction = action.invoke(late.controller);
    late.controller.dispose();
    const statesAfterDispose = late.states.length;
    resolveLate(action.success()[0]);
    assert.equal(await lateAction, false, `${action.name}: disposed late success`);
    assert.equal(late.states.length, statesAfterDispose);
    assert.deepEqual(mutationLog(late), [[action.method, action.path]]);
  }

  let cancelRefreshArmed = false;
  let cancelRefreshDisposed = false;
  let cancelRefreshStates = 0;
  let cancelRefresh;
  cancelRefresh = makeHarness({
    onPublish(state, controller) {
      if (
        cancelRefreshArmed &&
        !cancelRefreshDisposed &&
        !state.busy &&
        cancelRefresh.requests.at(-1)?.path === ROUTES.login
      ) {
        cancelRefreshDisposed = true;
        controller.dispose();
        cancelRefreshStates = cancelRefresh.states.length;
      }
    },
  });
  await load(cancelRefresh);
  cancelRefresh.queue.push(
    jsonResponse(snapshot(0, { sessionId: OTHER_SESSION_ID })),
    jsonResponse(snapshot()),
    jsonResponse({ state: "logged_out" }),
  );
  cancelRefreshArmed = true;
  assert.equal(await cancelRefresh.controller.cancelTurn(), false);
  assert.equal(cancelRefreshDisposed, true);
  assert.equal(cancelRefresh.states.length, cancelRefreshStates);
  assert.deepEqual(mutationLog(cancelRefresh), [["POST", ROUTES.turnCancel]]);

  const lateRefresh = makeHarness();
  let resolveLogin;
  lateRefresh.queue.push(
    jsonResponse(snapshot()),
    () =>
      new Promise((resolve) => {
        resolveLogin = resolve;
      }),
  );
  const pendingRefresh = lateRefresh.controller.load();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(lateRefresh.requests.length, 2);
  lateRefresh.controller.dispose();
  const refreshStatesAfterDispose = lateRefresh.states.length;
  resolveLogin(jsonResponse({ state: "logged_in" }));
  assert.equal(await pendingRefresh, false);
  assert.equal(lateRefresh.states.length, refreshStatesAfterDispose);
  assert.equal(lateRefresh.sockets.length, 0);

  const frameSchedules = [
    (socket) => openStream(socket, 0),
    (socket) => {
      socket.open();
      socket.message(frame({ type: "event", envelope: envelope(1) }));
    },
    (socket) => {
      socket.open();
      socket.message(
        frame({
          type: "error",
          code: "authentication_expired",
          message: "MODEL FRAME SECRET",
        }),
      );
    },
  ];
  for (const applyFrames of frameSchedules) {
    const frames = makeHarness();
    const socket = await load(frames);
    applyFrames(socket);
    socket.peerClose();
    assert.deepEqual(mutationLog(frames), []);
    assert.ok(!JSON.stringify(frames.states).includes("MODEL FRAME SECRET"));
  }

  const staleSocket = makeHarness();
  const oldSocket = await load(staleSocket);
  staleSocket.queue.push(jsonResponse(snapshot()), jsonResponse({ state: "logged_out" }));
  assert.equal(await staleSocket.controller.refresh(), true);
  const stateBeforeStaleFrame = staleSocket.states.at(-1);
  oldSocket.message(frame({ type: "event", envelope: envelope(1) }));
  assert.deepEqual(staleSocket.states.at(-1), stateBeforeStaleFrame);
  assert.deepEqual(mutationLog(staleSocket), []);
});
