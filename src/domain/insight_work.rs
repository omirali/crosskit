// The lifecycle of a queued job once a worker takes it: how long a claim lasts, how many
// attempts a job gets, why it can end without a result, and what one pass did.
//
// Nothing here knows a provider, a model or a prompt. A job is taken, attempted and either
// finished or handed back, and that is true whoever does the work.

use std::fmt;

use super::NormalizedPath;

/// How long a claim stays valid. The worker that took a job is expected to finish it
/// inside the lease; when the lease runs out the job is free again, whether or not the
/// worker still exists. The deadline is stored as an absolute moment, so it outlives the
/// process that wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsightLease {
    duration_ms: i64,
}

impl InsightLease {
    /// Long enough that an ordinary job finishes well inside it, short enough that a
    /// killed worker does not hold the queue for a working day.
    pub const PRODUCTION: Self = Self::of_milliseconds(5 * 60 * 1000);

    /// A negative duration would free a claim before it was taken, so it is read as none.
    pub const fn of_milliseconds(duration_ms: i64) -> Self {
        Self {
            duration_ms: if duration_ms < 0 { 0 } else { duration_ms },
        }
    }

    /// When a claim taken at `claimed_at_unix_ms` stops protecting the job. Saturating, so
    /// a clock near the end of the representable range cannot wrap into the past and free
    /// a claim that was just taken.
    pub fn expires_at(&self, claimed_at_unix_ms: i64) -> i64 {
        claimed_at_unix_ms.saturating_add(self.duration_ms)
    }
}

/// How many times a job may be attempted before it is retired. An attempt is counted when
/// the job is claimed, not when it finishes: a worker that dies mid-job has still spent
/// one, which is what stops a job that kills its worker from being retried forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsightRetryPolicy {
    max_attempts: i64,
}

impl InsightRetryPolicy {
    pub const PRODUCTION: Self = Self::of_attempts(3);

    /// A policy that allowed no attempt would retire every job before it was ever tried,
    /// so one is the least it can mean.
    pub const fn of_attempts(max_attempts: i64) -> Self {
        Self {
            max_attempts: if max_attempts < 1 { 1 } else { max_attempts },
        }
    }

    /// What becomes of a job that is out of a worker's hands again, having been attempted
    /// `attempts` times. It goes back in the queue until it has used up its attempts, and
    /// is retired after that; a retired job is never returned on its own.
    pub fn after(&self, attempts: i64) -> InsightRecoveryOutcome {
        if attempts >= self.max_attempts {
            InsightRecoveryOutcome::Retire
        } else {
            InsightRecoveryOutcome::Requeue
        }
    }
}

/// Where a job goes when nobody is working on it any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightRecoveryOutcome {
    Requeue,
    Retire,
}

/// Why a job is not done. Closed on purpose: the category is what crosses the boundary,
/// and the engine's words, a provider's words and anything read out of the file never do.
///
/// Each one also says whether trying again could ever help. That is a property of the
/// failure, not of the worker, so it lives with the category and is the same wherever the
/// category is read back from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightFailureCategory {
    /// The job's input could not be read from the graph: the revision is not there, or is
    /// not the content the job was queued for. The graph will not start holding it.
    Input,
    /// The handler ran and could not produce a result.
    Handler,
    /// The claim ran out before the worker finished the job.
    LeaseExpired,
    /// A provider could not be reached, was rate limited or was overloaded.
    Unavailable,
    /// What came back does not fit the schema this program accepts.
    Schema,
    /// The model declined to answer. Asking it the same thing again will not change that.
    Refused,
    /// The revision is larger than one job may ever send.
    InputTooLarge,
    /// This run has spent the budget it was given. The job itself is fine.
    BudgetSpent,
}

impl InsightFailureCategory {
    /// Every category, in one place, so nothing that walks them can miss the newest one.
    pub const ALL: [Self; 8] = [
        Self::Input,
        Self::Handler,
        Self::LeaseExpired,
        Self::Unavailable,
        Self::Schema,
        Self::Refused,
        Self::InputTooLarge,
        Self::BudgetSpent,
    ];

    /// The stored spelling, written once here like a state's.
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Handler => "handler",
            Self::LeaseExpired => "lease-expired",
            Self::Unavailable => "unavailable",
            Self::Schema => "schema",
            Self::Refused => "refused",
            Self::InputTooLarge => "input-too-large",
            Self::BudgetSpent => "budget-spent",
        }
    }

    /// The category a stored label means, or nothing when the label is not one this program
    /// wrote. A reader that cannot name the failure refuses rather than guessing, because
    /// guessing here decides whether a job is tried again.
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.label() == label)
    }

    /// Whether the job may be tried again. A failure that another attempt cannot change is
    /// retired at once instead of spending the remaining attempts to reach the same end.
    pub fn disposition(self) -> FailureDisposition {
        match self {
            Self::Handler | Self::LeaseExpired | Self::Unavailable | Self::BudgetSpent => {
                FailureDisposition::Retry
            }
            Self::Input | Self::Schema | Self::Refused | Self::InputTooLarge => {
                FailureDisposition::Permanent
            }
        }
    }
}

/// Whether trying again could help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDisposition {
    Retry,
    Permanent,
}

impl fmt::Display for InsightFailureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A job a worker holds. It carries what the work needs and nothing more: which job it is,
/// which revision it enriches and the content hash that revision was queued for. No path,
/// no file name, no text. How many attempts the job has cost belongs to the queue, which
/// counts it when it hands the job out and decides on it when it takes the job back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedInsightJob {
    id: String,
    source_revision_id: String,
    input_content_hash: String,
    /// Every path the graph still calls present for this revision's file. A job is worked
    /// only when the policy admits one of them, and the question is asked when the job is
    /// claimed rather than when it was queued: a queue outlives the policy that filled it.
    present_paths: Vec<NormalizedPath>,
}

impl ClaimedInsightJob {
    pub fn new(
        id: impl Into<String>,
        source_revision_id: impl Into<String>,
        input_content_hash: impl Into<String>,
        present_paths: Vec<NormalizedPath>,
    ) -> Self {
        Self {
            id: id.into(),
            source_revision_id: source_revision_id.into(),
            input_content_hash: input_content_hash.into(),
            present_paths,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn source_revision_id(&self) -> &str {
        &self.source_revision_id
    }

    pub fn input_content_hash(&self) -> &str {
        &self.input_content_hash
    }

    /// Whether any path this revision's file is still at is one `is_visible` admits.
    pub fn is_wanted_by(&self, is_visible: &dyn Fn(&NormalizedPath) -> bool) -> bool {
        any_path_is_wanted(&self.present_paths, is_visible)
    }
}

/// A job still standing in the queue, as the sweep that brings the queue to the rules reads
/// it: which job it is and where its revision's file still lies. Nothing else, because the
/// sweep never calls a model — it only decides whether the question is still being asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedInsightJob {
    id: String,
    present_paths: Vec<NormalizedPath>,
}

impl QueuedInsightJob {
    pub fn new(id: impl Into<String>, present_paths: Vec<NormalizedPath>) -> Self {
        Self {
            id: id.into(),
            present_paths,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_wanted_by(&self, is_visible: &dyn Fn(&NormalizedPath) -> bool) -> bool {
        any_path_is_wanted(&self.present_paths, is_visible)
    }
}

/// Whether a revision present at `paths` is still work this installation asks for.
///
/// One rule for both sides of the queue: the worker asks it of the job it has just claimed,
/// and the sweep asks it of every job still waiting. A revision at no present path at all
/// answers no — its file is gone, and paying a model to describe what nobody can open is
/// the same waste by another route.
fn any_path_is_wanted(
    paths: &[NormalizedPath],
    is_visible: &dyn Fn(&NormalizedPath) -> bool,
) -> bool {
    paths.iter().any(is_visible)
}

/// How many parts a revision may have and still be sent without being asked for. A transcript
/// of two thousand replies is one job and one call of several dollars; it is not something a
/// run should spend quietly because it happened to be next in the queue.
pub const PARTS_WITHOUT_ASKING: usize = 40;

/// A queued job a run did not take because its revision is larger than it may send unasked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldJob {
    id: String,
    parts: usize,
    bytes: i64,
}

impl HeldJob {
    pub fn new(id: impl Into<String>, parts: usize, bytes: i64) -> Self {
        Self {
            id: id.into(),
            parts,
            bytes,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn parts(&self) -> usize {
        self.parts
    }

    pub fn bytes(&self) -> i64 {
        self.bytes
    }
}

/// What a run does with jobs larger than it may send unasked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeWork {
    /// Leave every job of more than this many parts queued, and say so.
    HoldOver(usize),
    /// Take them: the owner asked for it by name.
    Take,
}

/// What one recovery pass moved. The two counters never describe the same job: a job is
/// either put back in the queue or retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsightRecovery {
    returned_to_queue: usize,
    retired_dead: usize,
}

impl InsightRecovery {
    pub fn returned_to_queue(&self) -> usize {
        self.returned_to_queue
    }

    pub fn retired_dead(&self) -> usize {
        self.retired_dead
    }

    pub fn record(&mut self, outcome: InsightRecoveryOutcome, count: usize) {
        match outcome {
            InsightRecoveryOutcome::Requeue => self.returned_to_queue += count,
            InsightRecoveryOutcome::Retire => self.retired_dead += count,
        }
    }
}

/// What one worker pass did. `claimed` is derived from the two outcomes, so the report can
/// never say it finished more jobs than it took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsightWorkReport {
    recovery: InsightRecovery,
    completed: usize,
    failed: usize,
    skipped: usize,
}

impl InsightWorkReport {
    pub fn recovery(&self) -> InsightRecovery {
        self.recovery
    }

    pub fn claimed(&self) -> usize {
        self.completed + self.failed + self.skipped
    }

    pub fn completed(&self) -> usize {
        self.completed
    }

    pub fn failed(&self) -> usize {
        self.failed
    }

    /// Jobs the policy no longer asked for. They cost a claim and nothing else: no model
    /// was called, so this number is the one that says what the queue stopped spending.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    pub fn record_recovery(&mut self, recovery: InsightRecovery) {
        self.recovery = recovery;
    }

    pub fn record_completed(&mut self) {
        self.completed += 1;
    }

    pub fn record_failed(&mut self) {
        self.failed += 1;
    }

    pub fn record_skipped(&mut self) {
        self.skipped += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FailureDisposition, InsightFailureCategory, InsightLease, InsightRecovery,
        InsightRecoveryOutcome, InsightRetryPolicy, InsightWorkReport,
    };

    #[test]
    fn a_claim_expires_a_lease_after_it_was_taken() {
        assert_eq!(InsightLease::of_milliseconds(500).expires_at(1_000), 1_500);
        // A lease of nothing is already over, which is how a test reproduces a worker that
        // died the moment it took the job.
        assert_eq!(InsightLease::of_milliseconds(0).expires_at(1_000), 1_000);
        // A negative duration would free a claim before it was taken, so it is refused.
        assert_eq!(InsightLease::of_milliseconds(-5).expires_at(1_000), 1_000);
        assert_eq!(InsightLease::PRODUCTION.expires_at(0), 300_000);
        assert_eq!(InsightLease::PRODUCTION.expires_at(i64::MAX), i64::MAX);
    }

    #[test]
    fn a_job_returns_to_the_queue_until_it_has_used_up_its_attempts() {
        let policy = InsightRetryPolicy::of_attempts(3);

        for attempts in [0, 1, 2] {
            assert_eq!(policy.after(attempts), InsightRecoveryOutcome::Requeue);
        }
        for attempts in [3, 4] {
            assert_eq!(policy.after(attempts), InsightRecoveryOutcome::Retire);
        }
    }

    #[test]
    fn a_policy_always_allows_at_least_one_attempt() {
        // Zero attempts would retire every job before it was ever tried.
        for requested in [0, -1] {
            let policy = InsightRetryPolicy::of_attempts(requested);
            assert_eq!(policy.after(0), InsightRecoveryOutcome::Requeue);
            assert_eq!(policy.after(1), InsightRecoveryOutcome::Retire);
        }
        assert_eq!(
            InsightRetryPolicy::of_attempts(1),
            InsightRetryPolicy::of_attempts(0)
        );
    }

    #[test]
    fn every_failure_category_has_its_own_stored_spelling_that_reads_back() {
        let mut labels: Vec<&str> = InsightFailureCategory::ALL
            .iter()
            .map(|it| it.label())
            .collect();
        for category in InsightFailureCategory::ALL {
            assert_eq!(
                InsightFailureCategory::from_label(category.label()),
                Some(category)
            );
        }
        labels.sort_unstable();
        labels.dedup();

        assert_eq!(
            labels.len(),
            InsightFailureCategory::ALL.len(),
            "{labels:?}"
        );
        assert_eq!(InsightFailureCategory::Handler.to_string(), "handler");
        assert_eq!(
            InsightFailureCategory::LeaseExpired.label(),
            "lease-expired"
        );
        // A label this program never wrote names no category, so a reader refuses instead
        // of deciding a job's fate from something it does not understand.
        assert_eq!(InsightFailureCategory::from_label("weird"), None);
        assert_eq!(InsightFailureCategory::from_label(""), None);
    }

    #[test]
    fn a_failure_another_attempt_cannot_change_is_permanent() {
        for permanent in [
            InsightFailureCategory::Input,
            InsightFailureCategory::Schema,
            InsightFailureCategory::Refused,
            InsightFailureCategory::InputTooLarge,
        ] {
            assert_eq!(
                permanent.disposition(),
                FailureDisposition::Permanent,
                "{permanent}"
            );
        }
        // The run budget belongs to the run, not to the job: the job is tried again.
        for retryable in [
            InsightFailureCategory::Handler,
            InsightFailureCategory::LeaseExpired,
            InsightFailureCategory::Unavailable,
            InsightFailureCategory::BudgetSpent,
        ] {
            assert_eq!(
                retryable.disposition(),
                FailureDisposition::Retry,
                "{retryable}"
            );
        }
    }

    #[test]
    fn a_report_claims_exactly_the_jobs_it_finished_or_failed() {
        let mut recovery = InsightRecovery::default();
        recovery.record(InsightRecoveryOutcome::Requeue, 2);
        recovery.record(InsightRecoveryOutcome::Retire, 1);
        let mut report = InsightWorkReport::default();
        report.record_recovery(recovery);
        report.record_completed();
        report.record_completed();
        report.record_failed();

        assert_eq!(report.recovery().returned_to_queue(), 2);
        assert_eq!(report.recovery().retired_dead(), 1);
        assert_eq!(report.completed(), 2);
        assert_eq!(report.failed(), 1);
        assert_eq!(report.claimed(), 3);
    }
}
