use std::sync::{Arc, Mutex};

#[cfg(test)]
use codebox_agent_codex::CloudLifecycleErrorCategory;
use codebox_agent_codex::{
    CloudDiff, CloudPrompt, CloudSubmitOperationId, LoginBroker, LoginBrokerError,
    LoginOperationId, LoginStatus, UnknownSubmitDecision,
};
use codebox_session_runtime::{
    P0Actor, P0RecoveryCandidates, P0SessionError, P0SessionIdentity, P0SessionRuntime,
    P0SessionSnapshot, P0TurnReceipt,
};

pub(crate) trait LoginPort: Send + Sync {
    fn status(&self) -> Result<LoginStatus, LoginPortError>;
    fn start_device_login(&self) -> Result<LoginInstructions, LoginPortError>;
    fn cancel(&self) -> Result<LoginStatus, LoginPortError>;
    fn shutdown_cleanup(&self) -> Result<(), LoginPortError>;
}

pub(crate) trait SessionPort: Send + Sync {
    fn identity(&self) -> P0SessionIdentity;
    fn snapshot(&self) -> Result<P0SessionSnapshot, SessionPortError>;
    fn start_turn(&self, prompt: CloudPrompt) -> Result<P0TurnReceipt, SessionPortError>;
    fn cancel_turn(&self, actor: P0Actor) -> Result<P0SessionSnapshot, SessionPortError>;
    fn reconcile_unknown(&self, actor: P0Actor) -> Result<P0RecoveryCandidates, SessionPortError>;
    fn resolve_unknown(
        &self,
        actor: P0Actor,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    ) -> Result<P0SessionSnapshot, SessionPortError>;
    fn read_diff(&self) -> Result<CloudDiff, SessionPortError>;
    fn shutdown(&self) -> Result<(), SessionPortError>;
}

pub(crate) struct LoginInstructions {
    pub(crate) operation_id: LoginOperationId,
    pub(crate) verification_url: &'static str,
    pub(crate) verification_code: String,
    pub(crate) expires_in_seconds: u16,
}

impl std::fmt::Debug for LoginInstructions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginInstructions")
            .field("operation_id", &self.operation_id)
            .field("verification_url", &self.verification_url)
            .field("verification_code", &"[REDACTED]")
            .field("expires_in_seconds", &self.expires_in_seconds)
            .finish()
    }
}

pub(crate) enum LoginPortError {
    Lower(LoginBrokerError),
    Unavailable,
}

impl std::fmt::Debug for LoginPortError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lower(error) => formatter.debug_tuple("Lower").field(error).finish(),
            Self::Unavailable => formatter.write_str("Unavailable"),
        }
    }
}

pub(crate) enum SessionPortError {
    Lower(P0SessionError),
    #[cfg(test)]
    Unavailable,
    #[cfg(test)]
    ProjectedLifecycle {
        category: CloudLifecycleErrorCategory,
        operation_id: Option<CloudSubmitOperationId>,
    },
}

impl std::fmt::Debug for SessionPortError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lower(error) => formatter.debug_tuple("Lower").field(error).finish(),
            #[cfg(test)]
            Self::Unavailable => formatter.write_str("Unavailable"),
            #[cfg(test)]
            Self::ProjectedLifecycle {
                category,
                operation_id,
            } => formatter
                .debug_struct("ProjectedLifecycle")
                .field("category", category)
                .field("operation_id", operation_id)
                .finish(),
        }
    }
}

pub(crate) struct ConcreteLoginPort {
    broker: Mutex<LoginBroker>,
}

impl ConcreteLoginPort {
    pub(crate) fn new(broker: LoginBroker) -> Self {
        Self {
            broker: Mutex::new(broker),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, LoginBroker>, LoginPortError> {
        self.broker.lock().map_err(|_| LoginPortError::Unavailable)
    }
}

impl LoginPort for ConcreteLoginPort {
    fn status(&self) -> Result<LoginStatus, LoginPortError> {
        self.lock()?.status().map_err(LoginPortError::Lower)
    }

    fn start_device_login(&self) -> Result<LoginInstructions, LoginPortError> {
        let interaction = self
            .lock()?
            .start_device_login()
            .map_err(LoginPortError::Lower)?;
        Ok(LoginInstructions {
            operation_id: interaction.operation_id(),
            verification_url: interaction.verification_url().as_str(),
            verification_code: interaction.verification_code().expose().to_owned(),
            expires_in_seconds: interaction.expires_in_seconds(),
        })
    }

    fn cancel(&self) -> Result<LoginStatus, LoginPortError> {
        self.lock()?.cancel().map_err(LoginPortError::Lower)
    }

    fn shutdown_cleanup(&self) -> Result<(), LoginPortError> {
        self.lock()?
            .cancel()
            .map(|_| ())
            .map_err(LoginPortError::Lower)
    }
}

pub(crate) struct ConcreteSessionPort {
    runtime: Arc<P0SessionRuntime>,
}

impl ConcreteSessionPort {
    pub(crate) fn new(runtime: Arc<P0SessionRuntime>) -> Self {
        Self { runtime }
    }
}

impl SessionPort for ConcreteSessionPort {
    fn identity(&self) -> P0SessionIdentity {
        self.runtime.identity()
    }

    fn snapshot(&self) -> Result<P0SessionSnapshot, SessionPortError> {
        self.runtime.snapshot().map_err(SessionPortError::Lower)
    }

    fn start_turn(&self, prompt: CloudPrompt) -> Result<P0TurnReceipt, SessionPortError> {
        self.runtime
            .start_turn(prompt)
            .map_err(SessionPortError::Lower)
    }

    fn cancel_turn(&self, actor: P0Actor) -> Result<P0SessionSnapshot, SessionPortError> {
        self.runtime
            .cancel_turn(actor)
            .map_err(SessionPortError::Lower)
    }

    fn reconcile_unknown(&self, actor: P0Actor) -> Result<P0RecoveryCandidates, SessionPortError> {
        self.runtime
            .reconcile_unknown(actor)
            .map_err(SessionPortError::Lower)
    }

    fn resolve_unknown(
        &self,
        actor: P0Actor,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    ) -> Result<P0SessionSnapshot, SessionPortError> {
        self.runtime
            .resolve_unknown(actor, operation_id, decision)
            .map_err(SessionPortError::Lower)
    }

    fn read_diff(&self) -> Result<CloudDiff, SessionPortError> {
        self.runtime.read_diff().map_err(SessionPortError::Lower)
    }

    fn shutdown(&self) -> Result<(), SessionPortError> {
        self.runtime.shutdown().map_err(SessionPortError::Lower)
    }
}
