use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{SEC_WEBSOCKET_VERSION, UPGRADE};
use axum::http::{HeaderValue, Response, StatusCode};
use bytes::Bytes;
use codebox_domain::{EventSeq, SessionId};
use codebox_session_runtime::{P0SessionEventEnvelope, P0SessionSnapshot};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::error::ApiError;
use crate::ports::{
    LiveEventError, SessionSubscribeError, SessionSubscribeErrorCategory, SessionSubscription,
};
use crate::state::{RequestAdmission, Shared};
use crate::transport::{CachedResponse, add_common_security_headers};

const MAX_CLIENT_MESSAGE_BYTES: usize = 1024;
const MAX_SERVER_MESSAGE_BYTES: usize = 64 * 1024;
const PROTOCOL_VERSION: u64 = 1;

#[derive(Clone, Copy)]
pub(crate) struct StreamDeadlines {
    subscribe: Duration,
    write: Duration,
    close_grace: Duration,
    validity_poll: Duration,
    live_poll: Duration,
    ping: Duration,
}

impl StreamDeadlines {
    const PRODUCTION: Self = Self {
        subscribe: Duration::from_secs(5),
        write: Duration::from_secs(10),
        close_grace: Duration::from_secs(2),
        validity_poll: Duration::from_millis(250),
        live_poll: Duration::from_millis(25),
        ping: Duration::from_secs(30),
    };

    #[cfg(test)]
    pub(crate) const fn for_test(
        subscribe: Duration,
        write: Duration,
        close_grace: Duration,
        validity_poll: Duration,
        live_poll: Duration,
        ping: Duration,
    ) -> Self {
        Self {
            subscribe,
            write,
            close_grace,
            validity_poll,
            live_poll,
            ping,
        }
    }
}

pub(crate) async fn upgrade(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let response = async {
        if !shared.lifecycle.is_running() {
            return Err(CachedResponse::error(ApiError::service_unavailable()).into_response());
        }
        if let Err(error) = crate::routes::validate_headers(request.headers()) {
            return Err(CachedResponse::error(error).into_response());
        }
        let app_session = match shared.authenticate_cookie(request.headers()) {
            Ok(app_session) => app_session,
            Err(error) => return Err(CachedResponse::error(error).into_response()),
        };
        let lifecycle = match shared.lifecycle.admit() {
            Ok(lifecycle) => lifecycle,
            Err(error) => return Err(CachedResponse::error(error).into_response()),
        };
        if let Err(error) = shared.validate_origin(request.headers()) {
            return Err(CachedResponse::error(error).into_response());
        }
        let admission = Arc::new(RequestAdmission::new(lifecycle, app_session));
        let (mut parts, _body) = request.into_parts();
        let websocket = match WebSocketUpgrade::from_request_parts(&mut parts, &shared).await {
            Ok(websocket) => websocket,
            Err(_) => return Err(upgrade_required()),
        };
        let session = Arc::clone(&shared.session);
        let mut response = websocket
            .read_buffer_size(MAX_CLIENT_MESSAGE_BYTES)
            .write_buffer_size(0)
            .max_write_buffer_size(MAX_SERVER_MESSAGE_BYTES)
            .max_message_size(MAX_CLIENT_MESSAGE_BYTES)
            .max_frame_size(MAX_CLIENT_MESSAGE_BYTES)
            .accept_unmasked_frames(false)
            .on_upgrade(move |socket| async move {
                run_axum_socket(socket, admission, session).await;
            });
        add_common_security_headers(response.headers_mut());
        Ok(response)
    }
    .await;

    match response {
        Ok(response) | Err(response) => response,
    }
}

fn upgrade_required() -> Response<Body> {
    let mut response = CachedResponse::error(ApiError::new(
        StatusCode::UPGRADE_REQUIRED,
        "websocket_upgrade_required",
        "a version-13 WebSocket upgrade is required",
    ))
    .into_response();
    response
        .headers_mut()
        .insert(UPGRADE, HeaderValue::from_static("websocket"));
    response
        .headers_mut()
        .insert(SEC_WEBSOCKET_VERSION, HeaderValue::from_static("13"));
    response
}

async fn run_axum_socket(
    socket: WebSocket,
    admission: Arc<RequestAdmission>,
    session: Arc<dyn crate::ports::SessionPort>,
) {
    let (writer, reader) = socket.split();
    run_socket(
        Box::new(AxumReader(reader)),
        Box::new(AxumWriter(writer)),
        admission,
        session,
        StreamDeadlines::PRODUCTION,
    )
    .await;
}

pub(crate) enum ClientFrame {
    Text(String),
    Binary,
    Ping,
    Pong,
    Close,
}

pub(crate) enum ServerFrame {
    Text(String),
    Ping,
    Close { code: u16, reason: &'static str },
}

type IoFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) trait SocketReader: Send {
    fn receive(&mut self) -> IoFuture<'_, Result<Option<ClientFrame>, ()>>;
}

pub(crate) trait SocketWriter: Send {
    fn send(&mut self, frame: ServerFrame) -> IoFuture<'_, Result<(), ()>>;
}

struct AxumReader(SplitStream<WebSocket>);

impl SocketReader for AxumReader {
    fn receive(&mut self) -> IoFuture<'_, Result<Option<ClientFrame>, ()>> {
        Box::pin(async move {
            match self.0.next().await {
                Some(Ok(Message::Text(text))) => Ok(Some(ClientFrame::Text(text.to_string()))),
                Some(Ok(Message::Binary(_))) => Ok(Some(ClientFrame::Binary)),
                Some(Ok(Message::Ping(_))) => Ok(Some(ClientFrame::Ping)),
                Some(Ok(Message::Pong(_))) => Ok(Some(ClientFrame::Pong)),
                Some(Ok(Message::Close(_))) | None => Ok(Some(ClientFrame::Close)),
                Some(Err(_)) => Err(()),
            }
        })
    }
}

struct AxumWriter(SplitSink<WebSocket, Message>);

impl SocketWriter for AxumWriter {
    fn send(&mut self, frame: ServerFrame) -> IoFuture<'_, Result<(), ()>> {
        Box::pin(async move {
            let message = match frame {
                ServerFrame::Text(text) => Message::Text(text.into()),
                ServerFrame::Ping => Message::Ping(Bytes::new()),
                ServerFrame::Close { code, reason } => Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                })),
            };
            self.0.send(message).await.map_err(|_| ())
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientApplicationFrame {
    Subscribe {
        protocol_version: u64,
        session_id: SessionId,
        after_seq: EventSeq,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ReplayFrame<'a> {
    ReplayBegin {
        session_id: SessionId,
        after_seq: EventSeq,
        high_water_seq: EventSeq,
    },
    Event {
        envelope: &'a P0SessionEventEnvelope,
    },
    Snapshot {
        snapshot: &'a P0SessionSnapshot,
        high_water_seq: EventSeq,
    },
    ReplayEnd {
        session_id: SessionId,
        high_water_seq: EventSeq,
    },
}

#[derive(Serialize)]
struct ErrorFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    oldest_available: Option<EventSeq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_available: Option<EventSeq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supported_version: Option<u64>,
}

#[derive(Clone, Copy)]
struct ApplicationError {
    code: &'static str,
    message: &'static str,
    close_code: u16,
    oldest_available: Option<EventSeq>,
    latest_available: Option<EventSeq>,
    supported_version: Option<u64>,
}

impl ApplicationError {
    const fn policy(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            close_code: 1008,
            oldest_available: None,
            latest_available: None,
            supported_version: None,
        }
    }

    const fn retry(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            close_code: 1013,
            oldest_available: None,
            latest_available: None,
            supported_version: None,
        }
    }

    const fn authentication_expired() -> Self {
        Self::policy("authentication_expired", "operator authentication expired")
    }

    const fn subscribe_timeout() -> Self {
        Self::policy(
            "subscribe_timeout",
            "subscription was not received before its deadline",
        )
    }

    const fn protocol_error() -> Self {
        Self::policy("protocol_error", "WebSocket application frame is invalid")
    }

    const fn unsupported_version() -> Self {
        Self {
            supported_version: Some(PROTOCOL_VERSION),
            ..Self::policy(
                "unsupported_version",
                "WebSocket protocol version is unsupported",
            )
        }
    }

    const fn stream_unavailable() -> Self {
        Self::retry("stream_unavailable", "session stream is unavailable")
    }

    fn from_subscribe(error: SessionSubscribeError) -> Self {
        match error.category {
            SessionSubscribeErrorCategory::WrongSession => Self::policy(
                "wrong_session",
                "session identity changed; refresh before reconnect",
            ),
            SessionSubscribeErrorCategory::HistoryGap => Self {
                oldest_available: error.oldest_available,
                latest_available: error.latest_available,
                ..Self::policy(
                    "history_gap",
                    "requested session history is no longer retained",
                )
            },
            SessionSubscribeErrorCategory::FutureCursor => Self {
                latest_available: error.latest_available,
                ..Self::policy("future_cursor", "requested session cursor is in the future")
            },
            SessionSubscribeErrorCategory::SubscriberLimit => {
                Self::retry("subscriber_limit", "session subscriber limit reached")
            }
            SessionSubscribeErrorCategory::Unavailable => Self::stream_unavailable(),
        }
    }

    fn serialize(self) -> Result<String, ()> {
        serialize_bounded(&ErrorFrame {
            frame_type: "error",
            code: self.code,
            message: self.message,
            oldest_available: self.oldest_available,
            latest_available: self.latest_available,
            supported_version: self.supported_version,
        })
    }
}

struct SubscribeRequest {
    session_id: SessionId,
    after_seq: EventSeq,
}

pub(crate) async fn run_socket(
    mut reader: Box<dyn SocketReader>,
    mut writer: Box<dyn SocketWriter>,
    admission: Arc<RequestAdmission>,
    session: Arc<dyn crate::ports::SessionPort>,
    deadlines: StreamDeadlines,
) {
    let subscribe = match receive_subscribe(&mut *reader, &admission, deadlines).await {
        InitialOutcome::Subscribed(subscribe) => subscribe,
        InitialOutcome::PeerClosed | InitialOutcome::TransportClosed => return,
        InitialOutcome::ServerShutdown => {
            if send_close(&mut *writer, 1012, "server_shutdown", deadlines).await {
                let _ = tokio::time::timeout(deadlines.close_grace, reader.receive()).await;
            }
            return;
        }
        InitialOutcome::Failed(error) => {
            fail_before_subscription(&mut *reader, &mut *writer, error, deadlines).await;
            return;
        }
    };

    let control = Arc::new(Control::default());
    let reader_task = spawn_control_reader(reader, Arc::clone(&control));
    let session_id = subscribe.session_id;
    let after_seq = subscribe.after_seq;
    let subscriber = Arc::clone(&session);
    let subscription =
        tokio::task::spawn_blocking(move || subscriber.subscribe(session_id, after_seq)).await;
    let subscription = match subscription {
        Ok(Ok(subscription)) => subscription,
        Ok(Err(error)) => {
            fail_after_subscription(
                &mut *writer,
                &control,
                reader_task,
                ApplicationError::from_subscribe(error),
                deadlines,
            )
            .await;
            return;
        }
        Err(_) => {
            fail_after_subscription(
                &mut *writer,
                &control,
                reader_task,
                ApplicationError::stream_unavailable(),
                deadlines,
            )
            .await;
            return;
        }
    };

    let outcome = stream_subscription(
        &mut *writer,
        &admission,
        &control,
        session_id,
        after_seq,
        subscription,
        deadlines,
    )
    .await;
    match outcome {
        StreamOutcome::PeerClosed | StreamOutcome::TransportClosed => {
            finish_reader(reader_task, deadlines.close_grace).await;
        }
        StreamOutcome::ServerShutdown => {
            send_close(&mut *writer, 1012, "server_shutdown", deadlines).await;
            finish_reader(reader_task, deadlines.close_grace).await;
        }
        StreamOutcome::Failed(error) => {
            fail_after_subscription(&mut *writer, &control, reader_task, error, deadlines).await;
        }
    }
}

async fn receive_subscribe(
    reader: &mut dyn SocketReader,
    admission: &RequestAdmission,
    deadlines: StreamDeadlines,
) -> InitialOutcome {
    let deadline = Instant::now() + deadlines.subscribe;
    loop {
        if !admission.is_auth_valid() {
            return InitialOutcome::Failed(ApplicationError::authentication_expired());
        }
        if !admission.is_control_plane_running() {
            return InitialOutcome::ServerShutdown;
        }
        let validity = tokio::time::sleep(deadlines.validity_poll);
        tokio::pin!(validity);
        let timeout = tokio::time::sleep_until(deadline);
        tokio::pin!(timeout);
        tokio::select! {
            _ = &mut timeout => {
                return InitialOutcome::Failed(ApplicationError::subscribe_timeout());
            }
            _ = &mut validity => {}
            received = reader.receive() => {
                let frame = match received {
                    Ok(frame) => frame,
                    Err(()) => return InitialOutcome::TransportClosed,
                };
                match frame {
                    None | Some(ClientFrame::Close) => {
                        return InitialOutcome::PeerClosed;
                    }
                    Some(ClientFrame::Ping | ClientFrame::Pong) => {}
                    Some(ClientFrame::Binary) => {
                        return InitialOutcome::Failed(ApplicationError::protocol_error());
                    }
                    Some(ClientFrame::Text(text)) => {
                        if text.len() > MAX_CLIENT_MESSAGE_BYTES {
                            return InitialOutcome::Failed(ApplicationError::protocol_error());
                        }
                        let decoded = match serde_json::from_str::<ClientApplicationFrame>(&text) {
                            Ok(decoded) => decoded,
                            Err(_) => {
                                return InitialOutcome::Failed(
                                    ApplicationError::protocol_error(),
                                );
                            }
                        };
                        let ClientApplicationFrame::Subscribe {
                            protocol_version,
                            session_id,
                            after_seq,
                        } = decoded;
                        if protocol_version != PROTOCOL_VERSION {
                            return InitialOutcome::Failed(
                                ApplicationError::unsupported_version(),
                            );
                        }
                        return InitialOutcome::Subscribed(SubscribeRequest {
                            session_id,
                            after_seq,
                        });
                    }
                }
            }
        }
    }
}

enum InitialOutcome {
    Subscribed(SubscribeRequest),
    PeerClosed,
    TransportClosed,
    ServerShutdown,
    Failed(ApplicationError),
}

async fn stream_subscription(
    writer: &mut dyn SocketWriter,
    admission: &RequestAdmission,
    control: &Control,
    session_id: SessionId,
    after_seq: EventSeq,
    subscription: SessionSubscription,
    deadlines: StreamDeadlines,
) -> StreamOutcome {
    let high_water = subscription.snapshot.high_water_seq;
    let replay = match prepare_replay(session_id, after_seq, high_water, &subscription) {
        Ok(replay) => replay,
        Err(error) => return StreamOutcome::Failed(error),
    };

    for frame in replay {
        if let Some(outcome) = connection_outcome(admission, control) {
            return outcome;
        }
        if send_text(writer, frame, deadlines).await.is_err() {
            return StreamOutcome::TransportClosed;
        }
    }

    let mut last_seq = high_water;
    let mut next_ping = Instant::now() + deadlines.ping;
    loop {
        if let Some(outcome) = connection_outcome(admission, control) {
            return outcome;
        }
        if Instant::now() >= next_ping {
            if send_frame(writer, ServerFrame::Ping, deadlines)
                .await
                .is_err()
            {
                return StreamOutcome::TransportClosed;
            }
            next_ping = Instant::now() + deadlines.ping;
            continue;
        }
        match subscription.live.try_recv() {
            Ok(envelope) => {
                let expected = match last_seq.checked_next() {
                    Ok(expected) => expected,
                    Err(_) => {
                        return StreamOutcome::Failed(ApplicationError::stream_unavailable());
                    }
                };
                if envelope.schema_version != 1
                    || envelope.session_id != session_id
                    || envelope.seq != expected
                {
                    return StreamOutcome::Failed(ApplicationError::stream_unavailable());
                }
                let frame = match serialize_bounded(&ReplayFrame::Event {
                    envelope: &envelope,
                }) {
                    Ok(frame) => frame,
                    Err(()) => {
                        return StreamOutcome::Failed(ApplicationError::stream_unavailable());
                    }
                };
                if let Some(outcome) = connection_outcome(admission, control) {
                    return outcome;
                }
                if send_text(writer, frame, deadlines).await.is_err() {
                    return StreamOutcome::TransportClosed;
                }
                last_seq = envelope.seq;
            }
            Err(LiveEventError::Empty) => {
                let until_ping = next_ping.saturating_duration_since(Instant::now());
                let wait = deadlines.live_poll.min(until_ping);
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = control.changed.notified() => {}
                }
            }
            Err(LiveEventError::Lagged) => {
                return StreamOutcome::Failed(ApplicationError::retry(
                    "subscriber_lagged",
                    "session subscriber fell behind",
                ));
            }
            Err(LiveEventError::RuntimeStopped | LiveEventError::Closed) => {
                return StreamOutcome::Failed(ApplicationError::stream_unavailable());
            }
        }
    }
}

fn prepare_replay(
    session_id: SessionId,
    after_seq: EventSeq,
    high_water: EventSeq,
    subscription: &SessionSubscription,
) -> Result<Vec<String>, ApplicationError> {
    if subscription.snapshot.identity.session_id != session_id
        || subscription.snapshot.high_water_seq != high_water
        || after_seq > high_water
    {
        return Err(ApplicationError::stream_unavailable());
    }
    let mut expected = after_seq;
    for envelope in &subscription.replay {
        expected = expected
            .checked_next()
            .map_err(|_| ApplicationError::stream_unavailable())?;
        if envelope.schema_version != 1
            || envelope.session_id != session_id
            || envelope.seq != expected
            || envelope.seq > high_water
        {
            return Err(ApplicationError::stream_unavailable());
        }
    }
    if expected != high_water {
        return Err(ApplicationError::stream_unavailable());
    }

    let mut frames = Vec::with_capacity(subscription.replay.len().saturating_add(3));
    frames.push(
        serialize_bounded(&ReplayFrame::ReplayBegin {
            session_id,
            after_seq,
            high_water_seq: high_water,
        })
        .map_err(|_| ApplicationError::stream_unavailable())?,
    );
    for envelope in &subscription.replay {
        frames.push(
            serialize_bounded(&ReplayFrame::Event { envelope })
                .map_err(|_| ApplicationError::stream_unavailable())?,
        );
    }
    frames.push(
        serialize_bounded(&ReplayFrame::Snapshot {
            snapshot: &subscription.snapshot,
            high_water_seq: high_water,
        })
        .map_err(|_| ApplicationError::stream_unavailable())?,
    );
    frames.push(
        serialize_bounded(&ReplayFrame::ReplayEnd {
            session_id,
            high_water_seq: high_water,
        })
        .map_err(|_| ApplicationError::stream_unavailable())?,
    );
    Ok(frames)
}

fn serialize_bounded<T: Serialize>(value: &T) -> Result<String, ()> {
    let bytes = serde_json::to_vec(value).map_err(|_| ())?;
    if bytes.len() > MAX_SERVER_MESSAGE_BYTES {
        return Err(());
    }
    String::from_utf8(bytes).map_err(|_| ())
}

#[derive(Default)]
struct Control {
    state: AtomicU8,
    changed: Notify,
}

impl Control {
    fn record(&self, state: ControlState) {
        if self
            .state
            .compare_exchange(
                ControlState::Open as u8,
                state as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.changed.notify_waiters();
        }
    }

    fn state(&self) -> ControlState {
        match self.state.load(Ordering::Acquire) {
            1 => ControlState::PeerClosed,
            2 => ControlState::ProtocolError,
            3 => ControlState::TransportClosed,
            _ => ControlState::Open,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(u8)]
enum ControlState {
    Open = 0,
    PeerClosed = 1,
    ProtocolError = 2,
    TransportClosed = 3,
}

fn spawn_control_reader(
    mut reader: Box<dyn SocketReader>,
    control: Arc<Control>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match reader.receive().await {
                Ok(Some(ClientFrame::Ping | ClientFrame::Pong)) => {}
                Ok(Some(ClientFrame::Text(_) | ClientFrame::Binary)) => {
                    control.record(ControlState::ProtocolError);
                    return;
                }
                Ok(Some(ClientFrame::Close)) | Ok(None) => {
                    control.record(ControlState::PeerClosed);
                    return;
                }
                Err(()) => {
                    control.record(ControlState::TransportClosed);
                    return;
                }
            }
        }
    })
}

enum StreamOutcome {
    PeerClosed,
    TransportClosed,
    ServerShutdown,
    Failed(ApplicationError),
}

fn connection_outcome(admission: &RequestAdmission, control: &Control) -> Option<StreamOutcome> {
    match control.state() {
        ControlState::PeerClosed => return Some(StreamOutcome::PeerClosed),
        ControlState::ProtocolError => {
            return Some(StreamOutcome::Failed(ApplicationError::protocol_error()));
        }
        ControlState::TransportClosed => return Some(StreamOutcome::TransportClosed),
        ControlState::Open => {}
    }
    if !admission.is_auth_valid() {
        Some(StreamOutcome::Failed(
            ApplicationError::authentication_expired(),
        ))
    } else if !admission.is_control_plane_running() {
        Some(StreamOutcome::ServerShutdown)
    } else {
        None
    }
}

async fn send_frame(
    writer: &mut dyn SocketWriter,
    frame: ServerFrame,
    deadlines: StreamDeadlines,
) -> Result<(), ()> {
    tokio::time::timeout(deadlines.write, writer.send(frame))
        .await
        .map_err(|_| ())?
}

async fn send_text(
    writer: &mut dyn SocketWriter,
    text: String,
    deadlines: StreamDeadlines,
) -> Result<(), ()> {
    send_frame(writer, ServerFrame::Text(text), deadlines).await
}

async fn send_close(
    writer: &mut dyn SocketWriter,
    code: u16,
    reason: &'static str,
    deadlines: StreamDeadlines,
) -> bool {
    send_frame(writer, ServerFrame::Close { code, reason }, deadlines)
        .await
        .is_ok()
}

async fn fail_before_subscription(
    reader: &mut dyn SocketReader,
    writer: &mut dyn SocketWriter,
    error: ApplicationError,
    deadlines: StreamDeadlines,
) {
    let Ok(text) = error.serialize() else {
        return;
    };
    if send_text(writer, text, deadlines).await.is_err() {
        return;
    }
    if !send_close(writer, error.close_code, error.code, deadlines).await {
        return;
    }
    let _ = tokio::time::timeout(deadlines.close_grace, reader.receive()).await;
}

async fn fail_after_subscription(
    writer: &mut dyn SocketWriter,
    control: &Control,
    reader_task: JoinHandle<()>,
    mut error: ApplicationError,
    deadlines: StreamDeadlines,
) {
    match control.state() {
        ControlState::PeerClosed | ControlState::TransportClosed => {
            finish_reader(reader_task, Duration::ZERO).await;
            return;
        }
        ControlState::ProtocolError => error = ApplicationError::protocol_error(),
        ControlState::Open => {}
    }
    let Ok(text) = error.serialize() else {
        finish_reader(reader_task, Duration::ZERO).await;
        return;
    };
    if send_text(writer, text, deadlines).await.is_err() {
        finish_reader(reader_task, Duration::ZERO).await;
        return;
    }
    if send_close(writer, error.close_code, error.code, deadlines).await {
        finish_reader(reader_task, deadlines.close_grace).await;
    } else {
        finish_reader(reader_task, Duration::ZERO).await;
    }
}

async fn finish_reader(mut reader_task: JoinHandle<()>, grace: Duration) {
    if tokio::time::timeout(grace, &mut reader_task).await.is_err() {
        reader_task.abort();
        let _ = reader_task.await;
    }
}
