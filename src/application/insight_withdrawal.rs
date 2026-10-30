// Use case: bring the whole queue to the rules in one pass, without claiming anything.
//
// The claim already asks the policy about every job it takes, and it stays — rules change
// between passes, so a claim that did not ask would spend money on a question nobody is
// asking any more. What it cannot do is reach the work: `--limit` bounds what is claimed
// and only what is worked costs anything, so a queue filled before the exclusions learned
// the machine hands out pages of jobs that are withdrawn for nothing and counted against
// the limit. On the owner's tree a run of twenty claimed twenty and completed three.
//
// So this is the same question asked the other way round: every job still queued, in
// bounded pages, withdrawn where no rule asks for it any more. It is the reverse of the
// backfill an approved mask runs (stage 23) and it is written the same way — a page read,
// a page written, one transaction each, so an interrupted sweep leaves whole pages done
// and the next one finishes it.
//
// Nothing here calls a model or reads a file. A withdrawn job cost nothing and is not
// terminal: the pass that observes its path again puts it back in the queue.
use crate::domain::{NormalizedPath, QueuedInsightJob};

use super::repository::RepositoryError;

/// How many queued jobs one page carries. The same bound as the review's inventory page,
/// for the same reason and on rows of the same weight — a job id and the paths its file is
/// present at — so a queue of a hundred thousand is a hundred statements and never a list.
const PAGE: usize = 1_000;

/// The waiting half of the outbox, read and retired in pages. Separate from the worker's
/// queue port because nothing here claims, attempts or completes anything.
pub trait InsightQueueWithdrawal {
    /// Jobs still queued, ordered by id and starting strictly after `after`. An empty
    /// `after` starts at the beginning, and a page shorter than `limit` is the last one.
    fn queued_insight_job_page(
        &self,
        after: &str,
        limit: usize,
    ) -> Result<Vec<QueuedInsightJob>, RepositoryError>;

    /// Retires the named jobs that are still queued and answers how many moved. One
    /// transaction per call, so a sweep interrupted halfway leaves whole pages withdrawn.
    fn withdraw_insight_jobs(&self, job_ids: &[String]) -> Result<usize, RepositoryError>;
}

/// What one sweep found and what it withdrew. `still_queued` is derived rather than counted
/// again, so the report can never say it withdrew more jobs than it looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsightWithdrawalReport {
    examined: usize,
    withdrawn: usize,
}

impl InsightWithdrawalReport {
    pub fn examined(&self) -> usize {
        self.examined
    }

    /// Jobs no rule asks for any more. They cost nothing: no attempt was made, no model was
    /// called, and the question comes back on its own when the path does.
    pub fn withdrawn(&self) -> usize {
        self.withdrawn
    }

    /// What the queue still holds for a worker to take, which is the number the owner is
    /// actually waiting on.
    pub fn still_queued(&self) -> usize {
        self.examined - self.withdrawn
    }
}

pub struct InsightWithdrawalService;

impl InsightWithdrawalService {
    /// Walks the queue once, withdrawing every job `is_wanted` no longer admits.
    ///
    /// Keyset by job id rather than by the queue's own order: withdrawing moves a job out of
    /// the state the page was read from, so a page never sees what an earlier page already
    /// left behind and the walk ends on the first short page.
    pub fn execute(
        &self,
        queue: &dyn InsightQueueWithdrawal,
        is_wanted: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<InsightWithdrawalReport, RepositoryError> {
        let mut report = InsightWithdrawalReport::default();
        let mut after = String::new();
        loop {
            let page = queue.queued_insight_job_page(&after, PAGE)?;
            let Some(last) = page.last() else {
                return Ok(report);
            };
            after = last.id().to_string();
            let short = page.len() < PAGE;
            report.examined += page.len();
            // Only the ids of one page are ever held, and they are written before the next
            // page is read.
            let unwanted: Vec<String> = page
                .iter()
                .filter(|job| !job.is_wanted_by(is_wanted))
                .map(|job| job.id().to_string())
                .collect();
            report.withdrawn += queue.withdraw_insight_jobs(&unwanted)?;
            if short {
                return Ok(report);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{
        InsightQueueWithdrawal, InsightWithdrawalService, PAGE, QueuedInsightJob, RepositoryError,
    };
    use crate::domain::NormalizedPath;

    fn path(text: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(text).unwrap()
    }

    /// A queue of `count` jobs that records the size of every page it was asked for and
    /// actually removes what is withdrawn, so a sweep that read the queue twice, or held it
    /// whole, is visible in what it asked for.
    struct FakeQueue {
        queued: RefCell<Vec<QueuedInsightJob>>,
        pages: RefCell<Vec<usize>>,
    }

    impl FakeQueue {
        /// Every fourth job is a document, the rest are revisions of somebody else's
        /// package — the shape the owner's queue had.
        fn of(count: usize) -> Self {
            let queued = (0..count)
                .map(|ordinal| {
                    let directory = match ordinal % 4 {
                        0 => "documents",
                        _ => "vendor",
                    };
                    QueuedInsightJob::new(
                        format!("insight-job:{ordinal:08}"),
                        vec![path(&format!("/tree/{directory}/file{ordinal}.md"))],
                    )
                })
                .collect();
            Self {
                queued: RefCell::new(queued),
                pages: RefCell::new(Vec::new()),
            }
        }

        fn holding(jobs: Vec<QueuedInsightJob>) -> Self {
            Self {
                queued: RefCell::new(jobs),
                pages: RefCell::new(Vec::new()),
            }
        }

        fn ids(&self) -> Vec<String> {
            self.queued
                .borrow()
                .iter()
                .map(|job| job.id().to_string())
                .collect()
        }
    }

    impl InsightQueueWithdrawal for FakeQueue {
        fn queued_insight_job_page(
            &self,
            after: &str,
            limit: usize,
        ) -> Result<Vec<QueuedInsightJob>, RepositoryError> {
            self.pages.borrow_mut().push(limit);
            Ok(self
                .queued
                .borrow()
                .iter()
                .filter(|job| job.id() > after)
                .take(limit)
                .cloned()
                .collect())
        }

        fn withdraw_insight_jobs(&self, job_ids: &[String]) -> Result<usize, RepositoryError> {
            self.queued
                .borrow_mut()
                .retain(|job| !job_ids.contains(&job.id().to_string()));
            Ok(job_ids.len())
        }
    }

    fn documents_only() -> impl Fn(&NormalizedPath) -> bool {
        |path: &NormalizedPath| path.as_str().contains("/documents/")
    }

    /// The property the whole point rests on: one sweep reaches every job, whatever the
    /// queue holds, and never asks for more than a page at a time. Before this, reaching the
    /// far end of a queue of 1 703 meant running `index jobs run --limit 20` eighty-six
    /// times, because a claim pays for the skips out of the limit that was set for money.
    #[test]
    fn one_sweep_reaches_a_queue_far_larger_than_one_page_and_never_holds_it() {
        let queue = FakeQueue::of(PAGE * 3 + 7);

        let report = InsightWithdrawalService
            .execute(&queue, &documents_only())
            .unwrap();

        assert_eq!(report.examined(), PAGE * 3 + 7);
        assert_eq!(report.withdrawn(), PAGE * 3 + 7 - report.still_queued());
        assert_eq!(report.still_queued(), (PAGE * 3 + 7).div_ceil(4));
        assert_eq!(queue.queued.borrow().len(), report.still_queued());
        let pages = queue.pages.borrow();
        assert!(pages.len() >= 4, "read in pages: {pages:?}");
        assert!(
            pages.iter().all(|asked| *asked == PAGE),
            "no page may be larger than the bound: {pages:?}"
        );
    }

    /// A job the rules still ask for is left exactly where it was, in the queue and untried.
    /// A sweep that took anything from the worker would be a sweep that costs work.
    #[test]
    fn a_job_the_rules_still_ask_for_is_left_in_the_queue() {
        let queue = FakeQueue::holding(vec![
            QueuedInsightJob::new("insight-job:a", vec![path("/tree/documents/kept.md")]),
            QueuedInsightJob::new("insight-job:b", vec![path("/tree/vendor/gone.md")]),
        ]);

        let report = InsightWithdrawalService
            .execute(&queue, &documents_only())
            .unwrap();

        assert_eq!((report.examined(), report.withdrawn()), (2, 1));
        assert_eq!(queue.ids(), vec!["insight-job:a".to_string()]);
    }

    /// A revision at no present path at all is work nobody can use: the file is gone, and
    /// the job goes with it. The same answer the claim gives, because it is the same rule.
    #[test]
    fn a_job_whose_file_is_at_no_present_path_is_withdrawn() {
        let queue = FakeQueue::holding(vec![QueuedInsightJob::new("insight-job:a", Vec::new())]);

        let report = InsightWithdrawalService.execute(&queue, &|_| true).unwrap();

        assert_eq!((report.examined(), report.withdrawn()), (1, 1));
    }

    #[test]
    fn an_empty_queue_is_examined_once_and_withdraws_nothing() {
        let queue = FakeQueue::holding(Vec::new());

        let report = InsightWithdrawalService
            .execute(&queue, &documents_only())
            .unwrap();

        assert_eq!(report, super::InsightWithdrawalReport::default());
        assert_eq!(*queue.pages.borrow(), vec![PAGE]);
    }
}
