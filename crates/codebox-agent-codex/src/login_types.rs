use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// A non-nil identifier for one device-login attempt.
///
/// Contract: `CU-AUTH-P0-01`.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LoginOperationId(Uuid);

impl LoginOperationId {
    /// Creates a fresh non-nil operation identifier.
    ///
    /// Contract: `CU-AUTH-P0-01`.
    pub fn new() -> Self {
        loop {
            let value = Uuid::new_v4();
            if !value.is_nil() {
                return Self(value);
            }
        }
    }

    /// Returns the underlying UUID without changing it.
    ///
    /// Contract: `CU-AUTH-P0-01`.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }

    fn try_from_uuid(value: Uuid) -> Result<Self, &'static str> {
        if value.is_nil() {
            Err("login operation ID cannot be nil")
        } else {
            Ok(Self(value))
        }
    }
}

impl Default for LoginOperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for LoginOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for LoginOperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for LoginOperationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LoginOperationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Uuid::deserialize(deserializer)?;
        Self::try_from_uuid(value).map_err(serde::de::Error::custom)
    }
}

/// The only verification origin accepted by the pinned CLI fixture.
///
/// Contract: `CU-AUTH-P0-01`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerificationUrl;

impl VerificationUrl {
    /// Returns the pinned OpenAI device-verification URL.
    ///
    /// Contract: `CU-AUTH-P0-01`.
    pub const fn as_str(self) -> &'static str {
        "https://auth.openai.com/codex/device"
    }
}

/// A short-lived provider verification code intentionally projected to the private operator.
///
/// Contract: `CU-AUTH-P0-01`. Debug formatting is always redacted, and this value is never
/// serialized into the login ledger.
#[derive(Clone, Eq, PartialEq)]
pub struct VerificationCode(String);

impl VerificationCode {
    pub(crate) fn from_validated(value: String) -> Self {
        Self(value)
    }

    /// Reveals the code to the private operator response boundary.
    ///
    /// Contract: `CU-AUTH-P0-01`. Callers must not log, persist, or place it in an artifact.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for VerificationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerificationCode([REDACTED])")
    }
}

/// Bounded device-verification instructions for one active login.
///
/// Contract: `CU-AUTH-P0-01`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginInteraction {
    operation_id: LoginOperationId,
    verification_url: VerificationUrl,
    verification_code: VerificationCode,
    expires_in_seconds: u16,
}

impl LoginInteraction {
    pub(crate) fn new(operation_id: LoginOperationId, verification_code: VerificationCode) -> Self {
        Self {
            operation_id,
            verification_url: VerificationUrl,
            verification_code,
            expires_in_seconds: 900,
        }
    }

    /// Returns the durable operation identifier.
    ///
    /// Contract: `CU-AUTH-P0-01`.
    pub const fn operation_id(&self) -> LoginOperationId {
        self.operation_id
    }

    /// Returns the exact pinned verification origin.
    ///
    /// Contract: `CU-AUTH-P0-01`.
    pub const fn verification_url(&self) -> VerificationUrl {
        self.verification_url
    }

    /// Returns the redacting verification-code value.
    ///
    /// Contract: `CU-AUTH-P0-01`.
    pub fn verification_code(&self) -> &VerificationCode {
        &self.verification_code
    }

    /// Returns the pinned 15-minute provider expiry.
    ///
    /// Contract: `CU-AUTH-P0-01`.
    pub const fn expires_in_seconds(&self) -> u16 {
        self.expires_in_seconds
    }
}

/// A normalized login-state projection.
///
/// Contract: `CU-AUTH-P0-01`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoginStatus {
    /// The pinned status fixture proved no active authentication.
    LoggedOut,
    /// A supervised device-login child remains active.
    DeviceLoginPending { operation_id: LoginOperationId },
    /// The pinned status fixture proved ChatGPT authentication.
    LoggedIn,
    /// A prior operation must be reconciled before retry.
    OutcomeUnknown { operation_id: LoginOperationId },
}
