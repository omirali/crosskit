// Deterministic tests of the watch loop: a scripted event source, a fake clock and a
// scripted reconcile pass stand in for the filesystem, time and the database.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::watch::{WatchSchedule, WatchService};
use super::watch_ports::{
    EventDelivery, QueueAnalysis, ReconcilePass, RootWatch, SubscribeFailure, UnconfirmedReason,
    WatchClock, WatchError, WatchEventSource, WatchObserver, WatchSignal, WatchSubscription,
};
use super::watch_report::{
    AnalysisPass, PassOutcome, PassRecord, PassTrigger, WatchMode, WatchReport,
};
use super::watch_scope::{ChangedPaths, PassScope};
use crate::application::{
    ContextPassReport, GraphCheckpoint, IncompleteReason, IndexError, PassFailure,
    ReconcileCompletion, ReconcileReport, RepositoryError, RootOutcome, ScanReport,
};
use crate::domain::{InsightWorkReport, NormalizedPath};

const ROOT: &str = "/watched";

fn ms(milliseconds: u64) -> Duration {
    Duration::from_millis(milliseconds)
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// Debounce 100 ms, periodic pass every 10 s, retries from 1 s doubling up to 4 s.
fn schedule() -> WatchSchedule {
    WatchSchedule::new(ms(100), secs(10), secs(1), secs(4))
}

fn schedule_with_periodic(periodic: Duration) -> WatchSchedule {
    WatchSchedule::new(ms(100), periodic, secs(1), secs(4))
}

#[derive(Clone)]
struct FakeClock(Rc<Cell<Instant>>);

impl FakeClock {
    fn advance(&self, elapsed: Duration) {
        self.0.set(self.0.get() + elapsed);
    }
}

impl WatchClock for FakeClock {
    fn now(&self) -> Instant {
        self.0.get()
    }
}

type Log = Rc<RefCell<Vec<String>>>;
type Steps = Rc<RefCell<VecDeque<(Duration, WatchSignal)>>>;

/// Signals arrive a scripted time after the wait for them begins; a wait that ends first
/// times out and the remaining time carries over. An exhausted script stops the watch.
struct ScriptedSource {
    steps: Steps,
    named: bool,
    clock: FakeClock,
    log: Log,
    delivery: EventDelivery,
}

impl WatchEventSource for ScriptedSource {
    fn subscribe(
        &self,
        roots: &[NormalizedPath],
    ) -> Result<Box<dyn WatchSubscription>, WatchError> {
        self.log.borrow_mut().push(format!(
            "subscribe {}",
            roots
                .iter()
                .map(NormalizedPath::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
        Ok(Box::new(ScriptedSubscription {
            steps: Rc::clone(&self.steps),
            named: self.named,
            clock: self.clock.clone(),
            delivery: self.delivery,
            roots: roots.iter().cloned().map(RootWatch::registered).collect(),
        }))
    }
}

struct ScriptedSubscription {
    steps: Steps,
    /// Whether hints name their paths, as the real source's do.
    named: bool,
    clock: FakeClock,
    delivery: EventDelivery,
    roots: Vec<RootWatch>,
}

impl WatchSubscription for ScriptedSubscription {
    fn event_delivery(&self) -> EventDelivery {
        self.delivery
    }

    fn roots(&self) -> &[RootWatch] {
        &self.roots
    }

    fn next_signal(&mut self, wait: Duration) -> WatchSignal {
        let mut steps = self.steps.borrow_mut();
        let Some(&(after, signal)) = steps.front() else {
            return WatchSignal::Stopped;
        };
        if after <= wait {
            steps.pop_front();
            self.clock.advance(after);
            return signal;
        }
        steps[0].0 = after - wait;
        self.clock.advance(wait);
        WatchSignal::Timeout
    }

    fn take_changes(&mut self) -> ChangedPaths {
        if !self.named {
            return ChangedPaths::unknown();
        }
        let mut changes = ChangedPaths::default();
        changes.note(NormalizedPath::from_absolute("/watched/notes.md").unwrap());
        changes
    }
}

thread_local! {
    /// Set by a scenario whose hints name their paths; the others keep the source that knows
    /// no paths, which is what every scenario before ADR 0050 was written against.
    static NAMED_CHANGES: Cell<bool> = const { Cell::new(false) };
}

/// Runs `scenario` with a source whose hints name their paths.
fn with_named_changes<T>(scenario: impl FnOnce() -> T) -> T {
    NAMED_CHANGES.with(|named| named.set(true));
    let outcome = scenario();
    NAMED_CHANGES.with(|named| named.set(false));
    outcome
}

#[derive(Clone, Copy)]
enum Scripted {
    Completed,
    Incomplete,
    Failed,
    /// The root walked completely, and a second root of the context is gone.
    RootGone,
}

/// Each pass takes `duration` of fake time and yields the next scripted outcome; once the
/// script is exhausted every pass completes.
struct ScriptedReconcile {
    scopes: RefCell<Vec<&'static str>>,
    outcomes: RefCell<VecDeque<Scripted>>,
    duration: Duration,
    clock: FakeClock,
    log: Log,
}

impl ReconcilePass for ScriptedReconcile {
    fn execute(&self, scope: &PassScope) -> Result<ContextPassReport, IndexError> {
        self.log.borrow_mut().push("reconcile".to_string());
        self.scopes.borrow_mut().push(match scope {
            PassScope::Whole => "whole",
            PassScope::Changes(changes) if changes.is_unknown() => "changes unknown",
            PassScope::Changes(_) => "changes",
        });
        self.clock.advance(self.duration);
        let root = NormalizedPath::from_absolute(ROOT).unwrap();
        let scan = ScanReport::empty(root.clone());
        let completion = match self
            .outcomes
            .borrow_mut()
            .pop_front()
            .unwrap_or(Scripted::Completed)
        {
            Scripted::Completed => ReconcileCompletion::Completed {
                marked_missing: 0,
                relocated: 0,
            },
            Scripted::Incomplete => {
                ReconcileCompletion::Incomplete(vec![IncompleteReason::UnstableSources(1)])
            }
            Scripted::Failed => {
                return Err(IndexError::Repository(RepositoryError::Storage(
                    "disk full".to_string(),
                )));
            }
            Scripted::RootGone => {
                let mut report = ContextPassReport::default();
                report.record(RootOutcome::Failed {
                    include: "/watched-gone".to_string(),
                    category: PassFailure::Source,
                });
                report.record(RootOutcome::Reconciled {
                    root: root.as_str().to_string(),
                    report: Box::new(ReconcileReport::new(
                        "reconcile-run:1",
                        scan,
                        ReconcileCompletion::Completed {
                            marked_missing: 0,
                            relocated: 0,
                        },
                    )),
                });
                return Ok(report);
            }
        };
        let mut report = ContextPassReport::default();
        report.record(RootOutcome::Reconciled {
            root: root.as_str().to_string(),
            report: Box::new(ReconcileReport::new("reconcile-run:1", scan, completion)),
        });
        Ok(report)
    }
}

/// The checkpoint the watcher takes after every pass. It logs, so the order in which the
/// two happen is something a test can read, and it can be made to fail.
struct ScriptedCheckpoint {
    log: Log,
    failing: bool,
}

impl GraphCheckpoint for ScriptedCheckpoint {
    fn checkpoint(&self) -> Result<(), RepositoryError> {
        self.log.borrow_mut().push("checkpoint".to_string());
        if self.failing {
            return Err(RepositoryError::Storage("checkpoint refused".to_string()));
        }
        Ok(())
    }
}

/// What one scripted sweep of the insight queue does.
#[derive(Clone, Copy)]
enum Sweep {
    Worked {
        completed: usize,
        failed: usize,
        left_queued: usize,
    },
    /// The durable side refused, so the sweep did not happen at all.
    Refused,
}

fn swept(completed: usize, failed: usize, left_queued: usize) -> AnalysisPass {
    let mut work = InsightWorkReport::default();
    for _ in 0..completed {
        work.record_completed();
    }
    for _ in 0..failed {
        work.record_failed();
    }
    AnalysisPass::new(work, left_queued)
}

/// How a scenario scripts the queue.
struct AnalysisScript {
    sweeps: Vec<Sweep>,
    jobs_per_pass: usize,
    /// The operator presses Ctrl-C while the first sweep is being worked. One thread is
    /// enough to reproduce it: the signal is queued from inside the sweep, exactly where a
    /// real handler would queue it.
    stop_during_the_first_sweep: bool,
}

impl AnalysisScript {
    fn of(sweeps: Vec<Sweep>, jobs_per_pass: usize) -> Self {
        Self {
            sweeps,
            jobs_per_pass,
            stop_during_the_first_sweep: false,
        }
    }

    fn interrupted(sweeps: Vec<Sweep>, jobs_per_pass: usize) -> Self {
        Self {
            sweeps,
            jobs_per_pass,
            stop_during_the_first_sweep: true,
        }
    }
}

/// The queue the watcher reaches: scripted sweeps, and every limit it was asked for. An
/// exhausted script answers with a sweep that found nothing.
struct ScriptedAnalysis {
    sweeps: RefCell<VecDeque<Sweep>>,
    limits: RefCell<Vec<usize>>,
    stop_during_the_first_sweep: bool,
    steps: Steps,
}

impl QueueAnalysis for ScriptedAnalysis {
    fn analyze(&self, limit: usize) -> Result<AnalysisPass, RepositoryError> {
        self.limits.borrow_mut().push(limit);
        if self.stop_during_the_first_sweep && self.limits.borrow().len() == 1 {
            self.steps
                .borrow_mut()
                .push_front((ms(0), WatchSignal::Stopped));
        }
        match self.sweeps.borrow_mut().pop_front() {
            Some(Sweep::Worked {
                completed,
                failed,
                left_queued,
            }) => Ok(swept(completed, failed, left_queued)),
            Some(Sweep::Refused) => Err(RepositoryError::Storage("the queue refused".to_string())),
            None => Ok(swept(0, 0, 0)),
        }
    }
}

#[derive(Default)]
struct RecordingObserver {
    started: Vec<(WatchMode, EventDelivery)>,
    passes: Vec<(PassTrigger, &'static str)>,
    retries: Vec<Duration>,
    degraded: Vec<Duration>,
    stopping: Vec<bool>,
    checkpoint_failures: Vec<String>,
    /// Pass number and the four counters of the sweep that followed it.
    analyses: Vec<(usize, usize, usize, usize, usize)>,
    analysis_failures: Vec<usize>,
    /// How many roots the subscription took, of how many it was given.
    watching: Option<(usize, usize)>,
}

impl WatchObserver for RecordingObserver {
    fn watching(&mut self, roots: &[RootWatch]) {
        self.watching = Some((
            roots.iter().filter(|root| root.is_registered()).count(),
            roots.len(),
        ));
    }

    fn started(&mut self, mode: WatchMode, delivery: EventDelivery) {
        self.started.push((mode, delivery));
    }

    fn pass_finished(&mut self, record: &PassRecord) {
        let outcome = match record.outcome() {
            PassOutcome::Completed(_) => "completed",
            PassOutcome::Incomplete(_) => "incomplete",
            PassOutcome::Failed(_) => "failed",
        };
        assert_eq!(record.number(), self.passes.len() + 1);
        self.passes.push((record.trigger(), outcome));
    }

    fn retry_scheduled(&mut self, delay: Duration) {
        self.retries.push(delay);
    }

    fn degraded(&mut self, periodic_interval: Duration) {
        self.degraded.push(periodic_interval);
    }

    fn checkpoint_failed(&mut self, error: &RepositoryError) {
        self.checkpoint_failures.push(error.to_string());
    }

    fn analysis_finished(&mut self, pass_number: usize, pass: &AnalysisPass) {
        self.analyses.push((
            pass_number,
            pass.claimed(),
            pass.completed(),
            pass.failed(),
            pass.left_queued(),
        ));
    }

    fn analysis_failed(&mut self, pass_number: usize, _error: &RepositoryError) {
        self.analysis_failures.push(pass_number);
    }

    fn stopping(&mut self, final_pass: bool) {
        self.stopping.push(final_pass);
    }
}

struct Outcome {
    result: Result<WatchReport, WatchError>,
    observer: RecordingObserver,
    /// What each pass was asked to walk, in order.
    scopes: Vec<&'static str>,
    log: Vec<String>,
    /// The limit every sweep was asked for, in order. Empty when the watcher never reached
    /// the queue.
    limits: Vec<usize>,
}

fn watch(
    schedule: WatchSchedule,
    steps: Vec<(Duration, WatchSignal)>,
    outcomes: Vec<Scripted>,
    pass_duration: Duration,
) -> Outcome {
    watch_with_delivery(
        schedule,
        steps,
        outcomes,
        pass_duration,
        EventDelivery::Confirmed,
        false,
        None,
    )
}

/// The same watch, told to work the queue after every pass that completes.
fn watch_analyzing(
    schedule: WatchSchedule,
    steps: Vec<(Duration, WatchSignal)>,
    outcomes: Vec<Scripted>,
    analysis: AnalysisScript,
) -> Outcome {
    watch_with_delivery(
        schedule,
        steps,
        outcomes,
        ms(0),
        EventDelivery::Confirmed,
        false,
        Some(analysis),
    )
}

fn watch_with_delivery(
    schedule: WatchSchedule,
    steps: Vec<(Duration, WatchSignal)>,
    outcomes: Vec<Scripted>,
    pass_duration: Duration,
    delivery: EventDelivery,
    failing_checkpoint: bool,
    analysis: Option<AnalysisScript>,
) -> Outcome {
    let clock = FakeClock(Rc::new(Cell::new(Instant::now())));
    let log: Log = Rc::default();
    let steps: Steps = Rc::new(RefCell::new(steps.into_iter().collect()));
    let source = ScriptedSource {
        steps: Rc::clone(&steps),
        named: NAMED_CHANGES.with(Cell::get),
        clock: clock.clone(),
        log: Rc::clone(&log),
        delivery,
    };
    let queue = analysis.as_ref().map(|script| ScriptedAnalysis {
        sweeps: RefCell::new(script.sweeps.iter().copied().collect()),
        limits: RefCell::new(Vec::new()),
        stop_during_the_first_sweep: script.stop_during_the_first_sweep,
        steps: Rc::clone(&steps),
    });
    let reconcile = ScriptedReconcile {
        scopes: RefCell::new(Vec::new()),
        outcomes: RefCell::new(outcomes.into_iter().collect()),
        duration: pass_duration,
        clock: clock.clone(),
        log: Rc::clone(&log),
    };
    let checkpoint = ScriptedCheckpoint {
        log: Rc::clone(&log),
        failing: failing_checkpoint,
    };
    let mut observer = RecordingObserver::default();
    let mut service = WatchService::new(
        &reconcile,
        &checkpoint,
        &source,
        &clock,
        &mut observer,
        schedule,
    );
    if let (Some(queue), Some(script)) = (&queue, &analysis) {
        service = service.analyzing(queue, script.jobs_per_pass);
    }
    let result = service.run(&[NormalizedPath::from_absolute(ROOT).unwrap()]);
    Outcome {
        result,
        observer,
        scopes: reconcile.scopes.into_inner(),
        log: log.borrow().clone(),
        limits: queue
            .map(|queue| queue.limits.into_inner())
            .unwrap_or_default(),
    }
}

fn completed(trigger: PassTrigger) -> (PassTrigger, &'static str) {
    (trigger, "completed")
}

#[test]
fn the_source_is_subscribed_before_the_bootstrap_pass_and_a_quiet_stop_needs_no_final_pass() {
    let outcome = watch(schedule(), vec![], vec![], ms(0));

    assert_eq!(
        outcome.log,
        ["subscribe /watched", "reconcile", "checkpoint"]
    );
    let report = outcome.result.unwrap();
    assert_eq!(report.completed_passes(), 1);
    assert_eq!(report.total_passes(), 1);
    assert_eq!(report.mode(), WatchMode::EventDriven);
    assert_eq!(
        outcome.observer.started,
        [(WatchMode::EventDriven, EventDelivery::Confirmed)]
    );
    assert_eq!(outcome.observer.passes, [completed(PassTrigger::Bootstrap)]);
    assert_eq!(outcome.observer.stopping, [false]);
}

#[test]
fn change_hints_inside_the_debounce_window_coalesce_into_one_pass() {
    let outcome = watch(
        schedule(),
        vec![
            (ms(10), WatchSignal::Changed),
            (ms(10), WatchSignal::Changed),
            (ms(10), WatchSignal::Changed),
            (ms(200), WatchSignal::Stopped),
        ],
        vec![],
        ms(0),
    );

    let report = outcome.result.unwrap();
    assert_eq!(
        outcome.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::Events)
        ]
    );
    assert_eq!(report.changed_signals(), 3);
    assert_eq!(report.coalesced_batches(), 1);
    assert_eq!(outcome.observer.stopping, [false]);
    // The pass receives nothing at all: it covers the context, never the root an event
    // came from — which is also why a root whose events were refused is still reconciled.
    assert!(
        outcome.log[1..]
            .iter()
            .all(|entry| entry == "reconcile" || entry == "checkpoint"),
        "{:?}",
        outcome.log
    );
}

#[test]
fn a_change_queued_during_the_bootstrap_pass_starts_the_next_pass() {
    let outcome = watch(
        schedule(),
        vec![
            (ms(0), WatchSignal::Changed),
            (secs(1), WatchSignal::Stopped),
        ],
        vec![],
        ms(50),
    );

    let report = outcome.result.unwrap();
    assert_eq!(
        outcome.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::Events)
        ]
    );
    assert_eq!(report.coalesced_batches(), 1);
}

#[test]
fn a_quiet_root_is_still_reconciled_periodically() {
    let outcome = watch(
        schedule_with_periodic(secs(1)),
        vec![(secs(5), WatchSignal::Stopped)],
        vec![],
        ms(0),
    );

    let report = outcome.result.unwrap();
    assert_eq!(
        outcome.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::Periodic),
            completed(PassTrigger::Periodic),
            completed(PassTrigger::Periodic),
            completed(PassTrigger::Periodic),
        ]
    );
    assert_eq!(report.changed_signals(), 0);
    assert_eq!(report.coalesced_batches(), 0);
}

#[test]
fn a_stream_error_reconciles_at_once_and_leaves_only_periodic_passes() {
    let outcome = watch(
        schedule_with_periodic(secs(1)),
        vec![
            (ms(50), WatchSignal::StreamError),
            (ms(100), WatchSignal::Changed),
            (secs(2), WatchSignal::Stopped),
        ],
        vec![],
        ms(0),
    );

    let report = outcome.result.unwrap();
    assert_eq!(
        outcome.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::StreamError),
            completed(PassTrigger::Periodic),
            completed(PassTrigger::Periodic),
        ]
    );
    assert_eq!(report.mode(), WatchMode::DegradedPeriodic);
    assert_eq!(outcome.observer.degraded, [secs(1)]);
    // The hint after the error is not trusted and does not become a batch.
    assert_eq!(report.changed_signals(), 0);
    assert_eq!(report.coalesced_batches(), 0);
}

#[test]
fn failed_and_incomplete_passes_retry_with_bounded_backoff_that_a_success_resets() {
    let outcome = watch(
        schedule_with_periodic(secs(100)),
        vec![
            (secs(20), WatchSignal::Changed),
            (secs(60), WatchSignal::Stopped),
        ],
        vec![
            Scripted::Failed,
            Scripted::Incomplete,
            Scripted::Failed,
            Scripted::Failed,
            Scripted::Completed,
            Scripted::Failed,
            Scripted::Completed,
        ],
        ms(0),
    );

    let report = outcome.result.unwrap();
    assert_eq!(
        outcome.observer.passes,
        [
            (PassTrigger::Bootstrap, "failed"),
            (PassTrigger::Retry, "incomplete"),
            (PassTrigger::Retry, "failed"),
            (PassTrigger::Retry, "failed"),
            (PassTrigger::Retry, "completed"),
            (PassTrigger::Events, "failed"),
            (PassTrigger::Retry, "completed"),
        ]
    );
    assert_eq!(
        outcome.observer.retries,
        [secs(1), secs(2), secs(4), secs(4), secs(1)]
    );
    assert_eq!(report.retries(), 5);
    assert_eq!(report.completed_passes(), 2);
    assert_eq!(report.incomplete_passes(), 1);
    assert_eq!(report.failed_passes(), 4);
    assert_eq!(outcome.observer.stopping, [false]);
}

#[test]
fn a_stop_with_pending_changes_runs_one_final_pass() {
    let outcome = watch(
        schedule(),
        vec![
            (ms(10), WatchSignal::Changed),
            (ms(10), WatchSignal::Stopped),
        ],
        vec![],
        ms(0),
    );

    let report = outcome.result.unwrap();
    assert_eq!(
        outcome.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::Shutdown)
        ]
    );
    assert_eq!(outcome.observer.stopping, [true]);
    assert_eq!(report.total_passes(), 2);
}

#[test]
fn a_stop_with_a_pending_retry_runs_a_final_pass_and_reports_its_failure() {
    let outcome = watch(
        schedule(),
        vec![(ms(10), WatchSignal::Stopped)],
        vec![Scripted::Failed, Scripted::Incomplete],
        ms(0),
    );

    assert_eq!(
        outcome.observer.passes,
        [
            (PassTrigger::Bootstrap, "failed"),
            (PassTrigger::Shutdown, "incomplete"),
        ]
    );
    assert_eq!(outcome.observer.stopping, [true]);
    let Err(WatchError::FinalPassFailed(report)) = outcome.result else {
        panic!("a failed final pass must be an error: {:?}", outcome.result);
    };
    assert_eq!(report.failed_passes(), 1);
    assert_eq!(report.incomplete_passes(), 1);
    assert_eq!(report.completed_passes(), 0);
}

#[test]
fn a_subscription_failure_runs_no_pass() {
    struct RefusingSource;

    impl WatchEventSource for RefusingSource {
        fn subscribe(
            &self,
            _roots: &[NormalizedPath],
        ) -> Result<Box<dyn WatchSubscription>, WatchError> {
            Err(WatchError::Subscribe(SubscribeFailure::ResourceLimit))
        }
    }

    let clock = FakeClock(Rc::new(Cell::new(Instant::now())));
    let log: Log = Rc::default();
    let reconcile = ScriptedReconcile {
        scopes: RefCell::new(Vec::new()),
        outcomes: RefCell::new(VecDeque::new()),
        duration: ms(0),
        clock: clock.clone(),
        log: Rc::clone(&log),
    };
    let mut observer = RecordingObserver::default();

    let checkpoint = ScriptedCheckpoint {
        log: Rc::clone(&log),
        failing: false,
    };
    let result = WatchService::new(
        &reconcile,
        &checkpoint,
        &RefusingSource,
        &clock,
        &mut observer,
        schedule(),
    )
    .run(&[NormalizedPath::from_absolute(ROOT).unwrap()]);

    assert_eq!(
        result,
        Err(WatchError::Subscribe(SubscribeFailure::ResourceLimit))
    );
    assert!(log.borrow().is_empty());
    assert!(observer.started.is_empty());
}

#[test]
fn an_unconfirmed_event_stream_starts_the_watch_in_degraded_periodic_mode() {
    let outcome = watch_with_delivery(
        schedule_with_periodic(secs(1)),
        vec![
            (ms(100), WatchSignal::Changed),
            (ms(2_500), WatchSignal::Stopped),
        ],
        vec![],
        ms(0),
        EventDelivery::Unconfirmed(UnconfirmedReason::ProbeOnOtherDevice),
        false,
        None,
    );

    let report = outcome.result.unwrap();
    assert_eq!(
        outcome.observer.started,
        [(
            WatchMode::DegradedPeriodic,
            EventDelivery::Unconfirmed(UnconfirmedReason::ProbeOnOtherDevice)
        )]
    );
    assert_eq!(
        outcome.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::Periodic),
            completed(PassTrigger::Periodic),
        ]
    );
    assert_eq!(report.mode(), WatchMode::DegradedPeriodic);
    // Hints from a stream that never proved itself are not trusted, and there is no
    // later degradation to announce: the watch was honest from the start.
    assert_eq!(report.changed_signals(), 0);
    assert_eq!(report.coalesced_batches(), 0);
    assert!(outcome.observer.degraded.is_empty());
}

/// Every pass is folded into the database file before the watcher waits again, so a stop
/// between passes costs nothing that a pass had finished writing.
#[test]
fn every_pass_is_followed_by_a_checkpoint() {
    let outcome = watch(
        schedule(),
        vec![
            (ms(100), WatchSignal::Changed),
            (ms(200), WatchSignal::Changed),
            (ms(200), WatchSignal::Stopped),
        ],
        vec![],
        ms(0),
    );

    let steps: Vec<&str> = outcome
        .log
        .iter()
        .filter_map(|line| match line.as_str() {
            "checkpoint" => Some("checkpoint"),
            line if line.starts_with("reconcile") => Some("reconcile"),
            _ => None,
        })
        .collect();
    assert!(
        steps.len() >= 4 && steps.len().is_multiple_of(2),
        "a pass and its checkpoint come in pairs: {steps:?}"
    );
    assert!(
        steps
            .chunks(2)
            .all(|pair| pair == ["reconcile", "checkpoint"]),
        "each pass is followed by its own checkpoint, before anything else happens: {:?}",
        outcome.log
    );
}

/// A checkpoint that fails changes nothing about the pass: what it wrote is committed, and
/// the only cost is that it stays in the log until the next one. The watcher says so and
/// keeps watching.
#[test]
fn a_failing_checkpoint_is_reported_and_the_watch_continues() {
    let outcome = watch_with_delivery(
        schedule(),
        vec![
            (ms(100), WatchSignal::Changed),
            (ms(200), WatchSignal::Changed),
            (ms(200), WatchSignal::Stopped),
        ],
        vec![],
        ms(0),
        EventDelivery::Confirmed,
        true,
        None,
    );

    let report = outcome.result.unwrap();
    assert!(report.completed_passes() >= 2);
    assert_eq!(
        outcome.observer.checkpoint_failures.len(),
        report.total_passes(),
        "one report per pass, and no pass turned into a failure"
    );
    assert_eq!(report.failed_passes(), 0);
    assert!(
        outcome.observer.checkpoint_failures[0].contains("checkpoint refused"),
        "{:?}",
        outcome.observer.checkpoint_failures
    );
}

/// The queue is work the watcher does beside watching, and everything below is about the
/// seam between the two: when it reaches the queue at all, and what the queue can do to the
/// watch. What the sweep itself does to a job belongs to stage 5 and 6, and is tested
/// against the real database.
#[test]
fn a_watch_that_was_not_asked_to_analyse_never_reaches_the_queue() {
    let outcome = watch(
        schedule(),
        vec![
            (ms(10), WatchSignal::Changed),
            (ms(200), WatchSignal::Stopped),
        ],
        vec![],
        ms(0),
    );

    let report = outcome.result.unwrap();
    assert!(report.completed_passes() >= 2);
    assert!(outcome.limits.is_empty(), "no sweep was ever asked for");
    assert!(outcome.observer.analyses.is_empty());
    assert_eq!(report.analysis(), Default::default());
}

#[test]
fn every_pass_that_completes_sweeps_the_queue_once_for_the_configured_number_of_jobs() {
    let outcome = watch_analyzing(
        schedule(),
        vec![
            (ms(10), WatchSignal::Changed),
            (ms(200), WatchSignal::Stopped),
        ],
        vec![],
        AnalysisScript::of(
            vec![
                Sweep::Worked {
                    completed: 3,
                    failed: 0,
                    left_queued: 4,
                },
                Sweep::Worked {
                    completed: 2,
                    failed: 1,
                    left_queued: 1,
                },
            ],
            3,
        ),
    );

    let report = outcome.result.unwrap();
    assert_eq!(report.completed_passes(), 2);
    assert_eq!(outcome.limits, [3, 3], "a sweep never asks for more");
    assert_eq!(
        outcome.observer.analyses,
        [(1, 3, 3, 0, 4), (2, 3, 2, 1, 1)],
        "each sweep is reported against the pass it followed"
    );
    // The session adds the sweeps up, and keeps the queue level of the last one rather than
    // summing it: the queue is a level, not a flow.
    let analysis = report.analysis();
    assert_eq!(analysis.sweeps(), 2);
    assert_eq!(analysis.claimed(), 6);
    assert_eq!(analysis.completed(), 5);
    assert_eq!(analysis.failed(), 1);
    assert_eq!(analysis.left_queued(), 1);
    assert_eq!(analysis.failed_sweeps(), 0);
}

/// A pass that could not vouch for the disk has nothing new to analyse, and the revisions
/// it did not commit queued no job.
#[test]
fn a_pass_that_did_not_complete_sweeps_nothing_and_the_pass_after_it_does() {
    let outcome = watch_analyzing(
        schedule_with_periodic(secs(1)),
        vec![(secs(3), WatchSignal::Stopped)],
        vec![Scripted::Incomplete],
        AnalysisScript::of(vec![], 5),
    );

    let report = outcome.result.unwrap();
    assert_eq!(report.incomplete_passes(), 1);
    assert!(report.completed_passes() >= 1);
    assert_eq!(
        report.analysis().sweeps(),
        report.completed_passes(),
        "one sweep per completed pass and not one more"
    );
    assert!(
        outcome
            .observer
            .analyses
            .iter()
            .all(|(number, ..)| *number > 1),
        "the bootstrap pass was incomplete, so it swept nothing: {:?}",
        outcome.observer.analyses
    );
}

/// Stopping is not the moment to start spending money. The final pass keeps the graph
/// honest; the queue keeps what it had and the next watcher finds it there.
#[test]
fn the_pass_that_runs_while_stopping_never_sweeps_the_queue() {
    let outcome = watch_analyzing(
        schedule(),
        vec![
            (ms(10), WatchSignal::Changed),
            (ms(10), WatchSignal::Stopped),
        ],
        vec![],
        AnalysisScript::of(vec![], 5),
    );

    let report = outcome.result.unwrap();
    assert_eq!(outcome.observer.stopping, [true], "a final pass ran");
    assert_eq!(
        outcome.observer.passes.last().map(|(trigger, _)| *trigger),
        Some(PassTrigger::Shutdown)
    );
    assert_eq!(report.total_passes(), 2);
    assert_eq!(outcome.limits.len(), 1, "only the bootstrap pass swept");
    assert_eq!(report.analysis().sweeps(), 1);
}

/// A queue that cannot be worked is not a failure of the watch. The pass is committed, the
/// watcher says what did not happen and goes on watching.
#[test]
fn a_sweep_that_could_not_run_is_reported_and_the_watch_carries_on() {
    let outcome = watch_analyzing(
        schedule(),
        vec![
            (ms(10), WatchSignal::Changed),
            (ms(200), WatchSignal::Stopped),
        ],
        vec![],
        AnalysisScript::of(
            vec![
                Sweep::Refused,
                Sweep::Worked {
                    completed: 1,
                    failed: 0,
                    left_queued: 0,
                },
            ],
            5,
        ),
    );

    let report = outcome.result.unwrap();
    assert_eq!(report.failed_passes(), 0, "the pass itself is committed");
    assert_eq!(report.completed_passes(), 2);
    assert_eq!(outcome.observer.analysis_failures, [1]);
    assert_eq!(report.analysis().failed_sweeps(), 1);
    assert_eq!(report.analysis().sweeps(), 1, "the refused sweep did none");
    assert_eq!(outcome.observer.analyses, [(2, 1, 1, 0, 0)]);
}

/// Ctrl-C while the queue is being worked. The sweep in flight finishes — it is bounded by
/// the number of jobs the configuration allowed — and the watcher stops without starting
/// another. What the sweep had claimed is either finished or failed by the time it returns,
/// so nothing is left held by a worker that is gone.
#[test]
fn a_stop_that_arrives_during_a_sweep_is_taken_as_soon_as_that_sweep_ends() {
    let outcome = watch_analyzing(
        schedule_with_periodic(ms(1)),
        vec![(secs(5), WatchSignal::Timeout)],
        vec![],
        AnalysisScript::interrupted(
            vec![Sweep::Worked {
                completed: 2,
                failed: 0,
                left_queued: 7,
            }],
            2,
        ),
    );

    let report = outcome.result.unwrap();
    assert_eq!(outcome.limits, [2], "no second sweep was started");
    assert_eq!(outcome.observer.analyses, [(1, 2, 2, 0, 7)]);
    assert_eq!(report.total_passes(), 1, "and no further pass either");
    assert_eq!(outcome.observer.stopping, [false]);
    assert_eq!(report.analysis().left_queued(), 7);
}

/// A change right after a pass that changes asked for waits out the rest — here so long that
/// the stop comes first and the change goes to the final pass — while the same change without
/// a rest is a pass of its own after the debounce. The first change after the start does not
/// wait: the rest spaces out a stream of changes, and that one starts it.
#[test]
fn a_rest_holds_back_a_pass_that_hints_ask_for_right_after_another() {
    let steps = || {
        vec![
            (secs(1), WatchSignal::Changed),
            (secs(1), WatchSignal::Changed),
            (secs(2), WatchSignal::Stopped),
        ]
    };

    let eager = watch(schedule(), steps(), vec![], ms(0));
    let rested = watch(schedule().with_rest(secs(5)), steps(), vec![], ms(0));

    assert_eq!(
        eager.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::Events),
            completed(PassTrigger::Events)
        ]
    );
    assert_eq!(
        rested.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::Events),
            completed(PassTrigger::Shutdown)
        ]
    );
}

/// After a quiet spell longer than the rest, a change costs the debounce alone.
#[test]
fn a_rest_already_over_adds_nothing_to_the_debounce() {
    let outcome = watch(
        schedule().with_rest(secs(5)),
        vec![
            (secs(6), WatchSignal::Changed),
            (ms(300), WatchSignal::Stopped),
        ],
        vec![],
        ms(0),
    );

    assert_eq!(
        outcome.observer.passes,
        [
            completed(PassTrigger::Bootstrap),
            completed(PassTrigger::Events)
        ]
    );
}

/// A root that is not there will not be there a second later: the pass is incomplete, and the
/// next one is the periodic pass, not a retry a second, two, four seconds on.
#[test]
fn a_root_that_is_gone_is_left_to_the_periodic_pass_and_not_retried() {
    let outcome = watch(
        schedule(),
        vec![(secs(8), WatchSignal::Stopped)],
        vec![Scripted::RootGone],
        ms(0),
    );

    assert_eq!(
        outcome.observer.passes,
        [(PassTrigger::Bootstrap, "incomplete")]
    );
    assert!(outcome.observer.retries.is_empty());
}

/// The start is whole; a change after it walks what the change named.
#[test]
fn after_the_whole_bootstrap_a_named_change_is_a_scoped_pass() {
    let outcome = with_named_changes(|| {
        watch(
            schedule(),
            vec![
                (secs(1), WatchSignal::Changed),
                (ms(300), WatchSignal::Stopped),
            ],
            vec![],
            ms(0),
        )
    });

    assert_eq!(outcome.scopes, ["whole", "changes"]);
}

/// Lost events cost a whole pass at once, and then watching by events goes on: the next
/// change is scoped again.
#[test]
fn lost_events_are_a_whole_pass_and_watching_by_events_goes_on() {
    let outcome = with_named_changes(|| {
        watch(
            schedule(),
            vec![
                (secs(1), WatchSignal::EventsLost),
                (secs(1), WatchSignal::Changed),
                (ms(300), WatchSignal::Stopped),
            ],
            vec![],
            ms(0),
        )
    });

    assert_eq!(outcome.scopes, ["whole", "whole", "changes"]);
    assert_eq!(
        outcome.observer.passes[1],
        completed(PassTrigger::EventsLost)
    );
    assert_eq!(outcome.result.unwrap().mode(), WatchMode::EventDriven);
}

/// A pass that failed vouches for nothing: its retry walks everything, and only after that
/// is a change scoped again.
#[test]
fn the_retry_of_a_failed_pass_is_whole() {
    let outcome = with_named_changes(|| {
        watch(
            schedule(),
            vec![
                (secs(1), WatchSignal::Changed),
                (secs(3), WatchSignal::Changed),
                (ms(300), WatchSignal::Stopped),
            ],
            vec![Scripted::Completed, Scripted::Failed],
            ms(0),
        )
    });

    assert_eq!(outcome.scopes, ["whole", "changes", "whole", "changes"]);
}

/// Hints closer together than the debounce, for longer than the batch may wait: the pass comes
/// anyway, at the batch's limit, instead of never.
#[test]
fn hints_that_never_settle_still_get_a_pass_at_the_batch_limit() {
    let mut steps: Vec<(Duration, WatchSignal)> =
        (0..80).map(|_| (ms(80), WatchSignal::Changed)).collect();
    steps.push((ms(10), WatchSignal::Stopped));
    let outcome = watch(schedule(), steps, vec![], ms(0));

    let events = outcome
        .observer
        .passes
        .iter()
        .filter(|(trigger, _)| *trigger == PassTrigger::Events)
        .count();
    assert!(events >= 1, "{:?}", outcome.observer.passes);
}

/// A pass that could not vouch for the tree leaves a whole pass owed, and the last pass on the
/// way out pays it — instead of walking what hints named and calling the watch complete.
#[test]
fn a_pending_retry_makes_the_final_pass_whole() {
    let outcome = with_named_changes(|| {
        watch(
            schedule(),
            vec![(ms(100), WatchSignal::Stopped)],
            vec![Scripted::Incomplete, Scripted::Incomplete],
            ms(0),
        )
    });

    assert_eq!(outcome.scopes, ["whole", "whole"]);
    assert!(matches!(
        outcome.result,
        Err(WatchError::FinalPassFailed(_))
    ));
}
