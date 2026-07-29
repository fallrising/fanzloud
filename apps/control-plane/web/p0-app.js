import { createP0Controller } from "./p0-client.js";

const EVENT_LABELS = Object.freeze({
  turn_accepted: "Turn accepted",
  turn_canceled_before_cloud_start: "Turn canceled before Cloud start",
  lifecycle_changed: "Cloud lifecycle changed",
  cancel_requested: "Cancellation requested",
  monitoring_degraded: "Monitoring degraded",
  recovery_observed: "Recovery state observed",
  recovery_resolved: "Recovery state resolved",
  runtime_stopped: "Runtime stopped",
});

function requiredElement(document, id) {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error("operator page element is missing");
  }
  return element;
}

export function mountP0App(environment = globalThis) {
  const document = environment.document;
  if (!document) {
    return null;
  }

  const elements = {
    connection: requiredElement(document, "connection-status"),
    bootstrapPanel: requiredElement(document, "bootstrap-panel"),
    bootstrapToken: requiredElement(document, "bootstrap-token"),
    authenticate: requiredElement(document, "authenticate"),
    operatorPanel: requiredElement(document, "operator-panel"),
    session: requiredElement(document, "session-status"),
    refresh: requiredElement(document, "refresh"),
    login: requiredElement(document, "login-status"),
    startLogin: requiredElement(document, "start-login"),
    cancelLogin: requiredElement(document, "cancel-login"),
    verificationUrl: requiredElement(document, "verification-url"),
    verificationCode: requiredElement(document, "verification-code"),
    prompt: requiredElement(document, "prompt"),
    submitTurn: requiredElement(document, "submit-turn"),
    cancelTurn: requiredElement(document, "cancel-turn"),
    stream: requiredElement(document, "stream-status"),
    events: requiredElement(document, "event-list"),
    showDiff: requiredElement(document, "show-diff"),
    diff: requiredElement(document, "diff-output"),
    logout: requiredElement(document, "logout"),
    error: requiredElement(document, "error-message"),
  };

  function render(state) {
    elements.bootstrapPanel.hidden = state.authenticated;
    elements.operatorPanel.hidden = !state.authenticated;
    elements.connection.textContent = state.supported
      ? state.authenticated
        ? "Authenticated private operator session"
        : "Private operator authentication required"
      : "Unsupported browser";
    elements.stream.textContent = state.connection;
    elements.login.textContent = state.login;
    elements.session.textContent = state.session
      ? state.session.recoveryRequired
        ? "Recovery required — use the trusted operator procedure, then refresh"
        : `${state.session.state} · ${state.session.phase} · sequence ${state.session.highWaterSeq}`
      : "Unavailable";
    elements.verificationUrl.textContent = state.device?.verificationUrl ?? "";
    elements.verificationCode.textContent = state.device?.verificationCode ?? "";
    elements.events.textContent =
      state.events.length === 0
        ? "No retained activity in this tab."
        : state.events
            .map((event) => `${event.seq}: ${EVENT_LABELS[event.type]}`)
            .join("\n");
    elements.diff.textContent = state.diff || "No diff loaded.";
    elements.error.textContent = state.error;

    for (const control of [
      elements.authenticate,
      elements.refresh,
      elements.startLogin,
      elements.cancelLogin,
      elements.submitTurn,
      elements.cancelTurn,
      elements.showDiff,
      elements.logout,
    ]) {
      control.disabled = state.busy || !state.supported;
    }
  }

  const controller = createP0Controller({
    fetch: typeof environment.fetch === "function" ? environment.fetch.bind(environment) : null,
    createWebSocket:
      typeof environment.WebSocket === "function"
        ? (url) => new environment.WebSocket(url)
        : null,
    randomUUID:
      typeof environment.crypto?.randomUUID === "function"
        ? environment.crypto.randomUUID.bind(environment.crypto)
        : null,
    storage: environment.sessionStorage,
    setTimeout:
      typeof environment.setTimeout === "function"
        ? environment.setTimeout.bind(environment)
        : null,
    clearTimeout:
      typeof environment.clearTimeout === "function"
        ? environment.clearTimeout.bind(environment)
        : null,
    location: environment.location,
    AbortController: environment.AbortController,
    TextEncoder: environment.TextEncoder,
    TextDecoder: environment.TextDecoder,
    publish: render,
  });

  elements.authenticate.addEventListener("click", async () => {
    const token = elements.bootstrapToken.value;
    elements.bootstrapToken.value = "";
    await controller.authenticate(token);
  });
  elements.refresh.addEventListener("click", async () => {
    await controller.refresh();
  });
  elements.startLogin.addEventListener("click", async () => {
    await controller.startDeviceLogin();
  });
  elements.cancelLogin.addEventListener("click", async () => {
    await controller.cancelDeviceLogin();
  });
  elements.submitTurn.addEventListener("click", async () => {
    const prompt = elements.prompt.value;
    elements.prompt.value = "";
    await controller.submitPrompt(prompt);
  });
  elements.cancelTurn.addEventListener("click", async () => {
    await controller.cancelTurn();
  });
  elements.showDiff.addEventListener("click", async () => {
    await controller.showDiff();
  });
  elements.logout.addEventListener("click", async () => {
    await controller.logout();
  });
  if (typeof environment.addEventListener === "function") {
    environment.addEventListener("pagehide", () => {
      controller.dispose();
    });
  }

  void controller.load();
  return controller;
}

if (typeof globalThis.document !== "undefined") {
  mountP0App(globalThis);
}
