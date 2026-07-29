use std::collections::VecDeque;
use std::convert::Infallible;
use std::future::Future;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::{
    fs,
    path::{Path, PathBuf},
};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{
    HeaderMap, HeaderValue, Method, Request, StatusCode, Version,
    header::{
        AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ORIGIN,
        SEC_WEBSOCKET_VERSION, SET_COOKIE, UPGRADE,
    },
};
use codebox_agent_codex::{
    CloudBranch, CloudCapture, CloudDiff, CloudDiffReadErrorCategory, CloudEnvironmentId,
    CloudLifecycleErrorCategory, CloudPrompt, CloudRunnerConfig, CloudSubmitOperationId,
    CloudTaskOrchestrator, CredentialScope, CredentialScopeConfig, CredentialScopeError,
    LoginBroker, LoginBrokerError, LoginOperationId, LoginStatus, UnknownSubmitDecision,
    decode_cloud_diff,
};
use codebox_domain::{CommandId, EventSeq, SessionId, TurnId};
use codebox_session_runtime::{
    P0Actor, P0CloudLifecycle, P0InstanceId, P0RecoveryCandidates, P0SessionConfig,
    P0SessionErrorCategory, P0SessionEvent, P0SessionEventEnvelope, P0SessionIdentity,
    P0SessionRuntime, P0SessionSnapshot, P0SessionState, P0TurnProjection, P0TurnReceipt,
    P0TurnSnapshot,
};
use futures_util::{SinkExt, StreamExt};
use http_body::{Body as HttpBody, Frame};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as ClientMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::Frame as ClientFrameFragment;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{
    Data as ClientFrameData, OpCode as ClientOpCode,
};
use tower::ServiceExt;
use uuid::Uuid;

use crate::config::{OperatorBootstrapToken, P0HttpConfig, P0PublicOrigin};
use crate::error::{
    ApiError, map_cloud_diff_error, map_cloud_lifecycle_error, map_login_error,
    map_session_category,
};
use crate::ports::{
    LiveEventError, LiveEventPort, LoginInstructions, LoginPort, LoginPortError, SessionPort,
    SessionPortError, SessionSubscribeError, SessionSubscription,
};
use crate::state::RequestAdmission;
use crate::state::{EntropySource, MonotonicClock, P0ControlPlane, cookie_comparison_work};
use crate::websocket::{
    ClientFrame, ServerFrame, SocketReader, SocketWriter, StreamDeadlines, run_socket,
};

const ORIGIN_VALUE: &str = "https://operator.example";
const BOOTSTRAP_SECRET: &str = "bootstrap-secret-32-bytes-value!";
const DEVICE_CODE: &str = "CODE-SECRET-123";
const PROMPT_CANARY: &str = "PROMPT-CANARY-SECRET";
const DIFF_CANARY: &str = "diff --git a/x b/x\n+<script>DIFF-CANARY</script>\n";

struct Harness {
    plane: Arc<P0ControlPlane>,
    router: Router,
    login: Arc<FakeLogin>,
    session: Arc<FakeSession>,
    clock: Arc<TestClock>,
}

#[derive(Clone)]
struct Auth {
    cookie: String,
    instance: String,
}

struct Captured {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Default)]
struct TestClock {
    seconds: AtomicU64,
}

impl TestClock {
    fn advance(&self, seconds: u64) {
        self.seconds.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl MonotonicClock for TestClock {
    fn now(&self) -> Duration {
        Duration::from_secs(self.seconds.load(Ordering::SeqCst))
    }
}

#[derive(Default)]
struct TestEntropy {
    next: AtomicU8,
}

impl EntropySource for TestEntropy {
    fn fill(&self, destination: &mut [u8]) -> Result<(), ()> {
        let value = self.next.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        destination.fill(value);
        Ok(())
    }
}

#[derive(Default)]
struct Gate {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl Gate {
    fn enter_and_wait(&self) {
        let mut state = self.state.lock().expect("gate state");
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self.changed.wait(state).expect("gate wait");
        }
    }

    fn wait_entered(&self) {
        let state = self.state.lock().expect("gate state");
        let (state, timeout) = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(2), |state| !state.0)
            .expect("gate entered wait");
        assert!(state.0, "lower fake was not entered before timeout");
        assert!(!timeout.timed_out() || state.0);
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("gate state");
        state.1 = true;
        self.changed.notify_all();
    }
}

struct GatedEmptyBody {
    gate: Arc<Gate>,
    released: bool,
}

impl GatedEmptyBody {
    fn new(gate: Arc<Gate>) -> Self {
        Self {
            gate,
            released: false,
        }
    }
}

impl HttpBody for GatedEmptyBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.released {
            return Poll::Ready(None);
        }
        self.gate.enter_and_wait();
        self.released = true;
        Poll::Ready(None)
    }
}

#[derive(Default)]
struct FakeLoginState {
    status_calls: usize,
    start_calls: usize,
    cancel_calls: usize,
    shutdown_calls: usize,
    status: Option<LoginStatus>,
}

#[derive(Default)]
struct FakeLogin {
    state: Mutex<FakeLoginState>,
}

impl FakeLogin {
    fn counts(&self) -> (usize, usize, usize, usize) {
        let state = self.state.lock().expect("fake login");
        (
            state.status_calls,
            state.start_calls,
            state.cancel_calls,
            state.shutdown_calls,
        )
    }
}

impl LoginPort for FakeLogin {
    fn status(&self) -> Result<LoginStatus, LoginPortError> {
        let mut state = self.state.lock().map_err(|_| LoginPortError::Unavailable)?;
        state.status_calls += 1;
        Ok(state.status.unwrap_or(LoginStatus::LoggedOut))
    }

    fn start_device_login(&self) -> Result<LoginInstructions, LoginPortError> {
        let mut state = self.state.lock().map_err(|_| LoginPortError::Unavailable)?;
        state.start_calls += 1;
        Ok(LoginInstructions {
            operation_id: LoginOperationId::new(),
            verification_url: "https://auth.openai.com/codex/device",
            verification_code: DEVICE_CODE.to_owned(),
            expires_in_seconds: 900,
        })
    }

    fn cancel(&self) -> Result<LoginStatus, LoginPortError> {
        let mut state = self.state.lock().map_err(|_| LoginPortError::Unavailable)?;
        state.cancel_calls += 1;
        Ok(state.status.unwrap_or(LoginStatus::LoggedOut))
    }

    fn shutdown_cleanup(&self) -> Result<(), LoginPortError> {
        let mut state = self.state.lock().map_err(|_| LoginPortError::Unavailable)?;
        state.shutdown_calls += 1;
        Ok(())
    }
}

struct FakeSessionState {
    snapshot: P0SessionSnapshot,
    start_calls: usize,
    cancel_calls: usize,
    reconcile_calls: usize,
    resolve_calls: usize,
    diff_calls: usize,
    shutdown_calls: usize,
    last_prompt: Option<String>,
    last_resolution: Option<&'static str>,
    start_gate: Option<Arc<Gate>>,
    diff_gate: Option<Arc<Gate>>,
    shutdown_gate: Option<Arc<Gate>>,
    subscribe_gate: Option<Arc<Gate>>,
    diff: String,
    subscription_error: Option<SessionSubscribeError>,
    replay: Vec<P0SessionEventEnvelope>,
    live: Arc<FakeLiveState>,
    subscribe_calls: usize,
    last_cursor: Option<(SessionId, EventSeq)>,
    publish_start_event: bool,
}

struct FakeSession {
    identity: P0SessionIdentity,
    state: Mutex<FakeSessionState>,
}

impl FakeSession {
    fn new(identity: P0SessionIdentity) -> Self {
        Self {
            identity,
            state: Mutex::new(FakeSessionState {
                snapshot: ready_snapshot(identity),
                start_calls: 0,
                cancel_calls: 0,
                reconcile_calls: 0,
                resolve_calls: 0,
                diff_calls: 0,
                shutdown_calls: 0,
                last_prompt: None,
                last_resolution: None,
                start_gate: None,
                diff_gate: None,
                shutdown_gate: None,
                subscribe_gate: None,
                diff: DIFF_CANARY.to_owned(),
                subscription_error: None,
                replay: Vec::new(),
                live: Arc::new(FakeLiveState::default()),
                subscribe_calls: 0,
                last_cursor: None,
                publish_start_event: false,
            }),
        }
    }

    fn set_snapshot(&self, snapshot: P0SessionSnapshot) {
        self.state.lock().expect("fake session").snapshot = snapshot;
    }

    fn set_start_gate(&self, gate: Arc<Gate>) {
        self.state.lock().expect("fake session").start_gate = Some(gate);
    }

    fn set_diff_gate(&self, gate: Arc<Gate>) {
        self.state.lock().expect("fake session").diff_gate = Some(gate);
    }

    fn set_shutdown_gate(&self, gate: Arc<Gate>) {
        self.state.lock().expect("fake session").shutdown_gate = Some(gate);
    }

    fn set_subscribe_gate(&self, gate: Arc<Gate>) {
        self.state.lock().expect("fake session").subscribe_gate = Some(gate);
    }

    fn counts(&self) -> (usize, usize, usize, usize, usize, usize) {
        let state = self.state.lock().expect("fake session");
        (
            state.start_calls,
            state.cancel_calls,
            state.reconcile_calls,
            state.resolve_calls,
            state.diff_calls,
            state.shutdown_calls,
        )
    }

    fn configure_subscription(
        &self,
        replay: Vec<P0SessionEventEnvelope>,
        snapshot: P0SessionSnapshot,
    ) -> Arc<FakeLiveState> {
        let mut state = self.state.lock().expect("fake session");
        state.replay = replay;
        state.snapshot = snapshot;
        state.subscription_error = None;
        Arc::clone(&state.live)
    }

    fn set_subscription_error(&self, error: SessionSubscribeError) {
        self.state.lock().expect("fake session").subscription_error = Some(error);
    }

    fn enable_composition_start_event(&self) {
        self.state.lock().expect("fake session").publish_start_event = true;
    }

    fn subscription_observation(&self) -> (usize, Option<(SessionId, EventSeq)>, usize) {
        let state = self.state.lock().expect("fake session");
        (
            state.subscribe_calls,
            state.last_cursor,
            state.live.drops.load(Ordering::SeqCst),
        )
    }
}

#[derive(Default)]
struct FakeLiveState {
    queue: Mutex<VecDeque<Result<P0SessionEventEnvelope, LiveEventError>>>,
    drops: AtomicUsize,
}

impl FakeLiveState {
    fn push(&self, item: Result<P0SessionEventEnvelope, LiveEventError>) {
        self.queue.lock().expect("fake live queue").push_back(item);
    }
}

struct FakeLive {
    state: Arc<FakeLiveState>,
}

impl LiveEventPort for FakeLive {
    fn try_recv(&self) -> Result<P0SessionEventEnvelope, LiveEventError> {
        self.state
            .queue
            .lock()
            .map_err(|_| LiveEventError::RuntimeStopped)?
            .pop_front()
            .unwrap_or(Err(LiveEventError::Empty))
    }
}

impl Drop for FakeLive {
    fn drop(&mut self) {
        self.state.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl SessionPort for FakeSession {
    fn identity(&self) -> P0SessionIdentity {
        self.identity
    }

    fn snapshot(&self) -> Result<P0SessionSnapshot, SessionPortError> {
        self.state
            .lock()
            .map(|state| state.snapshot.clone())
            .map_err(|_| SessionPortError::Unavailable)
    }

    fn start_turn(&self, prompt: CloudPrompt) -> Result<P0TurnReceipt, SessionPortError> {
        let (gate, receipt) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| SessionPortError::Unavailable)?;
            state.start_calls += 1;
            state.last_prompt = Some(prompt.as_str().to_owned());
            let turn_id = TurnId::new();
            let high_water_seq = if state.publish_start_event {
                let high_water_seq = state
                    .snapshot
                    .high_water_seq
                    .checked_next()
                    .map_err(|_| SessionPortError::Unavailable)?;
                let envelope = P0SessionEventEnvelope {
                    schema_version: 1,
                    session_id: self.identity.session_id,
                    seq: high_water_seq,
                    turn_id: Some(turn_id),
                    payload: P0SessionEvent::TurnAccepted,
                };
                state.snapshot.state = P0SessionState::Running;
                state.snapshot.current_turn = Some(P0TurnSnapshot {
                    turn_id,
                    projection: P0TurnProjection::Queued,
                });
                state.snapshot.high_water_seq = high_water_seq;
                state.replay.push(envelope.clone());
                state.live.push(Ok(envelope));
                high_water_seq
            } else {
                EventSeq::new(1)
            };
            (
                state.start_gate.clone(),
                P0TurnReceipt {
                    turn_id,
                    high_water_seq,
                },
            )
        };
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        Ok(receipt)
    }

    fn cancel_turn(&self, _actor: P0Actor) -> Result<P0SessionSnapshot, SessionPortError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SessionPortError::Unavailable)?;
        state.cancel_calls += 1;
        Ok(state.snapshot.clone())
    }

    fn reconcile_unknown(&self, _actor: P0Actor) -> Result<P0RecoveryCandidates, SessionPortError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SessionPortError::Unavailable)?;
        state.reconcile_calls += 1;
        let operation_id = current_operation(&state.snapshot).unwrap_or_default();
        Ok(P0RecoveryCandidates {
            operation_id,
            task_ids: Vec::new(),
            complete: true,
        })
    }

    fn resolve_unknown(
        &self,
        _actor: P0Actor,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    ) -> Result<P0SessionSnapshot, SessionPortError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SessionPortError::Unavailable)?;
        state.resolve_calls += 1;
        match decision {
            UnknownSubmitDecision::AdoptListedTask(task) => {
                state.last_resolution = Some("adopt");
                if task.as_str() != "task_allowed" {
                    return Err(SessionPortError::ProjectedLifecycle {
                        category: CloudLifecycleErrorCategory::TaskNotListed,
                        operation_id: Some(operation_id),
                    });
                }
            }
            UnknownSubmitDecision::AbandonAfterReconciliation(_) => {
                state.last_resolution = Some("abandon");
            }
        }
        Ok(state.snapshot.clone())
    }

    fn read_diff(&self) -> Result<CloudDiff, SessionPortError> {
        let (gate, diff) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| SessionPortError::Unavailable)?;
            state.diff_calls += 1;
            (state.diff_gate.clone(), state.diff.clone())
        };
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        decode_cloud_diff(&CloudCapture::new(
            diff.into_bytes(),
            Vec::new(),
            false,
            false,
            Some(0),
        ))
        .map_err(|_| SessionPortError::Unavailable)
    }

    fn subscribe(
        &self,
        session_id: SessionId,
        after_seq: EventSeq,
    ) -> Result<SessionSubscription, SessionSubscribeError> {
        let gate = {
            let mut state = self.state.lock().map_err(|_| {
                SessionSubscribeError::new(
                    crate::ports::SessionSubscribeErrorCategory::Unavailable,
                    None,
                    None,
                )
            })?;
            state.subscribe_calls += 1;
            state.last_cursor = Some((session_id, after_seq));
            state.subscribe_gate.clone()
        };
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        let state = self.state.lock().map_err(|_| {
            SessionSubscribeError::new(
                crate::ports::SessionSubscribeErrorCategory::Unavailable,
                None,
                None,
            )
        })?;
        if let Some(error) = state.subscription_error {
            return Err(error);
        }
        Ok(SessionSubscription {
            replay: state
                .replay
                .iter()
                .filter(|envelope| envelope.seq > after_seq)
                .cloned()
                .collect(),
            snapshot: state.snapshot.clone(),
            live: Box::new(FakeLive {
                state: Arc::clone(&state.live),
            }),
        })
    }

    fn shutdown(&self) -> Result<(), SessionPortError> {
        let gate = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| SessionPortError::Unavailable)?;
            state.shutdown_calls += 1;
            state.shutdown_gate.clone()
        };
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        Ok(())
    }
}

fn harness(lifetime_seconds: u64) -> Harness {
    let identity = P0SessionIdentity {
        session_id: SessionId::try_from_uuid(Uuid::from_u128(1)).expect("session id"),
        instance_id: P0InstanceId::try_from_uuid(Uuid::from_u128(2)).expect("instance id"),
    };
    let login = Arc::new(FakeLogin::default());
    let session = Arc::new(FakeSession::new(identity));
    let clock = Arc::new(TestClock::default());
    let entropy = Arc::new(TestEntropy::default());
    let origin = P0PublicOrigin::try_new(ORIGIN_VALUE).expect("origin");
    let bootstrap = OperatorBootstrapToken::try_new(BOOTSTRAP_SECRET).expect("bootstrap token");
    let config = P0HttpConfig::new(origin, bootstrap)
        .try_with_session_lifetime(Duration::from_secs(lifetime_seconds))
        .expect("session lifetime");
    let plane = Arc::new(P0ControlPlane::with_test_components(
        config,
        login.clone(),
        session.clone(),
        clock.clone(),
        entropy,
    ));
    Harness {
        router: plane.router(),
        plane,
        login,
        session,
        clock,
    }
}

fn ready_snapshot(identity: P0SessionIdentity) -> P0SessionSnapshot {
    P0SessionSnapshot {
        identity,
        state: P0SessionState::Ready,
        current_turn: None,
        high_water_seq: EventSeq::initial(),
    }
}

fn unknown_snapshot(
    identity: P0SessionIdentity,
    operation_id: CloudSubmitOperationId,
) -> P0SessionSnapshot {
    P0SessionSnapshot {
        identity,
        state: P0SessionState::RecoveryRequired,
        current_turn: Some(P0TurnSnapshot {
            turn_id: TurnId::new(),
            projection: P0TurnProjection::Cloud {
                lifecycle: P0CloudLifecycle::OutcomeUnknown { operation_id },
                cancel_requested: false,
            },
        }),
        high_water_seq: EventSeq::new(3),
    }
}

fn current_operation(snapshot: &P0SessionSnapshot) -> Option<CloudSubmitOperationId> {
    snapshot.current_turn.as_ref().and_then(|turn| {
        if let P0TurnProjection::Cloud {
            lifecycle: P0CloudLifecycle::OutcomeUnknown { operation_id },
            ..
        } = &turn.projection
        {
            Some(*operation_id)
        } else {
            None
        }
    })
}

async fn authenticate(harness: &Harness) -> (Auth, Captured) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/p0/v1/operator/session")
        .header(ORIGIN, ORIGIN_VALUE)
        .header(AUTHORIZATION, format!("Bearer {BOOTSTRAP_SECRET}"))
        .body(Body::empty())
        .expect("bootstrap request");
    let captured = send(&harness.router, request).await;
    let body: Value = serde_json::from_slice(&captured.body).expect("bootstrap JSON");
    let cookie = captured
        .headers
        .get(SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .expect("bootstrap cookie")
        .to_owned();
    (
        Auth {
            cookie,
            instance: body["instance_id"]
                .as_str()
                .expect("instance id")
                .to_owned(),
        },
        captured,
    )
}

fn protected_request(
    method: Method,
    uri: &str,
    auth: Option<&Auth>,
    origin: Option<&str>,
    key: Option<CommandId>,
    body: Option<&str>,
    content_type: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(auth) = auth {
        builder = builder
            .header(COOKIE, &auth.cookie)
            .header("codebox-instance-id", &auth.instance);
    }
    if let Some(origin) = origin {
        builder = builder.header(ORIGIN, origin);
    }
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key.to_string());
    }
    if let Some(content_type) = content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }
    builder
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_owned())))
        .expect("protected request")
}

async fn send(router: &Router, request: Request<Body>) -> Captured {
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes()
        .to_vec();
    Captured {
        status,
        headers,
        body,
    }
}

fn json_body(captured: &Captured) -> Value {
    serde_json::from_slice(&captured.body).expect("JSON response")
}

fn assert_common_headers(captured: &Captured) {
    assert_eq!(
        captured
            .headers
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        captured
            .headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
}

fn assert_api_error(
    error: ApiError,
    status: StatusCode,
    code: &str,
    message: &str,
    operation_id: Option<CloudSubmitOperationId>,
) {
    assert_eq!(error.status, status);
    assert_eq!(error.error.code, code);
    assert_eq!(error.error.message, message);
    assert_eq!(error.error.operation_id, operation_id);
    let encoded = serde_json::to_value(&error).expect("error serialization");
    let fields = encoded["error"].as_object().expect("error object");
    assert_eq!(fields.len(), if operation_id.is_some() { 3 } else { 2 });
    assert_eq!(fields["code"], code);
    assert_eq!(fields["message"], message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_bootstrap_sets_secure_host_cookie_and_redacts_secret() {
    let harness = harness(300);
    let token = OperatorBootstrapToken::try_new(BOOTSTRAP_SECRET).expect("token");
    assert!(!format!("{token:?}").contains(BOOTSTRAP_SECRET));
    let origin =
        P0PublicOrigin::try_new("https://OPERATOR.example:443/").expect("canonical origin");
    assert_eq!(origin.as_str(), ORIGIN_VALUE);

    let (_auth, captured) = authenticate(&harness).await;
    assert_eq!(captured.status, StatusCode::CREATED);
    assert_common_headers(&captured);
    assert_eq!(
        captured
            .headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    let cookie = captured
        .headers
        .get(SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("set-cookie");
    assert!(cookie.starts_with("__Host-codebox_p0="));
    assert!(cookie.contains("; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=300"));
    assert!(!cookie.contains("Domain="));
    assert_eq!(cookie.split(';').next().expect("cookie").len(), 18 + 43);
    let body = json_body(&captured);
    assert_eq!(body["actor"], "operator");
    assert_eq!(body["expires_in_seconds"], 300);
    assert!(
        !captured
            .body
            .windows(BOOTSTRAP_SECRET.len())
            .any(|part| { part == BOOTSTRAP_SECRET.as_bytes() })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_rejects_missing_cookie_and_wrong_origin_before_handler() {
    let harness = harness(300);
    let missing = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/login",
            None,
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(missing.status, StatusCode::UNAUTHORIZED);

    let (auth, _) = authenticate(&harness).await;
    let wrong_origin = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/login/device",
            Some(&auth),
            Some("https://evil.example"),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(wrong_origin.status, StatusCode::FORBIDDEN);
    assert_eq!(harness.login.counts(), (0, 0, 0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_login_status_and_device_code_are_exact_and_bounded() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let status = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/login",
            Some(&auth),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(json_body(&status), json!({"state":"logged_out"}));

    let device = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/login/device",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(device.status, StatusCode::ACCEPTED);
    let body = json_body(&device);
    assert_eq!(
        body["verification_url"],
        "https://auth.openai.com/codex/device"
    );
    assert_eq!(body["verification_code"], DEVICE_CODE);
    assert_eq!(body["expires_in_seconds"], 900);
    assert!(body["operation_id"].as_str().is_some());

    let canceled = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/login/cancel",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(json_body(&canceled), json!({"state":"logged_out"}));
    assert_eq!(harness.login.counts(), (1, 1, 1, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_device_code_never_enters_events_errors_or_logs() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let key = CommandId::new();
    let request = || {
        protected_request(
            Method::POST,
            "/api/p0/v1/login/device",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(key),
            None,
            None,
        )
    };
    let first = send(&harness.router, request()).await;
    let replay = send(&harness.router, request()).await;
    assert_eq!(first.body, replay.body);
    assert!(String::from_utf8_lossy(&first.body).contains(DEVICE_CODE));
    assert_eq!(harness.login.counts().1, 1);

    let session = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/session",
            Some(&auth),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    let error = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some("{"),
            Some("application/json"),
        ),
    )
    .await;
    assert!(!String::from_utf8_lossy(&session.body).contains(DEVICE_CODE));
    assert!(!String::from_utf8_lossy(&error.body).contains(DEVICE_CODE));
    let debug = format!(
        "{:?}",
        LoginInstructions {
            operation_id: LoginOperationId::new(),
            verification_url: "https://auth.openai.com/codex/device",
            verification_code: DEVICE_CODE.to_owned(),
            expires_in_seconds: 900,
        }
    );
    assert!(!debug.contains(DEVICE_CODE));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_start_turn_validates_prompt_and_returns_accepted_receipt() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let snapshot = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/session",
            Some(&auth),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(snapshot.status, StatusCode::OK);
    assert_eq!(json_body(&snapshot)["state"], "ready");

    let invalid = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(r#"{"prompt":"   "}"#),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);

    let accepted = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(&format!(r#"{{"prompt":"{PROMPT_CANARY}"}}"#)),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED);
    let body = json_body(&accepted);
    assert!(body["turn_id"].as_str().is_some());
    assert_eq!(body["high_water_seq"], 1);
    assert!(!String::from_utf8_lossy(&accepted.body).contains(PROMPT_CANARY));
    let state = harness.session.state.lock().expect("fake session");
    assert_eq!(state.start_calls, 1);
    assert_eq!(state.last_prompt.as_deref(), Some(PROMPT_CANARY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_mutations_require_current_instance_and_idempotency_key() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let missing_key = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/cancel",
            Some(&auth),
            Some(ORIGIN_VALUE),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(missing_key.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(&missing_key)["error"]["code"],
        "idempotency_key_invalid"
    );

    let mut stale = auth.clone();
    stale.instance = Uuid::new_v4().to_string();
    let stale_response = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/cancel",
            Some(&stale),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(stale_response.status, StatusCode::CONFLICT);
    assert_eq!(
        json_body(&stale_response)["error"]["code"],
        "instance_changed"
    );
    assert_eq!(harness.session.counts().1, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_same_key_replays_once_and_different_request_conflicts() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let key = CommandId::new();
    let first = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(key),
            Some(r#"{"prompt":"first"}"#),
            Some("application/json"),
        ),
    )
    .await;
    let replay = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(key),
            Some(r#"{"prompt":"first"}"#),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    assert_eq!(first.body, replay.body);
    let conflict = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(key),
            Some(r#"{"prompt":"different"}"#),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(
        json_body(&conflict)["error"]["code"],
        "idempotency_conflict"
    );
    assert_eq!(harness.session.counts().0, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_http_concurrent_same_key_joins_in_flight_response() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let gate = Arc::new(Gate::default());
    harness.session.set_start_gate(gate.clone());
    let key = CommandId::new();
    let first_request = protected_request(
        Method::POST,
        "/api/p0/v1/session/turns",
        Some(&auth),
        Some(ORIGIN_VALUE),
        Some(key),
        Some(r#"{"prompt":"concurrent"}"#),
        Some("application/json"),
    );
    let first_router = harness.router.clone();
    let first = tokio::spawn(async move { send(&first_router, first_request).await });
    let wait_gate = gate.clone();
    tokio::task::spawn_blocking(move || wait_gate.wait_entered())
        .await
        .expect("entered waiter");
    let second_router = harness.router.clone();
    let second_request = protected_request(
        Method::POST,
        "/api/p0/v1/session/turns",
        Some(&auth),
        Some(ORIGIN_VALUE),
        Some(key),
        Some(r#"{"prompt":"concurrent"}"#),
        Some("application/json"),
    );
    let second = tokio::spawn(async move { send(&second_router, second_request).await });
    std::thread::sleep(Duration::from_millis(20));
    gate.release();
    let first = first.await.expect("first response");
    let second = second.await.expect("second response");
    assert_eq!(first.body, second.body);
    assert_eq!(harness.session.counts().0, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_http_cancel_is_explicit_and_disconnect_is_not_cancel() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;

    let mutation_gate = Arc::new(Gate::default());
    harness.session.set_start_gate(mutation_gate.clone());
    let mutation_key = CommandId::new();
    let mutation_router = harness.router.clone();
    let mutation_auth = auth.clone();
    let disconnected_mutation = tokio::spawn(async move {
        send(
            &mutation_router,
            protected_request(
                Method::POST,
                "/api/p0/v1/session/turns",
                Some(&mutation_auth),
                Some(ORIGIN_VALUE),
                Some(mutation_key),
                Some(r#"{"prompt":"complete after disconnect"}"#),
                Some("application/json"),
            ),
        )
        .await
    });
    let wait_mutation_gate = mutation_gate.clone();
    tokio::task::spawn_blocking(move || wait_mutation_gate.wait_entered())
        .await
        .expect("mutation entered waiter");
    disconnected_mutation.abort();
    assert!(disconnected_mutation.await.is_err());
    mutation_gate.release();

    let replay = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(mutation_key),
            Some(r#"{"prompt":"complete after disconnect"}"#),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(replay.status, StatusCode::ACCEPTED);
    assert_eq!(harness.session.counts().0, 1);
    assert_eq!(harness.session.counts().1, 0);

    let gate = Arc::new(Gate::default());
    harness.session.set_diff_gate(gate.clone());
    let router = harness.router.clone();
    let auth_for_diff = auth.clone();
    let disconnected = tokio::spawn(async move {
        send(
            &router,
            protected_request(
                Method::GET,
                "/api/p0/v1/session/diff",
                Some(&auth_for_diff),
                None,
                None,
                None,
                None,
            ),
        )
        .await
    });
    let wait_gate = gate.clone();
    tokio::task::spawn_blocking(move || wait_gate.wait_entered())
        .await
        .expect("diff entered");
    disconnected.abort();
    gate.release();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(harness.session.counts().1, 0);

    let explicit = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/cancel",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(explicit.status, StatusCode::OK);
    assert_eq!(harness.session.counts().1, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_reconcile_does_not_resolve_or_retry() {
    let harness = harness(300);
    let operation_id = CloudSubmitOperationId::new();
    harness
        .session
        .set_snapshot(unknown_snapshot(harness.session.identity, operation_id));
    let (auth, _) = authenticate(&harness).await;
    let response = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/reconcile",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    let body = json_body(&response);
    assert_eq!(body["operation_id"], operation_id.to_string());
    assert_eq!(body["task_ids"], json!([]));
    assert_eq!(body["complete"], true);
    assert_eq!(harness.session.counts(), (0, 0, 1, 0, 0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_abandon_requires_exact_true_ack_and_current_operation() {
    let harness = harness(300);
    let operation_id = CloudSubmitOperationId::new();
    harness
        .session
        .set_snapshot(unknown_snapshot(harness.session.identity, operation_id));
    let (auth, _) = authenticate(&harness).await;
    for acknowledgement in ["false", "null"] {
        let response = send(
            &harness.router,
            protected_request(
                Method::POST,
                "/api/p0/v1/session/resolve",
                Some(&auth),
                Some(ORIGIN_VALUE),
                Some(CommandId::new()),
                Some(&format!(
                    r#"{{"operation_id":"{operation_id}","decision":{{"type":"abandon","acknowledge_duplicate_task_risk":{acknowledgement}}}}}"#
                )),
                Some("application/json"),
            ),
        )
        .await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(&response)["error"]["code"],
            "acknowledgement_required"
        );
    }
    let stale_id = CloudSubmitOperationId::new();
    let stale = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/resolve",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(&format!(
                r#"{{"operation_id":"{stale_id}","decision":{{"type":"abandon","acknowledge_duplicate_task_risk":true}}}}"#
            )),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);

    let accepted = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/resolve",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(&format!(
                r#"{{"operation_id":"{operation_id}","decision":{{"type":"abandon","acknowledge_duplicate_task_risk":true}}}}"#
            )),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::OK);
    let state = harness.session.state.lock().expect("fake session");
    assert_eq!(state.resolve_calls, 1);
    assert_eq!(state.last_resolution, Some("abandon"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_adopt_rejects_unlisted_or_stale_task() {
    let harness = harness(300);
    let operation_id = CloudSubmitOperationId::new();
    harness
        .session
        .set_snapshot(unknown_snapshot(harness.session.identity, operation_id));
    let (auth, _) = authenticate(&harness).await;
    let unlisted = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/resolve",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(&format!(
                r#"{{"operation_id":"{operation_id}","decision":{{"type":"adopt","task_id":"task_unlisted"}}}}"#
            )),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(unlisted.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(&unlisted)["error"]["code"], "task_not_listed");

    let stale_id = CloudSubmitOperationId::new();
    let stale = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/resolve",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(&format!(
                r#"{{"operation_id":"{stale_id}","decision":{{"type":"adopt","task_id":"task_allowed"}}}}"#
            )),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert_eq!(harness.session.counts().3, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_diff_is_plain_bounded_untrusted_and_not_cached() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    for _ in 0..2 {
        let response = send(
            &harness.router,
            protected_request(
                Method::GET,
                "/api/p0/v1/session/diff",
                Some(&auth),
                None,
                None,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, DIFF_CANARY.as_bytes());
        assert_eq!(
            response
                .headers
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(
            response
                .headers
                .get(CONTENT_SECURITY_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some("default-src 'none'; sandbox")
        );
        assert!(String::from_utf8_lossy(&response.body).contains("<script>"));
        assert!(response.body.len() <= 2 * 1024 * 1024);
    }
    assert_eq!(harness.session.counts().4, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_bounds_content_type_and_error_schema_fail_closed() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let wrong_media = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(r#"{"prompt":"x"}"#),
            Some("text/plain"),
        ),
    )
    .await;
    assert_eq!(wrong_media.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let oversized_body = format!(r#"{{"prompt":"{}"}}"#, "x".repeat(41 * 1024));
    let oversized = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(&oversized_body),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(oversized.status, StatusCode::PAYLOAD_TOO_LARGE);

    let malformed = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some("{"),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
    let error = json_body(&malformed);
    assert_eq!(error["error"]["code"], "malformed_json");
    assert_eq!(
        error["error"]
            .as_object()
            .expect("error object")
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["code", "message"]
    );
    let duplicate = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(r#"{"prompt":"first","prompt":"second"}"#),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(duplicate.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json_body(&duplicate)["error"]["code"], "invalid_request");

    for operation_id in ["not-a-uuid", "00000000-0000-0000-0000-000000000000"] {
        let invalid_uuid = send(
            &harness.router,
            protected_request(
                Method::POST,
                "/api/p0/v1/session/resolve",
                Some(&auth),
                Some(ORIGIN_VALUE),
                Some(CommandId::new()),
                Some(&format!(
                    r#"{{"operation_id":"{operation_id}","decision":{{"type":"abandon","acknowledge_duplicate_task_risk":true}}}}"#
                )),
                Some("application/json"),
            ),
        )
        .await;
        assert_eq!(invalid_uuid.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json_body(&invalid_uuid)["error"]["code"], "invalid_value");
    }

    let mut oversized_header_request = protected_request(
        Method::POST,
        "/api/p0/v1/session/cancel",
        Some(&auth),
        Some(ORIGIN_VALUE),
        Some(CommandId::new()),
        None,
        None,
    );
    oversized_header_request.headers_mut().insert(
        "x-oversized",
        HeaderValue::from_str(&"x".repeat(257)).expect("oversized header value"),
    );
    let oversized_header = send(&harness.router, oversized_header_request).await;
    assert_eq!(oversized_header.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        json_body(&oversized_header)["error"]["code"],
        "invalid_request"
    );
    assert_eq!(harness.session.counts().0, 0);

    let login_cases = vec![
        (
            LoginBrokerError::CredentialScope(CredentialScopeError::UnsupportedPlatform),
            StatusCode::SERVICE_UNAVAILABLE,
            "login_unavailable",
            "login credential scope is unavailable",
        ),
        (
            LoginBrokerError::VersionMismatch,
            StatusCode::SERVICE_UNAVAILABLE,
            "login_version_mismatch",
            "accepted login provider version is unavailable",
        ),
        (
            LoginBrokerError::LoginAlreadyRunning,
            StatusCode::CONFLICT,
            "login_already_running",
            "a device login is already running",
        ),
        (
            LoginBrokerError::AlreadyLoggedIn,
            StatusCode::CONFLICT,
            "already_logged_in",
            "operator is already logged in",
        ),
        (
            LoginBrokerError::ProviderOutputInvalid,
            StatusCode::SERVICE_UNAVAILABLE,
            "login_provider_drift",
            "login provider response is unavailable",
        ),
        (
            LoginBrokerError::OutputLimitExceeded,
            StatusCode::SERVICE_UNAVAILABLE,
            "login_output_limit",
            "login provider response exceeded its limit",
        ),
        (
            LoginBrokerError::StatusUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "login_status_unavailable",
            "login status is unavailable",
        ),
        (
            LoginBrokerError::LoginFailed,
            StatusCode::CONFLICT,
            "login_failed",
            "device login did not complete",
        ),
        (
            LoginBrokerError::OutcomeUnknown,
            StatusCode::CONFLICT,
            "login_outcome_unknown",
            "device login outcome requires reconciliation",
        ),
        (
            LoginBrokerError::Process {
                source: std::io::Error::other("RAW-PROCESS-CANARY"),
            },
            StatusCode::SERVICE_UNAVAILABLE,
            "login_process_unavailable",
            "login process is unavailable",
        ),
        (
            LoginBrokerError::LedgerUnavailable {
                source: std::io::Error::other("RAW-LEDGER-CANARY"),
            },
            StatusCode::SERVICE_UNAVAILABLE,
            "login_state_unavailable",
            "login state is unavailable",
        ),
        (
            LoginBrokerError::LedgerInvalid,
            StatusCode::SERVICE_UNAVAILABLE,
            "login_state_invalid",
            "login state requires operator repair",
        ),
    ];
    for (lower, status, code, message) in login_cases {
        let mapped = map_login_error(LoginPortError::Lower(lower));
        assert!(!format!("{mapped:?}").contains("RAW-"));
        assert_api_error(mapped, status, code, message, None);
    }

    let session_cases = [
        (
            P0SessionErrorCategory::InvalidConfig,
            StatusCode::SERVICE_UNAVAILABLE,
            "session_config_invalid",
            "session configuration is unavailable",
        ),
        (
            P0SessionErrorCategory::TurnAlreadyRunning,
            StatusCode::CONFLICT,
            "turn_already_running",
            "a turn is already active",
        ),
        (
            P0SessionErrorCategory::NoCurrentTurn,
            StatusCode::CONFLICT,
            "no_current_turn",
            "no current turn is available",
        ),
        (
            P0SessionErrorCategory::WrongState,
            StatusCode::CONFLICT,
            "session_wrong_state",
            "session state does not allow this operation",
        ),
        (
            P0SessionErrorCategory::WrongSession,
            StatusCode::CONFLICT,
            "session_changed",
            "session identity changed; refresh before retry",
        ),
        (
            P0SessionErrorCategory::WrongOperation,
            StatusCode::CONFLICT,
            "operation_changed",
            "current operation changed; refresh before retry",
        ),
        (
            P0SessionErrorCategory::RuntimeStopped,
            StatusCode::SERVICE_UNAVAILABLE,
            "session_stopped",
            "session runtime is stopped",
        ),
        (
            P0SessionErrorCategory::HistoryGap,
            StatusCode::CONFLICT,
            "history_gap",
            "requested session history is no longer retained",
        ),
        (
            P0SessionErrorCategory::FutureCursor,
            StatusCode::CONFLICT,
            "future_cursor",
            "requested session cursor is in the future",
        ),
        (
            P0SessionErrorCategory::SubscriberLimit,
            StatusCode::SERVICE_UNAVAILABLE,
            "subscriber_limit",
            "session subscriber limit reached",
        ),
        (
            P0SessionErrorCategory::SequenceExhausted,
            StatusCode::SERVICE_UNAVAILABLE,
            "session_sequence_exhausted",
            "session event sequence is exhausted",
        ),
        (
            P0SessionErrorCategory::LowerConflict,
            StatusCode::CONFLICT,
            "provider_state_conflict",
            "provider state changed incompatibly",
        ),
    ];
    for (category, status, code, message) in session_cases {
        let mapped = map_session_category(category).expect("non-nested session mapping");
        assert_api_error(mapped, status, code, message, None);
    }
    assert!(map_session_category(P0SessionErrorCategory::CloudLifecycle).is_none());
    assert!(map_session_category(P0SessionErrorCategory::CloudDiff).is_none());

    let lifecycle_cases = [
        (
            CloudLifecycleErrorCategory::Scope,
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_scope_unavailable",
            "provider credential scope is unavailable",
        ),
        (
            CloudLifecycleErrorCategory::Busy,
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_busy",
            "provider operation is busy",
        ),
        (
            CloudLifecycleErrorCategory::TurnAlreadyRunning,
            StatusCode::CONFLICT,
            "provider_turn_running",
            "a provider turn is already active",
        ),
        (
            CloudLifecycleErrorCategory::NoCurrentOperation,
            StatusCode::CONFLICT,
            "no_current_operation",
            "no provider operation is available",
        ),
        (
            CloudLifecycleErrorCategory::WrongState,
            StatusCode::CONFLICT,
            "provider_wrong_state",
            "provider state does not allow this operation",
        ),
        (
            CloudLifecycleErrorCategory::StaleDecision,
            StatusCode::CONFLICT,
            "recovery_decision_stale",
            "recovery decision is stale",
        ),
        (
            CloudLifecycleErrorCategory::TaskNotListed,
            StatusCode::UNPROCESSABLE_ENTITY,
            "task_not_listed",
            "task is not in the complete recovery set",
        ),
        (
            CloudLifecycleErrorCategory::AcknowledgementRequired,
            StatusCode::UNPROCESSABLE_ENTITY,
            "acknowledgement_required",
            "duplicate-task-risk acknowledgement is required",
        ),
        (
            CloudLifecycleErrorCategory::LowerRunner,
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_runner_unavailable",
            "provider runner is unavailable",
        ),
        (
            CloudLifecycleErrorCategory::ProviderRead,
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_read_unavailable",
            "provider state cannot be read",
        ),
        (
            CloudLifecycleErrorCategory::OperationConflict,
            StatusCode::CONFLICT,
            "provider_operation_conflict",
            "another provider operation owns current state",
        ),
        (
            CloudLifecycleErrorCategory::OutcomeUnknown,
            StatusCode::CONFLICT,
            "provider_outcome_unknown",
            "provider outcome requires explicit recovery",
        ),
        (
            CloudLifecycleErrorCategory::LedgerInvalid,
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_state_invalid",
            "provider state requires operator repair",
        ),
        (
            CloudLifecycleErrorCategory::LedgerUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_state_unavailable",
            "provider state is unavailable",
        ),
        (
            CloudLifecycleErrorCategory::RecoveryRequired,
            StatusCode::CONFLICT,
            "provider_recovery_required",
            "provider recovery requires operator action",
        ),
    ];
    let operation_id = CloudSubmitOperationId::new();
    for (category, status, code, message) in lifecycle_cases {
        assert_api_error(
            map_cloud_lifecycle_error(category, Some(operation_id)),
            status,
            code,
            message,
            Some(operation_id),
        );
    }
    let without_operation = map_cloud_lifecycle_error(CloudLifecycleErrorCategory::Busy, None);
    assert_api_error(
        without_operation,
        StatusCode::SERVICE_UNAVAILABLE,
        "provider_busy",
        "provider operation is busy",
        None,
    );

    let diff_cases = [
        (
            CloudDiffReadErrorCategory::IneligibleLifecycle,
            StatusCode::CONFLICT,
            "diff_not_ready",
            "current task is not eligible for diff retrieval",
        ),
        (
            CloudDiffReadErrorCategory::AuthorityMismatch,
            StatusCode::CONFLICT,
            "diff_authority_changed",
            "current task changed; refresh before retry",
        ),
        (
            CloudDiffReadErrorCategory::Scope,
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_scope_unavailable",
            "diff credential scope is unavailable",
        ),
        (
            CloudDiffReadErrorCategory::Busy,
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_busy",
            "diff provider operation is busy",
        ),
        (
            CloudDiffReadErrorCategory::Version,
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_version_mismatch",
            "accepted diff provider version is unavailable",
        ),
        (
            CloudDiffReadErrorCategory::DiagnosticBoundary,
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_boundary_unavailable",
            "diff diagnostic boundary is unavailable",
        ),
        (
            CloudDiffReadErrorCategory::Process,
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_process_unavailable",
            "diff process is unavailable",
        ),
        (
            CloudDiffReadErrorCategory::Timeout,
            StatusCode::GATEWAY_TIMEOUT,
            "diff_timeout",
            "diff retrieval timed out",
        ),
        (
            CloudDiffReadErrorCategory::Canceled,
            StatusCode::CONFLICT,
            "diff_canceled",
            "diff retrieval was canceled",
        ),
        (
            CloudDiffReadErrorCategory::OutputLimit,
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_output_limit",
            "diff exceeded its output limit",
        ),
        (
            CloudDiffReadErrorCategory::ProviderDrift,
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_provider_drift",
            "diff provider response is unavailable",
        ),
        (
            CloudDiffReadErrorCategory::InvalidDiff,
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_invalid",
            "diff display data is invalid",
        ),
    ];
    for (category, status, code, message) in diff_cases {
        assert_api_error(map_cloud_diff_error(category), status, code, message, None);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_logout_invalidates_only_current_app_session() {
    let harness = harness(300);
    let (first, _) = authenticate(&harness).await;
    let (second, _) = authenticate(&harness).await;
    let key = CommandId::new();

    let body_gate = Arc::new(Gate::default());
    let delayed_request = Request::builder()
        .method(Method::DELETE)
        .uri("/api/p0/v1/operator/session")
        .header(COOKIE, &first.cookie)
        .header("codebox-instance-id", &first.instance)
        .header(ORIGIN, ORIGIN_VALUE)
        .header("idempotency-key", key.to_string())
        .body(Body::new(GatedEmptyBody::new(body_gate.clone())))
        .expect("delayed logout request");
    let delayed_router = harness.router.clone();
    let delayed = tokio::spawn(async move { send(&delayed_router, delayed_request).await });
    let wait_body_gate = body_gate.clone();
    tokio::task::spawn_blocking(move || wait_body_gate.wait_entered())
        .await
        .expect("delayed body entered");

    let logout = send(
        &harness.router,
        protected_request(
            Method::DELETE,
            "/api/p0/v1/operator/session",
            Some(&first),
            Some(ORIGIN_VALUE),
            Some(key),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert!(logout.body.is_empty());
    assert_eq!(
        logout.headers.get(SET_COOKIE).and_then(|v| v.to_str().ok()),
        Some("__Host-codebox_p0=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
    );
    body_gate.release();
    let joined_logout = delayed.await.expect("delayed logout response");
    assert_eq!(joined_logout.status, StatusCode::NO_CONTENT);
    assert_eq!(joined_logout.body, logout.body);
    assert_eq!(
        joined_logout.headers.get(SET_COOKIE),
        logout.headers.get(SET_COOKIE)
    );
    assert_eq!(harness.plane.shared.logout_execution_count(), 1);
    assert_eq!(harness.plane.shared.idempotency.entry_count(), 0);

    let first_after = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/session",
            Some(&first),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    let second_after = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/session",
            Some(&second),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(first_after.status, StatusCode::UNAUTHORIZED);
    assert_eq!(second_after.status, StatusCode::OK);

    let duplicate = send(
        &harness.router,
        protected_request(
            Method::DELETE,
            "/api/p0/v1/operator/session",
            Some(&first),
            Some(ORIGIN_VALUE),
            Some(key),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(duplicate.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_forbids_browser_provider_and_host_configuration() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let forbidden = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(
                r#"{"prompt":"safe","executable":"/tmp/x","argv":["cloud","apply"],"path":"/repo","environment":"evil","branch":"main"}"#,
            ),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(harness.session.counts().0, 0);

    let unknown_route = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/apply",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(unknown_route.status, StatusCode::NOT_FOUND);
    assert_common_headers(&unknown_route);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_canaries_are_absent_from_debug_and_nonsecret_responses() {
    let harness = harness(300);
    let token = OperatorBootstrapToken::try_new(BOOTSTRAP_SECRET).expect("token");
    assert!(!format!("{token:?}").contains(BOOTSTRAP_SECRET));
    assert!(!format!("{:?}", harness.plane).contains(BOOTSTRAP_SECRET));
    let (auth, bootstrap) = authenticate(&harness).await;
    assert!(!String::from_utf8_lossy(&bootstrap.body).contains(BOOTSTRAP_SECRET));
    assert!(!String::from_utf8_lossy(&bootstrap.body).contains(&auth.cookie));

    let invalid = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(&format!(
                r#"{{"prompt":"{PROMPT_CANARY}","path":"/private/CREDENTIAL-CANARY"}}"#
            )),
            Some("application/json"),
        ),
    )
    .await;
    let body = String::from_utf8_lossy(&invalid.body);
    for canary in [
        BOOTSTRAP_SECRET,
        DEVICE_CODE,
        PROMPT_CANARY,
        DIFF_CANARY,
        "/private/CREDENTIAL-CANARY",
    ] {
        assert!(!body.contains(canary));
    }
    assert!(!format!("{:?}", invalid.body).contains(PROMPT_CANARY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_http_session_expiry_capacity_and_cookie_comparison_are_bounded() {
    let harness = harness(300);
    let mut sessions = Vec::new();
    for _ in 0..4 {
        let (auth, response) = authenticate(&harness).await;
        assert_eq!(response.status, StatusCode::CREATED);
        assert_eq!(json_body(&response)["expires_in_seconds"], 300);
        assert!(
            response
                .headers
                .get(SET_COOKIE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("Max-Age=300"))
        );
        sessions.push(auth);
    }
    let fifth = send(
        &harness.router,
        Request::builder()
            .method(Method::POST)
            .uri("/api/p0/v1/operator/session")
            .header(ORIGIN, ORIGIN_VALUE)
            .header(AUTHORIZATION, format!("Bearer {BOOTSTRAP_SECRET}"))
            .body(Body::empty())
            .expect("fifth bootstrap request"),
    )
    .await;
    assert_eq!(fifth.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(harness.plane.shared.app_session_count(), 4);

    let mut wrong = sessions[0].clone();
    let last = wrong.cookie.pop().expect("cookie byte");
    wrong.cookie.push(if last == 'A' { 'B' } else { 'A' });
    let wrong_response = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/session",
            Some(&wrong),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(wrong_response.status, StatusCode::UNAUTHORIZED);

    harness.clock.advance(300);
    let expired = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/session",
            Some(&sessions[0]),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(expired.status, StatusCode::UNAUTHORIZED);
    let (_, replacement) = authenticate(&harness).await;
    assert_eq!(replacement.status, StatusCode::CREATED);
    assert_eq!(harness.plane.shared.app_session_count(), 1);

    let bootstrap = OperatorBootstrapToken::try_new(BOOTSTRAP_SECRET).expect("bootstrap");
    assert!(bootstrap.matches(Some(BOOTSTRAP_SECRET.as_bytes())));
    assert!(!bootstrap.matches(Some(b"short")));
    assert!(!bootstrap.matches(Some(b"bootstrap-secret-32-bytes-value?")));
    for candidate in [
        None,
        Some(&b"x"[..]),
        Some(BOOTSTRAP_SECRET.as_bytes()),
        Some(&[b'x'; 128]),
    ] {
        assert_eq!(bootstrap.comparison_work(candidate), 128);
    }

    let cookie = [b'a'; 43];
    let mut first_mismatch = cookie;
    first_mismatch[0] = b'b';
    let mut last_mismatch = cookie;
    last_mismatch[42] = b'b';
    assert_eq!(cookie_comparison_work(&cookie, &cookie), 43);
    assert_eq!(cookie_comparison_work(&cookie, &first_mismatch), 43);
    assert_eq!(cookie_comparison_work(&cookie, &last_mismatch), 43);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_http_shutdown_drains_handlers_and_cleans_lower_runtime() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let gate = Arc::new(Gate::default());
    harness.session.set_start_gate(gate.clone());
    let shutdown_gate = Arc::new(Gate::default());
    harness.session.set_shutdown_gate(shutdown_gate.clone());
    let router = harness.router.clone();
    let start_request = protected_request(
        Method::POST,
        "/api/p0/v1/session/turns",
        Some(&auth),
        Some(ORIGIN_VALUE),
        Some(CommandId::new()),
        Some(r#"{"prompt":"shutdown race"}"#),
        Some("application/json"),
    );
    let start = tokio::spawn(async move { send(&router, start_request).await });
    let wait_gate = gate.clone();
    tokio::task::spawn_blocking(move || wait_gate.wait_entered())
        .await
        .expect("start entered");

    let plane = harness.plane.clone();
    let shutdown = tokio::spawn(async move { plane.shutdown().await });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    gate.release();
    assert_eq!(
        start.await.expect("start response").status,
        StatusCode::ACCEPTED
    );
    let wait_shutdown = shutdown_gate.clone();
    tokio::task::spawn_blocking(move || wait_shutdown.wait_entered())
        .await
        .expect("shutdown entered");
    let late_bootstrap = send(
        &harness.router,
        Request::builder()
            .method(Method::POST)
            .uri("/api/p0/v1/operator/session")
            .header(ORIGIN, ORIGIN_VALUE)
            .header(AUTHORIZATION, format!("Bearer {BOOTSTRAP_SECRET}"))
            .body(Body::empty())
            .expect("late bootstrap"),
    )
    .await;
    assert_eq!(late_bootstrap.status, StatusCode::SERVICE_UNAVAILABLE);
    shutdown_gate.release();
    shutdown
        .await
        .expect("shutdown join")
        .expect("shutdown result");
    assert_eq!(harness.session.counts().5, 1);
    assert_eq!(harness.login.counts().3, 1);
    assert_eq!(harness.plane.shared.app_session_count(), 0);
    assert_eq!(harness.plane.shared.idempotency.entry_count(), 0);

    harness.plane.shutdown().await.expect("replayed shutdown");
    assert_eq!(harness.session.counts().5, 1);
    assert_eq!(harness.login.counts().3, 1);
    let stopped = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/session",
            Some(&auth),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(stopped.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RecordedFrame {
    Text(String),
    Ping,
    Close { code: u16, reason: &'static str },
}

struct TestSocketReader {
    input: mpsc::UnboundedReceiver<ClientFrame>,
}

impl SocketReader for TestSocketReader {
    fn receive(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ClientFrame>, ()>> + Send + '_>> {
        Box::pin(async move { Ok(self.input.recv().await) })
    }
}

struct TestSocketWriter {
    output: Arc<Mutex<Vec<RecordedFrame>>>,
    started: Arc<AtomicUsize>,
    send_count: usize,
    fail_at: Option<usize>,
    delay: Duration,
}

impl SocketWriter for TestSocketWriter {
    fn send(
        &mut self,
        frame: ServerFrame,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + '_>> {
        Box::pin(async move {
            let index = self.send_count;
            self.send_count = self.send_count.saturating_add(1);
            self.started.fetch_add(1, Ordering::SeqCst);
            if self.fail_at == Some(index) {
                return Err(());
            }
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            let recorded = match frame {
                ServerFrame::Text(text) => RecordedFrame::Text(text),
                ServerFrame::Ping => RecordedFrame::Ping,
                ServerFrame::Close { code, reason } => RecordedFrame::Close { code, reason },
            };
            self.output.lock().map_err(|_| ())?.push(recorded);
            Ok(())
        })
    }
}

struct RunningSocket {
    input: mpsc::UnboundedSender<ClientFrame>,
    output: Arc<Mutex<Vec<RecordedFrame>>>,
    started: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

fn test_deadlines() -> StreamDeadlines {
    StreamDeadlines::for_test(
        Duration::from_millis(80),
        Duration::from_millis(80),
        Duration::from_millis(60),
        Duration::from_millis(10),
        Duration::from_millis(2),
        Duration::from_millis(100),
    )
}

fn websocket_admission(harness: &Harness, auth: &Auth) -> Arc<RequestAdmission> {
    let mut headers = HeaderMap::new();
    headers.insert(COOKIE, HeaderValue::from_str(&auth.cookie).expect("cookie"));
    let app_session = harness
        .plane
        .shared
        .authenticate_cookie(&headers)
        .expect("application session");
    let lifecycle = harness
        .plane
        .shared
        .lifecycle
        .admit()
        .expect("lifecycle admission");
    Arc::new(RequestAdmission::new(lifecycle, app_session))
}

fn launch_socket(
    harness: &Harness,
    auth: &Auth,
    frames: impl IntoIterator<Item = ClientFrame>,
) -> RunningSocket {
    launch_socket_with_writer(harness, auth, frames, None, Duration::ZERO)
}

fn launch_socket_with_writer(
    harness: &Harness,
    auth: &Auth,
    frames: impl IntoIterator<Item = ClientFrame>,
    fail_at: Option<usize>,
    delay: Duration,
) -> RunningSocket {
    let (input, receiver) = mpsc::unbounded_channel();
    for frame in frames {
        input.send(frame).expect("queue client frame");
    }
    let output = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(AtomicUsize::new(0));
    let reader: Box<dyn SocketReader> = Box::new(TestSocketReader { input: receiver });
    let writer: Box<dyn SocketWriter> = Box::new(TestSocketWriter {
        output: Arc::clone(&output),
        started: Arc::clone(&started),
        send_count: 0,
        fail_at,
        delay,
    });
    let admission = websocket_admission(harness, auth);
    let session: Arc<dyn SessionPort> = harness.session.clone();
    let task = tokio::spawn(run_socket(
        reader,
        writer,
        admission,
        session,
        test_deadlines(),
    ));
    RunningSocket {
        input,
        output,
        started,
        task,
    }
}

async fn wait_for_send_start(started: &AtomicUsize, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while started.load(Ordering::SeqCst) < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("socket send start deadline");
}

async fn wait_for_live_drops(session: &FakeSession, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while session.subscription_observation().2 < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("live receiver drop deadline");
}

async fn wait_for_output(output: &Arc<Mutex<Vec<RecordedFrame>>>, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if output.lock().expect("socket output").len() >= count {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("socket output deadline");
}

fn output_snapshot(output: &Arc<Mutex<Vec<RecordedFrame>>>) -> Vec<RecordedFrame> {
    output.lock().expect("socket output").clone()
}

fn subscribe_frame(session_id: SessionId, after_seq: EventSeq) -> ClientFrame {
    ClientFrame::Text(
        json!({
            "type": "subscribe",
            "protocol_version": 1,
            "session_id": session_id,
            "after_seq": after_seq
        })
        .to_string(),
    )
}

fn snapshot_at(identity: P0SessionIdentity, high_water_seq: u64) -> P0SessionSnapshot {
    P0SessionSnapshot {
        identity,
        state: P0SessionState::Ready,
        current_turn: None,
        high_water_seq: EventSeq::new(high_water_seq),
    }
}

fn event_at(session_id: SessionId, seq: u64) -> P0SessionEventEnvelope {
    P0SessionEventEnvelope {
        schema_version: 1,
        session_id,
        seq: EventSeq::new(seq),
        turn_id: None,
        payload: P0SessionEvent::TurnAccepted,
    }
}

fn text_values(output: &[RecordedFrame]) -> Vec<Value> {
    output
        .iter()
        .filter_map(|frame| match frame {
            RecordedFrame::Text(text) => {
                Some(serde_json::from_str(text).expect("WebSocket JSON frame"))
            }
            RecordedFrame::Ping | RecordedFrame::Close { .. } => None,
        })
        .collect()
}

async fn close_running(socket: RunningSocket) -> Vec<RecordedFrame> {
    let _ = socket.input.send(ClientFrame::Close);
    socket.task.await.expect("socket task");
    output_snapshot(&socket.output)
}

async fn start_loopback(
    router: Router,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let address = listener.local_addr().expect("loopback address");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .expect("loopback server");
    });
    (address, stop, task)
}

async fn connect_loopback(
    address: std::net::SocketAddr,
    auth: &Auth,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = format!("ws://{address}/api/p0/v1/session/stream")
        .into_client_request()
        .expect("WebSocket request");
    request.headers_mut().insert(
        COOKIE,
        HeaderValue::from_str(&auth.cookie).expect("WebSocket cookie"),
    );
    request
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static(ORIGIN_VALUE));
    let (socket, response) = connect_async(request).await.expect("WebSocket upgrade");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    socket
}

async fn receive_loopback_json(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    count: usize,
) -> Vec<Value> {
    let mut values = Vec::with_capacity(count);
    while values.len() < count {
        let message = socket
            .next()
            .await
            .expect("loopback frame")
            .expect("loopback WebSocket read");
        if let ClientMessage::Text(text) = message {
            values.push(serde_json::from_str(&text).expect("loopback frame JSON"));
        }
    }
    values
}

#[cfg(target_os = "linux")]
static NEXT_WS_LAYOUT: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "linux")]
struct ConcreteWsLayout {
    root: PathBuf,
    executable: PathBuf,
    codex_home: PathBuf,
    state_dir: PathBuf,
    working_dir: PathBuf,
}

#[cfg(target_os = "linux")]
impl ConcreteWsLayout {
    fn new() -> Self {
        let root = loop {
            let sequence = NEXT_WS_LAYOUT.fetch_add(1, Ordering::Relaxed);
            let candidate = Path::new("/dev/shm")
                .join(format!("codebox-t005c-{}-{sequence}", std::process::id()));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create concrete WebSocket fixture root: {error}"),
            }
        };
        set_private_mode(&root);
        let executable = root.join("codex-fixture");
        fs::write(&executable, concrete_ws_fixture()).expect("write concrete Codex fixture");
        set_private_mode(&executable);
        let codex_home = concrete_private_directory(&root, "codex-home");
        let state_dir = concrete_private_directory(&root, "state");
        let working_dir = concrete_private_directory(&root, "working");
        Self {
            root,
            executable,
            codex_home,
            state_dir,
            working_dir,
        }
    }

    fn scope(&self) -> CredentialScope {
        CredentialScope::validate(CredentialScopeConfig::new(
            self.executable.clone(),
            self.codex_home.clone(),
            self.state_dir.clone(),
            self.working_dir.clone(),
        ))
        .expect("concrete credential scope")
    }
}

#[cfg(target_os = "linux")]
impl Drop for ConcreteWsLayout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(target_os = "linux")]
fn concrete_private_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).expect("create concrete private directory");
    set_private_mode(&path);
    path
}

#[cfg(target_os = "linux")]
fn set_private_mode(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("set concrete fixture mode");
}

#[cfg(target_os = "linux")]
fn concrete_ws_fixture() -> &'static [u8] {
    br#"#!/bin/sh
case " $* " in
  " --version ")
    printf 'codex-cli 0.145.0\n'
    exit 0
    ;;
  *" login status "*)
    printf 'Logged in using ChatGPT\n' >&2
    exit 0
    ;;
esac
exit 97
"#
}

#[cfg(target_os = "linux")]
async fn assert_concrete_subscription_loopback() {
    let layout = ConcreteWsLayout::new();
    let login = LoginBroker::new(layout.scope()).expect("concrete login broker");
    let orchestrator = CloudTaskOrchestrator::new(CloudRunnerConfig::new(
        layout.scope(),
        CloudEnvironmentId::try_new("env_synthetic").expect("environment"),
        CloudBranch::try_new("main").expect("branch"),
    ))
    .expect("concrete cloud orchestrator");
    let runtime = Arc::new(
        P0SessionRuntime::new(
            orchestrator,
            P0SessionConfig::try_new(Duration::from_millis(250), 64, 8, 8).expect("session bounds"),
        )
        .expect("concrete session runtime"),
    );
    let identity = runtime.identity();
    let config = P0HttpConfig::new(
        P0PublicOrigin::try_new(ORIGIN_VALUE).expect("origin"),
        OperatorBootstrapToken::try_new(BOOTSTRAP_SECRET).expect("bootstrap"),
    )
    .try_with_session_lifetime(Duration::from_secs(300))
    .expect("session lifetime");
    let plane = Arc::new(
        P0ControlPlane::new(config, login, Arc::clone(&runtime)).expect("concrete control plane"),
    );
    let router = plane.router();
    let bootstrap = send(
        &router,
        Request::builder()
            .method(Method::POST)
            .uri("/api/p0/v1/operator/session")
            .header(ORIGIN, ORIGIN_VALUE)
            .header(AUTHORIZATION, format!("Bearer {BOOTSTRAP_SECRET}"))
            .body(Body::empty())
            .expect("concrete bootstrap"),
    )
    .await;
    let body = json_body(&bootstrap);
    let auth = Auth {
        cookie: bootstrap
            .headers
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .expect("concrete cookie")
            .to_owned(),
        instance: body["instance_id"]
            .as_str()
            .expect("concrete instance")
            .to_owned(),
    };
    let (address, stop, server) = start_loopback(router).await;
    let mut socket = connect_loopback(address, &auth).await;
    socket
        .send(ClientMessage::Text(
            json!({
                "type":"subscribe",
                "protocol_version":1,
                "session_id":identity.session_id,
                "after_seq":0
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("concrete subscribe");
    let mut kinds = Vec::new();
    while kinds.len() < 3 {
        let message = socket
            .next()
            .await
            .expect("concrete frame")
            .expect("concrete WebSocket read");
        if let ClientMessage::Text(text) = message {
            let value: Value = serde_json::from_str(&text).expect("concrete frame JSON");
            kinds.push(
                value["type"]
                    .as_str()
                    .expect("concrete frame type")
                    .to_owned(),
            );
        }
    }
    assert_eq!(kinds, ["replay_begin", "snapshot", "replay_end"]);
    socket.close(None).await.expect("concrete client close");
    let _ = stop.send(());
    server.await.expect("concrete server join");
    plane.shutdown().await.expect("concrete shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_requires_cookie_origin_and_valid_upgrade() {
    let harness = harness(300);
    let missing_cookie = send(
        &harness.router,
        Request::builder()
            .uri("/api/p0/v1/session/stream")
            .header(ORIGIN, ORIGIN_VALUE)
            .header("connection", "upgrade")
            .header(UPGRADE, "websocket")
            .header(SEC_WEBSOCKET_VERSION, "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .expect("upgrade request"),
    )
    .await;
    assert_eq!(missing_cookie.status, StatusCode::UNAUTHORIZED);

    let (auth, _) = authenticate(&harness).await;
    let wrong_origin = send(
        &harness.router,
        Request::builder()
            .uri("/api/p0/v1/session/stream")
            .header(COOKIE, &auth.cookie)
            .header(ORIGIN, "https://evil.example")
            .header("connection", "upgrade")
            .header(UPGRADE, "websocket")
            .header(SEC_WEBSOCKET_VERSION, "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .expect("wrong Origin upgrade"),
    )
    .await;
    assert_eq!(wrong_origin.status, StatusCode::FORBIDDEN);

    let invalid_upgrade = send(
        &harness.router,
        Request::builder()
            .uri("/api/p0/v1/session/stream")
            .header(COOKIE, &auth.cookie)
            .header(ORIGIN, ORIGIN_VALUE)
            .body(Body::empty())
            .expect("invalid upgrade"),
    )
    .await;
    assert_eq!(invalid_upgrade.status, StatusCode::UPGRADE_REQUIRED);
    assert_common_headers(&invalid_upgrade);
    assert_eq!(
        invalid_upgrade
            .headers
            .get(UPGRADE)
            .and_then(|value| value.to_str().ok()),
        Some("websocket")
    );
    assert_eq!(
        invalid_upgrade
            .headers
            .get(SEC_WEBSOCKET_VERSION)
            .and_then(|value| value.to_str().ok()),
        Some("13")
    );
    assert_eq!(
        json_body(&invalid_upgrade),
        json!({"error":{"code":"websocket_upgrade_required","message":"a version-13 WebSocket upgrade is required"}})
    );

    let http_10 = send(
        &harness.router,
        Request::builder()
            .version(Version::HTTP_10)
            .uri("/api/p0/v1/session/stream")
            .header(COOKIE, &auth.cookie)
            .header(ORIGIN, ORIGIN_VALUE)
            .header("connection", "upgrade")
            .header(UPGRADE, "websocket")
            .header(SEC_WEBSOCKET_VERSION, "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .expect("HTTP/1.0 upgrade"),
    )
    .await;
    assert_eq!(http_10.status, StatusCode::UPGRADE_REQUIRED);
    assert_common_headers(&http_10);
    assert_eq!(
        json_body(&http_10),
        json!({"error":{"code":"websocket_upgrade_required","message":"a version-13 WebSocket upgrade is required"}})
    );

    let (address, stop, server) = start_loopback(harness.router.clone()).await;
    let mut socket = connect_loopback(address, &auth).await;
    socket
        .send(ClientMessage::Text(
            json!({
                "type": "subscribe",
                "protocol_version": 1,
                "session_id": harness.session.identity().session_id,
                "after_seq": 0
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("subscribe");
    let first = socket
        .next()
        .await
        .expect("first frame")
        .expect("first text");
    assert!(matches!(first, ClientMessage::Text(_)));
    socket.close(None).await.expect("client close");
    let _ = stop.send(());
    server.await.expect("server join");

    #[cfg(target_os = "linux")]
    assert_concrete_subscription_loopback().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_requires_one_bounded_subscribe_before_deadline() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let timeout_socket = launch_socket(&harness, &auth, []);
    wait_for_output(&timeout_socket.output, 2).await;
    let timeout_output = close_running(timeout_socket).await;
    let timeout_values = text_values(&timeout_output);
    assert_eq!(timeout_values.len(), 1);
    assert_eq!(timeout_values[0]["code"], "subscribe_timeout");
    assert!(timeout_output.contains(&RecordedFrame::Close {
        code: 1008,
        reason: "subscribe_timeout"
    }));

    let (address, stop, server) = start_loopback(harness.router.clone()).await;
    let mut socket = connect_loopback(address, &auth).await;
    socket
        .send(ClientMessage::Frame(ClientFrameFragment::message(
            vec![b'x'; 600],
            ClientOpCode::Data(ClientFrameData::Text),
            false,
        )))
        .await
        .expect("first text fragment");
    socket
        .send(ClientMessage::Frame(ClientFrameFragment::message(
            vec![b'x'; 425],
            ClientOpCode::Data(ClientFrameData::Continue),
            true,
        )))
        .await
        .expect("final text fragment");
    let result = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("oversize rejection deadline");
    match result {
        None | Some(Err(_)) | Some(Ok(ClientMessage::Close(_))) => {}
        Some(Ok(message)) => {
            panic!("unexpected application response to oversize input: {message:?}")
        }
    }
    let _ = stop.send(());
    server.await.expect("server join");
    assert_eq!(harness.session.subscription_observation().0, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_replay_snapshot_end_then_live_order_is_exact() {
    let harness = harness(300);
    let identity = harness.session.identity();
    let replay = vec![
        event_at(identity.session_id, 1),
        event_at(identity.session_id, 2),
    ];
    let live = harness
        .session
        .configure_subscription(replay, snapshot_at(identity, 2));
    live.push(Ok(event_at(identity.session_id, 3)));
    let (auth, _) = authenticate(&harness).await;
    let socket = launch_socket(
        &harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
    );
    wait_for_output(&socket.output, 6).await;
    let output = close_running(socket).await;
    let values = text_values(&output);
    assert_eq!(
        values
            .iter()
            .map(|value| &value["type"])
            .collect::<Vec<_>>(),
        vec![
            "replay_begin",
            "event",
            "event",
            "snapshot",
            "replay_end",
            "event"
        ]
    );
    assert_eq!(values[0]["after_seq"], 0);
    assert_eq!(values[0]["high_water_seq"], 2);
    assert_eq!(values[1]["envelope"]["seq"], 1);
    assert_eq!(values[2]["envelope"]["seq"], 2);
    assert_eq!(values[3]["snapshot"]["high_water_seq"], 2);
    assert_eq!(values[3]["high_water_seq"], 2);
    assert_eq!(values[4]["high_water_seq"], 2);
    assert_eq!(values[5]["envelope"]["seq"], 3);
    assert_eq!(harness.session.subscription_observation().2, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_reconnect_after_each_retained_seq_has_no_loss_or_duplicate() {
    let harness = harness(300);
    let identity = harness.session.identity();
    let (auth, _) = authenticate(&harness).await;
    for cursor in 0..=3 {
        let replay = ((cursor + 1)..=3)
            .map(|seq| event_at(identity.session_id, seq))
            .collect();
        harness
            .session
            .configure_subscription(replay, snapshot_at(identity, 3));
        let socket = launch_socket(
            &harness,
            &auth,
            [subscribe_frame(identity.session_id, EventSeq::new(cursor))],
        );
        wait_for_output(
            &socket.output,
            usize::try_from(6 - cursor).expect("frame count"),
        )
        .await;
        let output = close_running(socket).await;
        let sequences: Vec<u64> = text_values(&output)
            .iter()
            .filter_map(|value| value["envelope"]["seq"].as_u64())
            .collect();
        assert_eq!(sequences, ((cursor + 1)..=3).collect::<Vec<_>>());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_rejects_history_gap_without_partial_replay() {
    let harness = harness(300);
    let identity = harness.session.identity();
    harness
        .session
        .set_subscription_error(SessionSubscribeError::new(
            crate::ports::SessionSubscribeErrorCategory::HistoryGap,
            Some(EventSeq::new(2)),
            Some(EventSeq::new(3)),
        ));
    let (auth, _) = authenticate(&harness).await;
    let socket = launch_socket(
        &harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
    );
    wait_for_output(&socket.output, 2).await;
    let output = close_running(socket).await;
    let values = text_values(&output);
    assert_eq!(
        values,
        vec![json!({
            "type": "error",
            "code": "history_gap",
            "message": "requested session history is no longer retained",
            "oldest_available": 2,
            "latest_available": 3
        })]
    );
    assert_eq!(
        output.last(),
        Some(&RecordedFrame::Close {
            code: 1008,
            reason: "history_gap"
        })
    );
    assert_eq!(harness.session.subscription_observation().2, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_rejects_future_wrong_session_and_unsupported_version() {
    let version_harness = harness(300);
    let identity = version_harness.session.identity();
    let (version_auth, _) = authenticate(&version_harness).await;
    let version = launch_socket(
        &version_harness,
        &version_auth,
        [ClientFrame::Text(
            json!({
                "type":"subscribe",
                "protocol_version":2,
                "session_id":identity.session_id,
                "after_seq":0
            })
            .to_string(),
        )],
    );
    wait_for_output(&version.output, 2).await;
    let version_output = close_running(version).await;
    assert_eq!(
        text_values(&version_output)[0],
        json!({
            "type":"error",
            "code":"unsupported_version",
            "message":"WebSocket protocol version is unsupported",
            "supported_version":1
        })
    );
    assert_eq!(version_harness.session.subscription_observation().0, 0);

    for invalid_id in [
        Value::String(Uuid::nil().to_string()),
        Value::String("malformed".to_owned()),
    ] {
        let harness = harness(300);
        let (auth, _) = authenticate(&harness).await;
        let socket = launch_socket(
            &harness,
            &auth,
            [ClientFrame::Text(
                json!({
                    "type":"subscribe",
                    "protocol_version":1,
                    "session_id":invalid_id,
                    "after_seq":0
                })
                .to_string(),
            )],
        );
        wait_for_output(&socket.output, 2).await;
        let output = close_running(socket).await;
        assert_eq!(text_values(&output)[0]["code"], "protocol_error");
        assert_eq!(harness.session.subscription_observation().0, 0);
    }

    let wrong_harness = harness(300);
    let (wrong_auth, _) = authenticate(&wrong_harness).await;
    wrong_harness
        .session
        .set_subscription_error(SessionSubscribeError::new(
            crate::ports::SessionSubscribeErrorCategory::WrongSession,
            None,
            None,
        ));
    let wrong_id = SessionId::try_from_uuid(Uuid::from_u128(99)).expect("wrong session");
    let wrong = launch_socket(
        &wrong_harness,
        &wrong_auth,
        [subscribe_frame(wrong_id, EventSeq::initial())],
    );
    wait_for_output(&wrong.output, 2).await;
    let wrong_output = close_running(wrong).await;
    assert_eq!(text_values(&wrong_output)[0]["code"], "wrong_session");

    let future_harness = harness(300);
    let identity = future_harness.session.identity();
    let (future_auth, _) = authenticate(&future_harness).await;
    future_harness
        .session
        .set_subscription_error(SessionSubscribeError::new(
            crate::ports::SessionSubscribeErrorCategory::FutureCursor,
            None,
            Some(EventSeq::new(3)),
        ));
    let future = launch_socket(
        &future_harness,
        &future_auth,
        [subscribe_frame(identity.session_id, EventSeq::new(4))],
    );
    wait_for_output(&future.output, 2).await;
    let future_output = close_running(future).await;
    assert_eq!(
        text_values(&future_output)[0],
        json!({
            "type":"error",
            "code":"future_cursor",
            "message":"requested session cursor is in the future",
            "latest_available":3
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_rejects_binary_unknown_fields_and_repeated_subscribe() {
    for frame in [
        ClientFrame::Binary,
        ClientFrame::Text(
            r#"{"type":"subscribe","protocol_version":1,"session_id":"00000000-0000-0000-0000-000000000001","after_seq":0,"extra":true}"#
                .to_owned(),
        ),
    ] {
        let harness = harness(300);
        let (auth, _) = authenticate(&harness).await;
        let socket = launch_socket(&harness, &auth, [frame]);
        wait_for_output(&socket.output, 2).await;
        let output = close_running(socket).await;
        assert_eq!(text_values(&output)[0]["code"], "protocol_error");
        assert_eq!(harness.session.subscription_observation().0, 0);
    }

    let deadline_harness = harness(300);
    let (deadline_auth, _) = authenticate(&deadline_harness).await;
    let deadline_socket = launch_socket(&deadline_harness, &deadline_auth, []);
    let pings = deadline_socket.input.clone();
    let pinger = tokio::spawn(async move {
        for sequence in 0..8 {
            tokio::time::sleep(Duration::from_millis(15)).await;
            let frame = if sequence % 2 == 0 {
                ClientFrame::Ping
            } else {
                ClientFrame::Pong
            };
            if pings.send(frame).is_err() {
                return;
            }
        }
    });
    wait_for_output(&deadline_socket.output, 2).await;
    let deadline_output = close_running(deadline_socket).await;
    pinger.await.expect("control-frame pinger");
    assert_eq!(
        text_values(&deadline_output)[0]["code"],
        "subscribe_timeout"
    );
    assert_eq!(deadline_harness.session.subscription_observation().0, 0);

    let heartbeat_harness = harness(300);
    let heartbeat_identity = heartbeat_harness.session.identity();
    heartbeat_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(heartbeat_identity, 0));
    let (heartbeat_auth, _) = authenticate(&heartbeat_harness).await;
    let heartbeat = launch_socket(
        &heartbeat_harness,
        &heartbeat_auth,
        [subscribe_frame(
            heartbeat_identity.session_id,
            EventSeq::initial(),
        )],
    );
    wait_for_output(&heartbeat.output, 3).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!output_snapshot(&heartbeat.output).contains(&RecordedFrame::Ping));
    wait_for_output(&heartbeat.output, 4).await;
    assert_eq!(
        output_snapshot(&heartbeat.output)
            .iter()
            .filter(|frame| **frame == RecordedFrame::Ping)
            .count(),
        1
    );
    let _ = close_running(heartbeat).await;

    let harness = harness(300);
    let identity = harness.session.identity();
    harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(identity, 0));
    let (auth, _) = authenticate(&harness).await;
    let socket = launch_socket(
        &harness,
        &auth,
        [
            ClientFrame::Ping,
            ClientFrame::Pong,
            subscribe_frame(identity.session_id, EventSeq::initial()),
        ],
    );
    wait_for_output(&socket.output, 3).await;
    socket.input.send(ClientFrame::Ping).expect("client ping");
    socket.input.send(ClientFrame::Pong).expect("client pong");
    socket
        .input
        .send(subscribe_frame(identity.session_id, EventSeq::initial()))
        .expect("repeated subscribe");
    wait_for_output(&socket.output, 5).await;
    let output = close_running(socket).await;
    let values = text_values(&output);
    assert_eq!(values[0]["type"], "replay_begin");
    assert_eq!(values[2]["type"], "replay_end");
    assert_eq!(values[3]["code"], "protocol_error");
    assert_eq!(
        output.last(),
        Some(&RecordedFrame::Close {
            code: 1008,
            reason: "protocol_error"
        })
    );
    assert_eq!(harness.session.subscription_observation().0, 1);
}

async fn assert_live_handoff_closes_replay_publication_race_once() {
    let harness = harness(300);
    let identity = harness.session.identity();
    let live = harness.session.configure_subscription(
        vec![event_at(identity.session_id, 1)],
        snapshot_at(identity, 1),
    );
    live.push(Ok(event_at(identity.session_id, 2)));
    live.push(Ok(event_at(identity.session_id, 3)));
    let (auth, _) = authenticate(&harness).await;
    let socket = launch_socket(
        &harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
    );
    wait_for_output(&socket.output, 7).await;
    let output = close_running(socket).await;
    let values = text_values(&output);
    let kinds: Vec<&str> = values
        .iter()
        .map(|value| value["type"].as_str().expect("frame type"))
        .collect();
    assert_eq!(
        kinds,
        [
            "replay_begin",
            "event",
            "snapshot",
            "replay_end",
            "event",
            "event"
        ]
    );
    let sequences: Vec<u64> = values
        .iter()
        .filter_map(|value| value["envelope"]["seq"].as_u64())
        .collect();
    assert_eq!(sequences, [1, 2, 3]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_live_handoff_closes_replay_publication_race() {
    for _ in 0..10 {
        assert_live_handoff_closes_replay_publication_race_once().await;
    }
}

async fn assert_slow_consumer_closes_only_its_connection_once() {
    let base_harness = harness(300);
    let identity = base_harness.session.identity();
    let live = base_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(identity, 0));
    live.push(Err(LiveEventError::Lagged));
    let (auth, _) = authenticate(&base_harness).await;
    let lagged = launch_socket(
        &base_harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
    );
    wait_for_output(&lagged.output, 5).await;
    let lagged_output = close_running(lagged).await;
    assert_eq!(text_values(&lagged_output)[3]["code"], "subscriber_lagged");
    assert_eq!(
        lagged_output.last(),
        Some(&RecordedFrame::Close {
            code: 1013,
            reason: "subscriber_lagged"
        })
    );

    base_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(identity, 0));
    let healthy = launch_socket(
        &base_harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
    );
    wait_for_output(&healthy.output, 3).await;
    let healthy_output = close_running(healthy).await;
    assert_eq!(text_values(&healthy_output).len(), 3);
    assert_eq!(base_harness.session.counts(), (0, 0, 0, 0, 0, 0));

    for (category, code) in [
        (
            crate::ports::SessionSubscribeErrorCategory::SubscriberLimit,
            "subscriber_limit",
        ),
        (
            crate::ports::SessionSubscribeErrorCategory::Unavailable,
            "stream_unavailable",
        ),
    ] {
        let rejected_harness = harness(300);
        let rejected_identity = rejected_harness.session.identity();
        rejected_harness
            .session
            .set_subscription_error(SessionSubscribeError::new(category, None, None));
        let (rejected_auth, _) = authenticate(&rejected_harness).await;
        let rejected = launch_socket(
            &rejected_harness,
            &rejected_auth,
            [subscribe_frame(
                rejected_identity.session_id,
                EventSeq::initial(),
            )],
        );
        wait_for_output(&rejected.output, 2).await;
        let rejected_output = close_running(rejected).await;
        assert_eq!(text_values(&rejected_output)[0]["code"], code);
        assert_eq!(
            rejected_output.last(),
            Some(&RecordedFrame::Close {
                code: 1013,
                reason: code
            })
        );
        assert_eq!(rejected_harness.session.subscription_observation().2, 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_slow_consumer_closes_only_its_connection() {
    for _ in 0..10 {
        assert_slow_consumer_closes_only_its_connection_once().await;
    }
}

async fn assert_disconnect_never_cancels_or_mutates_session_once() {
    let expiry_harness = harness(300);
    let identity = expiry_harness.session.identity();
    expiry_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(identity, 0));
    let (auth, _) = authenticate(&expiry_harness).await;
    let peer = launch_socket(
        &expiry_harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
    );
    wait_for_output(&peer.output, 3).await;
    let peer_output = close_running(peer).await;
    assert_eq!(text_values(&peer_output).len(), 3);

    let expired = launch_socket(
        &expiry_harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
    );
    wait_for_output(&expired.output, 3).await;
    expiry_harness.clock.advance(300);
    wait_for_output(&expired.output, 5).await;
    let expired_output = close_running(expired).await;
    assert_eq!(
        text_values(&expired_output)[3]["code"],
        "authentication_expired"
    );

    let admission_harness = harness(300);
    let admission_identity = admission_harness.session.identity();
    admission_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(admission_identity, 0));
    let admission_gate = Arc::new(Gate::default());
    admission_harness
        .session
        .set_subscribe_gate(Arc::clone(&admission_gate));
    let (admission_auth, _) = authenticate(&admission_harness).await;
    let admission_socket = launch_socket(
        &admission_harness,
        &admission_auth,
        [subscribe_frame(
            admission_identity.session_id,
            EventSeq::initial(),
        )],
    );
    let wait_gate = Arc::clone(&admission_gate);
    tokio::task::spawn_blocking(move || wait_gate.wait_entered())
        .await
        .expect("subscribe admission entered");
    admission_harness.clock.advance(300);
    wait_for_output(&admission_socket.output, 2).await;
    let admission_output = close_running(admission_socket).await;
    assert_eq!(
        text_values(&admission_output)[0]["code"],
        "authentication_expired"
    );
    admission_gate.release();
    wait_for_live_drops(&admission_harness.session, 1).await;

    let logout_harness = harness(300);
    let logout_identity = logout_harness.session.identity();
    logout_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(logout_identity, 0));
    let (logout_auth, _) = authenticate(&logout_harness).await;
    let logout_socket = launch_socket(
        &logout_harness,
        &logout_auth,
        [subscribe_frame(
            logout_identity.session_id,
            EventSeq::initial(),
        )],
    );
    wait_for_output(&logout_socket.output, 3).await;
    let logout = send(
        &logout_harness.router,
        protected_request(
            Method::DELETE,
            "/api/p0/v1/operator/session",
            Some(&logout_auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    wait_for_output(&logout_socket.output, 5).await;
    let logout_output = close_running(logout_socket).await;
    assert_eq!(
        text_values(&logout_output)[3]["code"],
        "authentication_expired"
    );
    assert_eq!(expiry_harness.session.counts(), (0, 0, 0, 0, 0, 0));
    assert_eq!(logout_harness.session.counts(), (0, 0, 0, 0, 0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_ws_disconnect_never_cancels_or_mutates_session() {
    for _ in 0..10 {
        assert_disconnect_never_cancels_or_mutates_session_once().await;
    }
}

async fn assert_shutdown_and_send_failure_remove_subscriber_once() {
    let shutdown_harness = harness(300);
    let identity = shutdown_harness.session.identity();
    shutdown_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(identity, 0));
    let (auth, _) = authenticate(&shutdown_harness).await;
    let socket = launch_socket_with_writer(
        &shutdown_harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
        None,
        Duration::from_millis(40),
    );
    wait_for_send_start(&socket.started, 1).await;
    let plane = Arc::clone(&shutdown_harness.plane);
    let shutdown = tokio::spawn(async move { plane.shutdown().await });
    wait_for_output(&socket.output, 2).await;
    let output = output_snapshot(&socket.output);
    assert!(matches!(output.first(), Some(RecordedFrame::Text(_))));
    assert_eq!(
        output.last(),
        Some(&RecordedFrame::Close {
            code: 1012,
            reason: "server_shutdown"
        })
    );
    assert!(!socket.task.is_finished(), "close grace was not observed");
    drop(socket.input);
    tokio::time::timeout(Duration::from_millis(150), socket.task)
        .await
        .expect("close grace bound")
        .expect("shutdown socket");
    shutdown
        .await
        .expect("shutdown join")
        .expect("shutdown result");
    assert_eq!(shutdown_harness.session.subscription_observation().2, 1);

    let admission_harness = harness(300);
    let admission_identity = admission_harness.session.identity();
    admission_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(admission_identity, 0));
    let admission_gate = Arc::new(Gate::default());
    admission_harness
        .session
        .set_subscribe_gate(Arc::clone(&admission_gate));
    let (admission_auth, _) = authenticate(&admission_harness).await;
    let admission_socket = launch_socket(
        &admission_harness,
        &admission_auth,
        [subscribe_frame(
            admission_identity.session_id,
            EventSeq::initial(),
        )],
    );
    let wait_gate = Arc::clone(&admission_gate);
    tokio::task::spawn_blocking(move || wait_gate.wait_entered())
        .await
        .expect("shutdown subscribe admission entered");
    let admission_plane = Arc::clone(&admission_harness.plane);
    let admission_shutdown = tokio::spawn(async move { admission_plane.shutdown().await });
    wait_for_output(&admission_socket.output, 1).await;
    assert_eq!(
        output_snapshot(&admission_socket.output).last(),
        Some(&RecordedFrame::Close {
            code: 1012,
            reason: "server_shutdown"
        })
    );
    drop(admission_socket.input);
    admission_socket
        .task
        .await
        .expect("admission shutdown socket");
    tokio::time::timeout(Duration::from_millis(150), admission_shutdown)
        .await
        .expect("shutdown did not wait for blocked subscribe")
        .expect("admission shutdown join")
        .expect("admission shutdown result");
    admission_gate.release();
    wait_for_live_drops(&admission_harness.session, 1).await;

    let failure_harness = harness(300);
    let failure_identity = failure_harness.session.identity();
    failure_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(failure_identity, 0));
    let (failure_auth, _) = authenticate(&failure_harness).await;
    let failure = launch_socket_with_writer(
        &failure_harness,
        &failure_auth,
        [subscribe_frame(
            failure_identity.session_id,
            EventSeq::initial(),
        )],
        Some(0),
        Duration::ZERO,
    );
    failure.task.await.expect("send failure socket");
    assert_eq!(failure_harness.session.subscription_observation().2, 1);
    assert!(output_snapshot(&failure.output).is_empty());

    let timeout_harness = harness(300);
    let timeout_identity = timeout_harness.session.identity();
    timeout_harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(timeout_identity, 0));
    let (timeout_auth, _) = authenticate(&timeout_harness).await;
    let timeout = launch_socket_with_writer(
        &timeout_harness,
        &timeout_auth,
        [subscribe_frame(
            timeout_identity.session_id,
            EventSeq::initial(),
        )],
        None,
        Duration::from_millis(200),
    );
    tokio::time::timeout(Duration::from_millis(150), timeout.task)
        .await
        .expect("bounded write timeout")
        .expect("write timeout socket");
    assert_eq!(timeout_harness.session.subscription_observation().2, 1);
    assert!(output_snapshot(&timeout.output).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_ws_shutdown_and_send_failure_remove_subscriber() {
    for _ in 0..10 {
        assert_shutdown_and_send_failure_remove_subscriber_once().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_frames_and_errors_exclude_sensitive_canaries() {
    let harness = harness(300);
    let (auth, _) = authenticate(&harness).await;
    let canary = format!(
        r#"{{"type":"subscribe","protocol_version":1,"session_id":"{PROMPT_CANARY}{DIFF_CANARY}{DEVICE_CODE}{BOOTSTRAP_SECRET}","after_seq":0}}"#
    );
    let socket = launch_socket(&harness, &auth, [ClientFrame::Text(canary)]);
    wait_for_output(&socket.output, 2).await;
    let output = close_running(socket).await;
    let rendered = format!("{output:?}");
    for forbidden in [
        PROMPT_CANARY,
        DIFF_CANARY,
        DEVICE_CODE,
        BOOTSTRAP_SECRET,
        &auth.cookie,
        "/private/operator/path",
    ] {
        assert!(!rendered.contains(forbidden));
    }
    assert_eq!(text_values(&output)[0]["code"], "protocol_error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_ws_chunk_partition_and_reconnect_model_preserves_order() {
    let harness = harness(300);
    let identity = harness.session.identity();
    harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(identity, 0));
    let (auth, _) = authenticate(&harness).await;
    let (address, stop, server) = start_loopback(harness.router.clone()).await;
    let subscribe = json!({
        "type":"subscribe",
        "protocol_version":1,
        "session_id":identity.session_id,
        "after_seq":0
    })
    .to_string();
    for split in [1, subscribe.len() / 2, subscribe.len() - 1] {
        let mut socket = connect_loopback(address, &auth).await;
        socket
            .send(ClientMessage::Frame(ClientFrameFragment::message(
                subscribe.as_bytes()[..split].to_vec(),
                ClientOpCode::Data(ClientFrameData::Text),
                false,
            )))
            .await
            .expect("first valid subscribe fragment");
        socket
            .send(ClientMessage::Frame(ClientFrameFragment::message(
                subscribe.as_bytes()[split..].to_vec(),
                ClientOpCode::Data(ClientFrameData::Continue),
                true,
            )))
            .await
            .expect("final valid subscribe fragment");
        let mut kinds = Vec::new();
        while kinds.len() < 3 {
            let message = socket
                .next()
                .await
                .expect("fragmented subscribe frame")
                .expect("fragmented subscribe read");
            if let ClientMessage::Text(text) = message {
                let value: Value = serde_json::from_str(&text).expect("fragmented frame JSON");
                kinds.push(
                    value["type"]
                        .as_str()
                        .expect("fragmented frame type")
                        .to_owned(),
                );
            }
        }
        assert_eq!(kinds, ["replay_begin", "snapshot", "replay_end"]);
        socket.close(None).await.expect("fragmented client close");
    }
    let _ = stop.send(());
    server.await.expect("fragmentation server join");

    for partitions in [
        vec![8],
        vec![1, 1, 1, 1, 1, 1, 1, 1],
        vec![3, 1, 4],
        vec![2, 5, 1],
    ] {
        let mut cursor = 0u64;
        let mut observed = Vec::new();
        for width in partitions {
            let high_water = (cursor + width).min(8);
            let replay = ((cursor + 1)..=high_water)
                .map(|seq| event_at(identity.session_id, seq))
                .collect();
            harness
                .session
                .configure_subscription(replay, snapshot_at(identity, high_water));
            let socket = launch_socket(
                &harness,
                &auth,
                [subscribe_frame(identity.session_id, EventSeq::new(cursor))],
            );
            let expected_frame_count =
                usize::try_from(high_water - cursor + 3).expect("bounded frame count");
            wait_for_output(&socket.output, expected_frame_count).await;
            let output = close_running(socket).await;
            let sequences: Vec<u64> = text_values(&output)
                .iter()
                .filter_map(|frame| frame["envelope"]["seq"].as_u64())
                .collect();
            assert_eq!(sequences, ((cursor + 1)..=high_water).collect::<Vec<_>>());
            observed.extend(sequences);
            cursor = high_water;
        }
        assert_eq!(cursor, 8);
        assert_eq!(observed, (1..=8).collect::<Vec<_>>());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_composition_operator_flow_replays_without_local_execution() {
    let harness = harness(300);
    let identity = harness.session.identity();
    harness.session.enable_composition_start_event();
    harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(identity, 0));
    let (auth, _) = authenticate(&harness).await;
    let (address, stop, server) = start_loopback(harness.router.clone()).await;

    let mut initial = connect_loopback(address, &auth).await;
    initial
        .send(ClientMessage::Text(
            json!({
                "type":"subscribe",
                "protocol_version":1,
                "session_id":identity.session_id,
                "after_seq":0
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("initial composition subscribe");
    let initial_frames = receive_loopback_json(&mut initial, 3).await;
    assert_eq!(
        initial_frames
            .iter()
            .map(|frame| frame["type"].as_str().expect("initial frame type"))
            .collect::<Vec<_>>(),
        ["replay_begin", "snapshot", "replay_end"]
    );

    let start = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(r#"{"prompt":"composition turn"}"#),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(start.status, StatusCode::ACCEPTED);
    assert_eq!(json_body(&start)["high_water_seq"], 1);
    let live = receive_loopback_json(&mut initial, 1).await;
    assert_eq!(live[0]["type"], "event");
    assert_eq!(live[0]["envelope"]["seq"], 1);
    assert_eq!(live[0]["envelope"]["payload"]["type"], "turn_accepted");
    initial.close(None).await.expect("initial disconnect");

    let mut replay = connect_loopback(address, &auth).await;
    replay
        .send(ClientMessage::Text(
            json!({
                "type":"subscribe",
                "protocol_version":1,
                "session_id":identity.session_id,
                "after_seq":0
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("composition replay subscribe");
    let replay_frames = receive_loopback_json(&mut replay, 4).await;
    assert_eq!(
        replay_frames
            .iter()
            .map(|frame| frame["type"].as_str().expect("replay frame type"))
            .collect::<Vec<_>>(),
        ["replay_begin", "event", "snapshot", "replay_end"]
    );
    assert_eq!(replay_frames[1]["envelope"]["seq"], 1);
    replay.close(None).await.expect("replay disconnect");

    let mut current = connect_loopback(address, &auth).await;
    current
        .send(ClientMessage::Text(
            json!({
                "type":"subscribe",
                "protocol_version":1,
                "session_id":identity.session_id,
                "after_seq":1
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("current composition subscribe");
    let current_frames = receive_loopback_json(&mut current, 3).await;
    assert_eq!(
        current_frames
            .iter()
            .map(|frame| frame["type"].as_str().expect("current frame type"))
            .collect::<Vec<_>>(),
        ["replay_begin", "snapshot", "replay_end"]
    );
    current.close(None).await.expect("current disconnect");
    let _ = stop.send(());
    server.await.expect("composition server join");

    assert_eq!(harness.session.counts(), (1, 0, 0, 0, 0, 0));
    assert_eq!(harness.login.counts(), (0, 0, 0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_composition_secret_and_disconnect_boundaries_hold() {
    let harness = harness(300);
    let identity = harness.session.identity();
    harness.session.enable_composition_start_event();
    harness
        .session
        .configure_subscription(Vec::new(), snapshot_at(identity, 0));
    let (auth, bootstrap) = authenticate(&harness).await;
    assert!(!String::from_utf8_lossy(&bootstrap.body).contains(BOOTSTRAP_SECRET));
    assert!(!String::from_utf8_lossy(&bootstrap.body).contains(&auth.cookie));

    let forbidden = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(
                r#"{"prompt":"safe","executable":"/private/CREDENTIAL-CANARY","argv":["cloud","apply"],"path":"/repo","environment":"evil","branch":"main"}"#,
            ),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(harness.session.counts().0, 0);
    let apply = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/apply",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(apply.status, StatusCode::NOT_FOUND);

    let socket = launch_socket(
        &harness,
        &auth,
        [subscribe_frame(identity.session_id, EventSeq::initial())],
    );
    wait_for_output(&socket.output, 3).await;
    let login = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/login/device",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(login.status, StatusCode::ACCEPTED);
    assert_eq!(json_body(&login)["verification_code"], DEVICE_CODE);

    let start = send(
        &harness.router,
        protected_request(
            Method::POST,
            "/api/p0/v1/session/turns",
            Some(&auth),
            Some(ORIGIN_VALUE),
            Some(CommandId::new()),
            Some(&format!(r#"{{"prompt":"{PROMPT_CANARY}"}}"#)),
            Some("application/json"),
        ),
    )
    .await;
    assert_eq!(start.status, StatusCode::ACCEPTED);
    wait_for_output(&socket.output, 4).await;
    let diff = send(
        &harness.router,
        protected_request(
            Method::GET,
            "/api/p0/v1/session/diff",
            Some(&auth),
            None,
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(diff.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&diff.body).contains(DIFF_CANARY));
    socket
        .input
        .send(ClientFrame::Text(
            r#"{"type":"subscribe","path":"/private/CREDENTIAL-CANARY"}"#.to_owned(),
        ))
        .expect("invalid repeated composition frame");
    wait_for_output(&socket.output, 6).await;
    let output = close_running(socket).await;
    let rendered = format!("{output:?}");
    for canary in [
        BOOTSTRAP_SECRET,
        DEVICE_CODE,
        PROMPT_CANARY,
        DIFF_CANARY,
        "/private/CREDENTIAL-CANARY",
        &auth.cookie,
    ] {
        assert!(!rendered.contains(canary));
    }
    assert_eq!(
        text_values(&output)[3]["envelope"]["payload"]["type"],
        "turn_accepted"
    );
    assert_eq!(text_values(&output)[4]["code"], "protocol_error");
    assert_eq!(harness.session.counts(), (1, 0, 0, 0, 1, 0));
    assert_eq!(harness.login.counts(), (0, 1, 0, 0));
}

fn assert_web_security_headers(captured: &Captured, content_type: &str) {
    assert_eq!(
        captured
            .headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(content_type)
    );
    assert_eq!(
        captured
            .headers
            .get(CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    for (name, expected) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
    ] {
        assert_eq!(
            captured.headers.get(name).and_then(|v| v.to_str().ok()),
            Some(expected),
            "{name}"
        );
    }
    assert_eq!(
        captured
            .headers
            .get(CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some(
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self' wss://operator.example; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none'"
        )
    );
}

#[tokio::test]
async fn p0_web_serves_exact_embedded_assets_with_security_headers() {
    let harness = harness(300);
    let assets = [
        ("/", crate::web::INDEX_HTML, "text/html; charset=utf-8"),
        (
            "/assets/p0-app.js",
            crate::web::APP_JS,
            "text/javascript; charset=utf-8",
        ),
        (
            "/assets/p0-client.js",
            crate::web::CLIENT_JS,
            "text/javascript; charset=utf-8",
        ),
        (
            "/assets/p0.css",
            crate::web::STYLE_CSS,
            "text/css; charset=utf-8",
        ),
    ];

    for (path, expected, content_type) in assets {
        let get = send(
            &harness.router,
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Body::empty())
                .expect("static GET"),
        )
        .await;
        assert_eq!(get.status, StatusCode::OK, "{path}");
        assert_eq!(get.body, expected, "{path}");
        assert_web_security_headers(&get, content_type);

        let head = send(
            &harness.router,
            Request::builder()
                .method(Method::HEAD)
                .uri(path)
                .body(Body::empty())
                .expect("static HEAD"),
        )
        .await;
        assert_eq!(head.status, StatusCode::OK, "{path}");
        assert!(head.body.is_empty(), "{path}");
        assert_web_security_headers(&head, content_type);
    }

    let html = String::from_utf8_lossy(crate::web::INDEX_HTML);
    assert!(html.contains(r#"<script type="module" src="/assets/p0-app.js"></script>"#));
    for forbidden in [
        "<style",
        "javascript:",
        "<iframe",
        "<object",
        "<embed",
        "<base",
        "<form",
        "http://",
        "https://",
    ] {
        assert!(!html.contains(forbidden), "{forbidden}");
    }
    let scripts = format!(
        "{}\n{}",
        String::from_utf8_lossy(crate::web::APP_JS),
        String::from_utf8_lossy(crate::web::CLIENT_JS)
    );
    for forbidden in [
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
    ] {
        assert!(!scripts.contains(forbidden), "{forbidden}");
    }
}

#[tokio::test]
async fn p0_web_rejects_unknown_paths_and_methods_without_filesystem_lookup() {
    let harness = harness(300);
    for path in [
        "/Cargo.toml",
        "/assets/Cargo.toml",
        "/assets/p0-client.js/extra",
        "/assets/%2e%2e/Cargo.toml",
    ] {
        let captured = send(
            &harness.router,
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Body::empty())
                .expect("unknown route"),
        )
        .await;
        assert_eq!(captured.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(json_body(&captured)["error"]["code"], "not_found");
        assert!(!captured.body.starts_with(b"<!doctype html>"));
    }

    for (method, path) in [
        (Method::POST, "/"),
        (Method::PUT, "/assets/p0-app.js"),
        (Method::DELETE, "/assets/p0.css"),
    ] {
        let captured = send(
            &harness.router,
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .expect("unsupported static method"),
        )
        .await;
        assert_eq!(captured.status, StatusCode::METHOD_NOT_ALLOWED, "{path}");
        assert_eq!(json_body(&captured)["error"]["code"], "method_not_allowed");
        assert!(!captured.body.starts_with(b"<!doctype html>"));
    }
}
