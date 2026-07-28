use std::fmt;

use axum::http::StatusCode;
use codebox_agent_codex::{
    CloudDiffReadErrorCategory, CloudLifecycleErrorCategory, CloudSubmitOperationId,
    LoginBrokerError,
};
use codebox_session_runtime::{P0SessionError, P0SessionErrorCategory};
use serde::Serialize;
use thiserror::Error;

use crate::ports::{LoginPortError, SessionPortError};

#[derive(Clone, Serialize)]
pub(crate) struct ApiError {
    #[serde(skip)]
    pub(crate) status: StatusCode,
    pub(crate) error: ApiErrorBody,
}

#[derive(Clone, Serialize)]
pub(crate) struct ApiErrorBody {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) operation_id: Option<CloudSubmitOperationId>,
}

impl ApiError {
    pub(crate) const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            error: ApiErrorBody {
                code,
                message,
                operation_id: None,
            },
        }
    }

    const fn with_operation(mut self, operation_id: Option<CloudSubmitOperationId>) -> Self {
        self.error.operation_id = operation_id;
        self
    }

    pub(crate) const fn authentication_required() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "operator authentication is required",
        )
    }

    pub(crate) const fn origin_forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "origin_forbidden",
            "request origin is not allowed",
        )
    }

    pub(crate) const fn service_unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "control-plane service is unavailable",
        )
    }

    pub(crate) const fn invalid_request() -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "request schema is invalid",
        )
    }

    pub(crate) const fn invalid_value() -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_value",
            "request value is invalid",
        )
    }
}

impl fmt::Debug for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiError")
            .field("status", &self.status)
            .field("code", &self.error.code)
            .field("operation_id", &self.error.operation_id)
            .finish()
    }
}

pub(crate) fn map_login_error(error: LoginPortError) -> ApiError {
    let LoginPortError::Lower(error) = error else {
        return ApiError::service_unavailable();
    };
    match error {
        LoginBrokerError::CredentialScope(_) => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_unavailable",
            "login credential scope is unavailable",
        ),
        LoginBrokerError::VersionMismatch => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_version_mismatch",
            "accepted login provider version is unavailable",
        ),
        LoginBrokerError::LoginAlreadyRunning => ApiError::new(
            StatusCode::CONFLICT,
            "login_already_running",
            "a device login is already running",
        ),
        LoginBrokerError::AlreadyLoggedIn => ApiError::new(
            StatusCode::CONFLICT,
            "already_logged_in",
            "operator is already logged in",
        ),
        LoginBrokerError::ProviderOutputInvalid => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_provider_drift",
            "login provider response is unavailable",
        ),
        LoginBrokerError::OutputLimitExceeded => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_output_limit",
            "login provider response exceeded its limit",
        ),
        LoginBrokerError::StatusUnavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_status_unavailable",
            "login status is unavailable",
        ),
        LoginBrokerError::LoginFailed => ApiError::new(
            StatusCode::CONFLICT,
            "login_failed",
            "device login did not complete",
        ),
        LoginBrokerError::OutcomeUnknown => ApiError::new(
            StatusCode::CONFLICT,
            "login_outcome_unknown",
            "device login outcome requires reconciliation",
        ),
        LoginBrokerError::Process { .. } => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_process_unavailable",
            "login process is unavailable",
        ),
        LoginBrokerError::LedgerUnavailable { .. } => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_state_unavailable",
            "login state is unavailable",
        ),
        LoginBrokerError::LedgerInvalid => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_state_invalid",
            "login state requires operator repair",
        ),
    }
}

pub(crate) fn map_session_error(error: SessionPortError) -> ApiError {
    match error {
        SessionPortError::Lower(error) => map_p0_session_error(&error),
        #[cfg(test)]
        SessionPortError::Unavailable => ApiError::service_unavailable(),
        #[cfg(test)]
        SessionPortError::ProjectedLifecycle {
            category,
            operation_id,
        } => map_cloud_lifecycle_error(category, operation_id),
    }
}

pub(crate) fn map_p0_session_error(error: &P0SessionError) -> ApiError {
    match error.category() {
        P0SessionErrorCategory::CloudLifecycle => error
            .cloud_lifecycle()
            .map(|category| map_cloud_lifecycle_error(category, error.operation_id()))
            .unwrap_or_else(ApiError::service_unavailable),
        P0SessionErrorCategory::CloudDiff => error
            .cloud_diff()
            .map(map_cloud_diff_error)
            .unwrap_or_else(ApiError::service_unavailable),
        category => map_session_category(category).unwrap_or_else(ApiError::service_unavailable),
    }
}

pub(crate) const fn map_session_category(category: P0SessionErrorCategory) -> Option<ApiError> {
    let mapped = match category {
        P0SessionErrorCategory::InvalidConfig => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "session_config_invalid",
            "session configuration is unavailable",
        ),
        P0SessionErrorCategory::TurnAlreadyRunning => ApiError::new(
            StatusCode::CONFLICT,
            "turn_already_running",
            "a turn is already active",
        ),
        P0SessionErrorCategory::NoCurrentTurn => ApiError::new(
            StatusCode::CONFLICT,
            "no_current_turn",
            "no current turn is available",
        ),
        P0SessionErrorCategory::WrongState => ApiError::new(
            StatusCode::CONFLICT,
            "session_wrong_state",
            "session state does not allow this operation",
        ),
        P0SessionErrorCategory::WrongSession => ApiError::new(
            StatusCode::CONFLICT,
            "session_changed",
            "session identity changed; refresh before retry",
        ),
        P0SessionErrorCategory::WrongOperation => ApiError::new(
            StatusCode::CONFLICT,
            "operation_changed",
            "current operation changed; refresh before retry",
        ),
        P0SessionErrorCategory::RuntimeStopped => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "session_stopped",
            "session runtime is stopped",
        ),
        P0SessionErrorCategory::HistoryGap => ApiError::new(
            StatusCode::CONFLICT,
            "history_gap",
            "requested session history is no longer retained",
        ),
        P0SessionErrorCategory::FutureCursor => ApiError::new(
            StatusCode::CONFLICT,
            "future_cursor",
            "requested session cursor is in the future",
        ),
        P0SessionErrorCategory::SubscriberLimit => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "subscriber_limit",
            "session subscriber limit reached",
        ),
        P0SessionErrorCategory::SequenceExhausted => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "session_sequence_exhausted",
            "session event sequence is exhausted",
        ),
        P0SessionErrorCategory::LowerConflict => ApiError::new(
            StatusCode::CONFLICT,
            "provider_state_conflict",
            "provider state changed incompatibly",
        ),
        P0SessionErrorCategory::CloudLifecycle | P0SessionErrorCategory::CloudDiff => return None,
    };
    Some(mapped)
}

pub(crate) const fn map_cloud_lifecycle_error(
    category: CloudLifecycleErrorCategory,
    operation_id: Option<CloudSubmitOperationId>,
) -> ApiError {
    let mapped = match category {
        CloudLifecycleErrorCategory::Scope => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_scope_unavailable",
            "provider credential scope is unavailable",
        ),
        CloudLifecycleErrorCategory::Busy => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_busy",
            "provider operation is busy",
        ),
        CloudLifecycleErrorCategory::TurnAlreadyRunning => ApiError::new(
            StatusCode::CONFLICT,
            "provider_turn_running",
            "a provider turn is already active",
        ),
        CloudLifecycleErrorCategory::NoCurrentOperation => ApiError::new(
            StatusCode::CONFLICT,
            "no_current_operation",
            "no provider operation is available",
        ),
        CloudLifecycleErrorCategory::WrongState => ApiError::new(
            StatusCode::CONFLICT,
            "provider_wrong_state",
            "provider state does not allow this operation",
        ),
        CloudLifecycleErrorCategory::StaleDecision => ApiError::new(
            StatusCode::CONFLICT,
            "recovery_decision_stale",
            "recovery decision is stale",
        ),
        CloudLifecycleErrorCategory::TaskNotListed => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "task_not_listed",
            "task is not in the complete recovery set",
        ),
        CloudLifecycleErrorCategory::AcknowledgementRequired => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "acknowledgement_required",
            "duplicate-task-risk acknowledgement is required",
        ),
        CloudLifecycleErrorCategory::LowerRunner => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_runner_unavailable",
            "provider runner is unavailable",
        ),
        CloudLifecycleErrorCategory::ProviderRead => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_read_unavailable",
            "provider state cannot be read",
        ),
        CloudLifecycleErrorCategory::OperationConflict => ApiError::new(
            StatusCode::CONFLICT,
            "provider_operation_conflict",
            "another provider operation owns current state",
        ),
        CloudLifecycleErrorCategory::OutcomeUnknown => ApiError::new(
            StatusCode::CONFLICT,
            "provider_outcome_unknown",
            "provider outcome requires explicit recovery",
        ),
        CloudLifecycleErrorCategory::LedgerInvalid => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_state_invalid",
            "provider state requires operator repair",
        ),
        CloudLifecycleErrorCategory::LedgerUnavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_state_unavailable",
            "provider state is unavailable",
        ),
        CloudLifecycleErrorCategory::RecoveryRequired => ApiError::new(
            StatusCode::CONFLICT,
            "provider_recovery_required",
            "provider recovery requires operator action",
        ),
    };
    mapped.with_operation(operation_id)
}

pub(crate) const fn map_cloud_diff_error(category: CloudDiffReadErrorCategory) -> ApiError {
    match category {
        CloudDiffReadErrorCategory::IneligibleLifecycle => ApiError::new(
            StatusCode::CONFLICT,
            "diff_not_ready",
            "current task is not eligible for diff retrieval",
        ),
        CloudDiffReadErrorCategory::AuthorityMismatch => ApiError::new(
            StatusCode::CONFLICT,
            "diff_authority_changed",
            "current task changed; refresh before retry",
        ),
        CloudDiffReadErrorCategory::Scope => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_scope_unavailable",
            "diff credential scope is unavailable",
        ),
        CloudDiffReadErrorCategory::Busy => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_busy",
            "diff provider operation is busy",
        ),
        CloudDiffReadErrorCategory::Version => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_version_mismatch",
            "accepted diff provider version is unavailable",
        ),
        CloudDiffReadErrorCategory::DiagnosticBoundary => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_boundary_unavailable",
            "diff diagnostic boundary is unavailable",
        ),
        CloudDiffReadErrorCategory::Process => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_process_unavailable",
            "diff process is unavailable",
        ),
        CloudDiffReadErrorCategory::Timeout => ApiError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "diff_timeout",
            "diff retrieval timed out",
        ),
        CloudDiffReadErrorCategory::Canceled => ApiError::new(
            StatusCode::CONFLICT,
            "diff_canceled",
            "diff retrieval was canceled",
        ),
        CloudDiffReadErrorCategory::OutputLimit => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_output_limit",
            "diff exceeded its output limit",
        ),
        CloudDiffReadErrorCategory::ProviderDrift => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_provider_drift",
            "diff provider response is unavailable",
        ),
        CloudDiffReadErrorCategory::InvalidDiff => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "diff_invalid",
            "diff display data is invalid",
        ),
    }
}

/// Safe shutdown failure classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum P0HttpShutdownErrorCategory {
    Session,
    Login,
    Worker,
}

/// Redacted idempotent control-plane shutdown failure.
#[derive(Clone, Error, PartialEq)]
#[error("P0 control-plane shutdown failed")]
pub struct P0HttpShutdownError {
    category: P0HttpShutdownErrorCategory,
}

impl P0HttpShutdownError {
    pub(crate) const fn new(category: P0HttpShutdownErrorCategory) -> Self {
        Self { category }
    }

    /// Returns the safe failing cleanup stage.
    pub const fn category(&self) -> P0HttpShutdownErrorCategory {
        self.category
    }
}

impl fmt::Debug for P0HttpShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("P0HttpShutdownError")
            .field("category", &self.category)
            .finish()
    }
}
