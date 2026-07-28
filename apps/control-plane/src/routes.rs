use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, Response, StatusCode, header::CONTENT_TYPE};
use axum::routing::{get, post};
use codebox_agent_codex::{CloudPrompt, CloudTaskId};
use codebox_domain::CommandId;
use codebox_session_runtime::P0InstanceId;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::error::{ApiError, map_login_error, map_session_error};
use crate::state::{RequestAdmission, Shared};
use crate::transport::CachedResponse;
use crate::types::{
    LoginStatusResponse, Mutation, PreparedMutation, RequestIdentity, ResolveBody,
    ResolveDecisionBody, StartTurnBody,
};

const MAX_HEADER_BYTES: usize = 256;
const MAX_BODY_BYTES: usize = 40 * 1024;
const HEADER_INSTANCE: &str = "codebox-instance-id";
const HEADER_IDEMPOTENCY: &str = "idempotency-key";

const ROUTE_OPERATOR_SESSION: &str = "/api/p0/v1/operator/session";
const ROUTE_LOGIN: &str = "/api/p0/v1/login";
const ROUTE_LOGIN_DEVICE: &str = "/api/p0/v1/login/device";
const ROUTE_LOGIN_CANCEL: &str = "/api/p0/v1/login/cancel";
const ROUTE_SESSION: &str = "/api/p0/v1/session";
const ROUTE_SESSION_TURNS: &str = "/api/p0/v1/session/turns";
const ROUTE_SESSION_CANCEL: &str = "/api/p0/v1/session/cancel";
const ROUTE_SESSION_RECONCILE: &str = "/api/p0/v1/session/reconcile";
const ROUTE_SESSION_RESOLVE: &str = "/api/p0/v1/session/resolve";
const ROUTE_SESSION_DIFF: &str = "/api/p0/v1/session/diff";

pub(crate) fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route(
            ROUTE_OPERATOR_SESSION,
            post(bootstrap_session).delete(logout),
        )
        .route(ROUTE_LOGIN, get(login_status))
        .route(ROUTE_LOGIN_DEVICE, post(start_device_login))
        .route(ROUTE_LOGIN_CANCEL, post(cancel_login))
        .route(ROUTE_SESSION, get(session_snapshot))
        .route(ROUTE_SESSION_TURNS, post(start_turn))
        .route(ROUTE_SESSION_CANCEL, post(cancel_turn))
        .route(ROUTE_SESSION_RECONCILE, post(reconcile))
        .route(ROUTE_SESSION_RESOLVE, post(resolve))
        .route(ROUTE_SESSION_DIFF, get(session_diff))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(shared)
}

async fn bootstrap_session(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result = async {
        require_running(&shared)?;
        validate_headers(request.headers())?;
        let headers = request.headers().clone();
        read_empty(request).await?;
        shared.validate_origin(&headers)?;
        shared.validate_bootstrap(&headers)?;
        let _operation = shared.lifecycle.admit()?;
        shared.create_app_session()
    }
    .await;
    into_response(result)
}

async fn logout(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result = prepare_empty_mutation(
        &shared,
        request,
        Method::DELETE,
        ROUTE_OPERATOR_SESSION,
        |admission| Mutation::Logout {
            cookie: admission.token(),
            session_seq: admission.session_seq(),
        },
    )
    .await;
    run_prepared(shared, result).await
}

async fn login_status(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result = prepare_observation(&shared, &request).and_then(|admission| {
        ensure_request_method(&request, Method::GET)?;
        Ok(admission)
    });
    let admission = match result {
        Ok(admission) => admission,
        Err(error) => return into_response(Err(error)),
    };
    if let Err(error) = read_empty(request).await {
        return into_response(Err(error));
    }
    let login = Arc::clone(&shared.login);
    let response = tokio::task::spawn_blocking(move || {
        let _admission = admission;
        login
            .status()
            .map(LoginStatusResponse::from)
            .map_err(map_login_error)
            .and_then(|status| CachedResponse::json(StatusCode::OK, &status))
    })
    .await
    .unwrap_or_else(|_| Err(ApiError::service_unavailable()));
    into_response(response)
}

async fn start_device_login(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result = prepare_empty_mutation(&shared, request, Method::POST, ROUTE_LOGIN_DEVICE, |_| {
        Mutation::StartDeviceLogin
    })
    .await;
    run_prepared(shared, result).await
}

async fn cancel_login(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result = prepare_empty_mutation(&shared, request, Method::POST, ROUTE_LOGIN_CANCEL, |_| {
        Mutation::CancelLogin
    })
    .await;
    run_prepared(shared, result).await
}

async fn session_snapshot(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result = prepare_observation(&shared, &request).and_then(|admission| {
        ensure_request_method(&request, Method::GET)?;
        Ok(admission)
    });
    let admission = match result {
        Ok(admission) => admission,
        Err(error) => return into_response(Err(error)),
    };
    if let Err(error) = read_empty(request).await {
        return into_response(Err(error));
    }
    let session = Arc::clone(&shared.session);
    let response = tokio::task::spawn_blocking(move || {
        let _admission = admission;
        session
            .snapshot()
            .map_err(map_session_error)
            .and_then(|snapshot| CachedResponse::json(StatusCode::OK, &snapshot))
    })
    .await
    .unwrap_or_else(|_| Err(ApiError::service_unavailable()));
    into_response(response)
}

async fn start_turn(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result =
        prepare_json_mutation::<StartTurnBody>(&shared, request, Method::POST, ROUTE_SESSION_TURNS)
            .await
            .and_then(|(base, body)| {
                let prompt =
                    CloudPrompt::try_new(body.prompt).map_err(|_| ApiError::invalid_value())?;
                Ok(PreparedMutation {
                    mutation: Mutation::StartTurn { prompt },
                    ..base
                })
            });
    run_prepared(shared, result).await
}

async fn cancel_turn(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result =
        prepare_empty_mutation(&shared, request, Method::POST, ROUTE_SESSION_CANCEL, |_| {
            Mutation::CancelTurn
        })
        .await;
    run_prepared(shared, result).await
}

async fn reconcile(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result = prepare_empty_mutation(
        &shared,
        request,
        Method::POST,
        ROUTE_SESSION_RECONCILE,
        |_| Mutation::Reconcile,
    )
    .await;
    run_prepared(shared, result).await
}

async fn resolve(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result =
        prepare_json_mutation::<ResolveBody>(&shared, request, Method::POST, ROUTE_SESSION_RESOLVE)
            .await
            .and_then(|(base, body)| {
                let operation_id = serde_json::from_value::<
                    codebox_agent_codex::CloudSubmitOperationId,
                >(body.operation_id)
                .map_err(|_| ApiError::invalid_value())?;
                let mutation = match body.decision {
                    ResolveDecisionBody::Adopt { task_id } => Mutation::Adopt {
                        operation_id,
                        task_id: CloudTaskId::try_new(task_id)
                            .map_err(|_| ApiError::invalid_value())?,
                    },
                    ResolveDecisionBody::Abandon {
                        acknowledge_duplicate_task_risk: Some(true),
                    } => Mutation::Abandon { operation_id },
                    ResolveDecisionBody::Abandon { .. } => {
                        return Err(ApiError::new(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "acknowledgement_required",
                            "duplicate-task-risk acknowledgement is required",
                        ));
                    }
                };
                Ok(PreparedMutation { mutation, ..base })
            });
    run_prepared(shared, result).await
}

async fn session_diff(State(shared): State<Arc<Shared>>, request: Request) -> Response<Body> {
    let result = prepare_observation(&shared, &request).and_then(|admission| {
        ensure_request_method(&request, Method::GET)?;
        Ok(admission)
    });
    let admission = match result {
        Ok(admission) => admission,
        Err(error) => return into_response(Err(error)),
    };
    if let Err(error) = read_empty(request).await {
        return into_response(Err(error));
    }
    let session = Arc::clone(&shared.session);
    let response = tokio::task::spawn_blocking(move || {
        let _admission = admission;
        session
            .read_diff()
            .map_err(map_session_error)
            .map(|diff| CachedResponse::diff(diff.as_str()))
    })
    .await
    .unwrap_or_else(|_| Err(ApiError::service_unavailable()));
    into_response(response)
}

async fn prepare_empty_mutation<F>(
    shared: &Arc<Shared>,
    request: Request,
    method: Method,
    route: &'static str,
    build: F,
) -> Result<PreparedMutation, ApiError>
where
    F: FnOnce(&RequestAdmission) -> Mutation,
{
    let admission = prepare_protected_mutation(shared, &request, &method)?;
    let instance_id = parse_instance(request.headers(), shared)?;
    let key = parse_idempotency_key(request.headers())?;
    let body = read_empty(request).await?;
    let mutation = build(&admission);
    let logout_session_seq = match &mutation {
        Mutation::Logout { session_seq, .. } => Some(*session_seq),
        _ => None,
    };
    Ok(PreparedMutation {
        key,
        identity: RequestIdentity {
            method: static_method_name(&method),
            route,
            body,
            instance_id,
            logout_session_seq,
        },
        mutation,
        admitted_at: shared.now(),
        admission,
    })
}

async fn prepare_json_mutation<T: DeserializeOwned>(
    shared: &Arc<Shared>,
    request: Request,
    method: Method,
    route: &'static str,
) -> Result<(PreparedMutation, T), ApiError> {
    let admission = prepare_protected_mutation(shared, &request, &method)?;
    let instance_id = parse_instance(request.headers(), shared)?;
    let key = parse_idempotency_key(request.headers())?;
    let body = read_json(request).await?;
    let parsed = serde_json::from_slice(&body).map_err(classify_json_error)?;
    Ok((
        PreparedMutation {
            key,
            identity: RequestIdentity {
                method: static_method_name(&method),
                route,
                body,
                instance_id,
                logout_session_seq: None,
            },
            mutation: Mutation::CancelTurn,
            admitted_at: shared.now(),
            admission,
        },
        parsed,
    ))
}

fn prepare_protected_mutation(
    shared: &Arc<Shared>,
    request: &Request,
    method: &Method,
) -> Result<Arc<RequestAdmission>, ApiError> {
    require_running(shared)?;
    ensure_request_method(request, method.clone())?;
    validate_headers(request.headers())?;
    let app_session = shared.authenticate_cookie(request.headers())?;
    let lifecycle = shared.lifecycle.admit()?;
    shared.validate_origin(request.headers())?;
    Ok(Arc::new(RequestAdmission::new(lifecycle, app_session)))
}

fn prepare_observation(
    shared: &Arc<Shared>,
    request: &Request,
) -> Result<Arc<RequestAdmission>, ApiError> {
    require_running(shared)?;
    validate_headers(request.headers())?;
    let app_session = shared.authenticate_cookie(request.headers())?;
    let lifecycle = shared.lifecycle.admit()?;
    Ok(Arc::new(RequestAdmission::new(lifecycle, app_session)))
}

async fn run_prepared(
    shared: Arc<Shared>,
    prepared: Result<PreparedMutation, ApiError>,
) -> Response<Body> {
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return into_response(Err(error)),
    };
    let operation = prepared.admission;
    let admission =
        match shared
            .idempotency
            .admit(prepared.key, prepared.identity, prepared.admitted_at)
        {
            Ok(admission) => admission,
            Err(error) => return into_response(Err(error)),
        };
    if admission.owner {
        let idempotency = Arc::clone(&shared.idempotency);
        let key = prepared.key;
        let admitted_at = prepared.admitted_at;
        let mutation = prepared.mutation;
        let worker = Arc::clone(&shared);
        let worker_operation = Arc::clone(&operation);
        tokio::spawn(async move {
            let _worker_operation = worker_operation;
            let result =
                tokio::task::spawn_blocking(move || worker.execute_mutation(mutation, admitted_at))
                    .await
                    .unwrap_or_else(|_| {
                        (
                            CachedResponse::error(ApiError::service_unavailable()),
                            crate::idempotency::CacheDisposition::Retain,
                        )
                    });
            idempotency.complete(key, result.0, result.1);
        });
    }
    let response = admission.waiter.response().await.into_response();
    drop(operation);
    response
}

fn require_running(shared: &Shared) -> Result<(), ApiError> {
    if shared.lifecycle.is_running() {
        Ok(())
    } else {
        Err(ApiError::service_unavailable())
    }
}

fn ensure_request_method(request: &Request, method: Method) -> Result<(), ApiError> {
    if request.method() == method {
        Ok(())
    } else {
        Err(ApiError::invalid_request())
    }
}

fn static_method_name(method: &Method) -> &'static str {
    if *method == Method::DELETE {
        "DELETE"
    } else {
        "POST"
    }
}

fn validate_headers(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers
        .values()
        .any(|value| value.as_bytes().len() > MAX_HEADER_BYTES)
    {
        Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "request schema is invalid",
        ))
    } else {
        Ok(())
    }
}

fn parse_idempotency_key(headers: &HeaderMap) -> Result<CommandId, ApiError> {
    let value = single_header(headers, HEADER_IDEMPOTENCY)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .and_then(|value| CommandId::try_from_uuid(value).ok());
    value.ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "idempotency_key_invalid",
            "idempotency key must be a non-nil UUID",
        )
    })
}

fn parse_instance(headers: &HeaderMap, shared: &Shared) -> Result<P0InstanceId, ApiError> {
    let supplied = single_header(headers, HEADER_INSTANCE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .and_then(|value| P0InstanceId::try_from_uuid(value).ok());
    match supplied {
        Some(supplied) if supplied == shared.identity.instance_id => Ok(supplied),
        _ => Err(ApiError::new(
            StatusCode::CONFLICT,
            "instance_changed",
            "control-plane instance changed; refresh before retry",
        )),
    }
}

async fn read_empty(request: Request) -> Result<Vec<u8>, ApiError> {
    validate_empty_content_type(request.headers())?;
    let body = read_bounded_body(request).await?;
    if body.is_empty() {
        Ok(body)
    } else {
        Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "body_not_empty",
            "request body must be empty",
        ))
    }
}

async fn read_json(request: Request) -> Result<Vec<u8>, ApiError> {
    validate_json_content_type(request.headers())?;
    read_bounded_body(request).await
}

async fn read_bounded_body(request: Request) -> Result<Vec<u8>, ApiError> {
    if single_header(
        request.headers(),
        axum::http::header::CONTENT_LENGTH.as_str(),
    )
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.parse::<usize>().ok())
    .is_some_and(|length| length > MAX_BODY_BYTES)
    {
        return Err(request_too_large());
    }
    to_bytes(request.into_body(), MAX_BODY_BYTES)
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|_| request_too_large())
}

fn validate_empty_content_type(headers: &HeaderMap) -> Result<(), ApiError> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    match (values.next(), values.next()) {
        (None, None) => Ok(()),
        (Some(value), None) if is_json_content_type(value.as_bytes()) => Ok(()),
        _ => Err(unsupported_media_type()),
    }
}

fn validate_json_content_type(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers
        .get_all(CONTENT_TYPE)
        .iter()
        .next()
        .filter(|_| headers.get_all(CONTENT_TYPE).iter().count() == 1)
        .is_some_and(|value| is_json_content_type(value.as_bytes()))
    {
        Ok(())
    } else {
        Err(unsupported_media_type())
    }
}

fn is_json_content_type(value: &[u8]) -> bool {
    value.eq_ignore_ascii_case(b"application/json")
        || value.eq_ignore_ascii_case(b"application/json; charset=utf-8")
}

fn unsupported_media_type() -> ApiError {
    ApiError::new(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported_media_type",
        "application/json is required",
    )
}

fn request_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "request_too_large",
        "request body exceeds its limit",
    )
}

fn classify_json_error(error: serde_json::Error) -> ApiError {
    if error.is_syntax() || error.is_eof() {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "malformed_json",
            "request body is not valid JSON",
        )
    } else {
        ApiError::invalid_request()
    }
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a axum::http::HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_none() {
        Some(value)
    } else {
        None
    }
}

fn into_response(result: Result<CachedResponse, ApiError>) -> Response<Body> {
    result.unwrap_or_else(CachedResponse::error).into_response()
}

async fn not_found() -> Response<Body> {
    CachedResponse::error(ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "requested route is not available",
    ))
    .into_response()
}

async fn method_not_allowed() -> Response<Body> {
    CachedResponse::error(ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "request method is not allowed",
    ))
    .into_response()
}
