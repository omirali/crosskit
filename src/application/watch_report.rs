// What one watch did, in memory only: the outcome of each pass and the counters of the
// whole session. Nothing here is persisted and nothing here names a path.
use crate::domain::{InsightWorkReport, RunSpend};

use super::context_pass::ContextPassReport;
use super::initial_scan::IndexError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchMode {
    /// Passes follow filesystem change hints, with a periodic pass as a safety net.
    EventDriven,
    /// The event stream failed; only periodic passes run and no event is trusted.
    DegradedPeriodic,
}

/// Why a pass ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassTrigger {
    Bootstrap,
    /// A debounced batch of change hints.
    Events,
    Periodic,
    Retry,
    StreamError,
    /// The OS said events were lost; a whole pass, and watching goes on by events.
    EventsLost,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassOutcome {
    Completed(ContextPassReport),
    /// The walk could not vouch for the disk; no status changed.
    Incomplete(ContextPassReport),
    /// The pass stopped early; nothing it did not commit exists.
    Failed(IndexError),
}

impl PassOutcome {
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassRecord {
    number: usize,
    trigger: PassTrigger,
    outcome: PassOutcome,
}

impl PassRecord {
    pub(super) fn new(number: usize, trigger: PassTrigger, outcome: PassOutcome) -> Self {
        Self {
            number,
            trigger,
            outcome,
        }
    }

    /// 1-based position of the pass within the watch.
    pub fn number(&self) -> usize {
        self.number
    }

    pub fn trigger(&self) -> PassTrigger {
        self.trigger
    }

    pub fn outcome(&self) -> &PassOutcome {
        &self.outcome
    }
}

/// What one sweep of the insight queue did, and what the queue held when it was over.
///
/// Counters only, like every other thing a watch reports: no job id, no path, no text. The
/// five numbers are the ones an operator decides by — how much was taken, how much of it
/// came to something, how much did not, how much nobody asks for any more, and how much is
/// still waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisPass {
    work: InsightWorkReport,
    left_queued: usize,
    /// What a model was paid and what of its answers was kept; nothing when none was asked.
    spend: Option<RunSpend>,
    /// Jobs left queued as too large to send unasked.
    held: usize,
}

impl AnalysisPass {
    /// `left_queued` is read from the queue after the sweep rather than subtracted from
    /// anything: the pass this sweep follows queues jobs of its own, so a difference would
    /// not be the number an operator sees in `index jobs`.
    pub fn new(work: InsightWorkReport, left_queued: usize) -> Self {
        Self {
            work,
            left_queued,
            spend: None,
            held: 0,
        }
    }

    pub fn with_held(mut self, held: usize) -> Self {
        self.held = held;
        self
    }

    pub fn held(&self) -> usize {
        self.held
    }

    pub fn with_spend(mut self, spend: Option<RunSpend>) -> Self {
        self.spend = spend;
        self
    }

    pub fn spend(&self) -> Option<&RunSpend> {
        self.spend.as_ref()
    }

    pub fn claimed(&self) -> usize {
        self.work.claimed()
    }

    pub fn completed(&self) -> usize {
        self.work.completed()
    }

    pub fn failed(&self) -> usize {
        self.work.failed()
    }

    /// Jobs this sweep took and then retired without calling anything: no rule asks for
    /// them any more. Reported rather than left inside the work report, because they are
    /// spent out of the limit the operator set for money — a sweep of twenty that skips
    /// seventeen bought three jobs, and a line that did not say so read as if it bought
    /// twenty.
    pub fn skipped(&self) -> usize {
        self.work.skipped()
    }

    pub fn left_queued(&self) -> usize {
        self.left_queued
    }
}

/// What every sweep of one watch did together.
///
/// `left_queued` is not a sum. The queue is a level, not a flow, so it holds what the last
/// sweep that finished saw; summing it would report a backlog nobody ever had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnalysisTotals {
    sweeps: usize,
    failed_sweeps: usize,
    claimed: usize,
    completed: usize,
    failed: usize,
    skipped: usize,
    left_queued: usize,
}

impl AnalysisTotals {
    pub fn sweeps(&self) -> usize {
        self.sweeps
    }

    /// Sweeps that could not be made at all. They are not failures of the watch: the pass
    /// they follow is committed either way.
    pub fn failed_sweeps(&self) -> usize {
        self.failed_sweeps
    }

    pub fn claimed(&self) -> usize {
        self.claimed
    }

    pub fn completed(&self) -> usize {
        self.completed
    }

    pub fn failed(&self) -> usize {
        self.failed
    }

    /// The session's share of the limit that bought nothing, for the same reason one sweep
    /// reports it: `claimed` is the sum of the other three, and a summary naming two of
    /// them leaves the third to be worked out.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// What the last sweep that finished left behind.
    pub fn left_queued(&self) -> usize {
        self.left_queued
    }

    fn record(&mut self, pass: &AnalysisPass) {
        self.sweeps += 1;
        self.claimed += pass.claimed();
        self.completed += pass.completed();
        self.failed += pass.failed();
        self.skipped += pass.skipped();
        self.left_queued = pass.left_queued();
    }

    fn record_failure(&mut self) {
        self.failed_sweeps += 1;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchReport {
    mode: WatchMode,
    completed_passes: usize,
    incomplete_passes: usize,
    failed_passes: usize,
    changed_signals: usize,
    coalesced_batches: usize,
    retries: usize,
    analysis: AnalysisTotals,
}

impl WatchReport {
    pub(super) fn new() -> Self {
        Self {
            mode: WatchMode::EventDriven,
            completed_passes: 0,
            incomplete_passes: 0,
            failed_passes: 0,
            changed_signals: 0,
            coalesced_batches: 0,
            retries: 0,
            analysis: AnalysisTotals::default(),
        }
    }

    pub fn mode(&self) -> WatchMode {
        self.mode
    }

    pub fn completed_passes(&self) -> usize {
        self.completed_passes
    }

    pub fn incomplete_passes(&self) -> usize {
        self.incomplete_passes
    }

    pub fn failed_passes(&self) -> usize {
        self.failed_passes
    }

    pub fn total_passes(&self) -> usize {
        self.completed_passes + self.incomplete_passes + self.failed_passes
    }

    /// Change hints received while event watching was trusted.
    pub fn changed_signals(&self) -> usize {
        self.changed_signals
    }

    /// Passes that a debounced batch of change hints started.
    pub fn coalesced_batches(&self) -> usize {
        self.coalesced_batches
    }

    pub fn retries(&self) -> usize {
        self.retries
    }

    /// What the watch did to the insight queue. All zero when the configuration did not
    /// turn analysis on, which is what a watcher that only watches reports.
    pub fn analysis(&self) -> AnalysisTotals {
        self.analysis
    }

    pub(super) fn degrade(&mut self) {
        self.mode = WatchMode::DegradedPeriodic;
    }

    pub(super) fn record_analysis(&mut self, pass: &AnalysisPass) {
        self.analysis.record(pass);
    }

    pub(super) fn record_analysis_failure(&mut self) {
        self.analysis.record_failure();
    }

    pub(super) fn record_change(&mut self) {
        self.changed_signals += 1;
    }

    pub(super) fn record_pass(&mut self, trigger: PassTrigger, outcome: &PassOutcome) {
        match trigger {
            PassTrigger::Events => self.coalesced_batches += 1,
            PassTrigger::Retry => self.retries += 1,
            PassTrigger::Bootstrap
            | PassTrigger::Periodic
            | PassTrigger::StreamError
            | PassTrigger::EventsLost
            | PassTrigger::Shutdown => {}
        }
        match outcome {
            PassOutcome::Completed(_) => self.completed_passes += 1,
            PassOutcome::Incomplete(_) => self.incomplete_passes += 1,
            PassOutcome::Failed(_) => self.failed_passes += 1,
        }
    }
}
