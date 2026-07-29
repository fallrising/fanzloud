use std::fmt;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;
use url::{Origin, Url};

const MIN_BOOTSTRAP_BYTES: usize = 32;
const MAX_BOOTSTRAP_BYTES: usize = 128;
const MIN_SESSION_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_SESSION_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
const DEFAULT_SESSION_LIFETIME: Duration = MAX_SESSION_LIFETIME;
const MAX_ORIGIN_BYTES: usize = 256;

/// Administrator bootstrap secret exchanged for one private application session.
///
/// Contract: `CU-API-P0-01`. Debug and display never reveal the secret.
#[derive(Clone)]
pub struct OperatorBootstrapToken {
    bytes: [u8; MAX_BOOTSTRAP_BYTES],
    len: u8,
}

impl OperatorBootstrapToken {
    /// Validates one opaque non-provider bootstrap value.
    pub fn try_new(value: impl AsRef<[u8]>) -> Result<Self, P0HttpConfigError> {
        let value = value.as_ref();
        if !(MIN_BOOTSTRAP_BYTES..=MAX_BOOTSTRAP_BYTES).contains(&value.len())
            || value.iter().any(u8::is_ascii_control)
        {
            return Err(P0HttpConfigError::new(P0HttpConfigField::BootstrapToken));
        }
        let mut bytes = [0; MAX_BOOTSTRAP_BYTES];
        bytes[..value.len()].copy_from_slice(value);
        Ok(Self {
            bytes,
            len: value.len() as u8,
        })
    }

    pub(crate) fn matches(&self, candidate: Option<&[u8]>) -> bool {
        self.compare(candidate).0
    }

    fn compare(&self, candidate: Option<&[u8]>) -> (bool, usize) {
        let candidate = candidate.unwrap_or_default();
        let mut padded = [0; MAX_BOOTSTRAP_BYTES];
        let copied = candidate.len().min(MAX_BOOTSTRAP_BYTES);
        padded[..copied].copy_from_slice(&candidate[..copied]);

        let mut difference = (candidate.len() ^ usize::from(self.len)) as u64;
        let mut compared = 0usize;
        for (expected, actual) in self.bytes.iter().zip(padded) {
            difference |= u64::from(*expected ^ actual);
            compared += 1;
        }
        (difference == 0, compared)
    }

    #[cfg(test)]
    pub(crate) fn comparison_work(&self, candidate: Option<&[u8]>) -> usize {
        self.compare(candidate).1
    }
}

impl fmt::Debug for OperatorBootstrapToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OperatorBootstrapToken([REDACTED])")
    }
}

/// Canonical HTTPS origin configured by the administrator.
#[derive(Clone, Eq, PartialEq)]
pub struct P0PublicOrigin(String);

impl P0PublicOrigin {
    /// Parses and canonicalizes one exact private HTTPS origin.
    pub fn try_new(value: &str) -> Result<Self, P0HttpConfigError> {
        if value.len() > MAX_ORIGIN_BYTES || !value.starts_with("https://") {
            return Err(P0HttpConfigError::new(P0HttpConfigField::PublicOrigin));
        }
        let parsed = Url::parse(value)
            .map_err(|_| P0HttpConfigError::new(P0HttpConfigField::PublicOrigin))?;
        if parsed.scheme() != "https"
            || parsed.host().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(P0HttpConfigError::new(P0HttpConfigField::PublicOrigin));
        }
        let Origin::Tuple(..) = parsed.origin() else {
            return Err(P0HttpConfigError::new(P0HttpConfigField::PublicOrigin));
        };
        Ok(Self(parsed.origin().ascii_serialization()))
    }

    /// Returns the exact canonical serialization required in `Origin`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn matches_header(&self, value: Option<&[u8]>) -> bool {
        let Some(value) = value else {
            return false;
        };
        let value = trim_optional_whitespace(value);
        value == self.0.as_bytes()
    }

    pub(crate) fn websocket_origin(&self) -> String {
        self.0.replacen("https://", "wss://", 1)
    }
}

impl fmt::Debug for P0PublicOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("P0PublicOrigin")
            .field(&self.0)
            .finish()
    }
}

fn trim_optional_whitespace(mut value: &[u8]) -> &[u8] {
    while value
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        value = &value[1..];
    }
    while value
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        value = &value[..value.len() - 1];
    }
    value
}

/// Administrator HTTP boundary configuration.
#[derive(Clone, Debug)]
pub struct P0HttpConfig {
    pub(crate) public_origin: P0PublicOrigin,
    pub(crate) bootstrap: OperatorBootstrapToken,
    pub(crate) session_lifetime: Duration,
}

impl P0HttpConfig {
    /// Uses the accepted twelve-hour default application-session lifetime.
    pub fn new(public_origin: P0PublicOrigin, bootstrap: OperatorBootstrapToken) -> Self {
        Self {
            public_origin,
            bootstrap,
            session_lifetime: DEFAULT_SESSION_LIFETIME,
        }
    }

    /// Selects one whole-second lifetime from five minutes through twelve hours.
    pub fn try_with_session_lifetime(
        mut self,
        lifetime: Duration,
    ) -> Result<Self, P0HttpConfigError> {
        if !(MIN_SESSION_LIFETIME..=MAX_SESSION_LIFETIME).contains(&lifetime)
            || lifetime.subsec_nanos() != 0
        {
            return Err(P0HttpConfigError::new(P0HttpConfigField::SessionLifetime));
        }
        self.session_lifetime = lifetime;
        Ok(self)
    }
}

/// Safe configuration field attached to a validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum P0HttpConfigField {
    PublicOrigin,
    BootstrapToken,
    SessionLifetime,
}

/// Redacted administrator-configuration failure.
#[derive(Clone, Error, PartialEq)]
#[error("P0 HTTP configuration is invalid")]
pub struct P0HttpConfigError {
    field: P0HttpConfigField,
}

impl P0HttpConfigError {
    const fn new(field: P0HttpConfigField) -> Self {
        Self { field }
    }

    /// Returns only the safe field classification.
    pub const fn field(&self) -> P0HttpConfigField {
        self.field
    }
}

impl fmt::Debug for P0HttpConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("P0HttpConfigError")
            .field("field", &self.field)
            .finish()
    }
}
