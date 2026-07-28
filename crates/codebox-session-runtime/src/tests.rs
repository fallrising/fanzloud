use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use codebox_agent_codex::{
    CloudCapture, CloudDiff, CloudDiffReadErrorCategory, CloudLifecycle,
    CloudLifecycleErrorCategory, CloudPrompt, CloudSubmitOperationId, CloudTaskId,
    DuplicateRiskAcknowledgement, UnknownSubmitDecision, decode_cloud_diff,
};
use codebox_domain::EventSeq;
use proptest::prelude::*;

use crate::runtime::{
    LowerDiffError, LowerLifecycleError, P0CloudControl, P0SessionRuntime, WorkerClaimHook,
};
use crate::{
    P0Actor, P0CloudLifecycle, P0LiveReceiveError, P0RecoveryCandidates, P0SessionConfig,
    P0SessionConfigField, P0SessionErrorCategory, P0SessionEvent, P0SessionState, P0TurnProjection,
};

const WAIT_LIMIT: Duration = Duration::from_secs(3);

struct FakeCloud {
    state: Mutex<FakeCloudState>,
    changed: Condvar,
}

struct FakeCloudState {
    inspect_results: VecDeque<Result<CloudLifecycle, LowerLifecycleError>>,
    start_results: VecDeque<Result<CloudLifecycle, LowerLifecycleError>>,
    cancel_results: VecDeque<Result<CloudLifecycle, LowerLifecycleError>>,
    reconcile_results: VecDeque<Result<P0RecoveryCandidates, LowerLifecycleError>>,
    resolve_results: VecDeque<Result<CloudLifecycle, LowerLifecycleError>>,
    diff_results: VecDeque<Result<CloudDiff, LowerDiffError>>,
    inspect_calls: usize,
    inspect_times: Vec<Instant>,
    start_calls: usize,
    cancel_calls: usize,
    reconcile_calls: usize,
    resolve_calls: usize,
    diff_calls: usize,
    prompt_bytes_seen: Option<usize>,
    block_start: bool,
    start_entered: bool,
    release_start: bool,
    block_inspect: bool,
    inspect_entered: bool,
    release_inspect: bool,
}

impl FakeCloud {
    fn new(initial: Result<CloudLifecycle, LowerLifecycleError>) -> Self {
        Self {
            state: Mutex::new(FakeCloudState {
                inspect_results: VecDeque::from([initial]),
                start_results: VecDeque::new(),
                cancel_results: VecDeque::new(),
                reconcile_results: VecDeque::new(),
                resolve_results: VecDeque::new(),
                diff_results: VecDeque::new(),
                inspect_calls: 0,
                inspect_times: Vec::new(),
                start_calls: 0,
                cancel_calls: 0,
                reconcile_calls: 0,
                resolve_calls: 0,
                diff_calls: 0,
                prompt_bytes_seen: None,
                block_start: false,
                start_entered: false,
                release_start: false,
                block_inspect: false,
                inspect_entered: false,
                release_inspect: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn empty() -> Self {
        Self::new(Err(lower_error(
            CloudLifecycleErrorCategory::NoCurrentOperation,
            None,
        )))
    }

    fn push_inspect(&self, result: Result<CloudLifecycle, LowerLifecycleError>) {
        lock(&self.state).inspect_results.push_back(result);
    }

    fn push_start(&self, result: Result<CloudLifecycle, LowerLifecycleError>) {
        lock(&self.state).start_results.push_back(result);
    }

    fn push_cancel(&self, result: Result<CloudLifecycle, LowerLifecycleError>) {
        lock(&self.state).cancel_results.push_back(result);
    }

    fn push_reconcile(&self, result: Result<P0RecoveryCandidates, LowerLifecycleError>) {
        lock(&self.state).reconcile_results.push_back(result);
    }

    fn push_resolve(&self, result: Result<CloudLifecycle, LowerLifecycleError>) {
        lock(&self.state).resolve_results.push_back(result);
    }

    fn push_diff(&self, result: Result<CloudDiff, LowerDiffError>) {
        lock(&self.state).diff_results.push_back(result);
    }

    fn block_start(&self) {
        lock(&self.state).block_start = true;
    }

    fn wait_for_start(&self) {
        let deadline = Instant::now() + WAIT_LIMIT;
        let mut state = lock(&self.state);
        while !state.start_entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "fake Cloud start did not begin");
            let waited = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = waited.0;
        }
    }

    fn release_start(&self) {
        let mut state = lock(&self.state);
        state.release_start = true;
        self.changed.notify_all();
    }

    fn counts(&self) -> (usize, usize, usize, usize, usize) {
        let state = lock(&self.state);
        (
            state.start_calls,
            state.cancel_calls,
            state.reconcile_calls,
            state.resolve_calls,
            state.diff_calls,
        )
    }

    fn inspect_times(&self) -> Vec<Instant> {
        lock(&self.state).inspect_times.clone()
    }

    fn block_inspect(&self) {
        lock(&self.state).block_inspect = true;
    }

    fn wait_for_inspect(&self) {
        let deadline = Instant::now() + WAIT_LIMIT;
        let mut state = lock(&self.state);
        while !state.inspect_entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "fake Cloud inspect did not begin");
            let waited = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = waited.0;
        }
    }

    fn release_inspect(&self) {
        let mut state = lock(&self.state);
        state.release_inspect = true;
        self.changed.notify_all();
    }
}

impl P0CloudControl for FakeCloud {
    fn inspect(&self) -> Result<CloudLifecycle, LowerLifecycleError> {
        let mut state = lock(&self.state);
        state.inspect_calls += 1;
        state.inspect_times.push(Instant::now());
        if state.block_inspect {
            state.inspect_entered = true;
            self.changed.notify_all();
            while !state.release_inspect {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }
        state
            .inspect_results
            .pop_front()
            .unwrap_or_else(|| Err(lower_error(CloudLifecycleErrorCategory::ProviderRead, None)))
    }

    fn start(&self, prompt: CloudPrompt) -> Result<CloudLifecycle, LowerLifecycleError> {
        let mut state = lock(&self.state);
        state.start_calls += 1;
        state.prompt_bytes_seen = Some(prompt.as_str().len());
        state.start_entered = true;
        self.changed.notify_all();
        while state.block_start && !state.release_start {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        state
            .start_results
            .pop_front()
            .unwrap_or_else(|| Err(lower_error(CloudLifecycleErrorCategory::LowerRunner, None)))
    }

    fn cancel(&self) -> Result<CloudLifecycle, LowerLifecycleError> {
        let mut state = lock(&self.state);
        state.cancel_calls += 1;
        if state.block_start && state.start_entered && !state.release_start {
            return Err(lower_error(
                CloudLifecycleErrorCategory::NoCurrentOperation,
                None,
            ));
        }
        state.cancel_results.pop_front().unwrap_or_else(|| {
            Err(lower_error(
                CloudLifecycleErrorCategory::NoCurrentOperation,
                None,
            ))
        })
    }

    fn reconcile_unknown(&self) -> Result<P0RecoveryCandidates, LowerLifecycleError> {
        let mut state = lock(&self.state);
        state.reconcile_calls += 1;
        state
            .reconcile_results
            .pop_front()
            .unwrap_or_else(|| Err(lower_error(CloudLifecycleErrorCategory::WrongState, None)))
    }

    fn resolve_unknown(
        &self,
        _operation_id: CloudSubmitOperationId,
        _decision: UnknownSubmitDecision,
    ) -> Result<CloudLifecycle, LowerLifecycleError> {
        let mut state = lock(&self.state);
        state.resolve_calls += 1;
        state
            .resolve_results
            .pop_front()
            .unwrap_or_else(|| Err(lower_error(CloudLifecycleErrorCategory::WrongState, None)))
    }

    fn read_diff(&self) -> Result<CloudDiff, LowerDiffError> {
        let mut state = lock(&self.state);
        state.diff_calls += 1;
        state
            .diff_results
            .pop_front()
            .unwrap_or_else(|| Err(LowerDiffError::new(CloudDiffReadErrorCategory::InvalidDiff)))
    }
}

#[test]
fn p0_session_start_commits_intent_before_cloud_call() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.block_start();
    fake.push_start(Ok(pending(operation_id, "task_intent")));
    let runtime = runtime_with(Arc::clone(&fake));

    let receipt = runtime.start_turn(prompt("repair the parser")).unwrap();
    fake.wait_for_start();

    let snapshot = runtime.snapshot().unwrap();
    assert_eq!(snapshot.high_water_seq, EventSeq::new(1));
    assert_eq!(snapshot.current_turn.unwrap().turn_id, receipt.turn_id);
    let subscription = runtime
        .subscribe(runtime.identity().session_id, EventSeq::initial())
        .unwrap();
    assert!(matches!(
        subscription.replay()[0].payload,
        P0SessionEvent::TurnAccepted
    ));

    fake.release_start();
    wait_for_state(&runtime, P0SessionState::Running);
}

#[test]
fn p0_session_allows_only_one_mutating_turn() {
    let fake = Arc::new(FakeCloud::empty());
    fake.block_start();
    fake.push_start(Ok(pending(CloudSubmitOperationId::new(), "task_single")));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt("first")).unwrap();
    fake.wait_for_start();

    let error = runtime.start_turn(prompt("second")).unwrap_err();
    assert_eq!(error.category(), P0SessionErrorCategory::TurnAlreadyRunning);
    assert_eq!(fake.counts().0, 1);
    fake.release_start();
}

#[test]
fn p0_session_projects_monotonic_cloud_lifecycle() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.push_start(Ok(pending(operation_id, "task_monotonic")));
    fake.push_inspect(Ok(ready(operation_id, "task_monotonic")));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt("monotonic")).unwrap();
    wait_for_state(&runtime, P0SessionState::Ready);

    let subscription = runtime
        .subscribe(runtime.identity().session_id, EventSeq::initial())
        .unwrap();
    let lifecycle: Vec<_> = subscription
        .replay()
        .iter()
        .filter_map(|event| match &event.payload {
            P0SessionEvent::LifecycleChanged { lifecycle } => Some(lifecycle),
            _ => None,
        })
        .collect();
    assert!(matches!(lifecycle[0], P0CloudLifecycle::Pending { .. }));
    assert!(matches!(lifecycle[1], P0CloudLifecycle::Ready { .. }));
}

#[test]
fn p0_session_polling_is_rate_limited_and_stops_at_terminal() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.push_start(Ok(pending(operation_id, "task_rate")));
    fake.push_inspect(Ok(pending(operation_id, "task_rate")));
    fake.push_inspect(Ok(ready(operation_id, "task_rate")));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt("rate")).unwrap();
    wait_for_state(&runtime, P0SessionState::Ready);

    let times = fake.inspect_times();
    assert_eq!(times.len(), 3);
    assert!(times[1].duration_since(times[0]) >= Duration::from_millis(200));
    assert!(times[2].duration_since(times[1]) >= Duration::from_millis(200));
    thread::sleep(Duration::from_millis(350));
    assert_eq!(fake.inspect_times().len(), 3);
}

#[test]
fn p0_session_subscriber_drop_does_not_cancel_turn() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.block_start();
    fake.push_start(Ok(pending(operation_id, "task_drop")));
    let runtime = runtime_with(Arc::clone(&fake));
    let subscription = runtime
        .subscribe(runtime.identity().session_id, EventSeq::initial())
        .unwrap();
    drop(subscription);
    runtime.start_turn(prompt("drop subscriber")).unwrap();
    fake.wait_for_start();
    assert_eq!(fake.counts().1, 0);
    fake.release_start();
}

#[test]
fn p0_session_explicit_cancel_targets_current_turn() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.block_start();
    fake.push_start(Ok(pending(operation_id, "task_cancel")));
    fake.push_cancel(Ok(CloudLifecycle::CanceledLocally {
        operation_id,
        task_id: Some(task("task_cancel")),
        provider_may_continue: true,
    }));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt("cancel")).unwrap();
    fake.wait_for_start();
    let snapshot = thread::scope(|scope| {
        let cancel = scope.spawn(|| runtime.cancel_turn(P0Actor::Operator));
        thread::sleep(Duration::from_millis(40));
        fake.release_start();
        cancel.join().unwrap().unwrap()
    });
    assert_eq!(snapshot.state, P0SessionState::Ready);
    assert!(matches!(
        snapshot.current_turn.unwrap().projection,
        P0TurnProjection::Cloud {
            lifecycle: P0CloudLifecycle::CanceledLocally {
                provider_may_continue: true,
                ..
            },
            ..
        }
    ));
    assert!(fake.counts().1 >= 2);
}

#[test]
fn p0_session_terminal_cancel_wins_over_stale_monitor_result() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.push_start(Ok(pending(operation_id, "task_terminal_wins")));
    fake.push_inspect(Ok(pending(operation_id, "task_terminal_wins")));
    fake.push_cancel(Ok(CloudLifecycle::CanceledLocally {
        operation_id,
        task_id: Some(task("task_terminal_wins")),
        provider_may_continue: true,
    }));
    let runtime = runtime_with(Arc::clone(&fake));
    fake.block_inspect();
    runtime.start_turn(prompt("terminal wins")).unwrap();
    fake.wait_for_inspect();

    let canceled = runtime.cancel_turn(P0Actor::Operator).unwrap();
    assert_eq!(canceled.state, P0SessionState::Ready);
    fake.release_inspect();
    thread::sleep(Duration::from_millis(100));

    let snapshot = runtime.snapshot().unwrap();
    assert_eq!(snapshot.state, P0SessionState::Ready);
    assert!(matches!(
        snapshot.current_turn.unwrap().projection,
        P0TurnProjection::Cloud {
            lifecycle: P0CloudLifecycle::CanceledLocally { .. },
            ..
        }
    ));
}

#[test]
fn p0_session_unknown_requires_operation_bound_decision() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.push_start(Ok(CloudLifecycle::OutcomeUnknown { operation_id }));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt("unknown")).unwrap();
    wait_for_state(&runtime, P0SessionState::RecoveryRequired);

    let wrong = CloudSubmitOperationId::new();
    let error = runtime
        .resolve_unknown(
            P0Actor::Operator,
            wrong,
            UnknownSubmitDecision::AbandonAfterReconciliation(
                DuplicateRiskAcknowledgement::for_operation(wrong),
            ),
        )
        .unwrap_err();
    assert_eq!(error.category(), P0SessionErrorCategory::WrongOperation);
    assert_eq!(fake.counts().3, 0);
}

#[test]
fn p0_session_recovery_resumes_pending_monitoring() {
    let operation_id = CloudSubmitOperationId::new();
    let task_id = task("task_recover");
    let fake = Arc::new(FakeCloud::empty());
    fake.push_start(Ok(CloudLifecycle::OutcomeUnknown { operation_id }));
    fake.push_reconcile(Ok(P0RecoveryCandidates::for_test(
        operation_id,
        vec![task_id.clone()],
        true,
    )));
    fake.push_resolve(Ok(CloudLifecycle::Pending {
        operation_id,
        task_id: task_id.clone(),
    }));
    fake.push_inspect(Ok(CloudLifecycle::Ready {
        operation_id,
        task_id: task_id.clone(),
    }));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt("recover")).unwrap();
    wait_for_state(&runtime, P0SessionState::RecoveryRequired);
    let candidates = runtime.reconcile_unknown(P0Actor::Operator).unwrap();
    assert_eq!(candidates.task_ids[0].as_str(), task_id.as_str());
    runtime
        .resolve_unknown(
            P0Actor::Operator,
            operation_id,
            UnknownSubmitDecision::AdoptListedTask(task_id),
        )
        .unwrap();
    wait_for_state(&runtime, P0SessionState::Ready);
    assert_eq!(fake.counts().2, 1);
    assert_eq!(fake.counts().3, 1);
}

#[test]
fn p0_session_diff_read_preserves_managed_state() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::new(Ok(ready(operation_id, "task_diff"))));
    fake.push_diff(Ok(diff("diff --git a/a b/a\n+canary-diff\n")));
    let runtime = runtime_with(Arc::clone(&fake));
    let before = runtime.snapshot().unwrap();
    let returned = runtime.read_diff().unwrap();
    let after = runtime.snapshot().unwrap();
    assert_eq!(returned.as_str(), "diff --git a/a b/a\n+canary-diff\n");
    assert_eq!(before, after);
    assert_eq!(fake.counts().4, 1);
}

#[test]
fn p0_session_events_are_gap_free_and_bounded() {
    let fake = Arc::new(FakeCloud::empty());
    let runtime = runtime_without_worker(Arc::clone(&fake));
    for index in 0..40 {
        runtime
            .start_turn(prompt(&format!("turn {index}")))
            .unwrap();
        runtime.cancel_turn(P0Actor::Operator).unwrap();
    }
    let snapshot = runtime.snapshot().unwrap();
    assert_eq!(snapshot.high_water_seq, EventSeq::new(80));
    let gap = runtime
        .subscribe(runtime.identity().session_id, EventSeq::initial())
        .unwrap_err();
    assert_eq!(gap.category(), P0SessionErrorCategory::HistoryGap);
    assert_eq!(gap.oldest_available(), Some(EventSeq::new(17)));
    let retained = runtime
        .subscribe(runtime.identity().session_id, EventSeq::new(16))
        .unwrap();
    assert_eq!(retained.replay().len(), 64);
    for pair in retained.replay().windows(2) {
        assert_eq!(pair[1].seq.value(), pair[0].seq.value() + 1);
    }
}

#[test]
fn p0_session_slow_subscriber_isolated_from_publication() {
    let fake = Arc::new(FakeCloud::empty());
    let runtime = runtime_without_worker(Arc::clone(&fake));
    let (_, _, live) = runtime
        .subscribe(runtime.identity().session_id, EventSeq::initial())
        .unwrap()
        .into_parts();
    for index in 0..6 {
        runtime
            .start_turn(prompt(&format!("slow {index}")))
            .unwrap();
        runtime.cancel_turn(P0Actor::Operator).unwrap();
    }
    assert_eq!(
        runtime.snapshot().unwrap().high_water_seq,
        EventSeq::new(12)
    );
    for _ in 0..8 {
        live.try_recv().unwrap();
    }
    assert_eq!(live.try_recv(), Err(P0LiveReceiveError::Lagged));
    assert_eq!(fake.counts().1, 0);
}

#[test]
fn p0_session_shutdown_joins_worker_without_provider_cancel() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.block_start();
    fake.push_start(Ok(pending(operation_id, "task_shutdown")));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt("shutdown admitted")).unwrap();
    fake.wait_for_start();
    thread::scope(|scope| {
        let shutdown = scope.spawn(|| runtime.shutdown());
        thread::sleep(Duration::from_millis(40));
        fake.release_start();
        shutdown.join().unwrap().unwrap();
    });
    assert_eq!(runtime.snapshot().unwrap().state, P0SessionState::Stopped);
    assert_eq!(fake.counts().1, 0);
}

#[test]
fn p0_session_startup_pending_arms_monitoring() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::new(Ok(pending(operation_id, "task_restart"))));
    fake.push_inspect(Ok(ready(operation_id, "task_restart")));
    let runtime = runtime_with(Arc::clone(&fake));
    assert_eq!(runtime.snapshot().unwrap().state, P0SessionState::Running);
    wait_for_state(&runtime, P0SessionState::Ready);
    assert_eq!(fake.inspect_times().len(), 2);
}

#[test]
fn p0_session_startup_provider_read_fails_closed() {
    let fake = Arc::new(FakeCloud::new(Err(lower_error(
        CloudLifecycleErrorCategory::ProviderRead,
        Some(CloudSubmitOperationId::new()),
    ))));
    let error = P0SessionRuntime::with_cloud(fake, test_config()).unwrap_err();
    assert_eq!(error.category(), P0SessionErrorCategory::CloudLifecycle);
    assert_eq!(
        error.cloud_lifecycle(),
        Some(CloudLifecycleErrorCategory::ProviderRead)
    );
}

#[test]
fn p0_session_invalid_config_identifies_safe_field() {
    let cases = [
        (
            P0SessionConfig::try_new(Duration::from_millis(1), 64, 8, 8),
            P0SessionConfigField::PollInterval,
        ),
        (
            P0SessionConfig::try_new(Duration::from_millis(250), 1, 8, 8),
            P0SessionConfigField::HistoryCapacity,
        ),
        (
            P0SessionConfig::try_new(Duration::from_millis(250), 64, 0, 8),
            P0SessionConfigField::MaxSubscribers,
        ),
        (
            P0SessionConfig::try_new(Duration::from_millis(250), 64, 8, 1),
            P0SessionConfigField::LiveCapacity,
        ),
    ];
    for (result, field) in cases {
        let error = result.unwrap_err();
        assert_eq!(error.category(), P0SessionErrorCategory::InvalidConfig);
        assert_eq!(error.config_field(), Some(field));
    }
}

#[test]
fn p0_session_monitoring_degraded_retains_pending_and_recovers() {
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.push_start(Ok(pending(operation_id, "task_degraded")));
    fake.push_inspect(Err(lower_error(
        CloudLifecycleErrorCategory::ProviderRead,
        Some(operation_id),
    )));
    fake.push_inspect(Ok(ready(operation_id, "task_degraded")));
    let runtime = runtime_with(fake);
    runtime.start_turn(prompt("degraded")).unwrap();
    wait_for_state(&runtime, P0SessionState::MonitoringDegraded);
    assert!(matches!(
        runtime.snapshot().unwrap().current_turn.unwrap().projection,
        P0TurnProjection::MonitoringDegraded {
            last_known_pending: P0CloudLifecycle::Pending { .. },
            ..
        }
    ));
    wait_for_state(&runtime, P0SessionState::Ready);
}

#[test]
fn p0_session_nonprojectable_lower_failure_stops_without_retry() {
    let fake = Arc::new(FakeCloud::empty());
    fake.push_start(Err(lower_error(
        CloudLifecycleErrorCategory::LedgerUnavailable,
        Some(CloudSubmitOperationId::new()),
    )));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt("fail closed")).unwrap();
    wait_for_state(&runtime, P0SessionState::Stopped);
    assert!(matches!(
        runtime.snapshot().unwrap().current_turn.unwrap().projection,
        P0TurnProjection::StoppedAfterLowerFailure
    ));
    assert_eq!(fake.counts().0, 1);
}

#[test]
fn p0_session_redacts_prompt_diff_and_lower_errors() {
    let canary_prompt = "PROMPT_CANARY_7f25";
    let operation_id = CloudSubmitOperationId::new();
    let fake = Arc::new(FakeCloud::empty());
    fake.block_start();
    fake.push_start(Ok(pending(operation_id, "task_redact")));
    let runtime = runtime_with(Arc::clone(&fake));
    runtime.start_turn(prompt(canary_prompt)).unwrap();
    fake.wait_for_start();
    let json = serde_json::to_string(
        runtime
            .subscribe(runtime.identity().session_id, EventSeq::initial())
            .unwrap()
            .replay(),
    )
    .unwrap();
    assert!(!json.contains(canary_prompt));
    assert!(!format!("{runtime:?}").contains(canary_prompt));
    fake.release_start();

    let diff_fake = Arc::new(FakeCloud::new(Ok(ready(
        CloudSubmitOperationId::new(),
        "task_redact_diff",
    ))));
    diff_fake.push_diff(Err(LowerDiffError::new(
        CloudDiffReadErrorCategory::ProviderDrift,
    )));
    let diff_runtime = runtime_with(diff_fake);
    let error = diff_runtime.read_diff().unwrap_err();
    assert_eq!(
        error.cloud_diff(),
        Some(CloudDiffReadErrorCategory::ProviderDrift)
    );
    assert!(!format!("{error:?}").contains("provider raw canary"));
}

proptest! {
    #[test]
    fn p0_session_model_preserves_single_turn_and_sequence_invariants(commands in prop::collection::vec(any::<bool>(), 1..80)) {
        let fake = Arc::new(FakeCloud::empty());
        let runtime = runtime_without_worker(fake);
        let mut active = false;
        let mut expected_seq = 0_u64;
        for start in commands {
            if start && !active {
                runtime.start_turn(prompt("model")).unwrap();
                active = true;
                expected_seq += 1;
            } else if !start && active {
                runtime.cancel_turn(P0Actor::Operator).unwrap();
                active = false;
                expected_seq += 1;
            } else if start {
                prop_assert_eq!(
                    runtime.start_turn(prompt("duplicate")).unwrap_err().category(),
                    P0SessionErrorCategory::TurnAlreadyRunning
                );
            }
            let snapshot = runtime.snapshot().unwrap();
            prop_assert_eq!(snapshot.high_water_seq.value(), expected_seq);
            prop_assert_eq!(snapshot.state == P0SessionState::Running, active);
        }
    }
}

#[test]
fn p0_session_cancel_or_shutdown_before_worker_claim_never_starts_cloud() {
    let cancel_fake = Arc::new(FakeCloud::empty());
    let cancel_hook = Arc::new(WorkerClaimHook::new());
    let cancel_runtime = P0SessionRuntime::with_cloud_claim_hook(
        Arc::clone(&cancel_fake) as Arc<dyn P0CloudControl>,
        test_config(),
        Arc::clone(&cancel_hook),
    )
    .unwrap();
    cancel_runtime.start_turn(prompt("cancel queued")).unwrap();
    cancel_hook.wait_claimed();
    let canceled = thread::scope(|scope| {
        let cancel = scope.spawn(|| cancel_runtime.cancel_turn(P0Actor::Operator));
        thread::sleep(Duration::from_millis(40));
        cancel_hook.release();
        cancel.join().unwrap().unwrap()
    });
    assert_eq!(canceled.state, P0SessionState::Ready);
    assert_eq!(cancel_fake.counts().0, 0);

    let shutdown_fake = Arc::new(FakeCloud::empty());
    let shutdown_hook = Arc::new(WorkerClaimHook::new());
    let shutdown_runtime = P0SessionRuntime::with_cloud_claim_hook(
        Arc::clone(&shutdown_fake) as Arc<dyn P0CloudControl>,
        test_config(),
        Arc::clone(&shutdown_hook),
    )
    .unwrap();
    shutdown_runtime
        .start_turn(prompt("shutdown queued"))
        .unwrap();
    shutdown_hook.wait_claimed();
    thread::scope(|scope| {
        let shutdown = scope.spawn(|| shutdown_runtime.shutdown());
        thread::sleep(Duration::from_millis(40));
        shutdown_hook.release();
        shutdown.join().unwrap().unwrap();
    });
    assert_eq!(shutdown_fake.counts().0, 0);
    assert!(matches!(
        shutdown_runtime
            .snapshot()
            .unwrap()
            .current_turn
            .unwrap()
            .projection,
        P0TurnProjection::StoppedBeforeCloudStart
    ));
}

#[test]
fn p0_session_subscription_handoff_is_atomic() {
    let fake = Arc::new(FakeCloud::empty());
    let runtime = runtime_without_worker(fake);
    runtime.start_turn(prompt("first")).unwrap();
    runtime.cancel_turn(P0Actor::Operator).unwrap();
    let (replay, snapshot, live) = runtime
        .subscribe(runtime.identity().session_id, EventSeq::new(1))
        .unwrap()
        .into_parts();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].seq, EventSeq::new(2));
    assert_eq!(snapshot.high_water_seq, EventSeq::new(2));

    runtime.start_turn(prompt("second")).unwrap();
    let live_event = live.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(live_event.seq, EventSeq::new(3));
    assert!(matches!(live_event.payload, P0SessionEvent::TurnAccepted));
}

fn runtime_with(fake: Arc<FakeCloud>) -> P0SessionRuntime {
    P0SessionRuntime::with_cloud(fake, test_config()).unwrap()
}

fn runtime_without_worker(fake: Arc<FakeCloud>) -> P0SessionRuntime {
    P0SessionRuntime::with_cloud_without_worker(fake, test_config()).unwrap()
}

fn test_config() -> P0SessionConfig {
    P0SessionConfig::try_new(Duration::from_millis(250), 64, 8, 8).unwrap()
}

fn lower_error(
    category: CloudLifecycleErrorCategory,
    operation_id: Option<CloudSubmitOperationId>,
) -> LowerLifecycleError {
    LowerLifecycleError::new(category, operation_id)
}

fn prompt(value: &str) -> CloudPrompt {
    CloudPrompt::try_new(value).unwrap()
}

fn task(value: &str) -> CloudTaskId {
    CloudTaskId::try_new(value).unwrap()
}

fn pending(operation_id: CloudSubmitOperationId, task_id: &str) -> CloudLifecycle {
    CloudLifecycle::Pending {
        operation_id,
        task_id: task(task_id),
    }
}

fn ready(operation_id: CloudSubmitOperationId, task_id: &str) -> CloudLifecycle {
    CloudLifecycle::Ready {
        operation_id,
        task_id: task(task_id),
    }
}

fn diff(value: &str) -> CloudDiff {
    decode_cloud_diff(&CloudCapture::new(
        value.as_bytes().to_vec(),
        Vec::new(),
        false,
        false,
        Some(0),
    ))
    .unwrap()
}

fn wait_for_state(runtime: &P0SessionRuntime, expected: P0SessionState) {
    wait_until(|| runtime.snapshot().unwrap().state == expected);
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT_LIMIT;
    while !predicate() {
        assert!(Instant::now() < deadline, "condition did not become true");
        thread::sleep(Duration::from_millis(5));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
