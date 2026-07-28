use std::fmt;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codebox_agent_codex::{DuplicateRiskAcknowledgement, LoginBroker, UnknownSubmitDecision};
use codebox_session_runtime::{P0Actor, P0CloudLifecycle, P0SessionRuntime, P0TurnProjection};

use crate::config::P0HttpConfig;
use crate::error::{
    ApiError, P0HttpShutdownError, P0HttpShutdownErrorCategory, map_login_error, map_session_error,
};
use crate::idempotency::{CacheDisposition, Idempotency};
use crate::ports::{ConcreteLoginPort, ConcreteSessionPort, LoginPort, SessionPort};
use crate::transport::CachedResponse;
use crate::types::{BootstrapResponse, DeviceLoginResponse, LoginStatusResponse, Mutation};

const COOKIE_NAME: &[u8] = b"__Host-codebox_p0";
const COOKIE_TOKEN_BYTES: usize = 43;
const MAX_APP_SESSIONS: usize = 4;
const MAX_COOKIE_DRAWS: usize = 8;

/// Authenticated private P0 control-plane composition.
pub struct P0ControlPlane {
    pub(crate) shared: Arc<Shared>,
}

impl P0ControlPlane {
    /// Binds accepted concrete login and session runtimes to the P0 HTTP policy.
    pub fn new(
        config: P0HttpConfig,
        login: LoginBroker,
        session: Arc<P0SessionRuntime>,
    ) -> Result<Self, crate::P0HttpConfigError> {
        Ok(Self::with_components(
            config,
            Arc::new(ConcreteLoginPort::new(login)),
            Arc::new(ConcreteSessionPort::new(session)),
            Arc::new(SystemClock::new()),
            Arc::new(SystemEntropy),
        ))
    }

    /// Returns a cloneable router sharing this control-plane's bounded registries.
    pub fn router(&self) -> Router {
        crate::routes::router(Arc::clone(&self.shared))
    }

    /// Drains admitted handlers and cleans up the accepted lower runtimes exactly once.
    pub async fn shutdown(&self) -> Result<(), P0HttpShutdownError> {
        let shared = Arc::clone(&self.shared);
        let worker = Arc::clone(&self.shared);
        match tokio::task::spawn_blocking(move || worker.shutdown_blocking()).await {
            Ok(result) => result,
            Err(_) => {
                shared
                    .lifecycle
                    .force_complete(Err(P0HttpShutdownErrorCategory::Worker));
                Err(P0HttpShutdownError::new(
                    P0HttpShutdownErrorCategory::Worker,
                ))
            }
        }
    }

    fn with_components(
        config: P0HttpConfig,
        login: Arc<dyn LoginPort>,
        session: Arc<dyn SessionPort>,
        clock: Arc<dyn MonotonicClock>,
        entropy: Arc<dyn EntropySource>,
    ) -> Self {
        let identity = session.identity();
        Self {
            shared: Arc::new(Shared {
                config,
                identity,
                login,
                session,
                clock,
                entropy,
                app_sessions: Mutex::new(AppSessionRegistry::default()),
                idempotency: Idempotency::new(),
                lifecycle: Arc::new(Lifecycle::default()),
                #[cfg(test)]
                logout_executions: AtomicUsize::new(0),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_components(
        config: P0HttpConfig,
        login: Arc<dyn LoginPort>,
        session: Arc<dyn SessionPort>,
        clock: Arc<dyn MonotonicClock>,
        entropy: Arc<dyn EntropySource>,
    ) -> Self {
        Self::with_components(config, login, session, clock, entropy)
    }
}

impl fmt::Debug for P0ControlPlane {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("P0ControlPlane")
            .field("identity", &self.shared.identity)
            .finish_non_exhaustive()
    }
}

pub(crate) struct Shared {
    pub(crate) config: P0HttpConfig,
    pub(crate) identity: codebox_session_runtime::P0SessionIdentity,
    pub(crate) login: Arc<dyn LoginPort>,
    pub(crate) session: Arc<dyn SessionPort>,
    pub(crate) clock: Arc<dyn MonotonicClock>,
    entropy: Arc<dyn EntropySource>,
    app_sessions: Mutex<AppSessionRegistry>,
    pub(crate) idempotency: Arc<Idempotency>,
    pub(crate) lifecycle: Arc<Lifecycle>,
    #[cfg(test)]
    logout_executions: AtomicUsize,
}

impl Shared {
    pub(crate) fn now(&self) -> Duration {
        self.clock.now()
    }

    pub(crate) fn validate_origin(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        if self.config.public_origin.matches_header(
            single_header(headers, axum::http::header::ORIGIN).map(HeaderValue::as_bytes),
        ) {
            Ok(())
        } else {
            Err(ApiError::origin_forbidden())
        }
    }

    pub(crate) fn validate_bootstrap(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let candidate = single_header(headers, axum::http::header::AUTHORIZATION)
            .map(HeaderValue::as_bytes)
            .and_then(|value| value.strip_prefix(b"Bearer "));
        if self.config.bootstrap.matches(candidate) {
            Ok(())
        } else {
            Err(ApiError::authentication_required())
        }
    }

    pub(crate) fn create_app_session(&self) -> Result<CachedResponse, ApiError> {
        let now = self.now();
        let mut registry = self
            .app_sessions
            .lock()
            .map_err(|_| ApiError::service_unavailable())?;
        registry.prune_expired(now);
        if registry
            .sessions
            .iter()
            .filter(|session| !session.invalidated)
            .count()
            >= MAX_APP_SESSIONS
        {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "session_limit",
                "operator session limit reached",
            ));
        }

        let mut token = None;
        for _ in 0..MAX_COOKIE_DRAWS {
            let mut random = [0u8; 32];
            self.entropy
                .fill(&mut random)
                .map_err(|_| ApiError::service_unavailable())?;
            let encoded = URL_SAFE_NO_PAD.encode(random);
            let bytes: [u8; COOKIE_TOKEN_BYTES] = encoded
                .as_bytes()
                .try_into()
                .map_err(|_| ApiError::service_unavailable())?;
            if !registry
                .sessions
                .iter()
                .any(|session| fixed_token_eq(&session.token, &bytes))
            {
                token = Some(bytes);
                break;
            }
        }
        let token = token.ok_or_else(ApiError::service_unavailable)?;

        registry.issuance_seq = registry.issuance_seq.saturating_add(1);
        let issuance_seq = registry.issuance_seq;
        let expires_at = now
            .checked_add(self.config.session_lifetime)
            .ok_or_else(ApiError::service_unavailable)?;
        registry.sessions.push(AppSession {
            token,
            expires_at,
            issuance_seq,
            invalidated: false,
            active_requests: 0,
        });

        let token_text =
            std::str::from_utf8(&token).map_err(|_| ApiError::service_unavailable())?;
        let lifetime = self.config.session_lifetime.as_secs();
        let cookie = HeaderValue::from_str(&format!(
            "__Host-codebox_p0={token_text}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age={lifetime}"
        ))
        .map_err(|_| ApiError::service_unavailable())?;
        CachedResponse::bootstrap(&BootstrapResponse::new(self.identity, lifetime), cookie)
    }

    pub(crate) fn authenticate_cookie(
        self: &Arc<Self>,
        headers: &HeaderMap,
    ) -> Result<AppSessionLease, ApiError> {
        let (candidate, syntactically_valid) = parse_cookie(headers);
        let now = self.now();
        let mut registry = self
            .app_sessions
            .lock()
            .map_err(|_| ApiError::service_unavailable())?;
        registry.prune_expired(now);

        let mut matched = None;
        if registry.sessions.is_empty() {
            let _ = fixed_token_eq(&[0; COOKIE_TOKEN_BYTES], &candidate);
        }
        for (index, session) in registry.sessions.iter().enumerate() {
            let equal = fixed_token_eq(&session.token, &candidate);
            if syntactically_valid && equal && !session.invalidated && now < session.expires_at {
                matched = Some(index);
            }
        }
        let index = matched.ok_or_else(ApiError::authentication_required)?;
        let session = &mut registry.sessions[index];
        session.active_requests = session.active_requests.saturating_add(1);
        Ok(AppSessionLease {
            shared: Arc::clone(self),
            token: session.token,
            issuance_seq: session.issuance_seq,
            active: true,
        })
    }

    pub(crate) fn execute_mutation(
        &self,
        mutation: Mutation,
        admitted_at: Duration,
    ) -> (CachedResponse, CacheDisposition) {
        match self.try_execute_mutation(mutation, admitted_at) {
            Ok(result) => result,
            Err(error) => (CachedResponse::error(error), CacheDisposition::Retain),
        }
    }

    fn try_execute_mutation(
        &self,
        mutation: Mutation,
        admitted_at: Duration,
    ) -> Result<(CachedResponse, CacheDisposition), ApiError> {
        match mutation {
            Mutation::Logout {
                cookie,
                session_seq,
            } => {
                #[cfg(test)]
                self.logout_executions.fetch_add(1, Ordering::SeqCst);
                self.invalidate_cookie(cookie, session_seq)?;
                let cookie = HeaderValue::from_static(
                    "__Host-codebox_p0=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
                );
                Ok((
                    CachedResponse::empty_logout(cookie),
                    CacheDisposition::RemoveAfterAuthSession(session_seq),
                ))
            }
            Mutation::StartDeviceLogin => {
                let interaction = self.login.start_device_login().map_err(map_login_error)?;
                let expires_at = admitted_at
                    .checked_add(Duration::from_secs(u64::from(
                        interaction.expires_in_seconds,
                    )))
                    .ok_or_else(ApiError::service_unavailable)?;
                let response = DeviceLoginResponse {
                    operation_id: interaction.operation_id,
                    verification_url: interaction.verification_url,
                    verification_code: interaction.verification_code,
                    expires_in_seconds: interaction.expires_in_seconds,
                };
                Ok((
                    CachedResponse::json(StatusCode::ACCEPTED, &response)?,
                    CacheDisposition::ExpireAt(expires_at),
                ))
            }
            Mutation::CancelLogin => {
                let status = self.login.cancel().map_err(map_login_error)?;
                Ok((
                    CachedResponse::json(StatusCode::OK, &LoginStatusResponse::from(status))?,
                    CacheDisposition::Retain,
                ))
            }
            Mutation::StartTurn { prompt } => {
                let receipt = self.session.start_turn(prompt).map_err(map_session_error)?;
                Ok((
                    CachedResponse::json(StatusCode::ACCEPTED, &receipt)?,
                    CacheDisposition::Retain,
                ))
            }
            Mutation::CancelTurn => {
                let snapshot = self
                    .session
                    .cancel_turn(P0Actor::Operator)
                    .map_err(map_session_error)?;
                Ok((
                    CachedResponse::json(StatusCode::OK, &snapshot)?,
                    CacheDisposition::Retain,
                ))
            }
            Mutation::Reconcile => {
                let candidates = self
                    .session
                    .reconcile_unknown(P0Actor::Operator)
                    .map_err(map_session_error)?;
                Ok((
                    CachedResponse::json(StatusCode::OK, &candidates)?,
                    CacheDisposition::Retain,
                ))
            }
            Mutation::Adopt {
                operation_id,
                task_id,
            } => {
                self.require_current_unknown(operation_id)?;
                let snapshot = self
                    .session
                    .resolve_unknown(
                        P0Actor::Operator,
                        operation_id,
                        UnknownSubmitDecision::AdoptListedTask(task_id),
                    )
                    .map_err(map_session_error)?;
                Ok((
                    CachedResponse::json(StatusCode::OK, &snapshot)?,
                    CacheDisposition::Retain,
                ))
            }
            Mutation::Abandon { operation_id } => {
                self.require_current_unknown(operation_id)?;
                let acknowledgement = DuplicateRiskAcknowledgement::for_operation(operation_id);
                let snapshot = self
                    .session
                    .resolve_unknown(
                        P0Actor::Operator,
                        operation_id,
                        UnknownSubmitDecision::AbandonAfterReconciliation(acknowledgement),
                    )
                    .map_err(map_session_error)?;
                Ok((
                    CachedResponse::json(StatusCode::OK, &snapshot)?,
                    CacheDisposition::Retain,
                ))
            }
        }
    }

    fn require_current_unknown(
        &self,
        operation_id: codebox_agent_codex::CloudSubmitOperationId,
    ) -> Result<(), ApiError> {
        let snapshot = self.session.snapshot().map_err(map_session_error)?;
        let exact = snapshot.current_turn.as_ref().is_some_and(|turn| {
            matches!(
                &turn.projection,
                P0TurnProjection::Cloud {
                    lifecycle: P0CloudLifecycle::OutcomeUnknown {
                        operation_id: current
                    },
                    ..
                } if *current == operation_id
            )
        });
        if exact {
            Ok(())
        } else {
            Err(ApiError::new(
                StatusCode::CONFLICT,
                "operation_changed",
                "current operation changed; refresh before retry",
            ))
        }
    }

    fn invalidate_cookie(
        &self,
        cookie: [u8; COOKIE_TOKEN_BYTES],
        session_seq: u64,
    ) -> Result<(), ApiError> {
        let mut registry = self
            .app_sessions
            .lock()
            .map_err(|_| ApiError::service_unavailable())?;
        if let Some(session) = registry.sessions.iter_mut().find(|session| {
            session.issuance_seq == session_seq && fixed_token_eq(&session.token, &cookie)
        }) {
            session.invalidated = true;
        }
        Ok(())
    }

    fn release_app_session(&self, session_seq: u64) {
        let mut remove_idempotency = false;
        if let Ok(mut registry) = self.app_sessions.lock() {
            if let Some(index) = registry
                .sessions
                .iter()
                .position(|session| session.issuance_seq == session_seq)
            {
                let session = &mut registry.sessions[index];
                session.active_requests = session.active_requests.saturating_sub(1);
                if session.invalidated && session.active_requests == 0 {
                    registry.sessions.remove(index);
                    remove_idempotency = true;
                }
            } else {
                remove_idempotency = true;
            }
        }
        if remove_idempotency {
            self.idempotency.release_auth_session(session_seq);
        }
    }

    fn app_session_is_valid(&self, session_seq: u64) -> bool {
        let now = self.now();
        let Ok(mut registry) = self.app_sessions.lock() else {
            return false;
        };
        registry.prune_expired(now);
        registry.sessions.iter().any(|session| {
            session.issuance_seq == session_seq && !session.invalidated && now < session.expires_at
        })
    }

    fn shutdown_blocking(&self) -> Result<(), P0HttpShutdownError> {
        match self.lifecycle.begin_shutdown() {
            ShutdownRole::Completed(result) => {
                return result.map_err(P0HttpShutdownError::new);
            }
            ShutdownRole::Leader => {}
        }

        let session_result = self.session.shutdown();
        let login_result = self.login.shutdown_cleanup();
        let result = if session_result.is_err() {
            Err(P0HttpShutdownErrorCategory::Session)
        } else if login_result.is_err() {
            Err(P0HttpShutdownErrorCategory::Login)
        } else {
            Ok(())
        };
        if let Ok(mut sessions) = self.app_sessions.lock() {
            sessions.sessions.clear();
        }
        self.idempotency.clear();
        self.lifecycle.force_complete(result);
        result.map_err(P0HttpShutdownError::new)
    }

    #[cfg(test)]
    pub(crate) fn app_session_count(&self) -> usize {
        self.app_sessions
            .lock()
            .map(|registry| registry.sessions.len())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn logout_execution_count(&self) -> usize {
        self.logout_executions.load(Ordering::SeqCst)
    }
}

impl fmt::Debug for Shared {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Shared")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct AppSessionRegistry {
    sessions: Vec<AppSession>,
    issuance_seq: u64,
}

impl AppSessionRegistry {
    fn prune_expired(&mut self, now: Duration) {
        for session in &mut self.sessions {
            if session.expires_at <= now {
                session.invalidated = true;
            }
        }
        self.sessions
            .retain(|session| !session.invalidated || session.active_requests != 0);
        self.sessions.sort_by_key(|session| session.issuance_seq);
    }
}

struct AppSession {
    token: [u8; COOKIE_TOKEN_BYTES],
    expires_at: Duration,
    issuance_seq: u64,
    invalidated: bool,
    active_requests: usize,
}

pub(crate) struct AppSessionLease {
    shared: Arc<Shared>,
    token: [u8; COOKIE_TOKEN_BYTES],
    issuance_seq: u64,
    active: bool,
}

impl AppSessionLease {
    pub(crate) const fn token(&self) -> [u8; COOKIE_TOKEN_BYTES] {
        self.token
    }

    pub(crate) const fn issuance_seq(&self) -> u64 {
        self.issuance_seq
    }

    fn is_valid(&self) -> bool {
        self.shared.app_session_is_valid(self.issuance_seq)
    }
}

impl Drop for AppSessionLease {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            self.shared.release_app_session(self.issuance_seq);
        }
    }
}

pub(crate) struct RequestAdmission {
    _lifecycle: ActiveOperation,
    app_session: AppSessionLease,
}

impl RequestAdmission {
    pub(crate) fn new(lifecycle: ActiveOperation, app_session: AppSessionLease) -> Self {
        Self {
            _lifecycle: lifecycle,
            app_session,
        }
    }

    pub(crate) const fn token(&self) -> [u8; COOKIE_TOKEN_BYTES] {
        self.app_session.token()
    }

    pub(crate) const fn session_seq(&self) -> u64 {
        self.app_session.issuance_seq()
    }

    pub(crate) fn is_auth_valid(&self) -> bool {
        self.app_session.is_valid()
    }

    pub(crate) fn is_control_plane_running(&self) -> bool {
        self._lifecycle.lifecycle.is_running()
    }
}

fn parse_cookie(headers: &HeaderMap) -> ([u8; COOKIE_TOKEN_BYTES], bool) {
    let mut candidate = [0u8; COOKIE_TOKEN_BYTES];
    let mut count = 0usize;
    let mut valid = false;
    for value in headers.get_all(axum::http::header::COOKIE) {
        for part in value.as_bytes().split(|byte| *byte == b';') {
            let part = trim_ascii_space(part);
            let Some(separator) = part.iter().position(|byte| *byte == b'=') else {
                continue;
            };
            let (name, value) = part.split_at(separator);
            let value = &value[1..];
            if name != COOKIE_NAME {
                continue;
            }
            count = count.saturating_add(1);
            if value.len() == COOKIE_TOKEN_BYTES {
                candidate.copy_from_slice(value);
                valid = true;
            }
        }
    }
    (candidate, valid && count == 1)
}

fn trim_ascii_space(mut value: &[u8]) -> &[u8] {
    while value.first() == Some(&b' ') {
        value = &value[1..];
    }
    while value.last() == Some(&b' ') {
        value = &value[..value.len() - 1];
    }
    value
}

fn fixed_token_eq(left: &[u8; COOKIE_TOKEN_BYTES], right: &[u8; COOKIE_TOKEN_BYTES]) -> bool {
    fixed_token_compare(left, right).0
}

fn fixed_token_compare(
    left: &[u8; COOKIE_TOKEN_BYTES],
    right: &[u8; COOKIE_TOKEN_BYTES],
) -> (bool, usize) {
    let mut difference = 0u8;
    let mut compared = 0usize;
    for (left, right) in left.iter().zip(right) {
        difference |= *left ^ *right;
        compared += 1;
    }
    (difference == 0, compared)
}

#[cfg(test)]
pub(crate) fn cookie_comparison_work(
    left: &[u8; COOKIE_TOKEN_BYTES],
    right: &[u8; COOKIE_TOKEN_BYTES],
) -> usize {
    fixed_token_compare(left, right).1
}

fn single_header(
    headers: &HeaderMap,
    name: axum::http::header::HeaderName,
) -> Option<&HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_none() {
        Some(value)
    } else {
        None
    }
}

pub(crate) trait MonotonicClock: Send + Sync {
    fn now(&self) -> Duration;
}

struct SystemClock {
    started: Instant,
}

impl SystemClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl MonotonicClock for SystemClock {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }
}

pub(crate) trait EntropySource: Send + Sync {
    fn fill(&self, destination: &mut [u8]) -> Result<(), ()>;
}

struct SystemEntropy;

impl EntropySource for SystemEntropy {
    fn fill(&self, destination: &mut [u8]) -> Result<(), ()> {
        getrandom::fill(destination).map_err(|_| ())
    }
}

#[derive(Default)]
pub(crate) struct Lifecycle {
    state: Mutex<LifecycleState>,
    changed: Condvar,
}

#[derive(Default)]
struct LifecycleState {
    phase: ShutdownPhase,
    active: usize,
}

#[derive(Clone, Copy, Default)]
enum ShutdownPhase {
    #[default]
    Running,
    Draining,
    Complete(Result<(), P0HttpShutdownErrorCategory>),
}

enum ShutdownRole {
    Leader,
    Completed(Result<(), P0HttpShutdownErrorCategory>),
}

impl Lifecycle {
    pub(crate) fn admit(self: &Arc<Self>) -> Result<ActiveOperation, ApiError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ApiError::service_unavailable())?;
        if !matches!(state.phase, ShutdownPhase::Running) {
            return Err(ApiError::service_unavailable());
        }
        state.active = state.active.saturating_add(1);
        Ok(ActiveOperation {
            lifecycle: Arc::clone(self),
        })
    }

    pub(crate) fn is_running(&self) -> bool {
        self.state
            .lock()
            .map(|state| matches!(state.phase, ShutdownPhase::Running))
            .unwrap_or(false)
    }

    fn begin_shutdown(&self) -> ShutdownRole {
        let Ok(mut state) = self.state.lock() else {
            return ShutdownRole::Completed(Err(P0HttpShutdownErrorCategory::Worker));
        };
        loop {
            match state.phase {
                ShutdownPhase::Running => {
                    state.phase = ShutdownPhase::Draining;
                    while state.active != 0 {
                        let Ok(next) = self.changed.wait(state) else {
                            return ShutdownRole::Completed(Err(
                                P0HttpShutdownErrorCategory::Worker,
                            ));
                        };
                        state = next;
                    }
                    return ShutdownRole::Leader;
                }
                ShutdownPhase::Draining => {
                    let Ok(next) = self.changed.wait(state) else {
                        return ShutdownRole::Completed(Err(P0HttpShutdownErrorCategory::Worker));
                    };
                    state = next;
                }
                ShutdownPhase::Complete(result) => return ShutdownRole::Completed(result),
            }
        }
    }

    fn force_complete(&self, result: Result<(), P0HttpShutdownErrorCategory>) {
        if let Ok(mut state) = self.state.lock() {
            state.phase = ShutdownPhase::Complete(result);
            self.changed.notify_all();
        }
    }
}

pub(crate) struct ActiveOperation {
    lifecycle: Arc<Lifecycle>,
}

impl Drop for ActiveOperation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.lifecycle.state.lock() {
            state.active = state.active.saturating_sub(1);
            self.lifecycle.changed.notify_all();
        }
    }
}
