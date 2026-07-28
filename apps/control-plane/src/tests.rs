use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{
    HeaderMap, HeaderValue, Method, Request, StatusCode,
    header::{
        AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ORIGIN,
        SET_COOKIE,
    },
};
use codebox_agent_codex::{
    CloudCapture, CloudDiff, CloudDiffReadErrorCategory, CloudLifecycleErrorCategory, CloudPrompt,
    CloudSubmitOperationId, CredentialScopeError, LoginBrokerError, LoginOperationId, LoginStatus,
    UnknownSubmitDecision, decode_cloud_diff,
};
use codebox_domain::{CommandId, EventSeq, SessionId, TurnId};
use codebox_session_runtime::{
    P0Actor, P0CloudLifecycle, P0InstanceId, P0RecoveryCandidates, P0SessionErrorCategory,
    P0SessionIdentity, P0SessionSnapshot, P0SessionState, P0TurnProjection, P0TurnReceipt,
    P0TurnSnapshot,
};
use http_body::{Body as HttpBody, Frame};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::config::{OperatorBootstrapToken, P0HttpConfig, P0PublicOrigin};
use crate::error::{
    ApiError, map_cloud_diff_error, map_cloud_lifecycle_error, map_login_error,
    map_session_category,
};
use crate::ports::{LoginInstructions, LoginPort, LoginPortError, SessionPort, SessionPortError};
use crate::state::{EntropySource, MonotonicClock, P0ControlPlane, cookie_comparison_work};

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
    diff: String,
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
                diff: DIFF_CANARY.to_owned(),
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
        let gate = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| SessionPortError::Unavailable)?;
            state.start_calls += 1;
            state.last_prompt = Some(prompt.as_str().to_owned());
            state.start_gate.clone()
        };
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        Ok(P0TurnReceipt {
            turn_id: TurnId::new(),
            high_water_seq: EventSeq::new(1),
        })
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

macro_rules! ws_contract_skeleton {
    ($name:ident) => {
        #[test]
        #[ignore = "T005C contract skeleton"]
        fn $name() {}
    };
}

ws_contract_skeleton!(p0_ws_requires_cookie_origin_and_valid_upgrade);
ws_contract_skeleton!(p0_ws_requires_one_bounded_subscribe_before_deadline);
ws_contract_skeleton!(p0_ws_replay_snapshot_end_then_live_order_is_exact);
ws_contract_skeleton!(p0_ws_reconnect_after_each_retained_seq_has_no_loss_or_duplicate);
ws_contract_skeleton!(p0_ws_rejects_history_gap_without_partial_replay);
ws_contract_skeleton!(p0_ws_rejects_future_wrong_session_and_unsupported_version);
ws_contract_skeleton!(p0_ws_rejects_binary_unknown_fields_and_repeated_subscribe);
ws_contract_skeleton!(p0_ws_live_handoff_closes_replay_publication_race);
ws_contract_skeleton!(p0_ws_slow_consumer_closes_only_its_connection);
ws_contract_skeleton!(p0_ws_disconnect_never_cancels_or_mutates_session);
ws_contract_skeleton!(p0_ws_shutdown_and_send_failure_remove_subscriber);
ws_contract_skeleton!(p0_ws_frames_and_errors_exclude_sensitive_canaries);
ws_contract_skeleton!(p0_ws_chunk_partition_and_reconnect_model_preserves_order);
