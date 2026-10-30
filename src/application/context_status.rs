// Use case: what one context's graph holds, for a report over every context.
//
// Read-only and aggregate, like the outbox summary it includes: counts and a schema
// version, never an id, a path or a text. Nothing here decides anything — which contexts
// are asked, and what is printed about the ones whose graph was not opened at all, is the
// caller's.
use super::insight_queue::{InsightQueueReader, InsightQueueSummary};
use super::repository::{KnowledgeGraphRepository, RepositoryError};

/// How much evidence a context's graph holds.
pub trait GraphInventoryReader {
    fn inventory(&self) -> Result<GraphInventory, RepositoryError>;
}

/// The two counts that say whether a context has been indexed and how much history it
/// carries: how many files it knows, and how many revisions of them it kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GraphInventory {
    files: u64,
    revisions: u64,
}

impl GraphInventory {
    pub fn new(files: u64, revisions: u64) -> Self {
        Self { files, revisions }
    }

    pub fn files(&self) -> u64 {
        self.files
    }

    /// Every revision the graph kept, earlier ones included. Only the current revision of
    /// each file is searchable, which is why this count and the index's are not compared.
    pub fn revisions(&self) -> u64 {
        self.revisions
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextStatusReport {
    schema_version: u64,
    inventory: GraphInventory,
    jobs: InsightQueueSummary,
}

impl ContextStatusReport {
    pub fn schema_version(&self) -> u64 {
        self.schema_version
    }

    pub fn inventory(&self) -> GraphInventory {
        self.inventory
    }

    pub fn jobs(&self) -> InsightQueueSummary {
        self.jobs
    }
}

pub struct ContextStatusService<'a, Graph> {
    graph: &'a Graph,
}

impl<'a, Graph> ContextStatusService<'a, Graph>
where
    Graph: KnowledgeGraphRepository + GraphInventoryReader + InsightQueueReader,
{
    pub fn new(graph: &'a Graph) -> Self {
        Self { graph }
    }

    pub fn execute(&self) -> Result<ContextStatusReport, RepositoryError> {
        Ok(ContextStatusReport {
            schema_version: self.graph.schema_version()?,
            inventory: self.graph.inventory()?,
            jobs: self.graph.insight_jobs_by_state()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextStatusService, GraphInventory, GraphInventoryReader};
    use crate::application::{
        CommitResult, InsightQueueReader, InsightQueueSummary, KnowledgeGraphRepository,
        RepositoryError,
    };
    use crate::domain::{InsightJobState, SourceCommit};

    struct StaticGraph {
        inventory: GraphInventory,
        jobs: InsightQueueSummary,
    }

    impl KnowledgeGraphRepository for StaticGraph {
        fn commit_source(&self, _commit: SourceCommit) -> Result<CommitResult, RepositoryError> {
            unreachable!("a status report writes nothing")
        }

        fn schema_version(&self) -> Result<u64, RepositoryError> {
            Ok(12)
        }
    }

    impl GraphInventoryReader for StaticGraph {
        fn inventory(&self) -> Result<GraphInventory, RepositoryError> {
            Ok(self.inventory)
        }
    }

    impl InsightQueueReader for StaticGraph {
        fn insight_jobs_by_state(&self) -> Result<InsightQueueSummary, RepositoryError> {
            Ok(self.jobs)
        }
    }

    /// One report carries all three answers, each from the reader that owns it.
    #[test]
    fn a_report_carries_the_schema_the_inventory_and_the_queue() {
        let mut jobs = InsightQueueSummary::default();
        jobs.set(InsightJobState::Queued, 7);
        let graph = StaticGraph {
            inventory: GraphInventory::new(120, 131),
            jobs,
        };

        let report = ContextStatusService::new(&graph).execute().unwrap();

        assert_eq!(report.schema_version(), 12);
        assert_eq!(report.inventory().files(), 120);
        assert_eq!(report.inventory().revisions(), 131);
        assert_eq!(report.jobs().count(InsightJobState::Queued), 7);
        assert_eq!(report.jobs().count(InsightJobState::Dead), 0);
    }

    /// More revisions than files is the ordinary state of a graph that has seen edits, and
    /// the report states both rather than deriving one from the other.
    #[test]
    fn revisions_are_counted_apart_from_files() {
        let inventory = GraphInventory::new(3, 11);
        assert_eq!((inventory.files(), inventory.revisions()), (3, 11));
    }
}
