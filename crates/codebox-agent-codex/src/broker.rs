use std::fmt;
use std::thread;
use std::time::Duration;

use crate::ledger::{LedgerPhase, LoginLedger, load_ledger, persist_ledger, process_is_alive};
use crate::parser::{PinnedStatus, parse_login_completion};
use crate::runtime::{CliRuntime, LoginSupervisor, ProcessRuntime, SupervisorEvent};
use crate::scope::OwnedCredentialScopeLease;
use crate::{CredentialScope, LoginBrokerError, LoginInteraction, LoginOperationId, LoginStatus};

const INITIAL_EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const INSTRUCTIONS_EVENT_TIMEOUT: Duration = Duration::from_secs(31);
const ORPHAN_DEATH_GRACE: Duration = Duration::from_secs(2);

/// Trusted single-operator Codex device-login broker.
///
/// Contract: `CU-AUTH-P0-01`. The broker owns no repository and exposes no raw CLI output or
/// credential bytes.
pub struct LoginBroker {
    scope: CredentialScope,
    runtime: Box<dyn CliRuntime>,
    active: Option<ActiveLogin>,
}

impl fmt::Debug for LoginBroker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginBroker")
            .field("has_active_login", &self.active.is_some())
            .finish_non_exhaustive()
    }
}

impl LoginBroker {
    /// Creates a broker over one previously validated credential scope.
    ///
    /// Contract: `CU-AUTH-P0-01`. Version and provider state are checked before each operation that
    /// could start or reconcile a login.
    pub fn new(scope: CredentialScope) -> Result<Self, LoginBrokerError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = scope;
            Err(crate::CredentialScopeError::UnsupportedPlatform.into())
        }

        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                scope,
                runtime: Box::<ProcessRuntime>::default(),
                active: None,
            })
        }
    }

    #[cfg(test)]
    pub(crate) fn with_runtime(scope: CredentialScope, runtime: Box<dyn CliRuntime>) -> Self {
        Self {
            scope,
            runtime,
            active: None,
        }
    }

    /// Starts one fixed device-code flow after a fail-closed status preflight.
    ///
    /// Contract: `CU-AUTH-P0-01`. Existing, unknown, or unrecognized authentication state never
    /// spawns the destructive pinned login command.
    pub fn start_device_login(&mut self) -> Result<LoginInteraction, LoginBrokerError> {
        if self.active.is_some() {
            return Err(LoginBrokerError::LoginAlreadyRunning);
        }

        let lease = self.scope.acquire_owned()?;
        self.runtime.verify_version(&lease)?;
        if let Some(existing) = load_ledger(lease.state_dir(), lease.runner_uid())?
            && existing.latest().is_uncertain()
        {
            return Err(LoginBrokerError::OutcomeUnknown);
        }

        match self.runtime.status(&lease)? {
            PinnedStatus::LoggedIn => return Err(LoginBrokerError::AlreadyLoggedIn),
            PinnedStatus::LoggedOut => {}
        }

        let operation_id = LoginOperationId::new();
        let mut ledger = LoginLedger::new(operation_id);
        persist_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
        let mut supervisor = self.runtime.start(&lease)?;

        let process = match supervisor.receive(INITIAL_EVENT_TIMEOUT) {
            Ok(SupervisorEvent::Started(process)) => process,
            Ok(SupervisorEvent::Failed(error)) => {
                ledger.append(LedgerPhase::Failed)?;
                persist_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
                return Err(error);
            }
            Ok(SupervisorEvent::Uncertain(error)) => {
                let mut active = ActiveLogin {
                    operation_id,
                    ledger,
                    supervisor,
                    lease,
                };
                return match self
                    .resolve_terminal_event(&mut active, SupervisorEvent::Uncertain(error))
                {
                    Err(error) => Err(error),
                    Ok(_) => Err(LoginBrokerError::OutcomeUnknown),
                };
            }
            Ok(_) | Err(_) => {
                mark_unknown(&lease, &mut ledger)?;
                return Err(LoginBrokerError::OutcomeUnknown);
            }
        };
        ledger.record_started(process)?;
        persist_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;

        match supervisor.receive(INSTRUCTIONS_EVENT_TIMEOUT) {
            Ok(SupervisorEvent::Instructions(code)) => {
                ledger.append(LedgerPhase::DeviceInstructions)?;
                persist_ledger(lease.state_dir(), lease.runner_uid(), &ledger)?;
                self.active = Some(ActiveLogin {
                    operation_id,
                    ledger,
                    supervisor,
                    lease,
                });
                Ok(LoginInteraction::new(operation_id, code))
            }
            Ok(event) => {
                let mut active = ActiveLogin {
                    operation_id,
                    ledger,
                    supervisor,
                    lease,
                };
                match self.resolve_terminal_event(&mut active, event)? {
                    LoginStatus::LoggedIn => Err(LoginBrokerError::AlreadyLoggedIn),
                    _ => Err(LoginBrokerError::LoginFailed),
                }
            }
            Err(_) => {
                mark_unknown(&lease, &mut ledger)?;
                Err(LoginBrokerError::OutcomeUnknown)
            }
        }
    }

    /// Returns a normalized login projection and reconciles any completed supervised child.
    ///
    /// Contract: `CU-AUTH-P0-01`.
    pub fn status(&mut self) -> Result<LoginStatus, LoginBrokerError> {
        if let Some(active) = self.active.as_mut() {
            let event = active.supervisor.try_receive()?;
            return match event {
                None | Some(SupervisorEvent::Instructions(_)) => {
                    Ok(LoginStatus::DeviceLoginPending {
                        operation_id: active.operation_id,
                    })
                }
                Some(event) => {
                    let mut active = self.active.take().ok_or(LoginBrokerError::OutcomeUnknown)?;
                    self.resolve_terminal_event(&mut active, event)
                }
            };
        }

        self.reconcile_without_active(false)
    }

    /// Reconciles a durable uncertain operation through the fixed status command.
    ///
    /// Contract: `CU-AUTH-P0-01`. This method never retries device login.
    pub fn reconcile(&mut self) -> Result<LoginStatus, LoginBrokerError> {
        if self.active.is_some() {
            return self.status();
        }
        self.reconcile_without_active(true)
    }

    /// Cancels and reaps the active process group, then reconciles provider state.
    ///
    /// Contract: `CU-AUTH-P0-01`. Cancellation is never treated as proof that provider
    /// authorization did not occur.
    pub fn cancel(&mut self) -> Result<LoginStatus, LoginBrokerError> {
        let Some(mut active) = self.active.take() else {
            return self.reconcile_without_active(true);
        };
        mark_unknown(&active.lease, &mut active.ledger)?;
        let event = active.supervisor.cancel()?;
        self.resolve_terminal_event(&mut active, event)
    }

    fn reconcile_without_active(
        &self,
        wait_intent_grace: bool,
    ) -> Result<LoginStatus, LoginBrokerError> {
        let lease = self.scope.acquire_owned()?;
        self.runtime.verify_version(&lease)?;
        let mut ledger = load_ledger(lease.state_dir(), lease.runner_uid())?;

        if let Some(existing) = ledger.as_ref()
            && existing.latest().is_uncertain()
        {
            if let Some(process) = existing.process() {
                if process_is_alive(process)? {
                    return Ok(LoginStatus::OutcomeUnknown {
                        operation_id: existing.operation_id(),
                    });
                }
            } else if wait_intent_grace {
                thread::sleep(ORPHAN_DEATH_GRACE);
            }
        }

        match ledger.as_mut() {
            Some(ledger) if ledger.latest().is_uncertain() => {
                reconcile_ledger(self.runtime.as_ref(), &lease, ledger)
            }
            _ => project_status(self.runtime.status(&lease)?),
        }
    }

    fn resolve_terminal_event(
        &self,
        active: &mut ActiveLogin,
        event: SupervisorEvent,
    ) -> Result<LoginStatus, LoginBrokerError> {
        match event {
            SupervisorEvent::Exited(output) => {
                if let Err(error) = parse_login_completion(&output)
                    && matches!(
                        error,
                        LoginBrokerError::OutputLimitExceeded
                            | LoginBrokerError::ProviderOutputInvalid
                    )
                {
                    mark_unknown(&active.lease, &mut active.ledger)?;
                    return Err(error);
                }
            }
            SupervisorEvent::InstructionDeadline(output)
            | SupervisorEvent::OverallDeadline(output)
            | SupervisorEvent::Canceled(output) => {
                mark_unknown(&active.lease, &mut active.ledger)?;
                if matches!(
                    parse_login_completion(&output),
                    Err(LoginBrokerError::OutputLimitExceeded)
                ) {
                    return Err(LoginBrokerError::OutputLimitExceeded);
                }
            }
            SupervisorEvent::Uncertain(error) => {
                mark_unknown(&active.lease, &mut active.ledger)?;
                reconcile_ledger(self.runtime.as_ref(), &active.lease, &mut active.ledger)?;
                return Err(error);
            }
            SupervisorEvent::Failed(error) => {
                active.ledger.append(LedgerPhase::Failed)?;
                persist_ledger(
                    active.lease.state_dir(),
                    active.lease.runner_uid(),
                    &active.ledger,
                )?;
                return Err(error);
            }
            SupervisorEvent::Started(_) | SupervisorEvent::Instructions(_) => {
                mark_unknown(&active.lease, &mut active.ledger)?;
                return Err(LoginBrokerError::ProviderOutputInvalid);
            }
        }

        reconcile_ledger(self.runtime.as_ref(), &active.lease, &mut active.ledger)
    }
}

impl Drop for LoginBroker {
    fn drop(&mut self) {
        if let Some(mut active) = self.active.take() {
            let _ = mark_unknown(&active.lease, &mut active.ledger);
        }
    }
}

struct ActiveLogin {
    operation_id: LoginOperationId,
    ledger: LoginLedger,
    supervisor: Box<dyn LoginSupervisor>,
    lease: OwnedCredentialScopeLease,
}

fn mark_unknown(
    lease: &OwnedCredentialScopeLease,
    ledger: &mut LoginLedger,
) -> Result<(), LoginBrokerError> {
    if ledger.latest() != LedgerPhase::OutcomeUnknown {
        ledger.append(LedgerPhase::OutcomeUnknown)?;
        persist_ledger(lease.state_dir(), lease.runner_uid(), ledger)?;
    }
    Ok(())
}

fn reconcile_ledger(
    runtime: &dyn CliRuntime,
    lease: &OwnedCredentialScopeLease,
    ledger: &mut LoginLedger,
) -> Result<LoginStatus, LoginBrokerError> {
    match runtime.status(lease) {
        Ok(PinnedStatus::LoggedIn) => {
            ledger.append(LedgerPhase::LoggedIn)?;
            persist_ledger(lease.state_dir(), lease.runner_uid(), ledger)?;
            Ok(LoginStatus::LoggedIn)
        }
        Ok(PinnedStatus::LoggedOut) => {
            ledger.append(LedgerPhase::LoggedOut)?;
            persist_ledger(lease.state_dir(), lease.runner_uid(), ledger)?;
            Ok(LoginStatus::LoggedOut)
        }
        Err(error) => {
            mark_unknown(lease, ledger)?;
            Err(error)
        }
    }
}

fn project_status(status: PinnedStatus) -> Result<LoginStatus, LoginBrokerError> {
    Ok(match status {
        PinnedStatus::LoggedIn => LoginStatus::LoggedIn,
        PinnedStatus::LoggedOut => LoginStatus::LoggedOut,
    })
}
