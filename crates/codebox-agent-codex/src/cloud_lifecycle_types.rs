use std::fmt;

use thiserror::Error;

use crate::{CloudSubmitOperationId, CloudTaskId};

/// Durable local projection of one provider-specific Codex Cloud operation.
///
/// Contract: `CU-AGT-P0-02`. A local cancellation never claims provider termination.
#[derive(Clone, Eq, PartialEq)]
pub enum CloudLifecycle {
    Submitting {
        operation_id: CloudSubmitOperationId,
    },
    FailedBeforeSubmit {
        operation_id: CloudSubmitOperationId,
    },
    OutcomeUnknown {
        operation_id: CloudSubmitOperationId,
    },
    Pending {
        operation_id: CloudSubmitOperationId,
        task_id: CloudTaskId,
    },
    Ready {
        operation_id: CloudSubmitOperationId,
        task_id: CloudTaskId,
    },
    Applied {
        operation_id: CloudSubmitOperationId,
        task_id: CloudTaskId,
    },
    ProviderError {
        operation_id: CloudSubmitOperationId,
        task_id: CloudTaskId,
    },
    CanceledLocally {
        operation_id: CloudSubmitOperationId,
        task_id: Option<CloudTaskId>,
        provider_may_continue: bool,
    },
    AbandonedUnknown {
        operation_id: CloudSubmitOperationId,
    },
}

impl CloudLifecycle {
    /// Returns the durable operation identity.
    ///
    /// Contract: `CU-AGT-P0-02`.
    pub const fn operation_id(&self) -> CloudSubmitOperationId {
        match self {
            Self::Submitting { operation_id }
            | Self::FailedBeforeSubmit { operation_id }
            | Self::OutcomeUnknown { operation_id }
            | Self::Pending { operation_id, .. }
            | Self::Ready { operation_id, .. }
            | Self::Applied { operation_id, .. }
            | Self::ProviderError { operation_id, .. }
            | Self::CanceledLocally { operation_id, .. }
            | Self::AbandonedUnknown { operation_id } => *operation_id,
        }
    }

    /// Returns the provider task when one is durably known.
    ///
    /// Contract: `CU-AGT-P0-02`.
    pub const fn task_id(&self) -> Option<&CloudTaskId> {
        match self {
            Self::Pending { task_id, .. }
            | Self::Ready { task_id, .. }
            | Self::Applied { task_id, .. }
            | Self::ProviderError { task_id, .. } => Some(task_id),
            Self::CanceledLocally { task_id, .. } => task_id.as_ref(),
            Self::Submitting { .. }
            | Self::FailedBeforeSubmit { .. }
            | Self::OutcomeUnknown { .. }
            | Self::AbandonedUnknown { .. } => None,
        }
    }
}

impl fmt::Debug for CloudLifecycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Submitting { operation_id } => formatter
                .debug_struct("Submitting")
                .field("operation_id", operation_id)
                .finish(),
            Self::FailedBeforeSubmit { operation_id } => formatter
                .debug_struct("FailedBeforeSubmit")
                .field("operation_id", operation_id)
                .finish(),
            Self::OutcomeUnknown { operation_id } => formatter
                .debug_struct("OutcomeUnknown")
                .field("operation_id", operation_id)
                .finish(),
            Self::Pending {
                operation_id,
                task_id,
            } => formatter
                .debug_struct("Pending")
                .field("operation_id", operation_id)
                .field("task_id", task_id)
                .finish(),
            Self::Ready {
                operation_id,
                task_id,
            } => formatter
                .debug_struct("Ready")
                .field("operation_id", operation_id)
                .field("task_id", task_id)
                .finish(),
            Self::Applied {
                operation_id,
                task_id,
            } => formatter
                .debug_struct("Applied")
                .field("operation_id", operation_id)
                .field("task_id", task_id)
                .finish(),
            Self::ProviderError {
                operation_id,
                task_id,
            } => formatter
                .debug_struct("ProviderError")
                .field("operation_id", operation_id)
                .field("task_id", task_id)
                .finish(),
            Self::CanceledLocally {
                operation_id,
                task_id,
                provider_may_continue,
            } => formatter
                .debug_struct("CanceledLocally")
                .field("operation_id", operation_id)
                .field("task_id", task_id)
                .field("provider_may_continue", provider_may_continue)
                .finish(),
            Self::AbandonedUnknown { operation_id } => formatter
                .debug_struct("AbandonedUnknown")
                .field("operation_id", operation_id)
                .finish(),
        }
    }
}

/// Explicit operator authority acknowledging duplicate-provider-task risk.
///
/// Contract: `CU-AGT-P0-02`. T005 creates this only after authenticated confirmation.
#[derive(Clone, Eq, PartialEq)]
pub struct DuplicateRiskAcknowledgement {
    operation_id: CloudSubmitOperationId,
}

impl DuplicateRiskAcknowledgement {
    /// Binds one acknowledgement to the exact unknown submit operation.
    ///
    /// Contract: `CU-AGT-P0-02`.
    pub const fn for_operation(operation_id: CloudSubmitOperationId) -> Self {
        Self { operation_id }
    }

    pub(crate) const fn operation_id(&self) -> CloudSubmitOperationId {
        self.operation_id
    }
}

impl fmt::Debug for DuplicateRiskAcknowledgement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DuplicateRiskAcknowledgement")
            .field("operation_id", &self.operation_id)
            .finish()
    }
}

/// Explicit resolution of one unknown Cloud submit.
///
/// Contract: `CU-AGT-P0-02`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnknownSubmitDecision {
    AdoptListedTask(CloudTaskId),
    AbandonAfterReconciliation(DuplicateRiskAcknowledgement),
}

/// Redacted lifecycle failure class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudLifecycleErrorCategory {
    Scope,
    Busy,
    TurnAlreadyRunning,
    NoCurrentOperation,
    WrongState,
    StaleDecision,
    TaskNotListed,
    AcknowledgementRequired,
    LowerRunner,
    ProviderRead,
    OperationConflict,
    OutcomeUnknown,
    LedgerInvalid,
    LedgerUnavailable,
    RecoveryRequired,
}

impl fmt::Display for CloudLifecycleErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Scope => "credential scope is unavailable",
            Self::Busy => "credential scope is busy",
            Self::TurnAlreadyRunning => "a Cloud turn is already running",
            Self::NoCurrentOperation => "no Cloud operation is recorded",
            Self::WrongState => "Cloud operation is in the wrong lifecycle state",
            Self::StaleDecision => "Cloud recovery decision is stale",
            Self::TaskNotListed => "Cloud task is not in the latest complete reconciliation",
            Self::AcknowledgementRequired => "duplicate-risk acknowledgement is required",
            Self::LowerRunner => "trusted Cloud runner failed",
            Self::ProviderRead => "Cloud provider status could not be inspected",
            Self::OperationConflict => "another Cloud operation owns durable state",
            Self::OutcomeUnknown => "Cloud submit outcome requires explicit recovery",
            Self::LedgerInvalid => "Cloud lifecycle ledger is invalid",
            Self::LedgerUnavailable => "Cloud lifecycle ledger is unavailable",
            Self::RecoveryRequired => "Cloud lifecycle recovery requires operator action",
        })
    }
}

/// Typed, redacted Cloud lifecycle error.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("Codex Cloud lifecycle: {category}")]
pub struct CloudLifecycleError {
    category: CloudLifecycleErrorCategory,
    operation_id: Option<CloudSubmitOperationId>,
}

impl CloudLifecycleError {
    pub(crate) const fn new(category: CloudLifecycleErrorCategory) -> Self {
        Self {
            category,
            operation_id: None,
        }
    }

    pub(crate) const fn for_operation(
        category: CloudLifecycleErrorCategory,
        operation_id: CloudSubmitOperationId,
    ) -> Self {
        Self {
            category,
            operation_id: Some(operation_id),
        }
    }

    /// Returns the safe failure classification.
    ///
    /// Contract: `CU-AGT-P0-02`.
    pub const fn category(&self) -> CloudLifecycleErrorCategory {
        self.category
    }

    /// Returns the affected operation when one is durably known.
    ///
    /// Contract: `CU-AGT-P0-02`.
    pub const fn operation_id(&self) -> Option<CloudSubmitOperationId> {
        self.operation_id
    }
}
