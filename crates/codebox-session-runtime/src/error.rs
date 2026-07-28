use std::fmt;

use codebox_agent_codex::{
    CloudDiffReadErrorCategory, CloudLifecycleErrorCategory, CloudSubmitOperationId,
};
use codebox_domain::{EventSeq, TurnId};
use thiserror::Error;

use crate::P0SessionConfigField;

/// Stable redacted P0 session failure class.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum P0SessionErrorCategory {
    InvalidConfig,
    TurnAlreadyRunning,
    NoCurrentTurn,
    WrongState,
    WrongSession,
    WrongOperation,
    RuntimeStopped,
    CloudLifecycle,
    CloudDiff,
    HistoryGap,
    FutureCursor,
    SubscriberLimit,
    SequenceExhausted,
    LowerConflict,
}

impl fmt::Display for P0SessionErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfig => "P0 session configuration is invalid",
            Self::TurnAlreadyRunning => "a P0 turn is already active",
            Self::NoCurrentTurn => "no P0 turn is recorded",
            Self::WrongState => "the P0 session is in the wrong state",
            Self::WrongSession => "the P0 session identifier does not match",
            Self::WrongOperation => "the Cloud operation identifier does not match",
            Self::RuntimeStopped => "the P0 session runtime is stopped",
            Self::CloudLifecycle => "the Cloud lifecycle operation failed",
            Self::CloudDiff => "the Cloud diff read failed",
            Self::HistoryGap => "the requested P0 event history was evicted",
            Self::FutureCursor => "the requested P0 event cursor is in the future",
            Self::SubscriberLimit => "the P0 subscriber limit was reached",
            Self::SequenceExhausted => "the P0 event sequence is exhausted",
            Self::LowerConflict => "the Cloud lifecycle moved incompatibly",
        })
    }
}

/// Typed redacted failure for the process-lifetime P0 session.
///
/// Contract: `CU-SES-P0-01`. It retains only safe categories, strong correlation IDs, and event
/// bounds. It never stores a prompt, diff, raw provider output, credential, or internal path.
#[derive(Clone, Error, PartialEq)]
#[error("P0 session: {category}")]
pub struct P0SessionError {
    category: P0SessionErrorCategory,
    turn_id: Option<TurnId>,
    operation_id: Option<CloudSubmitOperationId>,
    oldest_available: Option<EventSeq>,
    latest_available: Option<EventSeq>,
    cloud_lifecycle: Option<CloudLifecycleErrorCategory>,
    cloud_diff: Option<CloudDiffReadErrorCategory>,
    config_field: Option<P0SessionConfigField>,
}

impl P0SessionError {
    pub(crate) const fn new(category: P0SessionErrorCategory) -> Self {
        Self {
            category,
            turn_id: None,
            operation_id: None,
            oldest_available: None,
            latest_available: None,
            cloud_lifecycle: None,
            cloud_diff: None,
            config_field: None,
        }
    }

    pub(crate) const fn invalid_config(field: P0SessionConfigField) -> Self {
        let mut error = Self::new(P0SessionErrorCategory::InvalidConfig);
        error.config_field = Some(field);
        error
    }

    pub(crate) const fn for_turn(category: P0SessionErrorCategory, turn_id: TurnId) -> Self {
        let mut error = Self::new(category);
        error.turn_id = Some(turn_id);
        error
    }

    pub(crate) const fn for_operation(
        category: P0SessionErrorCategory,
        operation_id: CloudSubmitOperationId,
    ) -> Self {
        let mut error = Self::new(category);
        error.operation_id = Some(operation_id);
        error
    }

    pub(crate) const fn history_gap(oldest: EventSeq, latest: EventSeq) -> Self {
        let mut error = Self::new(P0SessionErrorCategory::HistoryGap);
        error.oldest_available = Some(oldest);
        error.latest_available = Some(latest);
        error
    }

    pub(crate) const fn future_cursor(latest: EventSeq) -> Self {
        let mut error = Self::new(P0SessionErrorCategory::FutureCursor);
        error.latest_available = Some(latest);
        error
    }

    pub(crate) const fn from_cloud_lifecycle(
        category: CloudLifecycleErrorCategory,
        operation_id: Option<CloudSubmitOperationId>,
    ) -> Self {
        let mut mapped = Self::new(P0SessionErrorCategory::CloudLifecycle);
        mapped.operation_id = operation_id;
        mapped.cloud_lifecycle = Some(category);
        mapped
    }

    pub(crate) const fn from_cloud_diff(category: CloudDiffReadErrorCategory) -> Self {
        let mut mapped = Self::new(P0SessionErrorCategory::CloudDiff);
        mapped.cloud_diff = Some(category);
        mapped
    }

    /// Returns the stable safe failure class.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn category(&self) -> P0SessionErrorCategory {
        self.category
    }

    /// Returns the affected local turn when safe and known.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn turn_id(&self) -> Option<TurnId> {
        self.turn_id
    }

    /// Returns the affected lower operation when safe and known.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn operation_id(&self) -> Option<CloudSubmitOperationId> {
        self.operation_id
    }

    /// Returns the oldest retained event sequence for a history gap.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn oldest_available(&self) -> Option<EventSeq> {
        self.oldest_available
    }

    /// Returns the current event high-water sequence when relevant.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn latest_available(&self) -> Option<EventSeq> {
        self.latest_available
    }

    /// Returns an accepted lower lifecycle failure category without its source text.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn cloud_lifecycle(&self) -> Option<CloudLifecycleErrorCategory> {
        self.cloud_lifecycle
    }

    /// Returns an accepted lower diff failure category without its source text.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn cloud_diff(&self) -> Option<CloudDiffReadErrorCategory> {
        self.cloud_diff
    }

    /// Returns the safe invalid configuration field without its rejected value.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub const fn config_field(&self) -> Option<P0SessionConfigField> {
        self.config_field
    }
}

impl fmt::Debug for P0SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("P0SessionError")
            .field("category", &self.category)
            .field("turn_id", &self.turn_id)
            .field("operation_id", &self.operation_id)
            .field("oldest_available", &self.oldest_available)
            .field("latest_available", &self.latest_available)
            .field("cloud_lifecycle", &self.cloud_lifecycle)
            .field("cloud_diff", &self.cloud_diff)
            .field("config_field", &self.config_field)
            .finish()
    }
}

/// Safe receive disposition for one live P0 subscription.
///
/// Contract: `CU-SES-P0-01`.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum P0LiveReceiveError {
    #[error("no P0 event is currently available")]
    Empty,
    #[error("the P0 subscriber fell behind")]
    Lagged,
    #[error("the P0 session runtime stopped")]
    RuntimeStopped,
    #[error("the P0 subscription closed")]
    Closed,
}
