use std::fmt;
use std::sync::Arc;

use thiserror::Error;

use crate::cloud_lifecycle::LifecycleCloudRunner;
use crate::{CloudCancellation, CloudDiff, CloudSubmitOperationId, CloudTaskId};

/// Opaque authority for retrieving the current accepted provider task diff.
///
/// Contract: `CU-CLOUD-P0-02`. Only `CloudTaskOrchestrator` can mint this value, and retrieval
/// revalidates its private operation and task identities against the durable lifecycle and submit
/// records. It grants no patch parsing or application authority.
pub struct DiffEligibleCloudTask {
    operation_id: CloudSubmitOperationId,
    task_id: CloudTaskId,
}

impl DiffEligibleCloudTask {
    pub(crate) const fn new(operation_id: CloudSubmitOperationId, task_id: CloudTaskId) -> Self {
        Self {
            operation_id,
            task_id,
        }
    }

    pub(crate) const fn operation_id(&self) -> CloudSubmitOperationId {
        self.operation_id
    }

    pub(crate) const fn task_id(&self) -> &CloudTaskId {
        &self.task_id
    }
}

impl fmt::Debug for DiffEligibleCloudTask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiffEligibleCloudTask")
            .finish_non_exhaustive()
    }
}

/// Provider-specific reader bound to the same trusted runner as its orchestrator.
///
/// Contract: `CU-CLOUD-P0-02`. This type has no public constructor from paths, configuration, a
/// process builder, or a raw task ID.
pub struct CloudDiffReader {
    runner: Arc<dyn LifecycleCloudRunner>,
}

impl CloudDiffReader {
    pub(crate) fn from_runner(runner: Arc<dyn LifecycleCloudRunner>) -> Self {
        Self { runner }
    }

    /// Retrieves one bounded, untrusted diff without applying or publishing it.
    ///
    /// Contract: `CU-CLOUD-P0-02`. Eligibility is revalidated under the held trusted scope lease,
    /// and every started process is terminated and reaped before this method returns.
    pub fn retrieve(
        &self,
        task: &DiffEligibleCloudTask,
        cancellation: CloudCancellation,
    ) -> Result<CloudDiff, CloudDiffReadError> {
        if cancellation.is_cancelled() {
            return Err(CloudDiffReadError::new(
                CloudDiffReadErrorCategory::Canceled,
            ));
        }
        self.runner.retrieve_diff(task, cancellation)
    }
}

impl fmt::Debug for CloudDiffReader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CloudDiffReader")
            .finish_non_exhaustive()
    }
}

/// Redacted failure class for one provider-managed diff read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloudDiffReadErrorCategory {
    IneligibleLifecycle,
    AuthorityMismatch,
    Scope,
    Busy,
    Version,
    DiagnosticBoundary,
    Process,
    Timeout,
    Canceled,
    OutputLimit,
    ProviderDrift,
    InvalidDiff,
}

impl fmt::Display for CloudDiffReadErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IneligibleLifecycle => "Cloud task is not eligible for diff retrieval",
            Self::AuthorityMismatch => "Cloud task authority does not match durable state",
            Self::Scope => "credential scope is unavailable",
            Self::Busy => "credential scope is busy",
            Self::Version => "CLI version is not accepted",
            Self::DiagnosticBoundary => "diagnostic write boundary is unsafe",
            Self::Process => "Cloud diff process failed",
            Self::Timeout => "Cloud diff process timed out",
            Self::Canceled => "Cloud diff retrieval was canceled",
            Self::OutputLimit => "Cloud diff output exceeded its limit",
            Self::ProviderDrift => "Cloud diff output does not match the pinned provider contract",
            Self::InvalidDiff => "Cloud diff contains invalid display data",
        })
    }
}

/// Typed failure that exposes only a safe category.
///
/// Contract: `CU-CLOUD-P0-02`. It never retains task identities, provider output, diff bytes,
/// configured paths, account data, or credential material.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("Codex Cloud diff reader: {category}")]
pub struct CloudDiffReadError {
    category: CloudDiffReadErrorCategory,
}

impl CloudDiffReadError {
    pub(crate) const fn new(category: CloudDiffReadErrorCategory) -> Self {
        Self { category }
    }

    /// Returns the safe failure classification.
    ///
    /// Contract: `CU-CLOUD-P0-02`.
    pub const fn category(&self) -> CloudDiffReadErrorCategory {
        self.category
    }
}
