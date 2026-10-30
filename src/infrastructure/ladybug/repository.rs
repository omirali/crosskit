// Transaction orchestration for Ladybug evidence persistence.
use std::collections::HashMap;
use std::sync::Arc;

use lbug::Connection;

use super::batch_projection::{BatchProjection, ObservationRow, Projected};
use super::conversation_store;
use super::current_state_store::CurrentCandidate;
use super::manager::LadybugDatabaseManager;
use super::migration;
use super::observation_store::ObservationOrderKey;
use super::root_coverage_store;
use super::store_support::{current_unix_ms, storage_error};
use super::{
    chunk_store, current_state_store, file_version_store, insight_job_store, insight_work_store,
    inventory_store, knowledge_store, location_store, observation_store, reconcile_store,
    run_retention_store, touch_store, work_scope_store,
};
use crate::application::{
    CommitResult, FileIdentityHistory, GraphCheckpoint, GraphInventory, GraphInventoryReader,
    InsightJobQueue, InsightQueueReader, InsightQueueSummary, InsightQueueWithdrawal,
    InventoriedRevision, JobScope, KnowledgeGraphRepository, KnowledgeReader, KnowledgeTotals,
    PassAttemptLog, PassFailure, ReconcileRepository, ReconcileRunMaintenance, RepositoryError,
    RetirementReport, RevisionInventory, RevisionSource, RootCoverage, RootCoverageReader,
    SourceBatchWriter, StoredTouch, TouchReader,
};
use crate::application::{DESCRIBED_BY, KnowledgeRewrite, Requeued, RevisionSize, WorkScopeReader};
use crate::domain::HeldJob;
use crate::domain::knowledge::{StoredFact, StoredMention};
use crate::domain::{
    ClaimedInsightJob, InsightFailureCategory, InsightFinding, InsightLease, InsightRecovery,
    InsightResult, InsightRetryPolicy, KnownIdentity, NormalizedPath, QueuedInsightJob,
    ReconcileRun, RetentionMode, RunRetention, RunRetentionReport, SourceCommit,
};

#[derive(Clone)]
pub struct LadybugGraphRepository {
    manager: Arc<LadybugDatabaseManager>,
}

impl LadybugGraphRepository {
    pub(super) fn new(manager: Arc<LadybugDatabaseManager>) -> Self {
        Self { manager }
    }

    /// A connection for a read of what is already durable: no transaction and no write
    /// lock, so it answers on a graph this process opened for reading only.
    pub(super) fn read_connection(&self) -> Result<Connection<'_>, RepositoryError> {
        Connection::new(&self.manager.owner.database)
            .map_err(|error| storage_error("open LadybugDB connection", error))
    }

    /// Runs `operation` in one transaction under the shared write lock. Every store
    /// receives the same open connection and never begins a transaction of its own, so a
    /// failure in any step rolls back everything the operation wrote.
    fn write_transaction<T>(
        &self,
        operation: impl FnOnce(&Connection<'_>) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let _write_guard = self.manager.owner.write_guard()?;
        let connection = Connection::new(&self.manager.owner.database)
            .map_err(|error| storage_error("open LadybugDB connection", error))?;
        migration::in_transaction_committing(&connection, operation, || {
            self.manager.owner.readers_kept_out()
        })
    }
}

/// Every unseen-location page is bounded to this many rows inside the finalizing
/// transaction, so reconciling a large tree never materializes all candidates at once.
const RETIREMENT_PAGE_SIZE: usize = 500;

/// Roots and closed runs are read in pages of this many rows inside one retention
/// transaction, so pruning a long history never materializes every run at once.
const RETENTION_PAGE_SIZE: usize = 500;

/// The location first, then the evidence, then the outbox entry for the revision: a
/// failure anywhere leaves nothing behind, so a revision never exists without its queued
/// insight job and a job never exists without its revision. The sighting is stamped with
/// the current reconciliation epoch, whichever command writes it.
fn persist_source(
    connection: &Connection<'_>,
    commit: &SourceCommit,
) -> Result<(), RepositoryError> {
    let epoch = reconcile_store::current_epoch(connection)?;
    location_store::persist_location(connection, commit.location(), epoch)?;
    let Some(evidence) = commit.evidence() else {
        return Ok(());
    };

    file_version_store::persist_file_version(connection, evidence.file_version())?;
    let order = observation_store::persist_observation(connection, evidence.observation())?;
    chunk_store::persist_chunks(connection, evidence.file_version(), evidence.chunks())?;
    current_state_store::advance_current_state(
        connection,
        evidence.file_version(),
        evidence.observation(),
        order,
    )?;
    // Only for a revision an analysis rule covers. The same question the batch path asks,
    // asked of the same commit, so the two paths cannot queue different work.
    if let Some(job) = commit.insight_job() {
        insight_job_store::persist_insight_job(connection, &job)?;
    }
    Ok(())
}

fn chunk_count(commit: &SourceCommit) -> usize {
    commit
        .evidence()
        .map_or(0, |evidence| evidence.chunks().len())
}

/// Publishes a whole batch inside the caller's transaction, in the order the scan read
/// it. The batch is projected into typed rows once and then written by each store in the
/// same domain order as a single source: location and file, revision, observation and its
/// order, chunks and extractors, current state, outbox entry. Every statement is static
/// and takes its values as `UNWIND` parameters, so a batch of a hundred sources costs a
/// fixed number of queries instead of a hundred times as many. A failure anywhere aborts
/// the transaction, so a batch is all or nothing and no source is ever half written.
pub(super) fn persist_batch(
    connection: &Connection<'_>,
    commits: &[SourceCommit],
) -> Result<(), RepositoryError> {
    if commits.is_empty() {
        return Ok(());
    }
    let projection = match BatchProjection::of(commits)? {
        // This batch repeats a location, either the same one twice or two of them at one
        // path, and only applying its sources one after another has a defined outcome.
        Projected::RequiresSequential => return persist_sources_one_by_one(connection, commits),
        Projected::SetBased(projection) => projection,
    };

    let epoch = reconcile_store::current_epoch(connection)?;
    location_store::persist_location_batch(
        connection,
        &projection.files,
        &projection.locations,
        epoch,
    )?;
    file_version_store::persist_file_version_batch(connection, &projection.versions)?;
    let order_keys =
        observation_store::persist_observation_batch(connection, &projection.observations)?;
    chunk_store::persist_chunk_batch(connection, &projection.chunks, &projection.extractors)?;
    conversation_store::persist_conversation_batch(
        connection,
        &projection.conversations,
        &projection.messages,
        &projection.touches,
    )?;
    current_state_store::advance_current_state_batch(
        connection,
        &current_candidates(&projection.observations, &order_keys)?,
    )?;
    insight_job_store::persist_insight_job_batch(connection, &projection.jobs)
}

/// The observation each file of the batch proposes as its current state: the newest by
/// `(observed_at_unix_ms, sequence)`, which is the one a source-by-source run would end
/// on. Sorted by file so the statements receive their rows in a fixed order.
fn current_candidates(
    observations: &[ObservationRow<'_>],
    order_keys: &HashMap<String, ObservationOrderKey>,
) -> Result<Vec<CurrentCandidate>, RepositoryError> {
    let mut best: HashMap<&str, CurrentCandidate> = HashMap::new();
    for row in observations {
        let key = *order_keys.get(row.id).ok_or_else(|| {
            RepositoryError::InvariantViolation(format!(
                "file observation {} was published without an order key",
                row.id
            ))
        })?;
        let wins = best
            .get(row.file_id)
            .is_none_or(|current| key > current.key);
        if wins {
            best.insert(
                row.file_id,
                CurrentCandidate {
                    file_id: row.file_id.to_string(),
                    version_id: row.file_version_id.to_string(),
                    observation_id: row.id.to_string(),
                    key,
                },
            );
        }
    }
    let mut candidates: Vec<CurrentCandidate> = best.into_values().collect();
    candidates.sort_by(|left, right| left.file_id.cmp(&right.file_id));
    Ok(candidates)
}

/// The fallback for a batch the set-based path cannot decide on its own. It writes the
/// same stores through the single-source orchestration, one source at a time.
fn persist_sources_one_by_one(
    connection: &Connection<'_>,
    commits: &[SourceCommit],
) -> Result<(), RepositoryError> {
    for commit in commits {
        persist_source(connection, commit)?;
    }
    Ok(())
}

/// One batch is one transaction under the shared write lock.
impl SourceBatchWriter for LadybugGraphRepository {
    fn commit_batch(&self, commits: Vec<SourceCommit>) -> Result<(), RepositoryError> {
        self.write_transaction(|connection| persist_batch(connection, &commits))
    }

    /// The run is verified inside the transaction, before the batch writes anything: a
    /// forged or already closed run cannot publish a single source of it.
    fn commit_observed_batch(
        &self,
        run: &ReconcileRun,
        commits: Vec<SourceCommit>,
    ) -> Result<(), RepositoryError> {
        self.write_transaction(|connection| {
            reconcile_store::require_running_run(connection, run)?;
            persist_batch(connection, &commits)
        })
    }
}

impl FileIdentityHistory for LadybugGraphRepository {
    /// Read outside any transaction and without the write lock: it reports only what is
    /// already durable, and the scan asks it of every file it meets, so it must not pay for
    /// a transaction it does not need.
    fn known_identity(&self, base_id: &str) -> Result<KnownIdentity, RepositoryError> {
        location_store::known_identity(&self.read_connection()?, base_id)
    }
}

impl TouchReader for LadybugGraphRepository {
    /// Read outside any transaction, like every other reading of what is already durable: the
    /// command that asks this writes nothing and opens nothing for writing (ADR 0034).
    fn touches_ending_with(&self, needle: &str) -> Result<Vec<StoredTouch>, RepositoryError> {
        touch_store::touches_ending_with(&self.read_connection()?, needle)
    }

    fn touches_of_conversations_matching(
        &self,
        needle: &str,
    ) -> Result<Vec<StoredTouch>, RepositoryError> {
        touch_store::touches_of_conversations_matching(&self.read_connection()?, needle)
    }
}

impl GraphInventoryReader for LadybugGraphRepository {
    /// Read outside any transaction, like every other count of what is already durable. A
    /// report over every context reaches graphs another process is writing to, and those
    /// are opened for reading only: a read that took the write lock could not run there.
    fn inventory(&self) -> Result<GraphInventory, RepositoryError> {
        inventory_store::inventory(&self.read_connection()?)
    }
}

impl KnowledgeGraphRepository for LadybugGraphRepository {
    /// The single-source compatibility path: one source is a batch of one.
    fn commit_source(&self, commit: SourceCommit) -> Result<CommitResult, RepositoryError> {
        let chunk_count = chunk_count(&commit);
        self.commit_batch(vec![commit])?;
        Ok(CommitResult::new(chunk_count))
    }

    fn schema_version(&self) -> Result<u64, RepositoryError> {
        Ok(self.manager.schema_version)
    }
}

impl GraphCheckpoint for LadybugGraphRepository {
    /// Asks the engine to fold its log into the database, under the same write lock as any
    /// other write: a checkpoint moves pages, and a batch landing halfway through it would
    /// be a second writer.
    fn checkpoint(&self) -> Result<(), RepositoryError> {
        let _write_guard = self.manager.owner.write_guard()?;
        let connection = Connection::new(&self.manager.owner.database)
            .map_err(|error| storage_error("open LadybugDB connection", error))?;
        let _readers_out = self.manager.owner.readers_kept_out();
        connection
            .query("CHECKPOINT;")
            .map(|_| ())
            .map_err(|error| storage_error("checkpoint", error))
    }
}

impl PassAttemptLog for LadybugGraphRepository {
    fn record_failed_pass(
        &self,
        root: &NormalizedPath,
        failure: PassFailure,
    ) -> Result<String, RepositoryError> {
        let at = current_unix_ms()?;
        self.write_transaction(|connection| {
            reconcile_store::record_failed_run(connection, root, failure.label(), at)
        })
    }
}

impl RootCoverageReader for LadybugGraphRepository {
    fn coverage_of_root(&self, root: &NormalizedPath) -> Result<RootCoverage, RepositoryError> {
        let connection = self.read_connection()?;
        root_coverage_store::coverage_of_root(&connection, root)
    }
}

impl ReconcileRepository for LadybugGraphRepository {
    fn begin_reconcile_run(&self, root: &NormalizedPath) -> Result<ReconcileRun, RepositoryError> {
        let started_at = current_unix_ms()?;
        self.write_transaction(|connection| {
            reconcile_store::begin_run(connection, root, started_at)
        })
    }

    fn commit_observed_source(
        &self,
        run: &ReconcileRun,
        commit: SourceCommit,
    ) -> Result<CommitResult, RepositoryError> {
        let chunk_count = chunk_count(&commit);
        self.commit_observed_batch(run, vec![commit])?;
        Ok(CommitResult::new(chunk_count))
    }

    fn finalize_reconcile_run(
        &self,
        run: &ReconcileRun,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<RetirementReport, RepositoryError> {
        let finished_at = current_unix_ms()?;
        self.write_transaction(|connection| {
            reconcile_store::finalize_run(
                connection,
                run,
                finished_at,
                is_visible,
                RETIREMENT_PAGE_SIZE,
            )
        })
    }

    fn abandon_reconcile_run(&self, run: &ReconcileRun) -> Result<(), RepositoryError> {
        let finished_at = current_unix_ms()?;
        self.write_transaction(|connection| {
            reconcile_store::abandon_run(connection, run, finished_at)
        })
    }
}

/// Both passes run in the same transaction shape under the write lock. The preview writes
/// nothing and commits an empty transaction, so it reports exactly what an apply at that
/// moment would remove; the apply deletes every page inside that one transaction.
impl ReconcileRunMaintenance for LadybugGraphRepository {
    fn preview_run_retention(
        &self,
        retention: RunRetention,
    ) -> Result<RunRetentionReport, RepositoryError> {
        self.write_transaction(|connection| {
            run_retention_store::prune_closed_runs(
                connection,
                retention,
                RetentionMode::Preview,
                RETENTION_PAGE_SIZE,
            )
        })
    }

    fn apply_run_retention(
        &self,
        retention: RunRetention,
    ) -> Result<RunRetentionReport, RepositoryError> {
        self.write_transaction(|connection| {
            run_retention_store::prune_closed_runs(
                connection,
                retention,
                RetentionMode::Apply,
                RETENTION_PAGE_SIZE,
            )
        })
    }
}

impl LadybugGraphRepository {
    /// Queued jobs larger than `most` parts, oldest first: what a run holds back unless asked.
    pub fn queued_jobs_over_parts(&self, most: usize) -> Result<Vec<HeldJob>, RepositoryError> {
        work_scope_store::queued_over_parts(&self.read_connection()?, most)
    }

    /// Retires the knowledge of every revision a newer one replaced, in one transaction under
    /// the write lock. A sweep asks this before it claims anything, so what a newer revision
    /// replaced does not outlive the next sweep (ADR 0048). Returns how many facts went.
    pub fn retire_stale_knowledge(&self) -> Result<usize, RepositoryError> {
        self.write_transaction(knowledge_store::retire_stale_knowledge)
    }
}

/// Read outside any transaction, like every other reading of what is already durable: the
/// commands that show knowledge write nothing and open nothing for writing (ADR 0034).
impl KnowledgeReader for LadybugGraphRepository {
    fn current_mentions(&self) -> Result<Vec<StoredMention>, RepositoryError> {
        knowledge_store::current_mentions(&self.read_connection()?)
    }

    fn current_facts(&self) -> Result<Vec<StoredFact>, RepositoryError> {
        knowledge_store::current_facts(&self.read_connection()?)
    }

    fn revision_sources(
        &self,
        revision_ids: &[String],
    ) -> Result<Vec<RevisionSource>, RepositoryError> {
        knowledge_store::revision_sources(&self.read_connection()?, revision_ids)
    }

    fn knowledge_totals(&self) -> Result<KnowledgeTotals, RepositoryError> {
        knowledge_store::knowledge_totals(&self.read_connection()?)
    }
}

/// Each rewrite is a transaction of its own under the write lock, like a completed job: a
/// recheck stopped halfway leaves every revision either as it was or as the new rules say.
impl KnowledgeRewrite for LadybugGraphRepository {
    fn analysed_current_jobs(&self) -> Result<Vec<ClaimedInsightJob>, RepositoryError> {
        knowledge_store::analysed_current_jobs(&self.read_connection()?)
    }

    fn rewrite_knowledge(
        &self,
        job: &ClaimedInsightJob,
        finding: &InsightFinding,
    ) -> Result<(), RepositoryError> {
        let at = current_unix_ms()?;
        self.write_transaction(|connection| {
            knowledge_store::rewrite_knowledge(connection, job, finding, at)
        })
    }

    fn requeue_asked_otherwise(
        &self,
        current_prompt: &str,
        job_ids: Option<&[String]>,
    ) -> Result<Requeued, RepositoryError> {
        self.write_transaction(|connection| {
            work_scope_store::requeue_asked_otherwise(
                connection,
                DESCRIBED_BY,
                current_prompt,
                job_ids,
            )
        })
    }
}

/// Read outside any transaction: choosing the work writes nothing; the claim that follows does.
impl WorkScopeReader for LadybugGraphRepository {
    fn current_revisions_under(
        &self,
        directory: &NormalizedPath,
    ) -> Result<Vec<String>, RepositoryError> {
        work_scope_store::current_revisions_under(&self.read_connection()?, directory)
    }

    fn touches_under(
        &self,
        directory: &NormalizedPath,
    ) -> Result<Vec<StoredTouch>, RepositoryError> {
        touch_store::touches_under(
            &self.read_connection()?,
            &work_scope_store::prefix_of(directory),
        )
    }

    fn current_revisions_of_sessions(
        &self,
        sessions: &[String],
    ) -> Result<Vec<String>, RepositoryError> {
        work_scope_store::current_revisions_of_sessions(&self.read_connection()?, sessions)
    }

    fn queued_among(&self, job_ids: &[String]) -> Result<usize, RepositoryError> {
        work_scope_store::queued_among(&self.read_connection()?, job_ids)
    }

    fn existing_among(&self, job_ids: &[String]) -> Result<usize, RepositoryError> {
        work_scope_store::existing_among(&self.read_connection()?, job_ids)
    }

    fn revision_sizes(
        &self,
        revision_ids: &[String],
    ) -> Result<Vec<RevisionSize>, RepositoryError> {
        work_scope_store::revision_sizes(&self.read_connection()?, revision_ids)
    }
}

impl InsightQueueReader for LadybugGraphRepository {
    /// Read outside any transaction and without the write lock, which is what the port
    /// says a summary is: counting the states a queue stands in needs neither, and taking
    /// the write lock for it would make the summary unreadable on a graph opened for
    /// reading only — which is every graph a report over all contexts meets.
    fn insight_jobs_by_state(&self) -> Result<InsightQueueSummary, RepositoryError> {
        insight_work_store::count_jobs_by_state(&self.read_connection()?)
    }
}

/// Each method is one transaction, so a job is claimed, completed, failed or recovered
/// whole. The moment each of them works from is read before the transaction opens, the
/// same way a reconciliation run takes its start and finish times.
impl InsightJobQueue for LadybugGraphRepository {
    fn recover_insight_jobs(
        &self,
        policy: InsightRetryPolicy,
    ) -> Result<InsightRecovery, RepositoryError> {
        let now = current_unix_ms()?;
        self.write_transaction(|connection| {
            insight_work_store::recover_jobs(connection, policy, now)
        })
    }

    fn claim_insight_jobs(
        &self,
        limit: usize,
        lease: InsightLease,
        scope: JobScope<'_>,
    ) -> Result<Vec<ClaimedInsightJob>, RepositoryError> {
        let now = current_unix_ms()?;
        self.write_transaction(|connection| {
            insight_work_store::claim_jobs(connection, limit, lease, now, scope)
        })
    }

    fn skip_insight_job(&self, job: &ClaimedInsightJob) -> Result<(), RepositoryError> {
        self.write_transaction(|connection| insight_work_store::skip_job(connection, job.id()))
    }

    fn complete_insight_job(
        &self,
        job: &ClaimedInsightJob,
        finding: InsightFinding,
    ) -> Result<(), RepositoryError> {
        let result = InsightResult::of(job, finding, current_unix_ms()?);
        self.write_transaction(|connection| insight_work_store::complete_job(connection, &result))
    }

    fn fail_insight_job(
        &self,
        job: &ClaimedInsightJob,
        category: InsightFailureCategory,
    ) -> Result<(), RepositoryError> {
        self.write_transaction(|connection| insight_work_store::fail_job(connection, job, category))
    }
}

/// The waiting half of the queue, read and retired in the same transaction shape as every
/// other write here: one page, one transaction, so an interrupted sweep leaves whole pages.
impl InsightQueueWithdrawal for LadybugGraphRepository {
    fn queued_insight_job_page(
        &self,
        after: &str,
        limit: usize,
    ) -> Result<Vec<QueuedInsightJob>, RepositoryError> {
        self.write_transaction(|connection| {
            insight_work_store::queued_job_page(connection, after, limit)
        })
    }

    fn withdraw_insight_jobs(&self, job_ids: &[String]) -> Result<usize, RepositoryError> {
        self.write_transaction(|connection| {
            insight_work_store::withdraw_queued_jobs(connection, job_ids)
        })
    }
}

impl RevisionInventory for LadybugGraphRepository {
    fn current_revision_page(
        &self,
        after: &str,
        limit: usize,
    ) -> Result<Vec<InventoriedRevision>, RepositoryError> {
        self.write_transaction(|connection| {
            insight_job_store::current_revision_page(connection, after, limit)
        })
    }

    fn queue_insight_jobs(&self, revision_ids: &[String]) -> Result<usize, RepositoryError> {
        self.write_transaction(|connection| {
            insight_job_store::queue_missing_insight_jobs(connection, revision_ids)
        })
    }
}
