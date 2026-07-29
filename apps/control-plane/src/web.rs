use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{
    HeaderName, HeaderValue, Response, StatusCode,
    header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE},
};
use bytes::Bytes;

use crate::error::ApiError;
use crate::state::Shared;
use crate::transport::CachedResponse;

pub(crate) const INDEX_HTML: &[u8] = include_bytes!("../web/index.html");
pub(crate) const APP_JS: &[u8] = include_bytes!("../web/p0-app.js");
pub(crate) const CLIENT_JS: &[u8] = include_bytes!("../web/p0-client.js");
pub(crate) const STYLE_CSS: &[u8] = include_bytes!("../web/p0.css");

const NOSNIFF: HeaderName = HeaderName::from_static("x-content-type-options");
const REFERRER_POLICY: HeaderName = HeaderName::from_static("referrer-policy");
const X_FRAME_OPTIONS: HeaderName = HeaderName::from_static("x-frame-options");
const CROSS_ORIGIN_OPENER_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-opener-policy");
const CROSS_ORIGIN_RESOURCE_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-resource-policy");

pub(crate) async fn index(State(shared): State<Arc<Shared>>) -> Response<Body> {
    asset_response(&shared, INDEX_HTML, "text/html; charset=utf-8")
}

pub(crate) async fn app(State(shared): State<Arc<Shared>>) -> Response<Body> {
    asset_response(&shared, APP_JS, "text/javascript; charset=utf-8")
}

pub(crate) async fn client(State(shared): State<Arc<Shared>>) -> Response<Body> {
    asset_response(&shared, CLIENT_JS, "text/javascript; charset=utf-8")
}

pub(crate) async fn style(State(shared): State<Arc<Shared>>) -> Response<Body> {
    asset_response(&shared, STYLE_CSS, "text/css; charset=utf-8")
}

fn asset_response(
    shared: &Shared,
    bytes: &'static [u8],
    content_type: &'static str,
) -> Response<Body> {
    let csp = format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self' {}; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none'",
        shared.config.public_origin.websocket_origin()
    );
    let Ok(csp) = HeaderValue::from_str(&csp) else {
        return CachedResponse::error(ApiError::service_unavailable()).into_response();
    };

    let mut response = Response::new(Body::from(Bytes::from_static(bytes)));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(NOSNIFF, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        CROSS_ORIGIN_OPENER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        CROSS_ORIGIN_RESOURCE_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(CONTENT_SECURITY_POLICY, csp);
    response
}
