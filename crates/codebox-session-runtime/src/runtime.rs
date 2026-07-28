use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use codebox_agent_codex::{
    CloudCancellation, CloudDiff, CloudDiffReadError, CloudDiffReadErrorCategory, CloudLifecycle,
    CloudLifecycleError, CloudLifecycleErrorCategory, CloudPrompt, CloudSubmitOperationId,
    CloudTaskOrchestrator, UnknownSubmitDecision,
};
use codebox_domain::{EventSeq, SessionId, TurnId};

use crate::config::MAX_BACKOFF;
use crate::{
    P0Actor, P0CloudLifecycle, P0InstanceId, P0LiveReceiveError, P0RecoveryCandidates,
    P0RecoveryDecisionKind, P0RuntimeStopReason, P0SessionConfig, P0SessionError,
    P0SessionErrorCategory, P0SessionEvent, P0SessionEventEnvelope, P0SessionIdentity,
    P0SessionSnapshot, P0SessionState, P0TurnProjection, P0TurnReceipt, P0TurnSnapshot,
};

const EVENT_SCHEMA_VERSION: u16 = 1;
const CANCEL_ADMISSION_POLL: Duration = Duration::from_millis(10);
const CANCEL_ADMISSION_LIMIT: Duration = Duration::from_secs(1);

const CLOSE_OPEN: u8 = 0;
const CLOSE_LAGGED: u8 = 1;
const CLOSE_RUNTIME_STOPPED: u8 = 2;
const CLOSE_NORMAL: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LowerLifecycleError {
    category: CloudLifecycleErrorCategory,
    operation_id: Option<CloudSubmitOperationId>,
}

impl LowerLifecycleError {
    #[cfg(test)]
    pub(crate) const fn new(
        category: CloudLifecycleErrorCategory,
        operation_id: Option<CloudSubmitOperationId>,
    ) -> Self {
        Self {
            category,
            operation_id,
        }
    }

    const fn category(self) -> CloudLifecycleErrorCategory {
        self.category
    }

    const fn operation_id(self) -> Option<CloudSubmitOperationId> {
        self.operation_id
    }
}

impl From<CloudLifecycleError> for LowerLifecycleError {
    fn from(error: CloudLifecycleError) -> Self {
        Self {
            category: error.category(),
            operation_id: error.operation_id(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LowerDiffError(CloudDiffReadErrorCategory);

impl LowerDiffError {
    #[cfg(test)]
    pub(crate) const fn new(category: CloudDiffReadErrorCategory) -> Self {
        Self(category)
    }

    const fn category(self) -> CloudDiffReadErrorCategory {
        self.0
    }
}

impl From<CloudDiffReadError> for LowerDiffError {
    fn from(error: CloudDiffReadError) -> Self {
        Self(error.category())
    }
}

pub(crate) trait P0CloudControl: Send + Sync {
    fn inspect(&self) -> Result<CloudLifecycle, LowerLifecycleError>;
    fn start(&self, prompt: CloudPrompt) -> Result<CloudLifecycle, LowerLifecycleError>;
    fn cancel(&self) -> Result<CloudLifecycle, LowerLifecycleError>;
    fn reconcile_unknown(&self) -> Result<P0RecoveryCandidates, LowerLifecycleError>;
    fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    ) -> Result<CloudLifecycle, LowerLifecycleError>;
    fn read_diff(&self) -> Result<CloudDiff, LowerDiffError>;
}

struct CodexCloudControl {
    orchestrator: CloudTaskOrchestrator,
}

impl P0CloudControl for CodexCloudControl {
    fn inspect(&self) -> Result<CloudLifecycle, LowerLifecycleError> {
        self.orchestrator.inspect().map_err(Into::into)
    }

    fn start(&self, prompt: CloudPrompt) -> Result<CloudLifecycle, LowerLifecycleError> {
        self.orchestrator.start(prompt).map_err(Into::into)
    }

    fn cancel(&self) -> Result<CloudLifecycle, LowerLifecycleError> {
        self.orchestrator.cancel().map_err(Into::into)
    }

    fn reconcile_unknown(&self) -> Result<P0RecoveryCandidates, LowerLifecycleError> {
        let reconciliation = self
            .orchestrator
            .reconcile_unknown()
            .map_err(LowerLifecycleError::from)?;
        Ok(P0RecoveryCandidates::from_summaries(
            reconciliation.operation_id(),
            reconciliation.tasks(),
            reconciliation.is_complete(),
        ))
    }

    fn resolve_unknown(
        &self,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    ) -> Result<CloudLifecycle, LowerLifecycleError> {
        self.orchestrator
            .resolve_unknown(operation_id, decision)
            .map_err(Into::into)
    }

    fn read_diff(&self) -> Result<CloudDiff, LowerDiffError> {
        let task = self
            .orchestrator
            .diff_eligible_task()
            .map_err(LowerDiffError::from)?;
        self.orchestrator
            .diff_reader()
            .retrieve(&task, CloudCancellation::new())
            .map_err(Into::into)
    }
}

struct Inner {
    cloud: Arc<dyn P0CloudControl>,
    config: P0SessionConfig,
    identity: P0SessionIdentity,
    state: Mutex<RuntimeState>,
    changed: Condvar,
    claim_hook: Option<Arc<WorkerClaimHook>>,
}

pub(crate) struct WorkerClaimHook {
    state: Mutex<WorkerClaimHookState>,
    changed: Condvar,
}

struct WorkerClaimHookState {
    claimed: bool,
    released: bool,
}

impl WorkerClaimHook {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(WorkerClaimHookState {
                claimed: false,
                released: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn pause_after_claim(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.claimed = true;
        self.changed.notify_all();
        while !state.released {
            let Ok(next) = self.changed.wait(state) else {
                return;
            };
            state = next;
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_claimed(&self) {
        let Ok(mut state) = self.state.lock() else {
            panic!("worker claim hook was poisoned");
        };
        while !state.claimed {
            let Ok(next) = self.changed.wait(state) else {
                panic!("worker claim hook was poisoned");
            };
            state = next;
        }
    }

    #[cfg(test)]
    pub(crate) fn release(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.released = true;
            self.changed.notify_all();
        }
    }
}

struct RuntimeState {
    session_state: P0SessionState,
    current_turn: Option<TurnRecord>,
    high_water_seq: EventSeq,
    history: VecDeque<P0SessionEventEnvelope>,
    subscribers: BTreeMap<u64, Subscriber>,
    next_subscriber_id: u64,
    work: Option<WorkerWork>,
    shutdown_requested: bool,
    worker_stopped: bool,
}

struct TurnRecord {
    turn_id: TurnId,
    projection: P0TurnProjection,
    lower_start_admitted: bool,
    cancel_in_progress: bool,
    cancel_completed: bool,
}

enum WorkerWork {
    Start {
        turn_id: TurnId,
        prompt: CloudPrompt,
    },
    Monitor {
        turn_id: TurnId,
        due: Instant,
        failures: u32,
    },
}

struct Subscriber {
    sender: SyncSender<P0SessionEventEnvelope>,
    close_reason: Arc<AtomicU8>,
}

enum WorkerAction {
    Start {
        turn_id: TurnId,
        prompt: CloudPrompt,
    },
    Monitor {
        turn_id: TurnId,
        failures: u32,
    },
    Cancel {
        turn_id: TurnId,
    },
    Stop,
}

/// One process-lifetime, single-writer P0 session over the accepted Cloud orchestrator.
///
/// Contract: `CU-SES-P0-01`. The runtime owns no HTTP, authentication, provider configuration,
/// repository command, artifact, diff-application, or crash-durable event-store authority.
pub struct P0SessionRuntime {
    inner: Arc<Inner>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl P0SessionRuntime {
    /// Constructs one P0 session, observes the accepted lower lifecycle once, and starts its worker.
    ///
    /// Contract: `CU-SES-P0-01`. Startup never submits, cancels, resolves, or reads a diff.
    pub fn new(
        cloud: CloudTaskOrchestrator,
        config: P0SessionConfig,
    ) -> Result<Self, P0SessionError> {
        Self::from_cloud(
            Arc::new(CodexCloudControl {
                orchestrator: cloud,
            }),
            config,
            true,
            None,
        )
    }

    fn from_cloud(
        cloud: Arc<dyn P0CloudControl>,
        config: P0SessionConfig,
        spawn_worker: bool,
        claim_hook: Option<Arc<WorkerClaimHook>>,
    ) -> Result<Self, P0SessionError> {
        let identity = P0SessionIdentity {
            session_id: SessionId::new(),
            instance_id: P0InstanceId::new(),
        };
        let (session_state, current_turn, work) = initial_state(cloud.as_ref(), config)?;
        let inner = Arc::new(Inner {
            cloud,
            config,
            identity,
            state: Mutex::new(RuntimeState {
                session_state,
                current_turn,
                high_water_seq: EventSeq::initial(),
                history: VecDeque::with_capacity(config.history_capacity),
                subscribers: BTreeMap::new(),
                next_subscriber_id: 1,
                work,
                shutdown_requested: false,
                worker_stopped: false,
            }),
            changed: Condvar::new(),
            claim_hook,
        });

        let handle = if spawn_worker {
            let worker_inner = Arc::clone(&inner);
            Some(
                thread::Builder::new()
                    .name("codebox-p0-session".to_owned())
                    .spawn(move || worker_loop(worker_inner))
                    .map_err(|_| P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))?,
            )
        } else {
            None
        };

        Ok(Self {
            inner,
            worker: Mutex::new(handle),
        })
    }

    /// Returns the immutable session and process-instance identities.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub fn identity(&self) -> P0SessionIdentity {
        self.inner.identity
    }

    /// Returns one redacted current snapshot without invoking the lower orchestrator.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub fn snapshot(&self) -> Result<P0SessionSnapshot, P0SessionError> {
        let state = self.lock_state()?;
        Ok(snapshot_from(&self.inner, &state))
    }

    /// Atomically commits and queues one turn intent from exactly `Ready`.
    ///
    /// Contract: `CU-SES-P0-01`. The worker cannot invoke lower Cloud start before the
    /// `TurnAccepted` event is committed.
    pub fn start_turn(&self, prompt: CloudPrompt) -> Result<P0TurnReceipt, P0SessionError> {
        let mut state = self.lock_state()?;
        require_running_worker(&state)?;
        if state.session_state != P0SessionState::Ready {
            return Err(state
                .current_turn
                .as_ref()
                .map(|turn| {
                    P0SessionError::for_turn(
                        P0SessionErrorCategory::TurnAlreadyRunning,
                        turn.turn_id,
                    )
                })
                .unwrap_or_else(|| {
                    P0SessionError::new(P0SessionErrorCategory::TurnAlreadyRunning)
                }));
        }
        reserve_events(&state, 1)?;

        let turn_id = TurnId::new();
        state.session_state = P0SessionState::Running;
        state.current_turn = Some(TurnRecord {
            turn_id,
            projection: P0TurnProjection::Queued,
            lower_start_admitted: false,
            cancel_in_progress: false,
            cancel_completed: false,
        });
        state.work = Some(WorkerWork::Start { turn_id, prompt });
        append_event(
            &self.inner,
            &mut state,
            Some(turn_id),
            P0SessionEvent::TurnAccepted,
        );
        let receipt = P0TurnReceipt {
            turn_id,
            high_water_seq: state.high_water_seq,
        };
        self.inner.changed.notify_all();
        Ok(receipt)
    }

    /// Applies one explicit operator cancellation to the current active turn.
    ///
    /// Contract: `CU-SES-P0-01`. Subscriber/transport lifetime never calls this method, and the
    /// result never claims provider-side cancellation.
    pub fn cancel_turn(&self, actor: P0Actor) -> Result<P0SessionSnapshot, P0SessionError> {
        let (turn_id, should_call_lower, admission_may_be_pending) = {
            let mut state = self.lock_state()?;
            require_running_worker(&state)?;
            if state.session_state == P0SessionState::RecoveryRequired {
                return Err(current_operation(&state)
                    .map(|operation_id| {
                        P0SessionError::for_operation(
                            P0SessionErrorCategory::WrongState,
                            operation_id,
                        )
                    })
                    .unwrap_or_else(|| P0SessionError::new(P0SessionErrorCategory::WrongState)));
            }
            if !matches!(
                state.session_state,
                P0SessionState::Running | P0SessionState::MonitoringDegraded
            ) {
                return Err(P0SessionError::new(P0SessionErrorCategory::WrongState));
            }
            let turn_id = state
                .current_turn
                .as_ref()
                .map(|turn| turn.turn_id)
                .ok_or_else(|| P0SessionError::new(P0SessionErrorCategory::NoCurrentTurn))?;

            if matches!(
                state.current_turn.as_ref().map(|turn| &turn.projection),
                Some(P0TurnProjection::Queued)
            ) {
                reserve_events(&state, 1)?;
                state.work = None;
                state.session_state = P0SessionState::Ready;
                if let Some(turn) = state.current_turn.as_mut() {
                    turn.projection = P0TurnProjection::CanceledBeforeCloudStart;
                    turn.cancel_completed = true;
                }
                append_event(
                    &self.inner,
                    &mut state,
                    Some(turn_id),
                    P0SessionEvent::TurnCanceledBeforeCloudStart,
                );
                self.inner.changed.notify_all();
                return Ok(snapshot_from(&self.inner, &state));
            }

            let already_requested = state
                .current_turn
                .as_ref()
                .is_some_and(|turn| projection_cancel_requested(&turn.projection));
            if !already_requested {
                reserve_events(&state, 1)?;
                if let Some(turn) = state.current_turn.as_mut() {
                    set_projection_cancel_requested(&mut turn.projection);
                }
                append_event(
                    &self.inner,
                    &mut state,
                    Some(turn_id),
                    P0SessionEvent::CancelRequested { actor },
                );
            }

            let (should_call_lower, admission_may_be_pending) = {
                let turn = state
                    .current_turn
                    .as_mut()
                    .ok_or_else(|| P0SessionError::new(P0SessionErrorCategory::NoCurrentTurn))?;
                if turn.cancel_completed {
                    return Ok(snapshot_from(&self.inner, &state));
                }
                let should_call_lower = !turn.cancel_in_progress;
                if should_call_lower {
                    turn.cancel_in_progress = true;
                }
                (
                    should_call_lower,
                    matches!(turn.projection, P0TurnProjection::Starting { .. }),
                )
            };
            if should_call_lower && !admission_may_be_pending {
                state.work = None;
            }
            (turn_id, should_call_lower, admission_may_be_pending)
        };

        if !should_call_lower {
            return self.snapshot();
        }

        let deadline = Instant::now() + CANCEL_ADMISSION_LIMIT;
        loop {
            match self.inner.cloud.cancel() {
                Ok(lifecycle) => return self.commit_cancel_result(turn_id, lifecycle),
                Err(error)
                    if admission_may_be_pending
                        && error.category() == CloudLifecycleErrorCategory::NoCurrentOperation
                        && Instant::now() < deadline =>
                {
                    let state = self.lock_state()?;
                    let completed = state
                        .current_turn
                        .as_ref()
                        .filter(|turn| turn.turn_id == turn_id)
                        .is_some_and(|turn| turn.cancel_completed);
                    if completed {
                        return Ok(snapshot_from(&self.inner, &state));
                    }
                    drop(state);
                    thread::sleep(CANCEL_ADMISSION_POLL);
                }
                Err(error)
                    if admission_may_be_pending
                        && error.category() == CloudLifecycleErrorCategory::NoCurrentOperation =>
                {
                    let mut state = self.lock_state()?;
                    if let Some(turn) = state
                        .current_turn
                        .as_mut()
                        .filter(|turn| turn.turn_id == turn_id)
                    {
                        turn.cancel_in_progress = false;
                    }
                    self.inner.changed.notify_all();
                    return Ok(snapshot_from(&self.inner, &state));
                }
                Err(error) => {
                    let mut state = self.lock_state()?;
                    if let Some(turn) = state
                        .current_turn
                        .as_mut()
                        .filter(|turn| turn.turn_id == turn_id)
                    {
                        turn.cancel_in_progress = false;
                    }
                    self.inner.changed.notify_all();
                    return Err(map_lower_error(error));
                }
            }
        }
    }

    /// Performs one explicit bounded reconciliation of the current unknown submit.
    ///
    /// Contract: `CU-SES-P0-01`. This method neither resolves the unknown operation nor starts a
    /// second submit.
    pub fn reconcile_unknown(
        &self,
        actor: P0Actor,
    ) -> Result<P0RecoveryCandidates, P0SessionError> {
        let expected = {
            let state = self.lock_state()?;
            require_running_worker(&state)?;
            if state.session_state != P0SessionState::RecoveryRequired {
                return Err(P0SessionError::new(P0SessionErrorCategory::WrongState));
            }
            current_operation(&state)
                .ok_or_else(|| P0SessionError::new(P0SessionErrorCategory::WrongOperation))?
        };
        let candidates = self
            .inner
            .cloud
            .reconcile_unknown()
            .map_err(map_lower_error)?;
        if candidates.operation_id != expected {
            return Err(P0SessionError::for_operation(
                P0SessionErrorCategory::LowerConflict,
                candidates.operation_id,
            ));
        }

        let mut state = self.lock_state()?;
        require_current_operation(&state, expected)?;
        reserve_events(&state, 1)?;
        let turn_id = state.current_turn.as_ref().map(|turn| turn.turn_id);
        append_event(
            &self.inner,
            &mut state,
            turn_id,
            P0SessionEvent::RecoveryObserved {
                actor,
                operation_id: expected,
                task_ids: candidates.task_ids.clone(),
                complete: candidates.complete,
            },
        );
        Ok(candidates)
    }

    /// Applies one authenticated caller-created decision to the exact current unknown operation.
    ///
    /// Contract: `CU-SES-P0-01`. The runtime does not infer or mint recovery policy.
    pub fn resolve_unknown(
        &self,
        actor: P0Actor,
        operation_id: CloudSubmitOperationId,
        decision: UnknownSubmitDecision,
    ) -> Result<P0SessionSnapshot, P0SessionError> {
        {
            let state = self.lock_state()?;
            require_running_worker(&state)?;
            if state.session_state != P0SessionState::RecoveryRequired {
                return Err(P0SessionError::for_operation(
                    P0SessionErrorCategory::WrongState,
                    operation_id,
                ));
            }
            require_current_operation(&state, operation_id)?;
        }
        let decision_kind = match &decision {
            UnknownSubmitDecision::AdoptListedTask(_) => P0RecoveryDecisionKind::Adopt,
            UnknownSubmitDecision::AbandonAfterReconciliation(_) => P0RecoveryDecisionKind::Abandon,
        };
        let lifecycle = self
            .inner
            .cloud
            .resolve_unknown(operation_id, decision)
            .map_err(map_lower_error)?;
        let projected = P0CloudLifecycle::from_cloud(lifecycle);
        if projected.operation_id() != operation_id {
            return Err(P0SessionError::for_operation(
                P0SessionErrorCategory::LowerConflict,
                projected.operation_id(),
            ));
        }

        let mut state = self.lock_state()?;
        require_current_operation(&state, operation_id)?;
        require_running_worker(&state)?;
        reserve_events(&state, 2)?;
        let turn_id = state
            .current_turn
            .as_ref()
            .map(|turn| turn.turn_id)
            .ok_or_else(|| P0SessionError::new(P0SessionErrorCategory::NoCurrentTurn))?;
        apply_projected_lifecycle(&mut state, turn_id, projected.clone(), false)?;
        append_event(
            &self.inner,
            &mut state,
            Some(turn_id),
            P0SessionEvent::RecoveryResolved {
                actor,
                operation_id,
                decision: decision_kind,
            },
        );
        append_event(
            &self.inner,
            &mut state,
            Some(turn_id),
            P0SessionEvent::LifecycleChanged {
                lifecycle: projected.clone(),
            },
        );
        if projected.is_pending() {
            state.work = Some(WorkerWork::Monitor {
                turn_id,
                due: Instant::now() + self.inner.config.poll_interval,
                failures: 0,
            });
            self.inner.changed.notify_all();
        }
        Ok(snapshot_from(&self.inner, &state))
    }

    /// Reads one current bounded untrusted provider diff without changing session/event state.
    ///
    /// Contract: `CU-SES-P0-01`. T004C revalidates opaque authority and owns timeout/reap.
    pub fn read_diff(&self) -> Result<CloudDiff, P0SessionError> {
        {
            let state = self.lock_state()?;
            require_running_worker(&state)?;
            let eligible = state.current_turn.as_ref().is_some_and(|turn| {
                matches!(
                    &turn.projection,
                    P0TurnProjection::Cloud {
                        lifecycle: P0CloudLifecycle::Ready { .. }
                            | P0CloudLifecycle::Applied { .. },
                        ..
                    }
                )
            });
            if !eligible {
                return Err(P0SessionError::new(P0SessionErrorCategory::WrongState));
            }
        }
        self.inner
            .cloud
            .read_diff()
            .map_err(|error| P0SessionError::from_cloud_diff(error.category()))
    }

    /// Atomically captures retained replay, snapshot/high-water, and later live delivery.
    ///
    /// Contract: `CU-SES-P0-01`. Invalid cursors return no partial subscription.
    pub fn subscribe(
        &self,
        session_id: SessionId,
        after_seq: EventSeq,
    ) -> Result<P0SessionSubscription, P0SessionError> {
        if session_id != self.inner.identity.session_id {
            return Err(P0SessionError::new(P0SessionErrorCategory::WrongSession));
        }
        let mut state = self.lock_state()?;
        require_running_worker(&state)?;
        let latest = state.high_water_seq;
        if after_seq > latest {
            return Err(P0SessionError::future_cursor(latest));
        }
        if let Some(oldest) = state.history.front().map(|event| event.seq) {
            let earliest_cursor = EventSeq::new(oldest.value().saturating_sub(1));
            if after_seq < earliest_cursor {
                return Err(P0SessionError::history_gap(oldest, latest));
            }
        }
        if state.subscribers.len() >= self.inner.config.max_subscribers {
            return Err(P0SessionError::new(P0SessionErrorCategory::SubscriberLimit));
        }
        let subscriber_id = state.next_subscriber_id;
        state.next_subscriber_id = state
            .next_subscriber_id
            .checked_add(1)
            .ok_or_else(|| P0SessionError::new(P0SessionErrorCategory::SubscriberLimit))?;
        let replay = state
            .history
            .iter()
            .filter(|event| event.seq > after_seq)
            .cloned()
            .collect();
        let snapshot = snapshot_from(&self.inner, &state);
        let (sender, receiver) = sync_channel(self.inner.config.live_capacity);
        let close_reason = Arc::new(AtomicU8::new(CLOSE_OPEN));
        state.subscribers.insert(
            subscriber_id,
            Subscriber {
                sender,
                close_reason: Arc::clone(&close_reason),
            },
        );
        let live = P0LiveReceiver {
            receiver,
            close_reason,
            subscriber_id,
            inner: Arc::downgrade(&self.inner),
        };
        Ok(P0SessionSubscription {
            replay: Some(replay),
            snapshot: Some(snapshot),
            live: Some(live),
        })
    }

    /// Stops and joins the local worker without invoking lower cancellation.
    ///
    /// Contract: `CU-SES-P0-01`. An admitted bounded lower call is allowed to return and its
    /// durable projection is retained before the worker exits.
    pub fn shutdown(&self) -> Result<(), P0SessionError> {
        {
            let mut state = self.lock_state()?;
            if !state.shutdown_requested {
                state.shutdown_requested = true;
                if matches!(
                    state.current_turn.as_ref().map(|turn| &turn.projection),
                    Some(P0TurnProjection::Queued)
                ) {
                    reserve_events(&state, 1)?;
                    state.work = None;
                    state.session_state = P0SessionState::Stopped;
                    state.worker_stopped = true;
                    let turn_id = state.current_turn.as_ref().map(|turn| turn.turn_id);
                    if let Some(turn) = state.current_turn.as_mut() {
                        turn.projection = P0TurnProjection::StoppedBeforeCloudStart;
                    }
                    append_event(
                        &self.inner,
                        &mut state,
                        turn_id,
                        P0SessionEvent::RuntimeStopped {
                            reason: P0RuntimeStopReason::Shutdown,
                        },
                    );
                    close_subscribers(&mut state, CLOSE_RUNTIME_STOPPED);
                }
                self.inner.changed.notify_all();
            }
        }

        let handle = self
            .worker
            .lock()
            .map_err(|_| P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))?
            .take();
        if let Some(handle) = handle {
            handle
                .join()
                .map_err(|_| P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))?;
        }
        Ok(())
    }

    fn commit_cancel_result(
        &self,
        turn_id: TurnId,
        lifecycle: CloudLifecycle,
    ) -> Result<P0SessionSnapshot, P0SessionError> {
        let projected = P0CloudLifecycle::from_cloud(lifecycle);
        let mut state = self.lock_state()?;
        if state.worker_stopped {
            return Err(P0SessionError::new(P0SessionErrorCategory::RuntimeStopped));
        }
        let turn = state
            .current_turn
            .as_ref()
            .filter(|turn| turn.turn_id == turn_id)
            .ok_or_else(|| P0SessionError::new(P0SessionErrorCategory::WrongState))?;
        if let Some(current) = projection_operation(&turn.projection)
            && current != projected.operation_id()
        {
            return Err(P0SessionError::for_operation(
                P0SessionErrorCategory::LowerConflict,
                projected.operation_id(),
            ));
        }
        let changed = projection_lifecycle(&turn.projection) != Some(&projected);
        if changed {
            reserve_events(&state, 1)?;
        }
        apply_projected_lifecycle(&mut state, turn_id, projected.clone(), true)?;
        if let Some(turn) = state.current_turn.as_mut() {
            turn.cancel_in_progress = false;
            turn.cancel_completed = true;
        }
        if changed {
            append_event(
                &self.inner,
                &mut state,
                Some(turn_id),
                P0SessionEvent::LifecycleChanged {
                    lifecycle: projected,
                },
            );
        }
        self.inner.changed.notify_all();
        Ok(snapshot_from(&self.inner, &state))
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, RuntimeState>, P0SessionError> {
        self.inner
            .state
            .lock()
            .map_err(|_| P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))
    }

    #[cfg(test)]
    pub(crate) fn with_cloud(
        cloud: Arc<dyn P0CloudControl>,
        config: P0SessionConfig,
    ) -> Result<Self, P0SessionError> {
        Self::from_cloud(cloud, config, true, None)
    }

    #[cfg(test)]
    pub(crate) fn with_cloud_without_worker(
        cloud: Arc<dyn P0CloudControl>,
        config: P0SessionConfig,
    ) -> Result<Self, P0SessionError> {
        Self::from_cloud(cloud, config, false, None)
    }

    #[cfg(test)]
    pub(crate) fn with_cloud_claim_hook(
        cloud: Arc<dyn P0CloudControl>,
        config: P0SessionConfig,
        claim_hook: Arc<WorkerClaimHook>,
    ) -> Result<Self, P0SessionError> {
        Self::from_cloud(cloud, config, true, Some(claim_hook))
    }
}

impl fmt::Debug for P0SessionRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("P0SessionRuntime")
            .field("identity", &self.inner.identity)
            .finish_non_exhaustive()
    }
}

impl Drop for P0SessionRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Atomic retained replay, captured snapshot/high-water, and live receiver.
///
/// Contract: `CU-SES-P0-01`.
pub struct P0SessionSubscription {
    replay: Option<Vec<P0SessionEventEnvelope>>,
    snapshot: Option<P0SessionSnapshot>,
    live: Option<P0LiveReceiver>,
}

impl P0SessionSubscription {
    /// Returns retained events above the requested cursor through high-water.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub fn replay(&self) -> &[P0SessionEventEnvelope] {
        self.replay.as_deref().unwrap_or_default()
    }

    /// Returns the snapshot atomically captured at `high_water_seq`.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub fn snapshot(&self) -> &P0SessionSnapshot {
        self.snapshot
            .as_ref()
            .expect("subscription snapshot is present until consumed")
    }

    /// Returns the replay/live handoff sequence.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub fn high_water_seq(&self) -> EventSeq {
        self.snapshot().high_water_seq
    }

    /// Consumes the handoff into owned replay, snapshot, and live components.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub fn into_parts(
        mut self,
    ) -> (
        Vec<P0SessionEventEnvelope>,
        P0SessionSnapshot,
        P0LiveReceiver,
    ) {
        let replay = self.replay.take().unwrap_or_default();
        let snapshot = self
            .snapshot
            .take()
            .expect("subscription snapshot is present until consumed");
        let live = self
            .live
            .take()
            .expect("subscription receiver is present until consumed");
        (replay, snapshot, live)
    }
}

impl fmt::Debug for P0SessionSubscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("P0SessionSubscription")
            .field("replay_count", &self.replay().len())
            .field("high_water_seq", &self.high_water_seq())
            .finish_non_exhaustive()
    }
}

/// Bounded live event receiver registered after one atomic replay high-water.
///
/// Contract: `CU-SES-P0-01`. Dropping it removes only this subscriber.
pub struct P0LiveReceiver {
    receiver: Receiver<P0SessionEventEnvelope>,
    close_reason: Arc<AtomicU8>,
    subscriber_id: u64,
    inner: Weak<Inner>,
}

impl P0LiveReceiver {
    /// Waits for one complete event for at most the caller-supplied duration.
    ///
    /// Contract: `CU-SES-P0-01`. Timeout is `Empty`; channel closure retains its safe reason.
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<P0SessionEventEnvelope, P0LiveReceiveError> {
        self.receiver
            .recv_timeout(timeout)
            .map_err(|error| match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => P0LiveReceiveError::Empty,
                std::sync::mpsc::RecvTimeoutError::Disconnected => self.close_error(),
            })
    }

    /// Attempts one nonblocking complete event receive.
    ///
    /// Contract: `CU-SES-P0-01`.
    pub fn try_recv(&self) -> Result<P0SessionEventEnvelope, P0LiveReceiveError> {
        self.receiver.try_recv().map_err(|error| match error {
            TryRecvError::Empty => P0LiveReceiveError::Empty,
            TryRecvError::Disconnected => self.close_error(),
        })
    }

    fn close_error(&self) -> P0LiveReceiveError {
        match self.close_reason.load(Ordering::Acquire) {
            CLOSE_LAGGED => P0LiveReceiveError::Lagged,
            CLOSE_RUNTIME_STOPPED => P0LiveReceiveError::RuntimeStopped,
            _ => P0LiveReceiveError::Closed,
        }
    }
}

impl fmt::Debug for P0LiveReceiver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("P0LiveReceiver")
            .finish_non_exhaustive()
    }
}

impl Drop for P0LiveReceiver {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade()
            && let Ok(mut state) = inner.state.lock()
            && let Some(subscriber) = state.subscribers.remove(&self.subscriber_id)
        {
            subscriber
                .close_reason
                .store(CLOSE_NORMAL, Ordering::Release);
        }
    }
}

fn initial_state(
    cloud: &dyn P0CloudControl,
    config: P0SessionConfig,
) -> Result<(P0SessionState, Option<TurnRecord>, Option<WorkerWork>), P0SessionError> {
    match cloud.inspect() {
        Ok(lifecycle) => {
            let projected = P0CloudLifecycle::from_cloud(lifecycle);
            if matches!(projected, P0CloudLifecycle::Submitting { .. }) {
                return Err(P0SessionError::for_operation(
                    P0SessionErrorCategory::LowerConflict,
                    projected.operation_id(),
                ));
            }
            let turn_id = TurnId::new();
            let session_state = state_for_lifecycle(&projected);
            let work = projected.is_pending().then_some(WorkerWork::Monitor {
                turn_id,
                due: Instant::now() + config.poll_interval,
                failures: 0,
            });
            Ok((
                session_state,
                Some(TurnRecord {
                    turn_id,
                    projection: P0TurnProjection::Cloud {
                        lifecycle: projected,
                        cancel_requested: false,
                    },
                    lower_start_admitted: false,
                    cancel_in_progress: false,
                    cancel_completed: false,
                }),
                work,
            ))
        }
        Err(error) if error.category() == CloudLifecycleErrorCategory::NoCurrentOperation => {
            Ok((P0SessionState::Ready, None, None))
        }
        Err(error)
            if error.category() == CloudLifecycleErrorCategory::OutcomeUnknown
                && error.operation_id().is_some() =>
        {
            let operation_id = error.operation_id().ok_or_else(|| map_lower_error(error))?;
            let turn_id = TurnId::new();
            Ok((
                P0SessionState::RecoveryRequired,
                Some(TurnRecord {
                    turn_id,
                    projection: P0TurnProjection::Cloud {
                        lifecycle: P0CloudLifecycle::OutcomeUnknown { operation_id },
                        cancel_requested: false,
                    },
                    lower_start_admitted: false,
                    cancel_in_progress: false,
                    cancel_completed: false,
                }),
                None,
            ))
        }
        Err(error) => Err(map_lower_error(error)),
    }
}

fn worker_loop(inner: Arc<Inner>) {
    loop {
        let action = next_worker_action(&inner);
        match action {
            Ok(WorkerAction::Start { turn_id, prompt }) => {
                if let Some(claim_hook) = &inner.claim_hook {
                    claim_hook.pause_after_claim();
                }
                if admit_start(&inner, turn_id) {
                    let result = inner.cloud.start(prompt);
                    handle_start_result(&inner, turn_id, result);
                }
            }
            Ok(WorkerAction::Monitor { turn_id, failures }) => {
                let result = inner.cloud.inspect();
                handle_monitor_result(&inner, turn_id, failures, result);
            }
            Ok(WorkerAction::Cancel { turn_id }) => match inner.cloud.cancel() {
                Ok(lifecycle) => {
                    if commit_worker_cancel(&inner, turn_id, lifecycle).is_err()
                        && let Ok(mut state) = inner.state.lock()
                    {
                        stop_after_lower_failure(&inner, &mut state, turn_id);
                    }
                }
                Err(_) => {
                    if let Ok(mut state) = inner.state.lock() {
                        stop_after_lower_failure(&inner, &mut state, turn_id);
                    }
                }
            },
            Ok(WorkerAction::Stop) | Err(_) => break,
        }
    }
}

fn next_worker_action(inner: &Arc<Inner>) -> Result<WorkerAction, P0SessionError> {
    let mut state = inner
        .state
        .lock()
        .map_err(|_| P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))?;
    loop {
        if state.shutdown_requested {
            stop_worker(inner, &mut state, P0RuntimeStopReason::Shutdown);
            return Ok(WorkerAction::Stop);
        }

        if let Some(turn) = state.current_turn.as_mut()
            && projection_cancel_requested(&turn.projection)
            && !turn.cancel_in_progress
            && !turn.cancel_completed
            && matches!(
                turn.projection,
                P0TurnProjection::Cloud {
                    lifecycle: P0CloudLifecycle::Pending { .. },
                    ..
                } | P0TurnProjection::MonitoringDegraded { .. }
            )
        {
            turn.cancel_in_progress = true;
            let turn_id = turn.turn_id;
            state.work = None;
            return Ok(WorkerAction::Cancel { turn_id });
        }

        match state.work.as_ref() {
            Some(WorkerWork::Start { .. }) => {
                let Some(WorkerWork::Start { turn_id, prompt }) = state.work.take() else {
                    continue;
                };
                if let Some(turn) = state
                    .current_turn
                    .as_mut()
                    .filter(|turn| turn.turn_id == turn_id)
                {
                    turn.projection = P0TurnProjection::Starting {
                        cancel_requested: false,
                    };
                    turn.lower_start_admitted = false;
                } else {
                    stop_worker(inner, &mut state, P0RuntimeStopReason::LowerFailure);
                    return Ok(WorkerAction::Stop);
                }
                return Ok(WorkerAction::Start { turn_id, prompt });
            }
            Some(WorkerWork::Monitor { due, .. }) if *due > Instant::now() => {
                let wait = due.saturating_duration_since(Instant::now());
                let (next, _) = inner
                    .changed
                    .wait_timeout(state, wait)
                    .map_err(|_| P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))?;
                state = next;
            }
            Some(WorkerWork::Monitor { .. }) => {
                let Some(WorkerWork::Monitor {
                    turn_id, failures, ..
                }) = state.work.take()
                else {
                    continue;
                };
                return Ok(WorkerAction::Monitor { turn_id, failures });
            }
            None => {
                state = inner
                    .changed
                    .wait(state)
                    .map_err(|_| P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))?;
            }
        }
    }
}

fn admit_start(inner: &Arc<Inner>, turn_id: TurnId) -> bool {
    let Ok(mut state) = inner.state.lock() else {
        return false;
    };
    if state.shutdown_requested {
        stop_worker(inner, &mut state, P0RuntimeStopReason::Shutdown);
        return false;
    }
    let cancel_requested = state
        .current_turn
        .as_ref()
        .filter(|turn| turn.turn_id == turn_id)
        .is_some_and(|turn| projection_cancel_requested(&turn.projection));
    if cancel_requested {
        if reserve_events(&state, 1).is_err() {
            stop_after_lower_failure(inner, &mut state, turn_id);
            return false;
        }
        state.session_state = P0SessionState::Ready;
        if let Some(turn) = state
            .current_turn
            .as_mut()
            .filter(|turn| turn.turn_id == turn_id)
        {
            turn.projection = P0TurnProjection::CanceledBeforeCloudStart;
            turn.cancel_in_progress = false;
            turn.cancel_completed = true;
        }
        append_event(
            inner,
            &mut state,
            Some(turn_id),
            P0SessionEvent::TurnCanceledBeforeCloudStart,
        );
        inner.changed.notify_all();
        return false;
    }
    let Some(turn) = state
        .current_turn
        .as_mut()
        .filter(|turn| turn.turn_id == turn_id)
    else {
        stop_after_lower_failure(inner, &mut state, turn_id);
        return false;
    };
    turn.lower_start_admitted = true;
    true
}

fn handle_start_result(
    inner: &Arc<Inner>,
    turn_id: TurnId,
    result: Result<CloudLifecycle, LowerLifecycleError>,
) {
    let Ok(mut state) = inner.state.lock() else {
        return;
    };
    let Ok(projected) = result.map(P0CloudLifecycle::from_cloud) else {
        stop_after_lower_failure(inner, &mut state, turn_id);
        return;
    };
    if projected.is_unknown() {
        state.session_state = P0SessionState::RecoveryRequired;
    }
    if reserve_events(&state, if state.shutdown_requested { 2 } else { 1 }).is_err() {
        stop_after_lower_failure(inner, &mut state, turn_id);
        return;
    }
    if apply_projected_lifecycle(&mut state, turn_id, projected.clone(), false).is_err() {
        stop_after_lower_failure(inner, &mut state, turn_id);
        return;
    }
    append_event(
        inner,
        &mut state,
        Some(turn_id),
        P0SessionEvent::LifecycleChanged {
            lifecycle: projected.clone(),
        },
    );

    if state.shutdown_requested {
        state.session_state = P0SessionState::Stopped;
        state.worker_stopped = true;
        append_event(
            inner,
            &mut state,
            Some(turn_id),
            P0SessionEvent::RuntimeStopped {
                reason: P0RuntimeStopReason::Shutdown,
            },
        );
        close_subscribers(&mut state, CLOSE_RUNTIME_STOPPED);
        inner.changed.notify_all();
        return;
    }

    let should_cancel = state.current_turn.as_ref().is_some_and(|turn| {
        turn.turn_id == turn_id
            && projection_cancel_requested(&turn.projection)
            && !turn.cancel_in_progress
            && !turn.cancel_completed
            && projected.is_pending()
    });
    if should_cancel {
        if let Some(turn) = state.current_turn.as_mut() {
            turn.cancel_in_progress = true;
        }
        drop(state);
        match inner.cloud.cancel() {
            Ok(lifecycle) => {
                let _ = commit_worker_cancel(inner, turn_id, lifecycle);
            }
            Err(_) => {
                if let Ok(mut state) = inner.state.lock() {
                    stop_after_lower_failure(inner, &mut state, turn_id);
                }
            }
        }
        return;
    }

    if projected.is_pending() {
        state.work = Some(WorkerWork::Monitor {
            turn_id,
            due: Instant::now() + inner.config.poll_interval,
            failures: 0,
        });
    }
    inner.changed.notify_all();
}

fn handle_monitor_result(
    inner: &Arc<Inner>,
    turn_id: TurnId,
    failures: u32,
    result: Result<CloudLifecycle, LowerLifecycleError>,
) {
    let Ok(mut state) = inner.state.lock() else {
        return;
    };
    if state
        .current_turn
        .as_ref()
        .and_then(|turn| projection_lifecycle(&turn.projection))
        .is_some_and(P0CloudLifecycle::is_terminal)
    {
        state.work = None;
        inner.changed.notify_all();
        return;
    }
    if state.shutdown_requested {
        if let Ok(lifecycle) = result {
            let projected = P0CloudLifecycle::from_cloud(lifecycle);
            if reserve_events(&state, 2).is_ok()
                && apply_projected_lifecycle(&mut state, turn_id, projected.clone(), false).is_ok()
            {
                append_event(
                    inner,
                    &mut state,
                    Some(turn_id),
                    P0SessionEvent::LifecycleChanged {
                        lifecycle: projected,
                    },
                );
            }
        }
        stop_worker(inner, &mut state, P0RuntimeStopReason::Shutdown);
        return;
    }

    match result {
        Ok(lifecycle) => {
            let projected = P0CloudLifecycle::from_cloud(lifecycle);
            let changed = state
                .current_turn
                .as_ref()
                .and_then(|turn| projection_lifecycle(&turn.projection))
                != Some(&projected);
            if changed && reserve_events(&state, 1).is_err() {
                stop_after_lower_failure(inner, &mut state, turn_id);
                return;
            }
            if apply_projected_lifecycle(&mut state, turn_id, projected.clone(), false).is_err() {
                stop_after_lower_failure(inner, &mut state, turn_id);
                return;
            }
            if changed {
                append_event(
                    inner,
                    &mut state,
                    Some(turn_id),
                    P0SessionEvent::LifecycleChanged {
                        lifecycle: projected.clone(),
                    },
                );
            }
            if projected.is_pending() {
                state.work = Some(WorkerWork::Monitor {
                    turn_id,
                    due: Instant::now() + inner.config.poll_interval,
                    failures: 0,
                });
            }
        }
        Err(error) if error.category() == CloudLifecycleErrorCategory::ProviderRead => {
            let Some(last_known_pending) = state
                .current_turn
                .as_ref()
                .and_then(|turn| projection_lifecycle(&turn.projection))
                .filter(|lifecycle| lifecycle.is_pending())
                .cloned()
            else {
                stop_after_lower_failure(inner, &mut state, turn_id);
                return;
            };
            let already_degraded = state.session_state == P0SessionState::MonitoringDegraded;
            if !already_degraded && reserve_events(&state, 1).is_err() {
                stop_after_lower_failure(inner, &mut state, turn_id);
                return;
            }
            let cancel_requested = state
                .current_turn
                .as_ref()
                .is_some_and(|turn| projection_cancel_requested(&turn.projection));
            state.session_state = P0SessionState::MonitoringDegraded;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.projection = P0TurnProjection::MonitoringDegraded {
                    operation_id: last_known_pending.operation_id(),
                    last_known_pending,
                    cancel_requested,
                };
            }
            if !already_degraded {
                append_event(
                    inner,
                    &mut state,
                    Some(turn_id),
                    P0SessionEvent::MonitoringDegraded,
                );
            }
            state.work = Some(WorkerWork::Monitor {
                turn_id,
                due: Instant::now() + backoff(inner.config.poll_interval, failures + 1),
                failures: failures.saturating_add(1),
            });
        }
        Err(_) => stop_after_lower_failure(inner, &mut state, turn_id),
    }
    inner.changed.notify_all();
}

fn commit_worker_cancel(
    inner: &Arc<Inner>,
    turn_id: TurnId,
    lifecycle: CloudLifecycle,
) -> Result<(), P0SessionError> {
    let projected = P0CloudLifecycle::from_cloud(lifecycle);
    let mut state = inner
        .state
        .lock()
        .map_err(|_| P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))?;
    reserve_events(&state, 1)?;
    apply_projected_lifecycle(&mut state, turn_id, projected.clone(), true)?;
    if let Some(turn) = state.current_turn.as_mut() {
        turn.cancel_in_progress = false;
        turn.cancel_completed = true;
    }
    append_event(
        inner,
        &mut state,
        Some(turn_id),
        P0SessionEvent::LifecycleChanged {
            lifecycle: projected,
        },
    );
    inner.changed.notify_all();
    Ok(())
}

fn apply_projected_lifecycle(
    state: &mut RuntimeState,
    turn_id: TurnId,
    projected: P0CloudLifecycle,
    cancel_completed: bool,
) -> Result<(), P0SessionError> {
    let invalid_transition = state
        .current_turn
        .as_ref()
        .filter(|turn| turn.turn_id == turn_id)
        .and_then(|turn| projection_lifecycle(&turn.projection))
        .is_some_and(|current| {
            current.operation_id() != projected.operation_id()
                || !lifecycle_transition_allowed(current, &projected)
        });
    if invalid_transition {
        return Err(P0SessionError::for_operation(
            P0SessionErrorCategory::LowerConflict,
            projected.operation_id(),
        ));
    }
    let turn = state
        .current_turn
        .as_mut()
        .filter(|turn| turn.turn_id == turn_id)
        .ok_or_else(|| P0SessionError::new(P0SessionErrorCategory::WrongState))?;
    let cancel_requested = projection_cancel_requested(&turn.projection);
    state.session_state = state_for_lifecycle(&projected);
    if !projected.is_pending() {
        state.work = None;
    }
    turn.projection = P0TurnProjection::Cloud {
        lifecycle: projected,
        cancel_requested,
    };
    turn.cancel_completed |= cancel_completed;
    Ok(())
}

fn lifecycle_transition_allowed(current: &P0CloudLifecycle, next: &P0CloudLifecycle) -> bool {
    if current == next {
        return true;
    }
    match current {
        P0CloudLifecycle::Submitting { .. } => matches!(
            next,
            P0CloudLifecycle::FailedBeforeSubmit { .. }
                | P0CloudLifecycle::OutcomeUnknown { .. }
                | P0CloudLifecycle::Pending { .. }
        ),
        P0CloudLifecycle::OutcomeUnknown { .. } => matches!(
            next,
            P0CloudLifecycle::Pending { .. }
                | P0CloudLifecycle::Ready { .. }
                | P0CloudLifecycle::Applied { .. }
                | P0CloudLifecycle::ProviderError { .. }
                | P0CloudLifecycle::AbandonedUnknown { .. }
        ),
        P0CloudLifecycle::Pending { .. } => {
            current.task_id() == next.task_id()
                && matches!(
                    next,
                    P0CloudLifecycle::Pending { .. }
                        | P0CloudLifecycle::Ready { .. }
                        | P0CloudLifecycle::Applied { .. }
                        | P0CloudLifecycle::ProviderError { .. }
                        | P0CloudLifecycle::CanceledLocally { .. }
                )
        }
        P0CloudLifecycle::FailedBeforeSubmit { .. }
        | P0CloudLifecycle::Ready { .. }
        | P0CloudLifecycle::Applied { .. }
        | P0CloudLifecycle::ProviderError { .. }
        | P0CloudLifecycle::CanceledLocally { .. }
        | P0CloudLifecycle::AbandonedUnknown { .. } => false,
    }
}

fn state_for_lifecycle(lifecycle: &P0CloudLifecycle) -> P0SessionState {
    if lifecycle.is_unknown() {
        P0SessionState::RecoveryRequired
    } else if lifecycle.is_pending() || matches!(lifecycle, P0CloudLifecycle::Submitting { .. }) {
        P0SessionState::Running
    } else {
        P0SessionState::Ready
    }
}

fn backoff(base: Duration, failures: u32) -> Duration {
    let exponent = failures.min(8);
    let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
    base.checked_mul(multiplier)
        .unwrap_or(MAX_BACKOFF)
        .min(MAX_BACKOFF)
}

fn stop_after_lower_failure(inner: &Arc<Inner>, state: &mut RuntimeState, turn_id: TurnId) {
    if let Some(turn) = state
        .current_turn
        .as_mut()
        .filter(|turn| turn.turn_id == turn_id)
    {
        turn.projection = P0TurnProjection::StoppedAfterLowerFailure;
    }
    stop_worker(inner, state, P0RuntimeStopReason::LowerFailure);
}

fn stop_worker(inner: &Arc<Inner>, state: &mut RuntimeState, reason: P0RuntimeStopReason) {
    if state.worker_stopped {
        return;
    }
    if reason == P0RuntimeStopReason::Shutdown {
        let start_not_admitted = state.current_turn.as_ref().is_some_and(|turn| {
            matches!(
                turn.projection,
                P0TurnProjection::Queued | P0TurnProjection::Starting { .. }
            ) && !turn.lower_start_admitted
        });
        if start_not_admitted && let Some(turn) = state.current_turn.as_mut() {
            turn.projection = P0TurnProjection::StoppedBeforeCloudStart;
        }
    }
    state.work = None;
    state.session_state = P0SessionState::Stopped;
    state.worker_stopped = true;
    if reserve_events(state, 1).is_ok() {
        let turn_id = state.current_turn.as_ref().map(|turn| turn.turn_id);
        append_event(
            inner,
            state,
            turn_id,
            P0SessionEvent::RuntimeStopped { reason },
        );
    }
    close_subscribers(state, CLOSE_RUNTIME_STOPPED);
    inner.changed.notify_all();
}

fn reserve_events(state: &RuntimeState, count: u64) -> Result<(), P0SessionError> {
    state
        .high_water_seq
        .value()
        .checked_add(count)
        .map(|_| ())
        .ok_or_else(|| P0SessionError::new(P0SessionErrorCategory::SequenceExhausted))
}

fn append_event(
    inner: &Arc<Inner>,
    state: &mut RuntimeState,
    turn_id: Option<TurnId>,
    payload: P0SessionEvent,
) {
    let seq = EventSeq::new(state.high_water_seq.value() + 1);
    state.high_water_seq = seq;
    let envelope = P0SessionEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        session_id: inner.identity.session_id,
        seq,
        turn_id,
        payload,
    };
    state.history.push_back(envelope.clone());
    while state.history.len() > inner.config.history_capacity {
        state.history.pop_front();
    }

    let mut remove = Vec::new();
    for (subscriber_id, subscriber) in &state.subscribers {
        match subscriber.sender.try_send(envelope.clone()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                subscriber
                    .close_reason
                    .store(CLOSE_LAGGED, Ordering::Release);
                remove.push(*subscriber_id);
            }
            Err(TrySendError::Disconnected(_)) => {
                subscriber
                    .close_reason
                    .store(CLOSE_NORMAL, Ordering::Release);
                remove.push(*subscriber_id);
            }
        }
    }
    for subscriber_id in remove {
        state.subscribers.remove(&subscriber_id);
    }
}

fn close_subscribers(state: &mut RuntimeState, reason: u8) {
    for subscriber in state.subscribers.values() {
        subscriber.close_reason.store(reason, Ordering::Release);
    }
    state.subscribers.clear();
}

fn snapshot_from(inner: &Inner, state: &RuntimeState) -> P0SessionSnapshot {
    P0SessionSnapshot {
        identity: inner.identity,
        state: state.session_state,
        current_turn: state.current_turn.as_ref().map(|turn| P0TurnSnapshot {
            turn_id: turn.turn_id,
            projection: turn.projection.clone(),
        }),
        high_water_seq: state.high_water_seq,
    }
}

fn require_running_worker(state: &RuntimeState) -> Result<(), P0SessionError> {
    if state.worker_stopped || state.shutdown_requested {
        Err(P0SessionError::new(P0SessionErrorCategory::RuntimeStopped))
    } else {
        Ok(())
    }
}

fn map_lower_error(error: LowerLifecycleError) -> P0SessionError {
    P0SessionError::from_cloud_lifecycle(error.category(), error.operation_id())
}

fn require_current_operation(
    state: &RuntimeState,
    operation_id: CloudSubmitOperationId,
) -> Result<(), P0SessionError> {
    match current_operation(state) {
        Some(current) if current == operation_id => Ok(()),
        _ => Err(P0SessionError::for_operation(
            P0SessionErrorCategory::WrongOperation,
            operation_id,
        )),
    }
}

fn current_operation(state: &RuntimeState) -> Option<CloudSubmitOperationId> {
    state
        .current_turn
        .as_ref()
        .and_then(|turn| projection_operation(&turn.projection))
}

fn projection_operation(projection: &P0TurnProjection) -> Option<CloudSubmitOperationId> {
    match projection {
        P0TurnProjection::Cloud { lifecycle, .. } => Some(lifecycle.operation_id()),
        P0TurnProjection::MonitoringDegraded { operation_id, .. } => Some(*operation_id),
        P0TurnProjection::Queued
        | P0TurnProjection::Starting { .. }
        | P0TurnProjection::CanceledBeforeCloudStart
        | P0TurnProjection::StoppedBeforeCloudStart
        | P0TurnProjection::StoppedAfterLowerFailure => None,
    }
}

fn projection_lifecycle(projection: &P0TurnProjection) -> Option<&P0CloudLifecycle> {
    match projection {
        P0TurnProjection::Cloud { lifecycle, .. } => Some(lifecycle),
        P0TurnProjection::MonitoringDegraded {
            last_known_pending, ..
        } => Some(last_known_pending),
        _ => None,
    }
}

fn projection_cancel_requested(projection: &P0TurnProjection) -> bool {
    match projection {
        P0TurnProjection::Starting { cancel_requested }
        | P0TurnProjection::Cloud {
            cancel_requested, ..
        }
        | P0TurnProjection::MonitoringDegraded {
            cancel_requested, ..
        } => *cancel_requested,
        _ => false,
    }
}

fn set_projection_cancel_requested(projection: &mut P0TurnProjection) {
    match projection {
        P0TurnProjection::Starting { cancel_requested }
        | P0TurnProjection::Cloud {
            cancel_requested, ..
        }
        | P0TurnProjection::MonitoringDegraded {
            cancel_requested, ..
        } => *cancel_requested = true,
        _ => {}
    }
}
