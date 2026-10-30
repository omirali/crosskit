// Use case: keep a root reconciled while the process runs. Filesystem events only mark the
// root dirty; every change to the graph goes through one sequential reconcile pass.
use std::time::{Duration, Instant};

use crate::domain::NormalizedPath;

use super::context_pass::{ContextCompletion, ContextIncomplete};
use super::reconcile::IncompleteReason;
use super::repository::GraphCheckpoint;
use super::watch_ports::{
    EventDelivery, QueueAnalysis, ReconcilePass, WatchClock, WatchError, WatchEventSource,
    WatchObserver, WatchSignal,
};
use super::watch_report::{PassOutcome, PassRecord, PassTrigger, WatchMode, WatchReport};
use super::watch_scope::{ChangedPaths, PassScope};

/// The fixed timing of a watch. Not user-configurable: the values are documented in the
/// README and chosen so that correctness never depends on them, only latency does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchSchedule {
    debounce: Duration,
    periodic_interval: Duration,
    retry_initial: Duration,
    retry_max: Duration,
    /// The least time from the end of one pass to the start of a pass that change hints ask
    /// for. Zero unless set: see `with_rest`.
    rest: Duration,
}

impl WatchSchedule {
    pub const fn new(
        debounce: Duration,
        periodic_interval: Duration,
        retry_initial: Duration,
        retry_max: Duration,
    ) -> Self {
        Self {
            debounce,
            periodic_interval,
            retry_initial,
            retry_max,
            rest: Duration::ZERO,
        }
    }

    /// The same schedule, with a pass that hints ask for held back until `rest` has gone by
    /// since the last such pass ended.
    ///
    /// A tree that changes all the time — a transcript a model session appends to every few
    /// seconds — would otherwise have its context walked end to end, one pass straight after
    /// another, for as long as the session lasts. With a rest, a burst of changes costs at most
    /// one pass per `rest` beyond the pass itself; a change after a quiet spell is still taken
    /// after the debounce alone. Only hints wait: the bootstrap, retries and the periodic pass
    /// keep their own deadlines. It bounds the work, not the moments a pass holds readers off:
    /// measured, the share of `busy` beside edits did not fall (ADR 0049, decision 9).
    pub const fn with_rest(mut self, rest: Duration) -> Self {
        self.rest = rest;
        self
    }

    pub fn rest(&self) -> Duration {
        self.rest
    }

    /// The same schedule with the periodic whole pass this far apart.
    pub const fn with_periodic_interval(mut self, periodic_interval: Duration) -> Self {
        self.periodic_interval = periodic_interval;
        self
    }

    /// Quiet time after the last change hint before a batch becomes a pass.
    pub fn debounce(&self) -> Duration {
        self.debounce
    }

    /// Time after the last pass before a pass runs with no hint at all, so a lost event
    /// is covered.
    pub fn periodic_interval(&self) -> Duration {
        self.periodic_interval
    }

    fn next_backoff(&self, current: Duration) -> Duration {
        current.saturating_mul(2).min(self.retry_max)
    }
}

impl Default for WatchSchedule {
    fn default() -> Self {
        Self::new(
            Duration::from_millis(500),
            Duration::from_secs(300),
            Duration::from_secs(1),
            Duration::from_secs(60),
        )
    }
}

/// What the watcher does to the insight queue and how much of it one pass may take.
#[derive(Clone, Copy)]
struct QueueSweeps<'a> {
    queue: &'a dyn QueueAnalysis,
    jobs_per_pass: usize,
}

pub struct WatchService<'a> {
    reconcile: &'a dyn ReconcilePass,
    checkpoint: &'a dyn GraphCheckpoint,
    events: &'a dyn WatchEventSource,
    clock: &'a dyn WatchClock,
    observer: &'a mut dyn WatchObserver,
    schedule: WatchSchedule,
    /// Absent unless the configuration turned analysis on and said how many jobs a pass may
    /// work. A watcher that was never told to spend money does not reach the queue at all.
    analysis: Option<QueueSweeps<'a>>,
}

impl<'a> WatchService<'a> {
    pub fn new(
        reconcile: &'a dyn ReconcilePass,
        checkpoint: &'a dyn GraphCheckpoint,
        events: &'a dyn WatchEventSource,
        clock: &'a dyn WatchClock,
        observer: &'a mut dyn WatchObserver,
        schedule: WatchSchedule,
    ) -> Self {
        Self {
            reconcile,
            checkpoint,
            events,
            clock,
            observer,
            schedule,
            analysis: None,
        }
    }

    /// The watcher also works the context's insight queue after every pass that completed,
    /// at most `jobs_per_pass` jobs each time.
    pub fn analyzing(mut self, queue: &'a dyn QueueAnalysis, jobs_per_pass: usize) -> Self {
        self.analysis = Some(QueueSweeps {
            queue,
            jobs_per_pass,
        });
        self
    }

    /// Subscribes, runs the bootstrap pass, then alternates between waiting for the next
    /// deadline or signal and running one pass at a time until the operator stops it.
    /// A subscription whose delivery is unconfirmed starts the watch in degraded periodic
    /// mode: nothing is claimed about events that were never seen to arrive.
    pub fn run(mut self, roots: &[NormalizedPath]) -> Result<WatchReport, WatchError> {
        let mut subscription = self.events.subscribe(roots)?;
        let delivery = subscription.event_delivery();
        let mut state = WatchState::bootstrap(self.clock.now(), &self.schedule, delivery);
        self.observer.started(state.report.mode(), delivery);
        self.observer.watching(subscription.roots());

        loop {
            let now = self.clock.now();
            if let Some(trigger) = state.due_pass(now) {
                let scope = state.scope_of(trigger, subscription.take_changes());
                self.run_pass(&mut state, trigger, &scope);
                continue;
            }
            let wait = state.next_deadline().saturating_duration_since(now);
            match subscription.next_signal(wait) {
                WatchSignal::Changed => state.note_change(self.clock.now(), &self.schedule),
                WatchSignal::StreamError => {
                    if state.degrade(self.clock.now()) {
                        self.observer.degraded(self.schedule.periodic_interval);
                    }
                }
                WatchSignal::EventsLost => state.lose_events(self.clock.now()),
                WatchSignal::Timeout => {}
                WatchSignal::Stopped => break,
            }
        }

        let final_pass = state.has_pending_work();
        self.observer.stopping(final_pass);
        if final_pass {
            let scope = state.scope_of(PassTrigger::Shutdown, subscription.take_changes());
            self.run_pass(&mut state, PassTrigger::Shutdown, &scope);
        }
        state.finish()
    }

    fn run_pass(&mut self, state: &mut WatchState, trigger: PassTrigger, scope: &PassScope) {
        let outcome = match self.reconcile.execute(scope) {
            Ok(report) => match report.completion().is_completed() {
                true => PassOutcome::Completed(report),
                false => PassOutcome::Incomplete(report),
            },
            Err(error) => PassOutcome::Failed(error),
        };
        // Folded into the database file before the pass is reported, not when this process
        // closes the database: a watcher may not close it for days, and everything it has
        // written stays in the log until then — a stop that is not a close would leave that
        // log unreplayable and the whole session lost. After this, such a stop costs the
        // pass in flight. Measured at 294 ms against a pass of 21 s over ten thousand
        // sources, and it is inside the pass rather than after it so that a reported pass
        // is one another process can already read.
        let checkpoint = self.checkpoint.checkpoint();
        let (record, retry_delay) =
            state.record_pass(trigger, scope, outcome, self.clock.now(), &self.schedule);
        self.observer.pass_finished(&record);
        if let Err(error) = checkpoint {
            self.observer.checkpoint_failed(&error);
        }
        if let Some(delay) = retry_delay {
            self.observer.retry_scheduled(delay);
        }
        self.sweep_queue(state, &record);
    }

    /// The queue is worked only after a pass that completed, and never by the pass that
    /// runs while stopping. A pass that could not vouch for the disk has nothing new to
    /// analyse, and an operator who asked the watcher to stop is not asking it to spend
    /// money on the way out.
    ///
    /// A stop that arrives while a sweep is running is taken after that sweep, which is why
    /// the sweep is bounded: `jobs_per_pass` is also the longest the watcher can go on
    /// working once it has been asked to stop.
    ///
    /// After the pass has been checkpointed and reported, not before either: the pass is
    /// what the operator is waiting to hear about, and a sweep that took a minute would
    /// hold that line back. What the sweep writes therefore waits for the next pass's
    /// checkpoint. That costs nothing worth a second checkpoint, because a result lost to
    /// an unclean stop is a job the lease hands back and the cache answers again for free.
    fn sweep_queue(&mut self, state: &mut WatchState, record: &PassRecord) {
        let Some(sweeps) = self.analysis else {
            return;
        };
        if record.trigger() == PassTrigger::Shutdown || !record.outcome().is_completed() {
            return;
        }
        match sweeps.queue.analyze(sweeps.jobs_per_pass) {
            Ok(pass) => {
                state.report.record_analysis(&pass);
                self.observer.analysis_finished(record.number(), &pass);
            }
            Err(error) => {
                state.report.record_analysis_failure();
                self.observer.analysis_failed(record.number(), &error);
            }
        }
    }
}

/// A pending pass and why it is pending.
#[derive(Debug, Clone, Copy)]
struct Dirty {
    due: Instant,
    trigger: PassTrigger,
    /// When the first hint of this batch arrived; the batch never waits past
    /// `first + BATCH_LIMIT`, however many hints keep coming.
    first: Instant,
}

/// The longest a batch of hints waits for quiet. The debounce restarts on every hint, and a
/// tree that changes more often than it — a model session writing its transcript several times
/// a second — would otherwise never settle: measured, 64 hints in 20 s and no pass at all.
const BATCH_LIMIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
struct Retry {
    due: Instant,
}

struct WatchState {
    report: WatchReport,
    passes: usize,
    dirty: Option<Dirty>,
    retry: Option<Retry>,
    /// Delay of the next retry, doubling on every failure until the schedule's maximum.
    backoff: Duration,
    periodic_due: Instant,
    last_pass_completed: bool,
    /// A pass over the whole context is owed: at the start, after events were lost, after a
    /// pass that failed. Until one has run, no pass is scoped to what hints named.
    whole_due: bool,
    /// When the last pass that hints asked for ended, for the schedule's rest. Only those:
    /// the rest spaces out a stream of changes, and the first change after a start or a
    /// periodic pass is not part of one.
    last_pass_ended: Option<Instant>,
}

impl WatchState {
    fn bootstrap(now: Instant, schedule: &WatchSchedule, delivery: EventDelivery) -> Self {
        let mut report = WatchReport::new();
        if !delivery.is_confirmed() {
            report.degrade();
        }
        Self {
            report,
            passes: 0,
            dirty: Some(Dirty {
                due: now,
                trigger: PassTrigger::Bootstrap,
                first: now,
            }),
            retry: None,
            backoff: schedule.retry_initial,
            periodic_due: now + schedule.periodic_interval,
            last_pass_completed: false,
            whole_due: true,
            last_pass_ended: None,
        }
    }

    /// A pass is due when a dirty batch has settled, a retry has waited long enough or the
    /// periodic interval has elapsed. Any pass covers all three.
    fn due_pass(&self, now: Instant) -> Option<PassTrigger> {
        if let Some(dirty) = self.dirty
            && dirty.due <= now
        {
            return Some(dirty.trigger);
        }
        if self.retry.is_some_and(|retry| retry.due <= now) {
            return Some(PassTrigger::Retry);
        }
        if self.periodic_due <= now {
            return Some(PassTrigger::Periodic);
        }
        None
    }

    fn next_deadline(&self) -> Instant {
        let mut deadline = self.periodic_due;
        if let Some(dirty) = self.dirty {
            deadline = deadline.min(dirty.due);
        }
        if let Some(retry) = self.retry {
            deadline = deadline.min(retry.due);
        }
        deadline
    }

    /// Trailing debounce: every hint pushes the batch's deadline out again. A hint after
    /// the stream degraded is not trusted and changes nothing.
    fn note_change(&mut self, now: Instant, schedule: &WatchSchedule) {
        if self.report.mode() == WatchMode::DegradedPeriodic {
            return;
        }
        self.report.record_change();
        let first = match self.dirty {
            Some(pending) if pending.trigger == PassTrigger::Events => pending.first,
            _ => now,
        };
        let mut due = (now + schedule.debounce).min(first + BATCH_LIMIT);
        if let Some(ended) = self.last_pass_ended {
            due = due.max(ended + schedule.rest);
        }
        let settled = Dirty {
            due,
            trigger: PassTrigger::Events,
            first,
        };
        self.dirty = match self.dirty {
            // A bootstrap or stream-error pass is already due; it will cover this hint.
            Some(pending) if pending.trigger != PassTrigger::Events => Some(pending),
            _ => Some(settled),
        };
    }

    /// What the pass `trigger` starts walks. Only a pass that hints asked for — or the last one,
    /// on the way out — is scoped, and only while no whole pass is owed; every other pass is
    /// whole, and the changes it was handed are covered by it.
    fn scope_of(&self, trigger: PassTrigger, changes: ChangedPaths) -> PassScope {
        match trigger {
            PassTrigger::Events | PassTrigger::Shutdown if !self.whole_due => {
                PassScope::Changes(changes)
            }
            _ => PassScope::Whole,
        }
    }

    /// The OS dropped events or the root moved: what changed is unknown, so the whole context
    /// is walked at once. Watching by events goes on — the stream itself is fine.
    fn lose_events(&mut self, now: Instant) {
        if self.report.mode() == WatchMode::DegradedPeriodic {
            return;
        }
        self.whole_due = true;
        self.dirty = Some(Dirty {
            due: now,
            trigger: PassTrigger::EventsLost,
            first: now,
        });
    }

    /// Returns `true` the first time: events may have been lost, so one pass runs at once
    /// and only periodic passes follow.
    fn degrade(&mut self, now: Instant) -> bool {
        if self.report.mode() == WatchMode::DegradedPeriodic {
            return false;
        }
        self.report.degrade();
        self.whole_due = true;
        self.dirty = Some(Dirty {
            due: now,
            trigger: PassTrigger::StreamError,
            first: now,
        });
        true
    }

    fn record_pass(
        &mut self,
        trigger: PassTrigger,
        scope: &PassScope,
        outcome: PassOutcome,
        now: Instant,
        schedule: &WatchSchedule,
    ) -> (PassRecord, Option<Duration>) {
        self.passes += 1;
        self.dirty = None;
        if trigger == PassTrigger::Events {
            self.last_pass_ended = Some(now);
        }
        self.periodic_due = now + schedule.periodic_interval;
        self.report.record_pass(trigger, &outcome);
        self.last_pass_completed = outcome.is_completed();
        match (&outcome, scope) {
            // A pass that failed vouches for nothing; the retry walks everything.
            (PassOutcome::Failed(_), _) => self.whole_due = true,
            (_, PassScope::Whole) => self.whole_due = false,
            (_, PassScope::Changes(_)) => {}
        }

        let retry_delay = if !worth_retrying(&outcome) {
            self.retry = None;
            self.backoff = schedule.retry_initial;
            None
        } else {
            let delay = self.backoff;
            self.retry = Some(Retry { due: now + delay });
            self.backoff = schedule.next_backoff(delay);
            // A retry is a whole pass owed: until one completes, no pass — the last one on
            // the way out included — may be narrowed to what hints named, or it would end the
            // watch "completed" over a tree it never vouched for.
            self.whole_due = true;
            Some(delay)
        };
        (PassRecord::new(self.passes, trigger, outcome), retry_delay)
    }

    fn has_pending_work(&self) -> bool {
        self.dirty.is_some() || self.retry.is_some()
    }

    fn finish(self) -> Result<WatchReport, WatchError> {
        if self.last_pass_completed {
            Ok(self.report)
        } else {
            Err(WatchError::FinalPassFailed(self.report))
        }
    }
}

/// Whether a pass that did not complete is worth running again soon, rather than at the next
/// periodic pass.
///
/// A failed pass is, and so is a walk that met files changing under it or paths it could not
/// read: those pass. A root that was not walked at all — gone, a pattern, kept closed by the
/// OS — or a name that cannot be represented stays exactly so until somebody changes the
/// configuration or the tree, and the tree's change is an event of its own. Retrying it
/// doubled up to a minute and then once a minute for ever, a whole pass each time: measured
/// on the owner's `work`, whose one transcript folder no longer exists, that was a pass of
/// every root every minute, and the graph held for writing through most of it.
fn worth_retrying(outcome: &PassOutcome) -> bool {
    match outcome {
        PassOutcome::Completed(_) => false,
        PassOutcome::Failed(_) => true,
        PassOutcome::Incomplete(report) => match report.completion() {
            ContextCompletion::Completed => false,
            ContextCompletion::Incomplete(reasons) => reasons.iter().any(|reason| {
                matches!(
                    reason,
                    ContextIncomplete::RootIncomplete(
                        IncompleteReason::UnstableSources(_) | IncompleteReason::FailedPaths(_)
                    )
                )
            }),
        },
    }
}
