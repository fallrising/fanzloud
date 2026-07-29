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

const CURSOR_KEY = "codebox.p0.cursor.v1";
const JSON_LIMIT = 64 * 1024;
const DIFF_LIMIT = 2 * 1024 * 1024;
const PROMPT_LIMIT = 32 * 1024;
const BODY_LIMIT = 40 * 1024;
const EVENT_LIMIT = 256;
const HTTP_TIMEOUT_MS = 15_000;
const RECONNECT_DELAYS = Object.freeze([250, 500, 1_000, 2_000, 4_000, 5_000]);

const LOGIN_STATES = new Set([
  "logged_out",
  "device_login_pending",
  "logged_in",
  "outcome_unknown",
]);
const SESSION_STATES = new Set([
  "ready",
  "running",
  "recovery_required",
  "monitoring_degraded",
  "stopped",
]);
const EVENT_TYPES = new Set([
  "turn_accepted",
  "turn_canceled_before_cloud_start",
  "lifecycle_changed",
  "cancel_requested",
  "monitoring_degraded",
  "recovery_observed",
  "recovery_resolved",
  "runtime_stopped",
]);
const CLOUD_STATES = new Set([
  "failed_before_submit",
  "outcome_unknown",
  "pending",
  "ready",
  "applied",
  "provider_error",
  "canceled_locally",
  "abandoned_unknown",
]);
const READY_CLOUD_STATES = new Set([
  "failed_before_submit",
  "ready",
  "applied",
  "provider_error",
  "canceled_locally",
  "abandoned_unknown",
]);

const REAUTH_CODES = new Set(["authentication_required"]);
const RESYNC_CODES = new Set([
  "instance_changed",
  "session_changed",
  "history_gap",
  "future_cursor",
  "operation_changed",
  "diff_authority_changed",
]);
const RECOVERY_CODES = new Set([
  "login_outcome_unknown",
  "provider_outcome_unknown",
  "provider_recovery_required",
]);
const REJECTED_CODES = new Set([
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
]);
const SERVICE_CODES = new Set([
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
]);
const HTTP_CODES = new Set([
  ...REAUTH_CODES,
  ...RESYNC_CODES,
  ...RECOVERY_CODES,
  ...REJECTED_CODES,
  ...SERVICE_CODES,
]);
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
const WS_CODES = new Set([
  "authentication_expired",
  "subscribe_timeout",
  "protocol_error",
  "unsupported_version",
  "wrong_session",
  "history_gap",
  "future_cursor",
  "subscriber_limit",
  "subscriber_lagged",
  "stream_unavailable",
]);

const MESSAGES = Object.freeze({
  authentication_required: "Authentication is required.",
  state_changed: "Current state changed; status was refreshed.",
  recovery_required: "Outcome requires the trusted operator recovery procedure.",
  action_rejected: "The explicit action was rejected; refresh before deciding whether to act again.",
  service_failed: "The service could not complete the request; no command was retried.",
  request_failed: "The request could not be completed safely.",
  response_too_large: "The response exceeded its safe display limit.",
  diff_too_large: "The diff exceeded its safe display limit.",
  invalid_bootstrap: "The bootstrap bearer must contain 32–128 non-control UTF-8 bytes.",
  invalid_prompt: "Enter a valid prompt within the safe request limits.",
  unsupported: "This browser does not provide the required private operator primitives.",
  protocol_error: "The session stream returned an invalid frame.",
  stream_waiting: "Session stream is reconnecting.",
});

function exactKeys(value, expected) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  return actual.length === wanted.length && actual.every((key, index) => key === wanted[index]);
}

function isUuid(value) {
  return (
    typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value) &&
    value !== "00000000-0000-0000-0000-000000000000"
  );
}

function isUuidV4(value) {
  return (
    typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value) &&
    value !== "00000000-0000-0000-0000-000000000000"
  );
}

function isSeq(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function validCloudTaskId(value) {
  return typeof value === "string" && value.startsWith("task_") && value.length <= 128;
}

function validCloudLifecycle(value) {
  if (!value || !CLOUD_STATES.has(value.state) || !isUuid(value.operation_id)) {
    return false;
  }
  switch (value.state) {
    case "submitting":
    case "failed_before_submit":
    case "outcome_unknown":
    case "abandoned_unknown":
      return exactKeys(value, ["state", "operation_id"]);
    case "pending":
    case "ready":
    case "applied":
    case "provider_error":
      return exactKeys(value, ["state", "operation_id", "task_id"]) && validCloudTaskId(value.task_id);
    case "canceled_locally":
      return (
        exactKeys(value, ["state", "operation_id", "task_id", "provider_may_continue"]) &&
        (value.task_id === null || validCloudTaskId(value.task_id)) &&
        typeof value.provider_may_continue === "boolean"
      );
    default:
      return false;
  }
}

function validProjection(value) {
  if (!value || typeof value.phase !== "string") {
    return false;
  }
  switch (value.phase) {
    case "queued":
    case "canceled_before_cloud_start":
    case "stopped_before_cloud_start":
    case "stopped_after_lower_failure":
      return exactKeys(value, ["phase"]);
    case "starting":
      return exactKeys(value, ["phase", "cancel_requested"]) && typeof value.cancel_requested === "boolean";
    case "cloud":
      return (
        exactKeys(value, ["phase", "lifecycle", "cancel_requested"]) &&
        validCloudLifecycle(value.lifecycle) &&
        typeof value.cancel_requested === "boolean"
      );
    case "monitoring_degraded":
      return (
        exactKeys(value, [
          "phase",
          "operation_id",
          "last_known_pending",
          "cancel_requested",
        ]) &&
        isUuid(value.operation_id) &&
        validCloudLifecycle(value.last_known_pending) &&
        value.last_known_pending.state === "pending" &&
        value.operation_id === value.last_known_pending.operation_id &&
        typeof value.cancel_requested === "boolean"
      );
    default:
      return false;
  }
}

function validSnapshotCombination(state, currentTurn) {
  if (currentTurn === null) {
    return state === "ready" || state === "stopped";
  }
  const projection = currentTurn.projection;
  switch (state) {
    case "ready":
      return (
        projection.phase === "canceled_before_cloud_start" ||
        (projection.phase === "cloud" &&
          READY_CLOUD_STATES.has(projection.lifecycle.state))
      );
    case "running":
      return (
        projection.phase === "queued" ||
        projection.phase === "starting" ||
        (projection.phase === "cloud" &&
          projection.lifecycle.state === "pending")
      );
    case "recovery_required":
      return (
        projection.phase === "cloud" &&
        projection.lifecycle.state === "outcome_unknown"
      );
    case "monitoring_degraded":
      return projection.phase === "monitoring_degraded";
    case "stopped":
      return (
        projection.phase === "canceled_before_cloud_start" ||
        projection.phase === "stopped_before_cloud_start" ||
        projection.phase === "stopped_after_lower_failure" ||
        projection.phase === "cloud" ||
        projection.phase === "monitoring_degraded"
      );
    default:
      return false;
  }
}

function validSnapshot(value) {
  if (
    !exactKeys(value, ["identity", "state", "current_turn", "high_water_seq"]) ||
    !exactKeys(value.identity, ["session_id", "instance_id"]) ||
    !isUuid(value.identity.session_id) ||
    !isUuid(value.identity.instance_id) ||
    !SESSION_STATES.has(value.state) ||
    !isSeq(value.high_water_seq)
  ) {
    return false;
  }
  if (value.current_turn === null) {
    return validSnapshotCombination(value.state, null);
  }
  return (
    exactKeys(value.current_turn, ["turn_id", "projection"]) &&
    isUuid(value.current_turn.turn_id) &&
    validProjection(value.current_turn.projection) &&
    validSnapshotCombination(value.state, value.current_turn)
  );
}

function validLoginStatus(value) {
  if (!value || !LOGIN_STATES.has(value.state)) {
    return false;
  }
  if (value.state === "device_login_pending" || value.state === "outcome_unknown") {
    return exactKeys(value, ["state", "operation_id"]) && isUuid(value.operation_id);
  }
  return exactKeys(value, ["state"]);
}

function validBootstrap(value) {
  return (
    exactKeys(value, ["actor", "expires_in_seconds", "p0_session_id", "instance_id"]) &&
    value.actor === "operator" &&
    Number.isInteger(value.expires_in_seconds) &&
    value.expires_in_seconds >= 300 &&
    value.expires_in_seconds <= 43_200 &&
    isUuid(value.p0_session_id) &&
    isUuid(value.instance_id)
  );
}

function validDeviceLogin(value) {
  return (
    exactKeys(value, [
      "operation_id",
      "verification_url",
      "verification_code",
      "expires_in_seconds",
    ]) &&
    isUuid(value.operation_id) &&
    value.verification_url === "https://auth.openai.com/codex/device" &&
    typeof value.verification_code === "string" &&
    /^[A-Z0-9]{4}-[A-Z0-9]{5}$/.test(value.verification_code) &&
    value.expires_in_seconds === 900
  );
}

function validTurnReceipt(value) {
  return exactKeys(value, ["turn_id", "high_water_seq"]) && isUuid(value.turn_id) && isSeq(value.high_water_seq);
}

function validPayload(value) {
  if (!value || !EVENT_TYPES.has(value.type)) {
    return false;
  }
  switch (value.type) {
    case "turn_accepted":
    case "turn_canceled_before_cloud_start":
    case "monitoring_degraded":
      return exactKeys(value, ["type"]);
    case "lifecycle_changed":
      return exactKeys(value, ["type", "lifecycle"]) && validCloudLifecycle(value.lifecycle);
    case "cancel_requested":
      return exactKeys(value, ["type", "actor"]) && value.actor === "operator";
    case "recovery_observed":
      return (
        exactKeys(value, ["type", "actor", "operation_id", "task_ids", "complete"]) &&
        value.actor === "operator" &&
        isUuid(value.operation_id) &&
        Array.isArray(value.task_ids) &&
        value.task_ids.length <= 100 &&
        value.task_ids.every(validCloudTaskId) &&
        typeof value.complete === "boolean"
      );
    case "recovery_resolved":
      return (
        exactKeys(value, ["type", "actor", "operation_id", "decision"]) &&
        value.actor === "operator" &&
        isUuid(value.operation_id) &&
        (value.decision === "adopt" || value.decision === "abandon")
      );
    case "runtime_stopped":
      return (
        exactKeys(value, ["type", "reason"]) &&
        (value.reason === "shutdown" || value.reason === "lower_failure")
      );
    default:
      return false;
  }
}

function validEnvelope(value, sessionId) {
  return (
    exactKeys(value, ["schema_version", "session_id", "seq", "turn_id", "payload"]) &&
    value.schema_version === 1 &&
    value.session_id === sessionId &&
    isSeq(value.seq) &&
    (value.turn_id === null || isUuid(value.turn_id)) &&
    validPayload(value.payload)
  );
}

function validStored(value) {
  return (
    exactKeys(value, ["schema_version", "instance_id", "session_id", "event_seq"]) &&
    value.schema_version === 1 &&
    isUuid(value.instance_id) &&
    isUuid(value.session_id) &&
    isSeq(value.event_seq)
  );
}

function errorDisposition(code) {
  if (REAUTH_CODES.has(code)) {
    return ["reauth", MESSAGES.authentication_required];
  }
  if (RESYNC_CODES.has(code)) {
    return ["resync", MESSAGES.state_changed];
  }
  if (RECOVERY_CODES.has(code)) {
    return ["recovery", MESSAGES.recovery_required];
  }
  if (REJECTED_CODES.has(code)) {
    return ["rejected", MESSAGES.action_rejected];
  }
  if (SERVICE_CODES.has(code)) {
    return ["service", MESSAGES.service_failed];
  }
  return ["unknown", MESSAGES.request_failed];
}

function safeStateCopy(state) {
  return {
    supported: state.supported,
    authenticated: state.authenticated,
    busy: state.busy,
    connection: state.connection,
    login: state.login,
    session: state.session ? { ...state.session } : null,
    device: state.device ? { ...state.device } : null,
    diff: state.diff,
    events: state.events.map((event) => ({ ...event })),
    error: state.error,
  };
}

export function createP0Controller(dependencies) {
  const fetchRequest = dependencies?.fetch;
  const createWebSocket = dependencies?.createWebSocket;
  const randomUUID = dependencies?.randomUUID;
  const storage = dependencies?.storage;
  const setTimer = dependencies?.setTimeout;
  const clearTimer = dependencies?.clearTimeout;
  const location = dependencies?.location;
  const publish = typeof dependencies?.publish === "function" ? dependencies.publish : () => {};
  const AbortControllerType = dependencies?.AbortController;
  const TextEncoderType = dependencies?.TextEncoder;
  const TextDecoderType = dependencies?.TextDecoder;

  const supported =
    typeof fetchRequest === "function" &&
    typeof createWebSocket === "function" &&
    typeof randomUUID === "function" &&
    storage &&
    typeof storage.getItem === "function" &&
    typeof storage.setItem === "function" &&
    typeof storage.removeItem === "function" &&
    typeof setTimer === "function" &&
    typeof clearTimer === "function" &&
    typeof AbortControllerType === "function" &&
    typeof TextEncoderType === "function" &&
    typeof TextDecoderType === "function" &&
    location &&
    typeof location.origin === "string" &&
    location.origin.startsWith("https://");

  const encoder = supported ? new TextEncoderType() : null;
  const decoder = supported ? new TextDecoderType("utf-8", { fatal: true }) : null;
  const state = {
    supported,
    authenticated: false,
    busy: false,
    connection: "disconnected",
    login: "unknown",
    session: null,
    device: null,
    diff: "",
    events: [],
    error: supported ? "" : MESSAGES.unsupported,
  };

  let disposed = false;
  let generation = 0;
  let socket = null;
  let reconnectTimer = null;
  let reconnectAttempt = 0;
  let currentAbort = null;
  let identity = null;
  let lastSeq = 0;
  let forceSnapshotCursor = false;
  let socketPolicy = "reconnect";

  function emit() {
    try {
      publish(safeStateCopy(state));
    } catch (_) {
      // The DOM adapter is not an authority and cannot alter controller behavior.
    }
  }

  function setError(message) {
    state.error = message;
    emit();
  }

  function clearStorage() {
    try {
      storage.removeItem(CURSOR_KEY);
    } catch (_) {
      // Storage is optional process-local convenience.
    }
  }

  function readStored() {
    try {
      const raw = storage.getItem(CURSOR_KEY);
      if (raw === null) {
        return null;
      }
      const parsed = JSON.parse(raw);
      if (validStored(parsed)) {
        return parsed;
      }
    } catch (_) {
      // Invalid or unavailable storage is handled identically.
    }
    clearStorage();
    return null;
  }

  function persistCursor() {
    if (!identity) {
      return;
    }
    try {
      storage.setItem(
        CURSOR_KEY,
        JSON.stringify({
          schema_version: 1,
          instance_id: identity.instanceId,
          session_id: identity.sessionId,
          event_seq: lastSeq,
        }),
      );
    } catch (_) {
      forceSnapshotCursor = true;
    }
  }

  function closeSocket(policy = "none") {
    socketPolicy = policy;
    const current = socket;
    socket = null;
    if (current) {
      try {
        current.close(1000, "");
      } catch (_) {
        // A failed local close has no session mutation authority.
      }
    }
    state.connection = "disconnected";
  }

  function cancelReconnect() {
    if (reconnectTimer !== null) {
      clearTimer(reconnectTimer);
      reconnectTimer = null;
    }
  }

  function enterBootstrap(message = MESSAGES.authentication_required) {
    generation += 1;
    cancelReconnect();
    if (currentAbort) {
      currentAbort.abort();
      currentAbort = null;
    }
    closeSocket("none");
    identity = null;
    lastSeq = 0;
    forceSnapshotCursor = false;
    clearStorage();
    state.authenticated = false;
    state.busy = false;
    state.connection = "disconnected";
    state.login = "unknown";
    state.session = null;
    state.device = null;
    state.diff = "";
    state.events = [];
    state.error = message;
    emit();
  }

  function clearVolatileForRead() {
    state.device = null;
    state.diff = "";
  }

  function validateIdempotencyKey() {
    try {
      const value = randomUUID();
      return isUuidV4(value) ? value : null;
    } catch (_) {
      return null;
    }
  }

  function byteStringBearer(token) {
    if (typeof token !== "string") {
      return null;
    }
    for (let index = 0; index < token.length; index += 1) {
      const unit = token.charCodeAt(index);
      if (unit >= 0xd800 && unit <= 0xdbff) {
        const next = token.charCodeAt(index + 1);
        if (!(next >= 0xdc00 && next <= 0xdfff)) {
          return null;
        }
        index += 1;
      } else if (unit >= 0xdc00 && unit <= 0xdfff) {
        return null;
      }
    }
    const bytes = encoder.encode(token);
    if (
      bytes.length < 32 ||
      bytes.length > 128 ||
      bytes.some((byte) => byte <= 0x1f || byte === 0x7f)
    ) {
      return null;
    }
    let encoded = "";
    for (const byte of bytes) {
      encoded += String.fromCharCode(byte);
    }
    return `Bearer ${encoded}`;
  }

  function validPromptBody(prompt) {
    if (
      typeof prompt !== "string" ||
      prompt.length === 0 ||
      prompt === "-" ||
      [...prompt].every((character) => /\s/u.test(character)) ||
      [...prompt].some((character) => {
        const code = character.codePointAt(0);
        return (code <= 0x1f && code !== 0x09 && code !== 0x0a) || code === 0x7f;
      })
    ) {
      return null;
    }
    if (encoder.encode(prompt).length > PROMPT_LIMIT) {
      return null;
    }
    const body = JSON.stringify({ prompt });
    return encoder.encode(body).length <= BODY_LIMIT ? body : null;
  }

  async function readBytes(response, limit, expectedContentType) {
    if (response.headers?.get("content-type") !== expectedContentType) {
      throw new Error("media");
    }
    const length = response.headers?.get("content-length");
    if (length !== null && length !== undefined) {
      if (!/^(0|[1-9][0-9]*)$/.test(length) || Number(length) > limit) {
        throw new Error("limit");
      }
    }
    if (!response.body || typeof response.body.getReader !== "function") {
      throw new Error("body");
    }
    const reader = response.body.getReader();
    const chunks = [];
    let total = 0;
    try {
      while (true) {
        const next = await reader.read();
        if (next.done) {
          break;
        }
        if (!(next.value instanceof Uint8Array)) {
          throw new Error("chunk");
        }
        total += next.value.byteLength;
        if (total > limit) {
          try {
            await reader.cancel();
          } catch (_) {
            // Cancellation is best-effort after the bound is enforced.
          }
          throw new Error("limit");
        }
        chunks.push(next.value);
      }
    } catch (error) {
      try {
        await reader.cancel();
      } catch (_) {
        // The original bounded-read failure remains authoritative.
      }
      throw error;
    }
    const combined = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      combined.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return decoder.decode(combined);
  }

  async function readJson(response) {
    const text = await readBytes(response, JSON_LIMIT, "application/json");
    return JSON.parse(text);
  }

  async function readEmpty(response) {
    if (response.headers?.get("content-type") !== null) {
      throw new Error("media");
    }
    if (!response.body) {
      return;
    }
    const reader = response.body.getReader();
    const first = await reader.read();
    if (!first.done && first.value.byteLength !== 0) {
      try {
        await reader.cancel();
      } catch (_) {
        // The nonempty-body rejection remains authoritative.
      }
      throw new Error("body");
    }
    const second = await reader.read();
    if (!second.done) {
      throw new Error("body");
    }
  }

  async function request(path, init, expectedStatus, validate, kind) {
    const abort = new AbortControllerType();
    currentAbort = abort;
    let timedOut = false;
    let boundedBody = "response";
    const timer = setTimer(() => {
      timedOut = true;
      abort.abort();
    }, HTTP_TIMEOUT_MS);
    try {
      const response = await fetchRequest(path, {
        ...init,
        cache: "no-store",
        credentials: "same-origin",
        mode: "same-origin",
        redirect: "error",
        referrerPolicy: "no-referrer",
        signal: abort.signal,
      });
      if (
        response.redirected === true ||
        (response.status >= 300 && response.status <= 399)
      ) {
        return { ok: false, ambiguous: kind === "mutation", disposition: "unknown", message: MESSAGES.request_failed };
      }
      if (response.status === expectedStatus) {
        if (path === ROUTES.diff) {
          boundedBody = "diff";
        }
        const value = await validate(response);
        return { ok: true, value };
      }
      const value = await readJson(response);
      if (
        !exactKeys(value, ["error"]) ||
        !value.error ||
        !HTTP_CODES.has(value.error.code) ||
        HTTP_STATUS.get(value.error.code) !== response.status ||
        typeof value.error.message !== "string"
      ) {
        throw new Error("error-schema");
      }
      if (
        !(
          exactKeys(value.error, ["code", "message"]) ||
          (OPERATION_ERROR_CODES.has(value.error.code) &&
            exactKeys(value.error, ["code", "message", "operation_id"]) &&
            isUuid(value.error.operation_id))
        )
      ) {
        throw new Error("error-schema");
      }
      const [disposition, message] = errorDisposition(value.error.code);
      return { ok: false, ambiguous: false, disposition, message };
    } catch (error) {
      const message =
        error instanceof Error && error.message === "limit"
          ? boundedBody === "diff"
            ? MESSAGES.diff_too_large
            : MESSAGES.response_too_large
          : MESSAGES.request_failed;
      return {
        ok: false,
        ambiguous: kind === "mutation",
        disposition: "unknown",
        message: timedOut ? MESSAGES.request_failed : message,
      };
    } finally {
      clearTimer(timer);
      if (currentAbort === abort) {
        currentAbort = null;
      }
    }
  }

  async function jsonSuccess(response, validator) {
    const value = await readJson(response);
    if (!validator(value)) {
      throw new Error("schema");
    }
    return value;
  }

  function applySnapshot(snapshot) {
    state.session = {
      state: snapshot.state,
      phase: snapshot.current_turn?.projection?.phase ?? "none",
      highWaterSeq: snapshot.high_water_seq,
      recoveryRequired: snapshot.state === "recovery_required",
    };
  }

  function applyLogin(login) {
    state.login = login.state;
    if (login.state !== "device_login_pending") {
      state.device = null;
    }
  }

  function beginHttp() {
    if (!supported || disposed || state.busy) {
      return false;
    }
    state.busy = true;
    state.error = "";
    return true;
  }

  function endHttp() {
    state.busy = false;
    emit();
  }

  function discardFinishedHttp() {
    if (currentAbort === null) {
      state.busy = false;
    }
  }

  async function handleFailure(result) {
    if (result.disposition === "reauth") {
      enterBootstrap(result.message);
      return false;
    }
    if (result.disposition === "resync" || result.ambiguous) {
      forceSnapshotCursor = true;
      clearStorage();
      const refreshPromise = refresh();
      const refreshGeneration = generation;
      const refreshed = await refreshPromise;
      if (
        refreshed &&
        !disposed &&
        generation === refreshGeneration &&
        identity &&
        state.authenticated
      ) {
        setError(result.message);
      }
      return false;
    }
    setError(result.message);
    return false;
  }

  async function refresh() {
    if (!beginHttp()) {
      return false;
    }
    const myGeneration = ++generation;
    cancelReconnect();
    closeSocket("none");
    clearVolatileForRead();
    emit();
    const stored = readStored();
    const sessionResult = await request(
      ROUTES.session,
      { method: "GET" },
      200,
      (response) => jsonSuccess(response, validSnapshot),
      "read",
    );
    if (disposed || generation !== myGeneration) {
      discardFinishedHttp();
      return false;
    }
    if (!sessionResult.ok) {
      endHttp();
      if (disposed || generation !== myGeneration) {
        return false;
      }
      if (sessionResult.disposition === "reauth") {
        enterBootstrap(sessionResult.message);
      } else {
        setError(sessionResult.message);
      }
      return false;
    }
    const snapshot = sessionResult.value;
    const sameStoredIdentity =
      stored &&
      stored.instance_id === snapshot.identity.instance_id &&
      stored.session_id === snapshot.identity.session_id &&
      stored.event_seq <= snapshot.high_water_seq &&
      !forceSnapshotCursor;
    identity = {
      instanceId: snapshot.identity.instance_id,
      sessionId: snapshot.identity.session_id,
    };
    lastSeq = sameStoredIdentity ? stored.event_seq : snapshot.high_water_seq;
    forceSnapshotCursor = false;
    state.authenticated = true;
    applySnapshot(snapshot);
    state.events = sameStoredIdentity ? state.events : [];
    persistCursor();

    const loginResult = await request(
      ROUTES.login,
      { method: "GET" },
      200,
      (response) => jsonSuccess(response, validLoginStatus),
      "read",
    );
    if (disposed || generation !== myGeneration) {
      discardFinishedHttp();
      return false;
    }
    endHttp();
    if (disposed || generation !== myGeneration) {
      return false;
    }
    if (!loginResult.ok) {
      if (loginResult.disposition === "reauth") {
        enterBootstrap(loginResult.message);
        return false;
      }
      setError(loginResult.message);
    } else {
      applyLogin(loginResult.value);
      emit();
    }
    if (!disposed && generation === myGeneration && identity) {
      connect(myGeneration);
    }
    return true;
  }

  function scheduleReconnect(myGeneration) {
    if (
      disposed ||
      !identity ||
      myGeneration !== generation ||
      reconnectTimer !== null ||
      socketPolicy === "terminal"
    ) {
      return;
    }
    state.connection = "reconnecting";
    state.error = MESSAGES.stream_waiting;
    emit();
    const delay = RECONNECT_DELAYS[Math.min(reconnectAttempt, RECONNECT_DELAYS.length - 1)];
    reconnectAttempt += 1;
    reconnectTimer = setTimer(async () => {
      reconnectTimer = null;
      if (disposed || myGeneration !== generation) {
        return;
      }
      if (state.busy) {
        scheduleReconnect(myGeneration);
        return;
      }
      await refresh();
    }, delay);
  }

  function streamFailure(policy, message) {
    socketPolicy = policy;
    state.error = message;
    emit();
    const current = socket;
    if (current) {
      try {
        current.close(1008, "");
      } catch (_) {
        // Closing the transport cannot cancel a turn.
      }
    }
  }

  function handleWsError(frame) {
    const keysAreValid =
      frame?.code === "subscriber_lagged"
        ? exactKeys(frame, ["type", "code", "message"]) ||
          exactKeys(frame, ["type", "code", "message", "latest_available"])
        : exactKeys(frame, expectedWsErrorKeys(frame?.code));
    if (!keysAreValid || !WS_CODES.has(frame.code) || typeof frame.message !== "string") {
      streamFailure("terminal", MESSAGES.protocol_error);
      return;
    }
    if (
      (frame.code === "history_gap" &&
        (!isSeq(frame.oldest_available) ||
          !isSeq(frame.latest_available) ||
          frame.oldest_available > frame.latest_available)) ||
      (frame.code === "future_cursor" && !isSeq(frame.latest_available)) ||
      (frame.code === "subscriber_lagged" &&
        "latest_available" in frame &&
        !isSeq(frame.latest_available)) ||
      (frame.code === "unsupported_version" && frame.supported_version !== 1)
    ) {
      streamFailure("terminal", MESSAGES.protocol_error);
      return;
    }
    if (frame.code === "authentication_expired") {
      enterBootstrap(MESSAGES.authentication_required);
      return;
    }
    if (frame.code === "history_gap" || frame.code === "future_cursor" || frame.code === "wrong_session") {
      forceSnapshotCursor = true;
      clearStorage();
      streamFailure("refresh", MESSAGES.state_changed);
      return;
    }
    if (frame.code === "protocol_error" || frame.code === "unsupported_version") {
      streamFailure("terminal", MESSAGES.protocol_error);
      return;
    }
    streamFailure("refresh", MESSAGES.stream_waiting);
  }

  function expectedWsErrorKeys(code) {
    if (code === "history_gap") {
      return ["type", "code", "message", "oldest_available", "latest_available"];
    }
    if (code === "future_cursor") {
      return ["type", "code", "message", "latest_available"];
    }
    if (code === "unsupported_version") {
      return ["type", "code", "message", "supported_version"];
    }
    return ["type", "code", "message"];
  }

  function handleFrame(text, streamState) {
    if (typeof text !== "string" || encoder.encode(text).length > JSON_LIMIT) {
      streamFailure("terminal", MESSAGES.protocol_error);
      return;
    }
    let frame;
    try {
      frame = JSON.parse(text);
    } catch (_) {
      streamFailure("terminal", MESSAGES.protocol_error);
      return;
    }
    if (!frame || typeof frame.type !== "string") {
      streamFailure("terminal", MESSAGES.protocol_error);
      return;
    }
    if (frame.type === "error") {
      handleWsError(frame);
      return;
    }
    if (frame.type === "replay_begin") {
      if (
        streamState.phase !== "begin" ||
        !exactKeys(frame, ["type", "session_id", "after_seq", "high_water_seq"]) ||
        frame.session_id !== identity.sessionId ||
        frame.after_seq !== streamState.afterSeq ||
        !isSeq(frame.high_water_seq) ||
        frame.high_water_seq < frame.after_seq
      ) {
        streamFailure("terminal", MESSAGES.protocol_error);
        return;
      }
      streamState.phase = "replay";
      streamState.highWater = frame.high_water_seq;
      return;
    }
    if (frame.type === "event") {
      if (
        (streamState.phase !== "replay" && streamState.phase !== "live") ||
        !exactKeys(frame, ["type", "envelope"]) ||
        !validEnvelope(frame.envelope, identity.sessionId)
      ) {
        streamFailure("terminal", MESSAGES.protocol_error);
        return;
      }
      const envelope = frame.envelope;
      if (streamState.phase === "replay" && envelope.seq > streamState.highWater) {
        streamFailure("terminal", MESSAGES.protocol_error);
        return;
      }
      if (streamState.phase === "live" && envelope.seq <= streamState.highWater) {
        streamFailure("terminal", MESSAGES.protocol_error);
        return;
      }
      if (envelope.seq <= lastSeq) {
        return;
      }
      if (envelope.seq !== lastSeq + 1) {
        forceSnapshotCursor = true;
        clearStorage();
        streamFailure("refresh", MESSAGES.state_changed);
        return;
      }
      lastSeq = envelope.seq;
      persistCursor();
      state.events.push({ seq: envelope.seq, type: envelope.payload.type });
      if (state.events.length > EVENT_LIMIT) {
        state.events.splice(0, state.events.length - EVENT_LIMIT);
      }
      emit();
      return;
    }
    if (frame.type === "snapshot") {
      if (
        streamState.phase !== "replay" ||
        !exactKeys(frame, ["type", "snapshot", "high_water_seq"]) ||
        !validSnapshot(frame.snapshot) ||
        frame.snapshot.identity.session_id !== identity.sessionId ||
        frame.snapshot.identity.instance_id !== identity.instanceId ||
        frame.high_water_seq !== streamState.highWater ||
        frame.snapshot.high_water_seq !== streamState.highWater ||
        lastSeq !== streamState.highWater
      ) {
        streamFailure("terminal", MESSAGES.protocol_error);
        return;
      }
      applySnapshot(frame.snapshot);
      streamState.phase = "snapshot";
      emit();
      return;
    }
    if (frame.type === "replay_end") {
      if (
        streamState.phase !== "snapshot" ||
        !exactKeys(frame, ["type", "session_id", "high_water_seq"]) ||
        frame.session_id !== identity.sessionId ||
        frame.high_water_seq !== streamState.highWater
      ) {
        streamFailure("terminal", MESSAGES.protocol_error);
        return;
      }
      streamState.phase = "live";
      state.connection = "connected";
      state.error = "";
      reconnectAttempt = 0;
      emit();
      return;
    }
    streamFailure("terminal", MESSAGES.protocol_error);
  }

  function connect(myGeneration) {
    if (disposed || !identity || myGeneration !== generation || socket) {
      return;
    }
    const afterSeq = lastSeq;
    const streamState = { phase: "begin", afterSeq, highWater: null };
    socketPolicy = "reconnect";
    state.connection = "connecting";
    emit();
    let next;
    try {
      next = createWebSocket(`${location.origin.replace(/^https:/, "wss:")}${ROUTES.stream}`);
    } catch (_) {
      scheduleReconnect(myGeneration);
      return;
    }
    socket = next;
    next.onopen = () => {
      if (disposed || myGeneration !== generation || socket !== next) {
        return;
      }
      try {
        next.send(
          JSON.stringify({
            type: "subscribe",
            protocol_version: 1,
            session_id: identity.sessionId,
            after_seq: afterSeq,
          }),
        );
      } catch (_) {
        streamFailure("refresh", MESSAGES.stream_waiting);
      }
    };
    next.onmessage = (event) => {
      if (!disposed && myGeneration === generation && socket === next) {
        handleFrame(event.data, streamState);
      }
    };
    next.onerror = () => {
      if (!disposed && myGeneration === generation && socket === next) {
        state.error = MESSAGES.stream_waiting;
        emit();
      }
    };
    next.onclose = (event) => {
      if (socket === next) {
        socket = null;
      }
      if (disposed || myGeneration !== generation) {
        return;
      }
      state.connection = "disconnected";
      emit();
      if (socketPolicy === "terminal") {
        return;
      }
      if (event?.code === 1012 || socketPolicy === "refresh" || socketPolicy === "reconnect") {
        scheduleReconnect(myGeneration);
      }
    };
  }

  async function authenticate(token) {
    if (!beginHttp()) {
      return false;
    }
    const myGeneration = generation;
    const bearer = byteStringBearer(token);
    if (!bearer) {
      state.error = MESSAGES.invalid_bootstrap;
      endHttp();
      return false;
    }
    state.device = null;
    state.diff = "";
    emit();
    const result = await request(
      ROUTES.bootstrap,
      { method: "POST", headers: { Authorization: bearer } },
      201,
      (response) => jsonSuccess(response, validBootstrap),
      "mutation",
    );
    if (disposed || generation !== myGeneration) {
      discardFinishedHttp();
      return false;
    }
    endHttp();
    if (disposed || generation !== myGeneration) {
      return false;
    }
    if (!result.ok) {
      return handleFailure(result);
    }
    identity = {
      instanceId: result.value.instance_id,
      sessionId: result.value.p0_session_id,
    };
    clearStorage();
    return refresh();
  }

  async function runMutation(path, body, status, validator, apply) {
    const requestIdentity = identity;
    if (!requestIdentity || !beginHttp()) {
      return false;
    }
    const myGeneration = generation;
    state.diff = "";
    state.device = null;
    emit();
    const key = validateIdempotencyKey();
    if (!key) {
      state.error = MESSAGES.request_failed;
      endHttp();
      return false;
    }
    const headers = {
      "Codebox-Instance-Id": requestIdentity.instanceId,
      "Idempotency-Key": key,
    };
    if (body !== null) {
      headers["Content-Type"] = "application/json";
    }
    const result = await request(
      path,
      { method: "POST", headers, ...(body === null ? {} : { body }) },
      status,
      validator,
      "mutation",
    );
    if (
      disposed ||
      generation !== myGeneration ||
      identity !== requestIdentity
    ) {
      discardFinishedHttp();
      return false;
    }
    endHttp();
    if (
      disposed ||
      generation !== myGeneration ||
      identity !== requestIdentity
    ) {
      return false;
    }
    if (!result.ok) {
      return handleFailure(result);
    }
    const applied = apply(result.value);
    if (applied === false) {
      const refreshPromise = refresh();
      const refreshGeneration = generation;
      const refreshed = await refreshPromise;
      if (
        refreshed &&
        !disposed &&
        generation === refreshGeneration &&
        identity &&
        state.authenticated
      ) {
        setError(MESSAGES.state_changed);
      }
      return false;
    }
    emit();
    return true;
  }

  async function startDeviceLogin() {
    return runMutation(
      ROUTES.loginDevice,
      null,
      202,
      (response) => jsonSuccess(response, validDeviceLogin),
      (value) => {
        state.login = "device_login_pending";
        state.device = {
          verificationUrl: value.verification_url,
          verificationCode: value.verification_code,
          expiresInSeconds: value.expires_in_seconds,
        };
      },
    );
  }

  async function cancelDeviceLogin() {
    return runMutation(
      ROUTES.loginCancel,
      null,
      200,
      (response) => jsonSuccess(response, validLoginStatus),
      applyLogin,
    );
  }

  async function submitPrompt(prompt) {
    const body = validPromptBody(prompt);
    if (!body) {
      setError(MESSAGES.invalid_prompt);
      return false;
    }
    return runMutation(
      ROUTES.turns,
      body,
      202,
      (response) => jsonSuccess(response, validTurnReceipt),
      () => {},
    );
  }

  async function cancelTurn() {
    return runMutation(
      ROUTES.turnCancel,
      null,
      200,
      (response) => jsonSuccess(response, validSnapshot),
      (value) => {
        if (
          value.identity.instance_id !== identity.instanceId ||
          value.identity.session_id !== identity.sessionId
        ) {
          forceSnapshotCursor = true;
          clearStorage();
          return false;
        }
        applySnapshot(value);
        return true;
      },
    );
  }

  async function showDiff() {
    const requestIdentity = identity;
    if (!requestIdentity || !beginHttp()) {
      return false;
    }
    const myGeneration = generation;
    state.diff = "";
    emit();
    const result = await request(
      ROUTES.diff,
      { method: "GET" },
      200,
      async (response) => readBytes(response, DIFF_LIMIT, "text/plain; charset=utf-8"),
      "read",
    );
    if (
      disposed ||
      generation !== myGeneration ||
      identity !== requestIdentity
    ) {
      discardFinishedHttp();
      return false;
    }
    endHttp();
    if (
      disposed ||
      generation !== myGeneration ||
      identity !== requestIdentity
    ) {
      return false;
    }
    if (!result.ok) {
      return handleFailure(result);
    }
    state.diff = result.value;
    emit();
    return true;
  }

  async function logout() {
    const requestIdentity = identity;
    if (!requestIdentity || !beginHttp()) {
      return false;
    }
    const myGeneration = generation;
    state.diff = "";
    state.device = null;
    emit();
    const key = validateIdempotencyKey();
    if (!key) {
      state.error = MESSAGES.request_failed;
      endHttp();
      return false;
    }
    const result = await request(
      ROUTES.bootstrap,
      {
        method: "DELETE",
        headers: {
          "Codebox-Instance-Id": requestIdentity.instanceId,
          "Idempotency-Key": key,
        },
      },
      204,
      async (response) => {
        await readEmpty(response);
        return null;
      },
      "mutation",
    );
    if (
      disposed ||
      generation !== myGeneration ||
      identity !== requestIdentity
    ) {
      discardFinishedHttp();
      return false;
    }
    endHttp();
    if (
      disposed ||
      generation !== myGeneration ||
      identity !== requestIdentity
    ) {
      return false;
    }
    if (!result.ok) {
      return handleFailure(result);
    }
    enterBootstrap("");
    return true;
  }

  function dispose() {
    if (disposed) {
      return;
    }
    disposed = true;
    generation += 1;
    cancelReconnect();
    if (currentAbort) {
      currentAbort.abort();
      currentAbort = null;
    }
    closeSocket("none");
    identity = null;
    lastSeq = 0;
    state.busy = false;
    state.device = null;
    state.diff = "";
    state.events = [];
    emit();
  }

  emit();
  return Object.freeze({
    load: refresh,
    refresh,
    authenticate,
    startDeviceLogin,
    cancelDeviceLogin,
    submitPrompt,
    cancelTurn,
    showDiff,
    logout,
    dispose,
  });
}
