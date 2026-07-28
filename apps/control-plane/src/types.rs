use codebox_agent_codex::{
    CloudPrompt, CloudSubmitOperationId, CloudTaskId, LoginOperationId, LoginStatus,
};
use codebox_domain::CommandId;
use codebox_session_runtime::P0SessionIdentity;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct BootstrapResponse {
    pub(crate) actor: &'static str,
    pub(crate) expires_in_seconds: u64,
    pub(crate) p0_session_id: codebox_domain::SessionId,
    pub(crate) instance_id: codebox_session_runtime::P0InstanceId,
}

impl BootstrapResponse {
    pub(crate) fn new(identity: P0SessionIdentity, expires_in_seconds: u64) -> Self {
        Self {
            actor: "operator",
            expires_in_seconds,
            p0_session_id: identity.session_id,
            instance_id: identity.instance_id,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum LoginStatusResponse {
    LoggedOut,
    DeviceLoginPending { operation_id: LoginOperationId },
    LoggedIn,
    OutcomeUnknown { operation_id: LoginOperationId },
}

impl From<LoginStatus> for LoginStatusResponse {
    fn from(value: LoginStatus) -> Self {
        match value {
            LoginStatus::LoggedOut => Self::LoggedOut,
            LoginStatus::DeviceLoginPending { operation_id } => {
                Self::DeviceLoginPending { operation_id }
            }
            LoginStatus::LoggedIn => Self::LoggedIn,
            LoginStatus::OutcomeUnknown { operation_id } => Self::OutcomeUnknown { operation_id },
        }
    }
}

#[derive(Serialize)]
pub(crate) struct DeviceLoginResponse {
    pub(crate) operation_id: LoginOperationId,
    pub(crate) verification_url: &'static str,
    pub(crate) verification_code: String,
    pub(crate) expires_in_seconds: u16,
}

impl std::fmt::Debug for DeviceLoginResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceLoginResponse")
            .field("operation_id", &self.operation_id)
            .field("verification_url", &self.verification_url)
            .field("verification_code", &"[REDACTED]")
            .field("expires_in_seconds", &self.expires_in_seconds)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartTurnBody {
    pub(crate) prompt: String,
}

impl std::fmt::Debug for StartTurnBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StartTurnBody")
            .field("prompt", &"[REDACTED]")
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolveBody {
    pub(crate) operation_id: Value,
    pub(crate) decision: ResolveDecisionBody,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ResolveDecisionBody {
    Adopt {
        task_id: String,
    },
    Abandon {
        acknowledge_duplicate_task_risk: Option<bool>,
    },
}

impl std::fmt::Debug for ResolveBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolveBody")
            .field("operation_id", &"[UNVALIDATED]")
            .field("decision", &self.decision)
            .finish()
    }
}

impl std::fmt::Debug for ResolveDecisionBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Adopt { .. } => formatter.write_str("Adopt"),
            Self::Abandon {
                acknowledge_duplicate_task_risk,
            } => formatter
                .debug_struct("Abandon")
                .field(
                    "acknowledge_duplicate_task_risk",
                    acknowledge_duplicate_task_risk,
                )
                .finish(),
        }
    }
}

pub(crate) enum Mutation {
    Logout {
        cookie: [u8; 43],
        session_seq: u64,
    },
    StartDeviceLogin,
    CancelLogin,
    StartTurn {
        prompt: CloudPrompt,
    },
    CancelTurn,
    Reconcile,
    Adopt {
        operation_id: CloudSubmitOperationId,
        task_id: CloudTaskId,
    },
    Abandon {
        operation_id: CloudSubmitOperationId,
    },
}

impl std::fmt::Debug for Mutation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Logout { .. } => formatter.write_str("Logout"),
            Self::StartDeviceLogin => formatter.write_str("StartDeviceLogin"),
            Self::CancelLogin => formatter.write_str("CancelLogin"),
            Self::StartTurn { .. } => formatter.write_str("StartTurn([REDACTED])"),
            Self::CancelTurn => formatter.write_str("CancelTurn"),
            Self::Reconcile => formatter.write_str("Reconcile"),
            Self::Adopt { operation_id, .. } => formatter
                .debug_struct("Adopt")
                .field("operation_id", operation_id)
                .finish(),
            Self::Abandon { operation_id } => formatter
                .debug_struct("Abandon")
                .field("operation_id", operation_id)
                .finish(),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct RequestIdentity {
    pub(crate) method: &'static str,
    pub(crate) route: &'static str,
    pub(crate) body: Vec<u8>,
    pub(crate) instance_id: codebox_session_runtime::P0InstanceId,
    pub(crate) logout_session_seq: Option<u64>,
}

impl RequestIdentity {
    pub(crate) fn storage_bytes(&self) -> usize {
        64usize
            .saturating_add(self.method.len())
            .saturating_add(self.route.len())
            .saturating_add(self.body.len())
            .saturating_add(
                self.logout_session_seq
                    .map_or(0, |_| std::mem::size_of::<u64>()),
            )
    }
}

impl std::fmt::Debug for RequestIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestIdentity")
            .field("method", &self.method)
            .field("route", &self.route)
            .field("body", &"[REDACTED]")
            .field("instance_id", &self.instance_id)
            .finish()
    }
}

pub(crate) struct PreparedMutation {
    pub(crate) key: CommandId,
    pub(crate) identity: RequestIdentity,
    pub(crate) mutation: Mutation,
    pub(crate) admitted_at: std::time::Duration,
    pub(crate) admission: std::sync::Arc<crate::state::RequestAdmission>,
}
