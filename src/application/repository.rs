use std::fmt;
use std::path::PathBuf;

use super::context_pass::PassFailure;
use crate::domain::{NormalizedPath, ReconcileRun, SourceCommit};

pub trait KnowledgeGraphRepository {
    /// Persists one source atomically: its location and, when present, its content
    /// evidence either both become visible or neither does.
    ///
    /// The scan path publishes bounded batches through `SourceBatchWriter`; this
    /// single-source path remains for a caller that holds exactly one prepared source,
    /// and is exercised by the storage tests.
    #[allow(
        dead_code,
        reason = "single-source compatibility path kept beside the batch port"
    )]
    fn commit_source(&self, commit: SourceCommit) -> Result<CommitResult, RepositoryError>;

    fn schema_version(&self) -> Result<u64, RepositoryError>;
}

/// Folding what is committed into the database file itself.
///
/// The engine keeps committed batches in a write-ahead log beside the database and folds
/// them in when it checkpoints: on its own once that log grows past its threshold, and when
/// the database is closed. A process that runs for days between closes therefore carries
/// every pass it has done in that log until the threshold is crossed, and a stop that is
/// not a close loses all of it — and leaves a log the engine will not replay. A pass that
/// ends by checkpointing bounds both to the pass in flight.
pub trait GraphCheckpoint {
    fn checkpoint(&self) -> Result<(), RepositoryError>;
}

/// Durable bookkeeping of reconciliation runs.
///
/// Starting a run advances a shared sighting epoch and the run keeps the new value as its
/// sequence. Every committed location, whether written by a run or by a plain scan, is
/// stamped with the epoch current at that moment. A run that finished a complete walk may
/// therefore retire only locations last seen before it started, and no writer can be
/// undercut by a run that was already going.
pub trait ReconcileRepository {
    /// Allocates a durable, ordered run for `root` in status `running`.
    fn begin_reconcile_run(&self, root: &NormalizedPath) -> Result<ReconcileRun, RepositoryError>;

    /// `commit_source` on behalf of `run`, after verifying that `run` is the persisted run
    /// with that id and is still `running`. The sighting is stamped like any other commit.
    /// The single-source counterpart of `commit_observed_batch`.
    #[allow(
        dead_code,
        reason = "single-source compatibility path kept beside the batch port"
    )]
    fn commit_observed_source(
        &self,
        run: &ReconcileRun,
        commit: SourceCommit,
    ) -> Result<CommitResult, RepositoryError>;

    /// Marks `missing` every `present` location inside the run's root that was last seen
    /// in an epoch before this run and that `is_visible` accepts, classifies each retired
    /// location, then records the run as completed. `is_visible` says which paths the run
    /// could have inspected: inside its root and not excluded by its policy. Only such
    /// locations are retired, and only such locations count as relocation destinations,
    /// so a hidden or out-of-scope path never feeds the report. All in one transaction, in
    /// bounded pages. Rejects a run that does not match its persisted identity or is no
    /// longer `running`.
    fn finalize_reconcile_run(
        &self,
        run: &ReconcileRun,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<RetirementReport, RepositoryError>;

    /// Records the run as incomplete without retiring anything. Rejects a run that does
    /// not match its persisted identity or is no longer `running`.
    fn abandon_reconcile_run(&self, run: &ReconcileRun) -> Result<(), RepositoryError>;
}

/// Where a pass that did not happen is written down.
///
/// A port of its own rather than a method on `ReconcileRepository`, because the two are
/// asked by different callers for opposite reasons: a reconciliation writes a run because it
/// is about to walk, and this writes one because a walk did not start. Until Stage 35 the
/// second had nowhere to go — the failure happened before `begin_reconcile_run` — so the
/// only record of a root that was never indexed was a line on somebody's terminal.
pub trait PassAttemptLog {
    /// Records an attempt on `root` that ended without a walk, under the category of the
    /// failure that stopped it. Returns the id of the record, so the command that failed can
    /// name what it left behind.
    fn record_failed_pass(
        &self,
        root: &NormalizedPath,
        failure: PassFailure,
    ) -> Result<String, RepositoryError>;
}

/// What one finalization retired. A retired location is `relocated` when the same `File`
/// still has another location that is `present`, was confirmed in this run's epoch or
/// later and lies at a path this run could inspect: the file identity remains reachable
/// under another visible path. That is a classification by shared identity, not a
/// recorded rename event; hard links produce the same picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetirementReport {
    marked_missing: usize,
    relocated: usize,
}

impl RetirementReport {
    pub fn marked_missing(&self) -> usize {
        self.marked_missing
    }

    pub fn relocated(&self) -> usize {
        self.relocated
    }

    /// Counts one retired location; `relocated` marks the same file as still present.
    pub fn record(&mut self, relocated: bool) {
        self.marked_missing += 1;
        if relocated {
            self.relocated += 1;
        }
    }
}

/// What the single-source compatibility path reports about one published source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "returned by the single-source compatibility path")]
pub struct CommitResult {
    chunk_count: usize,
}

#[allow(dead_code, reason = "returned by the single-source compatibility path")]
impl CommitResult {
    pub fn new(chunk_count: usize) -> Self {
        Self { chunk_count }
    }

    pub fn chunk_count(&self) -> usize {
        self.chunk_count
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    Storage(String),
    InvariantViolation(String),
    /// The database found no memory for the operation: its page cache is full and nothing
    /// in it could be released. It names the operation, which is this program's own word
    /// for it, and carries nothing from the engine, whose message names internal
    /// structures a reader can neither act on nor be shown.
    StorageExhausted(&'static str),
    /// The failing operation could not be undone afterwards, so what it wrote may still be
    /// stored. It carries the failure that started it, which is the one worth reading.
    RollbackFailed(Box<RepositoryError>),
    /// The graph did not open because the write-ahead log beside it cannot be replayed.
    ///
    /// That is what a process stopped without closing the database leaves behind: the
    /// engine folds the log into the database only when it checkpoints, and a log that
    /// ends where the writer was stopped is one it refuses. Nothing in the graph is
    /// readable until that log is out of the way, so this is not a failure of the
    /// operation that met it — it is the state of the context.
    ///
    /// It carries the log, because moving that one file aside is the whole of the repair.
    GraphLogUnreadable {
        log: PathBuf,
    },
    /// The graph is held for writing by another process, and this command needs to write.
    ///
    /// A reader is not refused for this reason: the engine locks the database file for a
    /// writer and takes no lock at all for a reader. A reader that met this could not read
    /// what the writer had written but not yet checkpointed, which is the same category
    /// seen from the other side.
    GraphBusy,
    /// The graph is written in a schema this build does not read, and this opening applies
    /// no migration.
    ///
    /// Typed rather than a message, because two layers have something to say about it and
    /// neither may say the other's half: the numbers are what storage knows, and which
    /// command repairs it is what the command layer knows. While this was a `Storage`
    /// string, the only way to recognise it was to look for words inside it, and the
    /// recogniser then wrote its own copy of the sentence it had just matched.
    SchemaNotThisBuild {
        recorded: u64,
        expected: u64,
    },
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "knowledge graph storage error: {message}"),
            Self::InvariantViolation(message) => {
                write!(f, "knowledge graph invariant violation: {message}")
            }
            Self::StorageExhausted(operation) => write!(
                f,
                "knowledge graph storage ran out of memory during {operation}"
            ),
            Self::RollbackFailed(source) => write!(
                f,
                "knowledge graph transaction could not be rolled back after: {source}"
            ),
            Self::GraphLogUnreadable { log } => write!(
                f,
                "the knowledge graph did not open: its write-ahead log {} ends where the process writing it was stopped, and the storage engine will not replay it",
                log.display()
            ),
            Self::GraphBusy => write!(
                f,
                "the knowledge graph is being written by another process right now"
            ),
            Self::SchemaNotThisBuild { recorded, expected } => write!(
                f,
                "this graph records schema version {recorded} and this build reads version {expected}; an opening that writes nothing applies no migration"
            ),
        }
    }
}

impl std::error::Error for RepositoryError {}
