// Use case: apply the rules of this build to the knowledge already stored, without the model.
//
// The rules that decide whether a fact is proved change — ADR 0048's «Поправка» added that a
// quote names both ends — and the facts written under the old rule do not change with them. A
// fact the new rule refuses would stay on the map until its file changed. So the answers already
// bought are checked again: every analysed job whose revision is still current, its cached
// answer proved against its own chunks, and its knowledge written anew. A job with no cached
// answer is counted and left as it is; asking the model again is a run, not a recheck.
use crate::domain::{ClaimedInsightJob, InsightFailureCategory, InsightFinding};

use super::repository::RepositoryError;

/// The durable side of a recheck.
pub trait KnowledgeRewrite {
    /// The analysed jobs whose revision is still its file's current one.
    fn analysed_current_jobs(&self) -> Result<Vec<ClaimedInsightJob>, RepositoryError>;

    /// Replaces what `job`'s revision states with `finding`, and the counts on its result, in
    /// one transaction.
    fn rewrite_knowledge(
        &self,
        job: &ClaimedInsightJob,
        finding: &InsightFinding,
    ) -> Result<(), RepositoryError>;

    /// Puts back in the queue every analysed job of a current revision that a model answered
    /// under a question other than `current_prompt`, among `job_ids` when given, and forgets
    /// its result. Its knowledge stays until the new answer replaces it.
    fn requeue_asked_otherwise(
        &self,
        current_prompt: &str,
        job_ids: Option<&[String]>,
    ) -> Result<Requeued, RepositoryError>;
}

/// What a requeue put back, and how much it is to ask again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Requeued {
    pub jobs: usize,
    pub parts: usize,
    pub bytes: i64,
}

/// What one recheck did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecheckReport {
    pub rechecked: usize,
    pub without_cached_answer: usize,
    pub failed: usize,
}

/// Rechecks every analysed current job with `recheck`, which answers from the cache or not at
/// all.
pub fn recheck_knowledge(
    store: &dyn KnowledgeRewrite,
    recheck: &dyn Fn(&ClaimedInsightJob) -> Result<Option<InsightFinding>, InsightFailureCategory>,
) -> Result<RecheckReport, RepositoryError> {
    let mut report = RecheckReport::default();
    for job in store.analysed_current_jobs()? {
        match recheck(&job) {
            Ok(Some(finding)) => {
                store.rewrite_knowledge(&job, &finding)?;
                report.rechecked += 1;
            }
            Ok(None) => report.without_cached_answer += 1,
            Err(_) => report.failed += 1,
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{KnowledgeRewrite, recheck_knowledge};
    use crate::application::RepositoryError;
    use crate::domain::{
        ClaimedInsightJob, InsightFailureCategory, InsightFinding, InsightProvenance,
        RevisionSummary,
    };

    struct Store {
        rewritten: RefCell<Vec<String>>,
    }

    impl KnowledgeRewrite for Store {
        fn analysed_current_jobs(&self) -> Result<Vec<ClaimedInsightJob>, RepositoryError> {
            Ok(["cached", "evicted", "broken"]
                .into_iter()
                .map(|id| ClaimedInsightJob::new(id, format!("{id}@r"), "h", vec![]))
                .collect())
        }

        fn rewrite_knowledge(
            &self,
            job: &ClaimedInsightJob,
            _finding: &InsightFinding,
        ) -> Result<(), RepositoryError> {
            self.rewritten.borrow_mut().push(job.id().to_string());
            Ok(())
        }

        fn requeue_asked_otherwise(
            &self,
            _current_prompt: &str,
            _job_ids: Option<&[String]>,
        ) -> Result<super::Requeued, RepositoryError> {
            Ok(super::Requeued::default())
        }
    }

    #[test]
    fn only_a_job_with_a_cached_answer_is_rewritten_and_the_rest_are_counted() {
        let store = Store {
            rewritten: RefCell::new(Vec::new()),
        };

        let report = recheck_knowledge(&store, &|job: &ClaimedInsightJob| match job.id() {
            "cached" => Ok(Some(InsightFinding::counted(
                RevisionSummary::new(1, 1),
                InsightProvenance::local("test"),
            ))),
            "evicted" => Ok(None),
            _ => Err(InsightFailureCategory::Input),
        })
        .unwrap();

        assert_eq!(*store.rewritten.borrow(), ["cached"]);
        assert_eq!(report.rechecked, 1);
        assert_eq!(report.without_cached_answer, 1);
        assert_eq!(report.failed, 1);
    }
}
