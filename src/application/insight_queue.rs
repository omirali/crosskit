// Use case: an operator's view of the insight outbox. Read-only and aggregate: the queue
// is inspected by count per state, never by id, path, hash or text.
use crate::domain::InsightJobState;

use super::repository::RepositoryError;

/// Read access to the outbox, separate from the evidence repository so a summary never
/// needs the write contract.
pub trait InsightQueueReader {
    /// How many jobs stand in each state. States with no job report zero rather than being
    /// left out, so a summary is the same shape whatever the queue holds.
    fn insight_jobs_by_state(&self) -> Result<InsightQueueSummary, RepositoryError>;
}

/// One count per state, in the order `InsightJobState::ALL` declares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsightQueueSummary {
    counts: [usize; InsightJobState::ALL.len()],
}

impl InsightQueueSummary {
    pub fn count(&self, state: InsightJobState) -> usize {
        self.counts[Self::index(state)]
    }

    pub fn set(&mut self, state: InsightJobState, count: usize) {
        self.counts[Self::index(state)] = count;
    }

    /// Every state with its count, so a caller renders all of them without naming each.
    pub fn counts(&self) -> impl Iterator<Item = (InsightJobState, usize)> + '_ {
        InsightJobState::ALL
            .into_iter()
            .map(|state| (state, self.count(state)))
    }

    fn index(state: InsightJobState) -> usize {
        InsightJobState::ALL
            .iter()
            .position(|candidate| *candidate == state)
            .expect("InsightJobState::ALL lists every state")
    }
}

pub struct InsightQueueService;

impl InsightQueueService {
    pub fn execute(
        &self,
        reader: &impl InsightQueueReader,
    ) -> Result<InsightQueueSummary, RepositoryError> {
        reader.insight_jobs_by_state()
    }
}

#[cfg(test)]
mod tests {
    use super::{InsightQueueReader, InsightQueueService, InsightQueueSummary};
    use crate::application::RepositoryError;
    use crate::domain::InsightJobState;

    struct StaticQueue(InsightQueueSummary);

    impl InsightQueueReader for StaticQueue {
        fn insight_jobs_by_state(&self) -> Result<InsightQueueSummary, RepositoryError> {
            Ok(self.0)
        }
    }

    #[test]
    fn a_summary_reports_one_count_per_state_and_zero_for_the_rest() {
        let mut summary = InsightQueueSummary::default();
        summary.set(InsightJobState::Queued, 4);
        summary.set(InsightJobState::Dead, 1);

        let reported = InsightQueueService.execute(&StaticQueue(summary)).unwrap();

        assert_eq!(
            reported.counts().collect::<Vec<_>>(),
            vec![
                (InsightJobState::Queued, 4),
                (InsightJobState::Claimed, 0),
                (InsightJobState::Done, 0),
                (InsightJobState::Failed, 0),
                (InsightJobState::Dead, 1),
                (InsightJobState::Skipped, 0),
            ]
        );
    }
}
