use axum::body::Body;
use axum::http::{
    HeaderMap, HeaderName, HeaderValue, Response, StatusCode,
    header::{
        CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, SET_COOKIE,
    },
};
use bytes::Bytes;
use serde::Serialize;

use crate::error::ApiError;

const NOSNIFF: HeaderName = HeaderName::from_static("x-content-type-options");

#[derive(Clone)]
pub(crate) struct CachedResponse {
    pub(crate) status: StatusCode,
    pub(crate) headers: Vec<(HeaderName, HeaderValue)>,
    pub(crate) body: Bytes,
}

impl CachedResponse {
    pub(crate) fn json<T: Serialize>(status: StatusCode, value: &T) -> Result<Self, ApiError> {
        let body = serde_json::to_vec(value).map_err(|_| ApiError::service_unavailable())?;
        Ok(Self {
            status,
            headers: common_headers(Some("application/json")),
            body: Bytes::from(body),
        })
    }

    pub(crate) fn error(error: ApiError) -> Self {
        match serde_json::to_vec(&error) {
            Ok(body) => Self {
                status: error.status,
                headers: common_headers(Some("application/json")),
                body: Bytes::from(body),
            },
            Err(_) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                headers: common_headers(Some("application/json")),
                body: Bytes::from_static(
                    br#"{"error":{"code":"service_unavailable","message":"control-plane service is unavailable"}}"#,
                ),
            },
        }
    }

    pub(crate) fn empty_logout(cookie: HeaderValue) -> Self {
        let mut headers = common_headers(None);
        headers.push((SET_COOKIE, cookie));
        Self {
            status: StatusCode::NO_CONTENT,
            headers,
            body: Bytes::new(),
        }
    }

    pub(crate) fn bootstrap<T: Serialize>(
        value: &T,
        cookie: HeaderValue,
    ) -> Result<Self, ApiError> {
        let mut response = Self::json(StatusCode::CREATED, value)?;
        response.headers.push((SET_COOKIE, cookie));
        Ok(response)
    }

    pub(crate) fn diff(value: &str) -> Self {
        let mut headers = common_headers(Some("text/plain; charset=utf-8"));
        headers.push((
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        ));
        headers.push((CONTENT_DISPOSITION, HeaderValue::from_static("inline")));
        Self {
            status: StatusCode::OK,
            headers,
            body: Bytes::copy_from_slice(value.as_bytes()),
        }
    }

    pub(crate) fn storage_bytes(&self) -> usize {
        self.headers.iter().fold(
            64usize.saturating_add(self.body.len()),
            |total, (name, value)| {
                total
                    .saturating_add(name.as_str().len())
                    .saturating_add(value.as_bytes().len())
            },
        )
    }

    pub(crate) fn into_response(self) -> Response<Body> {
        let mut response = Response::new(Body::from(self.body));
        *response.status_mut() = self.status;
        for (name, value) in self.headers {
            response.headers_mut().append(name, value);
        }
        response
    }
}

impl std::fmt::Debug for CachedResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CachedResponse")
            .field("status", &self.status)
            .field("header_count", &self.headers.len())
            .field("body", &"[REDACTED]")
            .finish()
    }
}

fn common_headers(content_type: Option<&'static str>) -> Vec<(HeaderName, HeaderValue)> {
    let mut headers = vec![
        (CACHE_CONTROL, HeaderValue::from_static("no-store")),
        (NOSNIFF, HeaderValue::from_static("nosniff")),
    ];
    if let Some(content_type) = content_type {
        headers.push((CONTENT_TYPE, HeaderValue::from_static(content_type)));
    }
    headers
}

pub(crate) fn add_common_security_headers(headers: &mut HeaderMap) {
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(NOSNIFF, HeaderValue::from_static("nosniff"));
}
