// Use case: take queued insight jobs and bring them to a result.
//
// One pass does three things in order. It first recovers whatever no worker holds any
// more — claims whose lease ran out and jobs that failed — putting each back in the queue
// or retiring it when its attempts are spent. It then claims jobs a page at a time, oldest
// revision first, never more than the caller asked for. Each claimed job is handed
// to the handler outside any transaction, and its outcome is written in one transaction of
// its own.
//
// The handler is a port. This stage's implementation is deterministic; an AI provider will
// take the same place without the lifecycle changing, because nothing here knows what the
// work is, only that it either produced a result or did not.
use crate::domain::NormalizedPath;
use crate::domain::{
    ClaimedInsightJob, InsightFailureCategory, InsightFinding, InsightLease, InsightRecovery,
    InsightRetryPolicy, InsightWorkReport,
};

use super::repository::{GraphCheckpoint, RepositoryError};

/// How many jobs one claim takes. The queue is walked a page at a time so no statement
/// materializes it whole, whatever the caller's limit is.
const CLAIM_PAGE: usize = 100;

/// How many jobs a pass finishes before it asks the engine to fold its log into the
/// database.
///
/// Memory, not durability: what the log holds is already durable. Measured in Stage 17,
/// against the engine with nothing of this program above it: twenty thousand transactions
/// that each write one node cost 86 MiB, and twenty thousand that each write a node and a
/// relationship cost 639 MiB — the engine holds what a relationship costs until a
/// checkpoint. Folding every thousand brings the same run to 177 MiB for 8% more wall
/// time. Without it a worker's memory grows with the number of jobs it works, and under a
/// hard limit the kernel ends it partway through.
///
/// A pass shorter than this never reaches one, which is right: a watcher that works twenty
/// jobs a pass has its own checkpoint after every pass, and a one-off `index jobs run` that
/// small never grows enough to matter.
const CHECKPOINT_EVERY: usize = 1_000;

/// Which queued jobs a pass may take. The order inside is the queue's own, oldest revision
/// first, whichever jobs are in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobScope<'a> {
    /// The whole queue.
    Everything,
    /// Only these jobs, by id: the work under one directory, or of the conversations that
    /// worked there. Every other job stays queued, untouched.
    Among(&'a [String]),
    /// The whole queue but these jobs: the ones held back as too large to send unasked.
    Except(&'a [String]),
}

/// The durable side of the lifecycle. Every method is one transaction: a job is claimed,
/// completed, failed or recovered entirely or not at all.
pub trait InsightJobQueue {
    /// Frees jobs no worker holds: a claim whose lease has run out and a job that failed
    /// go back in the queue, unless `policy` says their attempts are spent, in which case
    /// they are retired. Bounded pages; a retired job is never returned again.
    fn recover_insight_jobs(
        &self,
        policy: InsightRetryPolicy,
    ) -> Result<InsightRecovery, RepositoryError>;

    /// Claims at most `limit` queued jobs of `scope`, **oldest revision first**: the job whose
    /// revision was observed earliest is taken first, whatever order the rows were written
    /// in. Each claim counts an attempt and holds the job for `lease`.
    fn claim_insight_jobs(
        &self,
        limit: usize,
        lease: InsightLease,
        scope: JobScope<'_>,
    ) -> Result<Vec<ClaimedInsightJob>, RepositoryError>;

    /// Retires a job the policy no longer asks for, without a result and without counting
    /// the attempt against it. Terminal until the revision is published again.
    fn skip_insight_job(&self, job: &ClaimedInsightJob) -> Result<(), RepositoryError>;

    /// Stores the job's result and marks it done, together. A job that already has a
    /// result is refused by the schema, so completing twice cannot store a second one.
    fn complete_insight_job(
        &self,
        job: &ClaimedInsightJob,
        finding: InsightFinding,
    ) -> Result<(), RepositoryError>;

    /// Marks the job failed and records why, as a category. Nothing partial is stored:
    /// a failed job has no result.
    fn fail_insight_job(
        &self,
        job: &ClaimedInsightJob,
        category: InsightFailureCategory,
    ) -> Result<(), RepositoryError>;
}

/// What actually does the work of one job. It receives the claim and nothing else, and
/// answers with a finding or with the category of its failure; it never reports an engine's
/// or a provider's own words, and never anything read out of the file.
pub trait InsightHandler {
    fn handle(&self, job: &ClaimedInsightJob) -> Result<InsightFinding, InsightFailureCategory>;
}

pub struct InsightWorkerService<'a, Queue, Handler> {
    queue: &'a Queue,
    handler: &'a Handler,
    lease: InsightLease,
    retry: InsightRetryPolicy,
}

impl<'a, Queue: InsightJobQueue + GraphCheckpoint, Handler: InsightHandler>
    InsightWorkerService<'a, Queue, Handler>
{
    /// A worker with the lease and retry limit a released Crosskit runs with.
    pub fn new(queue: &'a Queue, handler: &'a Handler) -> Self {
        Self::with_lifecycle(
            queue,
            handler,
            InsightLease::PRODUCTION,
            InsightRetryPolicy::PRODUCTION,
        )
    }

    /// A worker with an explicit lease and retry limit, so a test can reach the states a
    /// five-minute lease would only reach after five minutes.
    pub fn with_lifecycle(
        queue: &'a Queue,
        handler: &'a Handler,
        lease: InsightLease,
        retry: InsightRetryPolicy,
    ) -> Self {
        Self {
            queue,
            handler,
            lease,
            retry,
        }
    }

    /// The pass over the whole queue, as every test of the lifecycle itself asks for it.
    #[cfg(test)]
    pub fn execute(
        &self,
        limit: usize,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<InsightWorkReport, RepositoryError> {
        self.execute_within(limit, is_visible, JobScope::Everything)
    }

    /// Recovers the queue, then works at most `limit` jobs of `scope`. Stops early when the
    /// scope runs out. A failure of the durable side ends the pass: it is not a property of one
    /// job. Recovery covers the whole queue whatever the scope: a claim nobody holds any more
    /// is freed whatever was asked for this time.
    ///
    /// Every `CHECKPOINT_EVERY` finished jobs the engine is asked to fold its log, because
    /// otherwise the memory of this process grows with the number of jobs it works. A
    /// checkpoint that fails ends the pass rather than being passed over: what is done stays
    /// done, and carrying on would mean growing until something else stops the process.
    pub fn execute_within(
        &self,
        limit: usize,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
        scope: JobScope<'_>,
    ) -> Result<InsightWorkReport, RepositoryError> {
        let mut report = InsightWorkReport::default();
        report.record_recovery(self.queue.recover_insight_jobs(self.retry)?);

        let mut remaining = limit;
        let mut since_checkpoint = 0;
        while remaining > 0 {
            let page =
                self.queue
                    .claim_insight_jobs(remaining.min(CLAIM_PAGE), self.lease, scope)?;
            if page.is_empty() {
                break;
            }
            for job in &page {
                self.work(job, is_visible, &mut report)?;
                remaining -= 1;
                since_checkpoint += 1;
                if since_checkpoint == CHECKPOINT_EVERY {
                    self.queue.checkpoint()?;
                    since_checkpoint = 0;
                }
            }
        }
        Ok(report)
    }

    /// The handler runs outside any transaction; only its outcome is written, in one.
    fn work(
        &self,
        job: &ClaimedInsightJob,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
        report: &mut InsightWorkReport,
    ) -> Result<(), RepositoryError> {
        // Asked here and not when the job was queued, because a queue outlives the rules
        // that filled it. Two things make a job unwanted and both answer here: the policy no
        // longer admits the file's path, or no analysis rule covers it any more. Neither has
        // a state of its own — one outcome, said once, with no model called.
        if !job.is_wanted_by(is_visible) {
            self.queue.skip_insight_job(job)?;
            report.record_skipped();
            return Ok(());
        }
        match self.handler.handle(job) {
            Ok(finding) => {
                self.queue.complete_insight_job(job, finding)?;
                report.record_completed();
            }
            Err(category) => {
                self.queue.fail_insight_job(job, category)?;
                report.record_failed();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::NormalizedPath;
    use std::cell::RefCell;

    use super::{
        CHECKPOINT_EVERY, CLAIM_PAGE, GraphCheckpoint, InsightHandler, InsightJobQueue,
        InsightWorkerService, JobScope,
    };
    use crate::application::RepositoryError;
    use crate::domain::{
        ClaimedInsightJob, InsightFailureCategory, InsightFinding, InsightLease, InsightProvenance,
        InsightRecovery, InsightRecoveryOutcome, InsightRetryPolicy, RevisionSummary,
    };

    fn counted() -> InsightFinding {
        InsightFinding::counted(
            RevisionSummary::new(1, 5),
            InsightProvenance::local("test-handler"),
        )
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Step {
        Recovered(InsightRetryPolicy),
        Claimed(usize),
        Completed(String),
        Failed(String, InsightFailureCategory),
        Skipped(String),
        Checkpointed,
    }

    /// The predicate of a context that still indexes everything it ever indexed, which is
    /// what every test about the lifecycle itself wants to say about the policy.
    fn everything_visible() -> impl Fn(&NormalizedPath) -> bool {
        |_: &NormalizedPath| true
    }

    /// A queue of ids that hands out claims a page at a time and records every call, so the
    /// order of the lifecycle is checked rather than assumed.
    struct FakeQueue {
        queued: RefCell<Vec<String>>,
        steps: RefCell<Vec<Step>>,
        /// A checkpoint the engine refuses, so the pass that meets one can be checked.
        refuse_checkpoint: bool,
    }

    impl FakeQueue {
        fn holding(ids: &[&str]) -> Self {
            Self::of(ids.iter().map(|id| id.to_string()).collect(), false)
        }

        /// A queue of `count` jobs, for a pass long enough to reach a checkpoint.
        fn of_size(count: usize) -> Self {
            Self::of((0..count).map(|job| format!("job:{job}")).collect(), false)
        }

        fn refusing_to_checkpoint(count: usize) -> Self {
            Self::of((0..count).map(|job| format!("job:{job}")).collect(), true)
        }

        fn of(queued: Vec<String>, refuse_checkpoint: bool) -> Self {
            Self {
                queued: RefCell::new(queued),
                steps: RefCell::new(Vec::new()),
                refuse_checkpoint,
            }
        }

        fn checkpoints(&self) -> usize {
            self.steps
                .borrow()
                .iter()
                .filter(|step| **step == Step::Checkpointed)
                .count()
        }
    }

    impl GraphCheckpoint for FakeQueue {
        fn checkpoint(&self) -> Result<(), RepositoryError> {
            self.steps.borrow_mut().push(Step::Checkpointed);
            if self.refuse_checkpoint {
                return Err(RepositoryError::Storage("checkpoint refused".to_string()));
            }
            Ok(())
        }
    }

    impl InsightJobQueue for FakeQueue {
        fn recover_insight_jobs(
            &self,
            policy: InsightRetryPolicy,
        ) -> Result<InsightRecovery, RepositoryError> {
            self.steps.borrow_mut().push(Step::Recovered(policy));
            let mut recovery = InsightRecovery::default();
            recovery.record(InsightRecoveryOutcome::Requeue, 1);
            Ok(recovery)
        }

        fn claim_insight_jobs(
            &self,
            limit: usize,
            _lease: InsightLease,
            scope: JobScope<'_>,
        ) -> Result<Vec<ClaimedInsightJob>, RepositoryError> {
            self.steps.borrow_mut().push(Step::Claimed(limit));
            let mut queued = self.queued.borrow_mut();
            match scope {
                JobScope::Everything => {}
                JobScope::Among(wanted) => queued.retain(|id| wanted.contains(id)),
                JobScope::Except(held) => queued.retain(|id| !held.contains(id)),
            }
            let taken = limit.min(queued.len());
            Ok(queued
                .drain(..taken)
                .map(|id| {
                    ClaimedInsightJob::new(
                        id,
                        "file:1@hash:a",
                        "hash:a",
                        vec![NormalizedPath::from_absolute("/scan/note.md").unwrap()],
                    )
                })
                .collect())
        }

        fn skip_insight_job(&self, job: &ClaimedInsightJob) -> Result<(), RepositoryError> {
            self.steps
                .borrow_mut()
                .push(Step::Skipped(job.id().to_string()));
            Ok(())
        }

        fn complete_insight_job(
            &self,
            job: &ClaimedInsightJob,
            _finding: InsightFinding,
        ) -> Result<(), RepositoryError> {
            self.steps
                .borrow_mut()
                .push(Step::Completed(job.id().to_string()));
            Ok(())
        }

        fn fail_insight_job(
            &self,
            job: &ClaimedInsightJob,
            category: InsightFailureCategory,
        ) -> Result<(), RepositoryError> {
            self.steps
                .borrow_mut()
                .push(Step::Failed(job.id().to_string(), category));
            Ok(())
        }
    }

    struct CountingHandler;

    impl InsightHandler for CountingHandler {
        fn handle(
            &self,
            _job: &ClaimedInsightJob,
        ) -> Result<InsightFinding, InsightFailureCategory> {
            Ok(counted())
        }
    }

    /// Fails only the jobs it was told to, so one pass can hold both outcomes.
    struct RefusingHandler(&'static [&'static str]);

    impl InsightHandler for RefusingHandler {
        fn handle(
            &self,
            job: &ClaimedInsightJob,
        ) -> Result<InsightFinding, InsightFailureCategory> {
            if self.0.contains(&job.id()) {
                return Err(InsightFailureCategory::Handler);
            }
            Ok(counted())
        }
    }

    #[test]
    fn a_pass_recovers_before_it_claims_anything() {
        let queue = FakeQueue::holding(&["job:a"]);
        let handler = CountingHandler;

        let report = InsightWorkerService::with_lifecycle(
            &queue,
            &handler,
            InsightLease::of_milliseconds(10),
            InsightRetryPolicy::of_attempts(2),
        )
        .execute(4, &everything_visible())
        .unwrap();

        assert_eq!(
            *queue.steps.borrow(),
            vec![
                Step::Recovered(InsightRetryPolicy::of_attempts(2)),
                Step::Claimed(4),
                Step::Completed("job:a".to_string()),
                Step::Claimed(3),
            ]
        );
        assert_eq!(report.recovery().returned_to_queue(), 1);
        assert_eq!(report.completed(), 1);
        assert_eq!(report.claimed(), 1);
    }

    #[test]
    fn a_pass_never_works_more_jobs_than_it_was_asked_for() {
        let queue = FakeQueue::holding(&["job:a", "job:b", "job:c"]);
        let handler = CountingHandler;

        let report = InsightWorkerService::new(&queue, &handler)
            .execute(2, &everything_visible())
            .unwrap();

        assert_eq!(report.claimed(), 2);
        assert_eq!(report.completed(), 2);
        assert_eq!(queue.queued.borrow().len(), 1, "the third job is untouched");
        // The limit bounds the claim, and a limit above one page is still claimed a page
        // at a time.
        assert_eq!(queue.steps.borrow()[1], Step::Claimed(2));
    }

    #[test]
    fn a_claim_never_asks_for_more_than_one_page() {
        let queue = FakeQueue::holding(&[]);
        let handler = CountingHandler;

        InsightWorkerService::new(&queue, &handler)
            .execute(CLAIM_PAGE * 4, &everything_visible())
            .unwrap();

        assert_eq!(queue.steps.borrow()[1], Step::Claimed(CLAIM_PAGE));
    }

    #[test]
    fn a_refused_job_is_failed_by_category_and_the_pass_carries_on() {
        let queue = FakeQueue::holding(&["job:a", "job:b"]);
        let handler = RefusingHandler(&["job:a"]);

        let report = InsightWorkerService::new(&queue, &handler)
            .execute(2, &everything_visible())
            .unwrap();

        assert_eq!(
            queue.steps.borrow()[2..],
            [
                Step::Failed("job:a".to_string(), InsightFailureCategory::Handler),
                Step::Completed("job:b".to_string()),
            ]
        );
        assert_eq!(report.failed(), 1);
        assert_eq!(report.completed(), 1);
        assert_eq!(report.claimed(), 2);
    }

    #[test]
    fn a_pass_with_nothing_to_do_still_recovers_and_stops() {
        let queue = FakeQueue::holding(&[]);
        let handler = CountingHandler;

        let report = InsightWorkerService::new(&queue, &handler)
            .execute(5, &everything_visible())
            .unwrap();

        assert_eq!(report.claimed(), 0);
        assert_eq!(
            *queue.steps.borrow(),
            vec![
                Step::Recovered(InsightRetryPolicy::PRODUCTION),
                Step::Claimed(5)
            ]
        );
    }

    /// Memory, not durability: without this a worker's memory grows with the number of jobs
    /// it works, and under a hard limit the kernel ends it partway through (Stage 17).
    #[test]
    fn a_long_pass_folds_the_log_every_so_often_and_a_short_one_never_does() {
        let handler = CountingHandler;

        let short = FakeQueue::of_size(CHECKPOINT_EVERY - 1);
        InsightWorkerService::new(&short, &handler)
            .execute(CHECKPOINT_EVERY, &everything_visible())
            .unwrap();
        assert_eq!(short.checkpoints(), 0, "a short pass never reaches one");

        let long = FakeQueue::of_size(CHECKPOINT_EVERY * 2);
        let report = InsightWorkerService::new(&long, &handler)
            .execute(CHECKPOINT_EVERY * 2, &everything_visible())
            .unwrap();

        assert_eq!(report.completed(), CHECKPOINT_EVERY * 2);
        assert_eq!(long.checkpoints(), 2);
        // After the thousandth job, not before it and not between two pages of a hundred.
        let steps = long.steps.borrow();
        let first = steps
            .iter()
            .position(|step| *step == Step::Checkpointed)
            .expect("a long pass folds the log");
        assert_eq!(
            steps[..first]
                .iter()
                .filter(|step| matches!(step, Step::Completed(_)))
                .count(),
            CHECKPOINT_EVERY
        );
    }

    /// A refused checkpoint ends the pass. Carrying on would mean growing until something
    /// else stops the process, and everything already finished stays finished.
    #[test]
    fn a_pass_whose_checkpoint_is_refused_stops_there() {
        let queue = FakeQueue::refusing_to_checkpoint(CHECKPOINT_EVERY * 2);
        let handler = CountingHandler;

        let refused = InsightWorkerService::new(&queue, &handler)
            .execute(CHECKPOINT_EVERY * 2, &everything_visible());

        assert!(refused.is_err(), "the pass must not carry on");
        assert_eq!(queue.checkpoints(), 1);
        assert_eq!(
            queue
                .steps
                .borrow()
                .iter()
                .filter(|step| matches!(step, Step::Completed(_)))
                .count(),
            CHECKPOINT_EVERY,
            "what was done before it stays done"
        );
    }

    #[test]
    fn a_limit_of_zero_recovers_and_claims_nothing() {
        let queue = FakeQueue::holding(&["job:a"]);
        let handler = CountingHandler;

        let report = InsightWorkerService::new(&queue, &handler)
            .execute(0, &everything_visible())
            .unwrap();

        assert_eq!(report.claimed(), 0);
        assert_eq!(
            *queue.steps.borrow(),
            vec![Step::Recovered(InsightRetryPolicy::PRODUCTION)]
        );
    }
}
