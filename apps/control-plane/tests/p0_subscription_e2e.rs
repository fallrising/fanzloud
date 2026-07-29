#![cfg(target_os = "linux")]

use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE,
};
use axum::http::{Method, Request, StatusCode};
use codebox_agent_codex::{
    CloudBranch, CloudEnvironmentId, CloudRunnerConfig, CloudTaskOrchestrator, CredentialScope,
    CredentialScopeConfig, LoginBroker,
};
use codebox_control_plane::{OperatorBootstrapToken, P0ControlPlane, P0HttpConfig, P0PublicOrigin};
use codebox_session_runtime::{P0SessionConfig, P0SessionRuntime};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt;

const ORIGIN_VALUE: &str = "https://operator.example";
const BOOTSTRAP_SECRET: &str = "T007-bootstrap-secret-32-bytes-value!";
const CREDENTIAL_CANARY: &str = "T007-CREDENTIAL-CANARY";
const PROMPT_CANARY: &str = "T007 deterministic prompt canary";
const STATUS_CANARY: &str = "[READY] Synthetic documentation update";
const DIFF_CANARY: &str = concat!(
    "diff --git a/README.md b/README.md\n",
    "index 1111111..2222222 100644\n",
    "--- a/README.md\n",
    "+++ b/README.md\n",
    "@@ -1 +1,2 @@\n",
    " Existing text\n",
    "+Synthetic text\n",
);
const TASK_ID: &str = "task_i_abc123";
const TURN_KEY: &str = "11111111-1111-4111-8111-111111111111";
const LOGOUT_KEY: &str = "22222222-2222-4222-8222-222222222222";
const DEADLINE: Duration = Duration::from_secs(5);
const MAX_LEDGER_BYTES: u64 = 4 * 1024;

static NEXT_LAYOUT: AtomicU64 = AtomicU64::new(0);

type TestResult<T> = Result<T, &'static str>;
type TestSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct FixtureLayout {
    root: PathBuf,
    executable: PathBuf,
    codex_home: PathBuf,
    state_dir: PathBuf,
    working_dir: PathBuf,
    ledger: PathBuf,
    cleaned: bool,
}

impl FixtureLayout {
    fn new() -> TestResult<Self> {
        let root = loop {
            let sequence = NEXT_LAYOUT.fetch_add(1, Ordering::Relaxed);
            let candidate = Path::new("/dev/shm")
                .join(format!("codebox-t007-{}-{sequence}", std::process::id()));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err("T007 fixture root creation failed"),
            }
        };
        set_mode(&root, 0o700)?;
        let executable = root.join("codex-fixture");
        let codex_home = private_directory(&root, "codex-home")?;
        let state_dir = private_directory(&root, "state")?;
        let working_dir = private_directory(&root, "working")?;
        let ledger = root.join("invocations.log");
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&ledger)
            .map_err(|_| "T007 ledger creation failed")?;
        set_mode(&ledger, 0o600)?;

        let credential = codex_home.join("auth.json");
        fs::write(&credential, CREDENTIAL_CANARY)
            .map_err(|_| "T007 credential canary creation failed")?;
        set_mode(&credential, 0o600)?;

        fs::write(&executable, fixture_script(&ledger)?)
            .map_err(|_| "T007 fake Codex creation failed")?;
        set_mode(&executable, 0o700)?;

        Ok(Self {
            root,
            executable,
            codex_home,
            state_dir,
            working_dir,
            ledger,
            cleaned: false,
        })
    }

    fn scope(&self) -> TestResult<CredentialScope> {
        CredentialScope::validate(CredentialScopeConfig::new(
            self.executable.clone(),
            self.codex_home.clone(),
            self.state_dir.clone(),
            self.working_dir.clone(),
        ))
        .map_err(|_| "T007 credential scope validation failed")
    }

    fn read_ledger(&self) -> TestResult<String> {
        let metadata =
            fs::metadata(&self.ledger).map_err(|_| "T007 ledger metadata read failed")?;
        if metadata.len() > MAX_LEDGER_BYTES {
            return Err("T007 ledger exceeded its bound");
        }
        fs::read_to_string(&self.ledger).map_err(|_| "T007 ledger read failed")
    }

    fn cleanup(mut self) -> TestResult<()> {
        fs::remove_dir_all(&self.root).map_err(|_| "T007 exact fixture cleanup failed")?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for FixtureLayout {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct Captured {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

struct FlowEvidence {
    nonsecret_outputs: Vec<String>,
    cookie: String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p0_subscription_e2e_fake_codex_reaches_final_diff() {
    let layout = FixtureLayout::new().expect("T007 fixture setup");
    let login = LoginBroker::new(layout.scope().expect("T007 login scope"))
        .expect("T007 concrete login broker");
    let orchestrator = CloudTaskOrchestrator::new(CloudRunnerConfig::new(
        layout.scope().expect("T007 Cloud scope"),
        CloudEnvironmentId::try_new("env_synthetic").expect("T007 environment"),
        CloudBranch::try_new("main").expect("T007 branch"),
    ))
    .expect("T007 concrete Cloud orchestrator");
    let session = Arc::new(
        P0SessionRuntime::new(
            orchestrator,
            P0SessionConfig::try_new(Duration::from_millis(250), 64, 8, 8)
                .expect("T007 session bounds"),
        )
        .expect("T007 concrete session runtime"),
    );
    let plane = Arc::new(
        P0ControlPlane::new(
            P0HttpConfig::new(
                P0PublicOrigin::try_new(ORIGIN_VALUE).expect("T007 Origin"),
                OperatorBootstrapToken::try_new(BOOTSTRAP_SECRET).expect("T007 bootstrap"),
            ),
            login,
            Arc::clone(&session),
        )
        .expect("T007 concrete control plane"),
    );
    let router = plane.router();
    let (address, stop, server) = start_loopback(router.clone())
        .await
        .expect("T007 loopback server");

    let flow = run_flow(&router, address, &layout).await;
    let _ = stop.send(());
    let server_result = join_server(server).await;
    let shutdown_result = plane
        .shutdown()
        .await
        .map_err(|_| "T007 control-plane shutdown failed");
    let ledger_result = layout.read_ledger();
    drop(plane);
    drop(session);

    let validation = flow.and_then(|evidence| {
        server_result?;
        shutdown_result?;
        let ledger = ledger_result?;
        validate_ledger(&ledger)?;
        validate_secret_boundaries(&evidence, &ledger, &layout.root)
    });
    let cleanup_result = layout.cleanup();

    match (validation, cleanup_result) {
        (Ok(()), Ok(())) => {}
        (Err(_), Err(_)) => panic!("T007 flow and exact cleanup failed"),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => panic!("{error}"),
    }
}

async fn run_flow(
    router: &Router,
    address: std::net::SocketAddr,
    layout: &FixtureLayout,
) -> TestResult<FlowEvidence> {
    let mut evidence = Vec::new();

    let page = send(
        router,
        Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .map_err(|_| "T007 page request construction failed")?,
    )
    .await?;
    require(page.status == StatusCode::OK, "T007 page status mismatch")?;
    require_header(&page, CONTENT_TYPE.as_str(), "text/html; charset=utf-8")?;
    require_header(&page, CACHE_CONTROL.as_str(), "no-store")?;
    require_header(&page, "x-content-type-options", "nosniff")?;
    let csp = page
        .headers
        .get(CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .ok_or("T007 page CSP missing")?;
    require(
        csp.contains("connect-src 'self' wss://operator.example"),
        "T007 page WSS CSP mismatch",
    )?;
    let page_text = body_text(&page)?;
    require(
        page_text.starts_with("<!doctype html>") && page_text.contains("Codebox operator"),
        "T007 embedded page mismatch",
    )?;
    evidence.push(page_text);

    let bootstrap = send(
        router,
        Request::builder()
            .method(Method::POST)
            .uri("/api/p0/v1/operator/session")
            .header(ORIGIN, ORIGIN_VALUE)
            .header(AUTHORIZATION, format!("Bearer {BOOTSTRAP_SECRET}"))
            .body(Body::empty())
            .map_err(|_| "T007 bootstrap request construction failed")?,
    )
    .await?;
    require(
        bootstrap.status == StatusCode::CREATED,
        "T007 bootstrap status mismatch",
    )?;
    require_header(&bootstrap, CACHE_CONTROL.as_str(), "no-store")?;
    let bootstrap_value = json_body(&bootstrap)?;
    require(
        bootstrap_value["actor"] == "operator",
        "T007 bootstrap actor mismatch",
    )?;
    let session_id = json_string(&bootstrap_value, "p0_session_id")?;
    let instance_id = json_string(&bootstrap_value, "instance_id")?;
    let cookie = bootstrap
        .headers
        .get(SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .ok_or("T007 bootstrap cookie missing")?
        .to_owned();
    require(
        cookie.starts_with("__Host-codebox_p0="),
        "T007 bootstrap cookie mismatch",
    )?;
    evidence.push(body_text(&bootstrap)?);

    let login = send(router, get_request("/api/p0/v1/login", &cookie)?).await?;
    require(login.status == StatusCode::OK, "T007 login status mismatch")?;
    require(
        json_body(&login)? == json!({"state":"logged_in"}),
        "T007 login body mismatch",
    )?;
    evidence.push(body_text(&login)?);

    let initial_snapshot = send(router, get_request("/api/p0/v1/session", &cookie)?).await?;
    require(
        initial_snapshot.status == StatusCode::OK,
        "T007 initial snapshot status mismatch",
    )?;
    let initial_value = json_body(&initial_snapshot)?;
    require(
        initial_value["identity"]["session_id"] == session_id
            && initial_value["identity"]["instance_id"] == instance_id
            && initial_value["state"] == "ready"
            && initial_value["current_turn"].is_null()
            && initial_value["high_water_seq"] == 0,
        "T007 initial snapshot mismatch",
    )?;
    evidence.push(body_text(&initial_snapshot)?);

    let mut initial_socket = connect_loopback(address, &cookie).await?;
    initial_socket
        .send(Message::Text(
            json!({
                "type":"subscribe",
                "protocol_version":1,
                "session_id":session_id,
                "after_seq":0
            })
            .to_string()
            .into(),
        ))
        .await
        .map_err(|_| "T007 initial subscribe send failed")?;
    let initial_frames = receive_json_frames(&mut initial_socket, 3, &mut evidence).await?;
    require_frame_types(
        &initial_frames,
        &["replay_begin", "snapshot", "replay_end"],
        "T007 initial replay order mismatch",
    )?;
    require(
        initial_frames[0]["session_id"] == session_id
            && initial_frames[0]["after_seq"] == 0
            && initial_frames[0]["high_water_seq"] == 0
            && initial_frames[1]["snapshot"] == initial_value
            && initial_frames[2]["session_id"] == session_id
            && initial_frames[2]["high_water_seq"] == 0,
        "T007 initial replay value mismatch",
    )?;

    let turn = send(
        router,
        Request::builder()
            .method(Method::POST)
            .uri("/api/p0/v1/session/turns")
            .header(COOKIE, &cookie)
            .header(ORIGIN, ORIGIN_VALUE)
            .header("codebox-instance-id", &instance_id)
            .header("idempotency-key", TURN_KEY)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"prompt":PROMPT_CANARY}).to_string()))
            .map_err(|_| "T007 turn request construction failed")?,
    )
    .await?;
    require(
        turn.status == StatusCode::ACCEPTED,
        "T007 turn status mismatch",
    )?;
    let receipt = json_body(&turn)?;
    let turn_id = json_string(&receipt, "turn_id")?;
    require(
        receipt["high_water_seq"] == 1,
        "T007 turn receipt sequence mismatch",
    )?;
    evidence.push(body_text(&turn)?);

    let live_frames = receive_json_frames(&mut initial_socket, 3, &mut evidence).await?;
    for (index, frame) in live_frames.iter().enumerate() {
        require(frame["type"] == "event", "T007 live frame type mismatch")?;
        let envelope = &frame["envelope"];
        require(
            envelope["session_id"] == session_id
                && envelope["turn_id"] == turn_id
                && envelope["seq"] == u64::try_from(index + 1).unwrap_or_default(),
            "T007 live envelope mismatch",
        )?;
    }
    require(
        live_frames[0]["envelope"]["payload"]["type"] == "turn_accepted",
        "T007 accepted event mismatch",
    )?;
    require(
        live_frames[1]["envelope"]["payload"]["type"] == "lifecycle_changed"
            && live_frames[1]["envelope"]["payload"]["lifecycle"]["state"] == "pending"
            && live_frames[1]["envelope"]["payload"]["lifecycle"]["task_id"] == TASK_ID,
        "T007 pending lifecycle mismatch",
    )?;
    let operation_id = json_string(
        &live_frames[1]["envelope"]["payload"]["lifecycle"],
        "operation_id",
    )?;
    require(
        live_frames[2]["envelope"]["payload"]["type"] == "lifecycle_changed"
            && live_frames[2]["envelope"]["payload"]["lifecycle"]["state"] == "ready"
            && live_frames[2]["envelope"]["payload"]["lifecycle"]["operation_id"] == operation_id
            && live_frames[2]["envelope"]["payload"]["lifecycle"]["task_id"] == TASK_ID,
        "T007 Ready lifecycle mismatch",
    )?;

    let final_snapshot = send(router, get_request("/api/p0/v1/session", &cookie)?).await?;
    require(
        final_snapshot.status == StatusCode::OK,
        "T007 final snapshot status mismatch",
    )?;
    let final_value = json_body(&final_snapshot)?;
    require(
        final_value["identity"] == initial_value["identity"]
            && final_value["state"] == "ready"
            && final_value["high_water_seq"] == 3
            && final_value["current_turn"]["turn_id"] == turn_id
            && final_value["current_turn"]["projection"]["phase"] == "cloud"
            && final_value["current_turn"]["projection"]["lifecycle"]["state"] == "ready"
            && final_value["current_turn"]["projection"]["lifecycle"]["operation_id"]
                == operation_id
            && final_value["current_turn"]["projection"]["lifecycle"]["task_id"] == TASK_ID,
        "T007 final snapshot mismatch",
    )?;
    evidence.push(body_text(&final_snapshot)?);

    let ledger_before_disconnect = layout.read_ledger()?;
    close_socket(&mut initial_socket).await?;
    let mut resumed = connect_loopback(address, &cookie).await?;
    resumed
        .send(Message::Text(
            json!({
                "type":"subscribe",
                "protocol_version":1,
                "session_id":session_id,
                "after_seq":3
            })
            .to_string()
            .into(),
        ))
        .await
        .map_err(|_| "T007 resume subscribe send failed")?;
    let resumed_frames = receive_json_frames(&mut resumed, 3, &mut evidence).await?;
    require_frame_types(
        &resumed_frames,
        &["replay_begin", "snapshot", "replay_end"],
        "T007 resume replay order mismatch",
    )?;
    require(
        resumed_frames[0]["after_seq"] == 3
            && resumed_frames[0]["high_water_seq"] == 3
            && resumed_frames[1]["snapshot"] == final_value
            && resumed_frames[2]["high_water_seq"] == 3,
        "T007 resume replay value mismatch",
    )?;
    require(
        tokio::time::timeout(Duration::from_millis(100), resumed.next())
            .await
            .is_err(),
        "T007 resume emitted a duplicate live frame",
    )?;
    close_socket(&mut resumed).await?;
    tokio::time::sleep(Duration::from_millis(25)).await;
    require(
        layout.read_ledger()? == ledger_before_disconnect,
        "T007 disconnect or reconnect invoked Codex",
    )?;

    let diff = send(router, get_request("/api/p0/v1/session/diff", &cookie)?).await?;
    require(diff.status == StatusCode::OK, "T007 diff status mismatch")?;
    require_header(&diff, CONTENT_TYPE.as_str(), "text/plain; charset=utf-8")?;
    require_header(&diff, CACHE_CONTROL.as_str(), "no-store")?;
    require(body_text(&diff)? == DIFF_CANARY, "T007 diff body mismatch")?;

    let after_diff = send(router, get_request("/api/p0/v1/session", &cookie)?).await?;
    require(
        after_diff.status == StatusCode::OK && json_body(&after_diff)? == final_value,
        "T007 diff changed session state",
    )?;
    evidence.push(body_text(&after_diff)?);

    let logout = send(
        router,
        Request::builder()
            .method(Method::DELETE)
            .uri("/api/p0/v1/operator/session")
            .header(COOKIE, &cookie)
            .header(ORIGIN, ORIGIN_VALUE)
            .header("codebox-instance-id", &instance_id)
            .header("idempotency-key", LOGOUT_KEY)
            .body(Body::empty())
            .map_err(|_| "T007 logout request construction failed")?,
    )
    .await?;
    require(
        logout.status == StatusCode::NO_CONTENT && logout.body.is_empty(),
        "T007 logout mismatch",
    )?;

    let rejected = send(router, get_request("/api/p0/v1/session", &cookie)?).await?;
    require(
        rejected.status == StatusCode::UNAUTHORIZED
            && json_body(&rejected)?["error"]["code"] == "authentication_required",
        "T007 invalidated cookie was accepted",
    )?;
    evidence.push(body_text(&rejected)?);

    Ok(FlowEvidence {
        nonsecret_outputs: evidence,
        cookie,
    })
}

async fn send(router: &Router, request: Request<Body>) -> TestResult<Captured> {
    let response = tokio::time::timeout(DEADLINE, router.clone().oneshot(request))
        .await
        .map_err(|_| "T007 HTTP response deadline exceeded")?
        .map_err(|_| "T007 router response failed")?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = tokio::time::timeout(DEADLINE, response.into_body().collect())
        .await
        .map_err(|_| "T007 HTTP body deadline exceeded")?
        .map_err(|_| "T007 HTTP body read failed")?
        .to_bytes()
        .to_vec();
    Ok(Captured {
        status,
        headers,
        body,
    })
}

fn get_request(path: &'static str, cookie: &str) -> TestResult<Request<Body>> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(COOKIE, cookie)
        .body(Body::empty())
        .map_err(|_| "T007 GET request construction failed")
}

fn json_body(response: &Captured) -> TestResult<Value> {
    serde_json::from_slice(&response.body).map_err(|_| "T007 JSON response invalid")
}

fn body_text(response: &Captured) -> TestResult<String> {
    String::from_utf8(response.body.clone()).map_err(|_| "T007 response UTF-8 invalid")
}

fn json_string(value: &Value, key: &str) -> TestResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or("T007 JSON string field invalid")
}

fn require(condition: bool, message: &'static str) -> TestResult<()> {
    if condition { Ok(()) } else { Err(message) }
}

fn require_header(response: &Captured, name: &str, expected: &str) -> TestResult<()> {
    require(
        response
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            == Some(expected),
        "T007 response header mismatch",
    )
}

fn require_frame_types(
    frames: &[Value],
    expected: &[&str],
    message: &'static str,
) -> TestResult<()> {
    require(
        frames
            .iter()
            .map(|frame| frame["type"].as_str())
            .eq(expected.iter().copied().map(Some)),
        message,
    )
}

async fn start_loopback(
    router: Router,
) -> TestResult<(
    std::net::SocketAddr,
    oneshot::Sender<()>,
    JoinHandle<Result<(), std::io::Error>>,
)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "T007 loopback bind failed")?;
    let address = listener
        .local_addr()
        .map_err(|_| "T007 loopback address failed")?;
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    Ok((address, stop, server))
}

async fn join_server(server: JoinHandle<Result<(), std::io::Error>>) -> TestResult<()> {
    tokio::time::timeout(DEADLINE, server)
        .await
        .map_err(|_| "T007 loopback shutdown deadline exceeded")?
        .map_err(|_| "T007 loopback task failed")?
        .map_err(|_| "T007 loopback server failed")
}

async fn connect_loopback(address: std::net::SocketAddr, cookie: &str) -> TestResult<TestSocket> {
    let mut request = format!("ws://{address}/api/p0/v1/session/stream")
        .into_client_request()
        .map_err(|_| "T007 WebSocket request construction failed")?;
    request.headers_mut().insert(
        COOKIE,
        cookie
            .parse()
            .map_err(|_| "T007 WebSocket cookie invalid")?,
    );
    request.headers_mut().insert(
        ORIGIN,
        ORIGIN_VALUE
            .parse()
            .map_err(|_| "T007 WebSocket Origin invalid")?,
    );
    let (socket, response) = tokio::time::timeout(DEADLINE, connect_async(request))
        .await
        .map_err(|_| "T007 WebSocket connect deadline exceeded")?
        .map_err(|_| "T007 WebSocket connect failed")?;
    require(
        response.status() == StatusCode::SWITCHING_PROTOCOLS,
        "T007 WebSocket upgrade status mismatch",
    )?;
    require(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            == Some("no-store")
            && response
                .headers()
                .get("x-content-type-options")
                .and_then(|value| value.to_str().ok())
                == Some("nosniff"),
        "T007 WebSocket upgrade headers mismatch",
    )?;
    Ok(socket)
}

async fn receive_json_frames(
    socket: &mut TestSocket,
    count: usize,
    evidence: &mut Vec<String>,
) -> TestResult<Vec<Value>> {
    let mut frames = Vec::with_capacity(count);
    while frames.len() < count {
        let message = tokio::time::timeout(DEADLINE, socket.next())
            .await
            .map_err(|_| "T007 WebSocket frame deadline exceeded")?
            .ok_or("T007 WebSocket ended early")?
            .map_err(|_| "T007 WebSocket frame read failed")?;
        let Message::Text(text) = message else {
            return Err("T007 WebSocket returned a non-text frame");
        };
        evidence.push(text.to_string());
        frames.push(serde_json::from_str(&text).map_err(|_| "T007 WebSocket JSON frame invalid")?);
    }
    Ok(frames)
}

async fn close_socket(socket: &mut TestSocket) -> TestResult<()> {
    tokio::time::timeout(DEADLINE, socket.close(None))
        .await
        .map_err(|_| "T007 WebSocket close deadline exceeded")?
        .map_err(|_| "T007 WebSocket close failed")
}

fn validate_ledger(ledger: &str) -> TestResult<()> {
    let entries: Vec<_> = ledger.lines().collect();
    require(
        entries.iter().all(|entry| {
            matches!(
                *entry,
                "version" | "login_status" | "cloud_exec" | "cloud_status" | "cloud_diff"
            )
        }),
        "T007 ledger contains an unknown invocation",
    )?;
    require(
        entries
            .iter()
            .filter(|entry| **entry == "cloud_exec")
            .count()
            == 1,
        "T007 Cloud submit count mismatch",
    )?;
    let status_count = entries
        .iter()
        .filter(|entry| **entry == "cloud_status")
        .count();
    require(
        (1..=8).contains(&status_count),
        "T007 Cloud status count is unbounded or absent",
    )?;
    require(
        entries
            .iter()
            .filter(|entry| **entry == "cloud_diff")
            .count()
            == 1,
        "T007 Cloud diff count mismatch",
    )
}

fn validate_secret_boundaries(
    evidence: &FlowEvidence,
    ledger: &str,
    root: &Path,
) -> TestResult<()> {
    let root = root.to_str().ok_or("T007 fixture root UTF-8 invalid")?;
    let forbidden = [
        BOOTSTRAP_SECRET,
        evidence.cookie.as_str(),
        CREDENTIAL_CANARY,
        PROMPT_CANARY,
        STATUS_CANARY,
        DIFF_CANARY,
        root,
    ];
    for output in &evidence.nonsecret_outputs {
        require(
            forbidden.iter().all(|value| !output.contains(value)),
            "T007 sensitive value escaped into a nonsecret response or frame",
        )?;
    }
    require(
        forbidden.iter().all(|value| !ledger.contains(value)),
        "T007 sensitive value escaped into the invocation ledger",
    )
}

fn private_directory(root: &Path, name: &str) -> TestResult<PathBuf> {
    let path = root.join(name);
    fs::create_dir(&path).map_err(|_| "T007 private directory creation failed")?;
    set_mode(&path, 0o700)?;
    Ok(path)
}

fn set_mode(path: &Path, mode: u32) -> TestResult<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|_| "T007 fixture mode update failed")
}

fn fixture_script(ledger: &Path) -> TestResult<String> {
    let ledger = ledger.to_str().ok_or("T007 ledger path is not UTF-8")?;
    require(
        ledger
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.')),
        "T007 ledger path contains an unsafe script byte",
    )?;
    Ok(format!(
        r#"#!/bin/sh
ledger='{ledger}'
record() {{
  printf '%s\n' "$1" >> "$ledger"
}}
if [ "$#" -eq 1 ] && [ "$1" = '--version' ]; then
  record version
  printf 'codex-cli 0.145.0\n'
  exit 0
fi
if [ "$#" -eq 6 ] &&
   [ "$1" = '-c' ] &&
   [ "$2" = 'forced_login_method="chatgpt"' ] &&
   [ "$3" = '-c' ] &&
   [ "$4" = 'cli_auth_credentials_store="file"' ] &&
   [ "$5" = 'login' ] &&
   [ "$6" = 'status' ]; then
  record login_status
  printf 'Logged in using ChatGPT\n' >&2
  exit 0
fi
if [ "$#" -eq 7 ] &&
   [ "$1" = 'cloud' ] &&
   [ "$2" = 'exec' ] &&
   [ "$3" = '--env=env_synthetic' ] &&
   [ "$4" = '--attempts=1' ] &&
   [ "$5" = '--branch=main' ] &&
   [ "$6" = '--' ] &&
   [ "$7" = '{PROMPT_CANARY}' ]; then
  record cloud_exec
  printf 'https://chatgpt.com/codex/tasks/{TASK_ID}\n'
  exit 0
fi
if [ "$#" -eq 3 ] &&
   [ "$1" = 'cloud' ] &&
   [ "$2" = 'status' ] &&
   [ "$3" = '{TASK_ID}' ]; then
  record cloud_status
  printf '[READY] Synthetic documentation update\n'
  printf 'private-environment  •  10s ago\n'
  printf '+5/-1 • 1 file\n'
  exit 0
fi
if [ "$#" -eq 4 ] &&
   [ "$1" = 'cloud' ] &&
   [ "$2" = 'diff' ] &&
   [ "$3" = '--attempt=1' ] &&
   [ "$4" = '{TASK_ID}' ]; then
  record cloud_diff
  printf 'diff --git a/README.md b/README.md\n'
  printf 'index 1111111..2222222 100644\n'
  printf '%s\n' '--- a/README.md'
  printf '+++ b/README.md\n'
  printf '@@ -1 +1,2 @@\n'
  printf ' Existing text\n'
  printf '+Synthetic text\n'
  exit 0
fi
record unknown
exit 97
"#
    ))
}
