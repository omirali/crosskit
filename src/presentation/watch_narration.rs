// What a watch says about itself while it runs: the opening notice, one line per pass and
// the counters of the session when it stops.
//
// Written to whatever `WatchLines` it is given — the terminal for `index watch`, the
// service's journal for `crosskit service`. One wording for both, because a journal that
// said a pass differently from the terminal would be a second vocabulary for one event.
// Nothing here names a changed or excluded path: a filesystem event is a hint, and only the
// reconcile pass decides anything.
use std::time::Duration;

use crate::application::{
    AnalysisPass, AnalysisTotals, ContextCompletion, ContextIncomplete, ContextPassReport,
    EventDelivery, PassOutcome, PassRecord, PassTrigger, ReconcileReport, RepositoryError,
    RootWatch, UnconfirmedReason, WatchMode, WatchObserver, WatchReport, WatchSchedule,
};

use super::index::describe_reasons;
use super::run_cost::spend_lines;

/// Where a watch's lines go. Two kinds, because the terminal keeps them apart: what happened,
/// and what went wrong beside it.
pub(super) trait WatchLines {
    fn say(&mut self, line: &str);
    fn warn(&mut self, line: &str);
}

/// The terminal of a foreground `index watch`.
pub(super) struct Console;

impl WatchLines for Console {
    fn say(&mut self, line: &str) {
        println!("{line}");
    }

    fn warn(&mut self, line: &str) {
        eprintln!("{line}");
    }
}

pub(super) struct NarratedWatch<'a> {
    lines: &'a mut dyn WatchLines,
    pub(super) context: String,
    roots: usize,
    policy_file: String,
    schedule: WatchSchedule,
    /// Said once, in the opening notice, because it is a property of the whole session.
    analysis: Option<String>,
    /// Whether the queue has already been pointed at `index jobs withdraw` this session.
    ///
    /// A watcher runs for hours. The state it would be repairing does not change between
    /// passes — a queue full of work nobody asks for stays that way until somebody clears
    /// it — so saying it after every sweep would bury the lifecycle this output exists to
    /// show, the way the search-index failure was buried before it was moved to the end.
    withdrawal_offered: bool,
    stop_hint: &'static str,
}

impl<'a> NarratedWatch<'a> {
    pub(super) fn new(
        lines: &'a mut dyn WatchLines,
        context: String,
        roots: usize,
        policy_file: String,
        schedule: WatchSchedule,
        analysis: Option<String>,
    ) -> Self {
        Self {
            lines,
            context,
            roots,
            policy_file,
            schedule,
            analysis,
            withdrawal_offered: false,
            stop_hint: " Press Ctrl-C to stop.",
        }
    }

    /// The same narration for a watcher nobody stops from a keyboard: launchd stops the
    /// service, and a journal that said "Press Ctrl-C" would be telling no one.
    pub(super) fn without_stop_hint(mut self) -> Self {
        self.stop_hint = "";
        self
    }
}

impl WatchObserver for NarratedWatch<'_> {
    fn started(&mut self, mode: WatchMode, delivery: EventDelivery) {
        self.lines.say(&format!(
            "Watching context {} — {} root(s) ({}).{}",
            self.context,
            self.roots,
            mode_label(mode),
            self.stop_hint
        ));
        if let EventDelivery::Unconfirmed(reason) = delivery {
            self.lines.say(&format!(
                "Change notifications could not be confirmed: {} Events are not trusted and only periodic passes run.",
                unconfirmed_label(reason)
            ));
        }
        self.lines
            .say(&format!("Policy file: {}", self.policy_file));
        self.lines.say(&format!(
            "Debounce: {}; periodic reconcile every {} even without events.",
            format_duration(self.schedule.debounce()),
            format_duration(self.schedule.periodic_interval())
        ));
        if !self.schedule.rest().is_zero() {
            self.lines.say(&format!(
                "Rest: a pass that changes ask for starts at least {} after the last such pass ended.",
                format_duration(self.schedule.rest())
            ));
        }
        if let Some(analysis) = &self.analysis {
            self.lines.say(analysis);
        }
    }

    fn watching(&mut self, roots: &[RootWatch]) {
        let registered = roots.iter().filter(|root| root.is_registered()).count();
        self.lines.say(&format!(
            "Watching: {registered} of {} root(s).",
            roots.len()
        ));
        for refused in roots.iter().filter(|root| !root.is_registered()) {
            let failure = refused
                .failure()
                .map_or_else(|| "unknown".to_string(), |failure| failure.to_string());
            self.lines.say(&format!(
                "  {}: no change notifications — {failure}",
                refused.root()
            ));
        }
        if registered < roots.len() {
            self.lines.say(
                "  Every pass still walks every root of this context, so what these roots \
                 lose is the latency of an event and not their place in the index.",
            );
        }
    }

    fn pass_finished(&mut self, record: &PassRecord) {
        self.lines.say(&describe_pass(record));
    }

    fn analysis_finished(&mut self, pass_number: usize, pass: &AnalysisPass) {
        self.lines.say(&format!(
            "Pass {pass_number} analysis: {} claimed, {} done, {} failed, {} skipped, {} left in the queue.",
            pass.claimed(),
            pass.completed(),
            pass.failed(),
            pass.skipped(),
            pass.left_queued()
        ));
        if let Some(spend) = pass.spend() {
            for line in spend_lines(spend) {
                self.lines.say(&format!("Pass {pass_number} {line}"));
            }
        }
        if pass.held() > 0 {
            self.lines.say(&format!(
                "Pass {pass_number} held back {} job(s) larger than a watcher sends unasked; `crosskit index jobs run --allow-large` takes them, with their price first.",
                pass.held()
            ));
        }
        self.offer_withdrawal(pass);
    }

    fn analysis_failed(&mut self, pass_number: usize, error: &RepositoryError) {
        self.lines.warn(&format!(
            "Pass {pass_number} analysis did not run: {error}. The pass itself is committed \
             and watching continues; the queue keeps everything it had."
        ));
    }

    fn checkpoint_failed(&mut self, error: &RepositoryError) {
        self.lines.warn(&format!(
            "The pass was committed, but folding it into the database file failed: {error}. \
             What it wrote is in the graph; until the next checkpoint it lives in the log \
             beside it, which a stop that is not a clean one would leave unreadable."
        ));
    }

    fn retry_scheduled(&mut self, delay: Duration) {
        self.lines
            .say(&format!("Retry in {}.", format_duration(delay)));
    }

    fn degraded(&mut self, periodic_interval: Duration) {
        self.lines.say(&format!(
            "Event stream error: {} Switching to degraded periodic mode: reconciling every {} without events.",
            degraded_label(),
            format_duration(periodic_interval)
        ));
    }

    fn stopping(&mut self, final_pass: bool) {
        if final_pass {
            self.lines
                .say("Stopping: running one final reconcile pass for pending changes.");
        } else {
            self.lines.say("Stopping: nothing pending.");
        }
    }
}

impl NarratedWatch<'_> {
    /// Points at the one command that clears the queue of what nobody asks for, the first
    /// time a sweep spends part of its limit on it.
    ///
    /// The watcher does not do it itself and says so. A full walk of the queue after every
    /// pass would be tens of scans of the job table, every pass, for all the hours a
    /// watcher runs — against a backlog that is cleared once. The sweep keeps asking about
    /// the jobs it claims, which is the check that has to stay; this names the rest.
    fn offer_withdrawal(&mut self, pass: &AnalysisPass) {
        if pass.skipped() == 0 || self.withdrawal_offered {
            return;
        }
        self.withdrawal_offered = true;
        self.lines.say(&format!(
            "Part of that limit went on jobs no rule asks for any more. `crosskit --context \
             {} index jobs withdraw` retires all of them in one pass; this watcher does not, \
             and says this once.",
            self.context
        ));
    }
}

pub(super) fn describe_pass(record: &PassRecord) -> String {
    let heading = format!(
        "Pass {} ({})",
        record.number(),
        trigger_label(record.trigger())
    );
    match record.outcome() {
        PassOutcome::Completed(report) => {
            format!("{heading} completed: {}.", describe_counters(report))
        }
        PassOutcome::Incomplete(report) => format!(
            "{heading} incomplete over {} of {} root(s): {}; missing statuses unchanged.",
            report.walked(),
            report.roots(),
            describe_incomplete(&report.completion())
        ),
        PassOutcome::Failed(error) => {
            format!("{heading} failed: {error}; missing statuses unchanged.")
        }
    }
}

/// Counters only, summed over every root the pass walked: the per-path failures are not
/// repeated here, and no root is named — a watcher prints this line every few seconds.
fn describe_counters(report: &ContextPassReport) -> String {
    let unstable: usize = report
        .reconciliations()
        .map(|root: &ReconcileReport| root.scan().unstable_sources())
        .sum();
    let (marked_missing, relocated) = report.marked_missing();
    // Counted and never named: the directories a scoped pass walks are where somebody just
    // worked, which is exactly what this line must not say.
    let walked = match report.is_scoped() {
        true => format!("{} changed directory(ies)", report.walked()),
        false => format!("{} root(s)", report.walked()),
    };
    format!(
        "{walked}, indexed {} file(s) and {} chunk(s), {} metadata-only, {} excluded, {} unstable, {} marked missing, {} relocated",
        report.indexed_files(),
        report.indexed_chunks(),
        report.metadata_only_files(),
        report.excluded_paths(),
        unstable,
        marked_missing,
        relocated
    )
}

/// Why a pass over a context may not retire what it did not see. A root that was not walked
/// and a walk that could not be vouched for are two different answers, and both are given.
fn describe_incomplete(completion: &ContextCompletion) -> String {
    let ContextCompletion::Incomplete(reasons) = completion else {
        return "no reason recorded".to_string();
    };
    let not_walked: usize = reasons
        .iter()
        .filter_map(|reason| match reason {
            ContextIncomplete::RootsNotWalked(count) => Some(*count),
            ContextIncomplete::RootIncomplete(_) => None,
        })
        .sum();
    let walked: Vec<_> = reasons
        .iter()
        .filter_map(|reason| match reason {
            ContextIncomplete::RootIncomplete(reason) => Some(*reason),
            ContextIncomplete::RootsNotWalked(_) => None,
        })
        .collect();

    let mut described = Vec::new();
    if not_walked > 0 {
        described.push(format!("{not_walked} root(s) not walked"));
    }
    if !walked.is_empty() {
        described.push(describe_reasons(&walked));
    }
    described.join(", ")
}

/// The counters of a whole session, printed once when the watcher stops.
///
/// Counters only, and the same shape whatever the session did: a run that was quiet and a
/// run that retried all day are read against each other without counting the words.
pub(super) fn render_summary(lines: &mut dyn WatchLines, report: &WatchReport, analysed: bool) {
    lines.say("Watch stopped.");
    lines.say(&format!("Mode: {}", mode_label(report.mode())));
    lines.say(&format!("Passes: {}", report.total_passes()));
    lines.say(&format!("Passes completed: {}", report.completed_passes()));
    lines.say(&format!(
        "Passes incomplete: {}",
        report.incomplete_passes()
    ));
    lines.say(&format!("Passes failed: {}", report.failed_passes()));
    lines.say(&format!(
        "Coalesced event batches: {} ({} change hints)",
        report.coalesced_batches(),
        report.changed_signals()
    ));
    lines.say(&format!("Retries: {}", report.retries()));
    if analysed {
        render_analysis(lines, report.analysis());
    }
}

/// What the watcher did to the queue this session, said only when it was asked to touch it
/// at all.
fn render_analysis(lines: &mut dyn WatchLines, totals: AnalysisTotals) {
    lines.say(&format!(
        "Jobs claimed: {}; done: {}; failed: {}; skipped: {}; left in the queue: {}",
        totals.claimed(),
        totals.completed(),
        totals.failed(),
        totals.skipped(),
        totals.left_queued()
    ));
    lines.say(&format!(
        "Analysis sweeps: {} ({} could not run)",
        totals.sweeps(),
        totals.failed_sweeps()
    ));
}

pub(super) fn mode_label(mode: WatchMode) -> &'static str {
    match mode {
        WatchMode::EventDriven => "event-driven mode",
        WatchMode::DegradedPeriodic => "degraded periodic mode",
    }
}

/// Fixed sentences only: no reason ever names a path.
pub(super) fn unconfirmed_label(reason: UnconfirmedReason) -> &'static str {
    match reason {
        UnconfirmedReason::NoHint => {
            "a probe file on this stream produced no hint within the wait, so the stream was created and is not delivering."
        }
        UnconfirmedReason::ProbeOnOtherDevice => {
            "the data directory lies on another filesystem than one of the roots, so a probe there would prove nothing about that root."
        }
        UnconfirmedReason::ProbeInsideRoot => {
            "the data directory lies inside a watched root, and the watcher must not create events in a tree it scans."
        }
        UnconfirmedReason::NothingWatched => {
            "no root of this context could be watched at all, so there is no stream to probe; every pass still walks every root."
        }
        UnconfirmedReason::NoProbe => {
            "no delivery probe with the required safety guarantees was available."
        }
        UnconfirmedReason::StartTimedOut => {
            "the filesystem event backend did not start within the limit, so the watch went on without it; a restart tries again."
        }
    }
}

/// Why a watch that started on events went over to the timer.
pub(super) fn degraded_label() -> &'static str {
    "events may have been lost and event watching is no longer trusted."
}

pub(super) fn trigger_label(trigger: PassTrigger) -> &'static str {
    match trigger {
        PassTrigger::Bootstrap => "bootstrap",
        PassTrigger::Events => "events",
        PassTrigger::Periodic => "periodic",
        PassTrigger::Retry => "retry",
        PassTrigger::StreamError => "event stream error",
        PassTrigger::EventsLost => "events lost",
        PassTrigger::Shutdown => "final",
    }
}

pub(super) fn format_duration(duration: Duration) -> String {
    if duration.as_millis().is_multiple_of(1_000) {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{format_duration, mode_label, trigger_label, unconfirmed_label};
    use crate::application::{PassTrigger, UnconfirmedReason, WatchMode};

    #[test]
    fn durations_are_printed_in_whole_units() {
        assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration(Duration::from_secs(300)), "300s");
    }

    #[test]
    fn every_mode_reason_and_trigger_has_a_label() {
        assert_eq!(mode_label(WatchMode::EventDriven), "event-driven mode");
        assert!(mode_label(WatchMode::DegradedPeriodic).contains("degraded"));
        for reason in [
            UnconfirmedReason::NoHint,
            UnconfirmedReason::ProbeOnOtherDevice,
            UnconfirmedReason::ProbeInsideRoot,
            UnconfirmedReason::NoProbe,
            UnconfirmedReason::NothingWatched,
            UnconfirmedReason::StartTimedOut,
        ] {
            assert!(!unconfirmed_label(reason).contains('/'));
        }
        for trigger in [
            PassTrigger::Bootstrap,
            PassTrigger::Events,
            PassTrigger::Periodic,
            PassTrigger::Retry,
            PassTrigger::StreamError,
            PassTrigger::EventsLost,
            PassTrigger::Shutdown,
        ] {
            assert!(!trigger_label(trigger).is_empty());
        }
    }
}
