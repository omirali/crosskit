use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak,
};

use lbug::{Connection, Database, SystemConfig};

use crate::application::RepositoryError;
use crate::domain::WorkspaceName;
use crate::infrastructure::app_directories;

use super::engine_tuning::EngineTuning;
use super::migration;
use super::repository::LadybugGraphRepository;

/// What a process may do with the graph it opened.
///
/// The engine takes an exclusive lock for a writer and none at all for a reader, so the two
/// are different openings of the same file rather than two ways of using one. Which one a
/// manager holds decides whether a write may even be attempted, and it is part of the key
/// this process caches managers under: a read-only opening must never be handed to a caller
/// that asked to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphAccess {
    ReadWrite,
    ReadOnly,
}

impl GraphAccess {
    fn read_only(self) -> bool {
        self == Self::ReadOnly
    }
}

pub(super) struct DatabaseOwner {
    pub(super) database: Database,
    access: GraphAccess,
    write_lock: Mutex<()>,
    /// Readers in this process against the two moments of a write they may not overlap.
    ///
    /// Measured, twice, as a crash of the whole process (SIGSEGV in `ColumnChunk::scan`): a read
    /// on one connection while another ran a checkpoint, which moves pages the read was about
    /// to copy from. A commit may run the engine's own checkpoint, so it is one of the two
    /// moments; the explicit `CHECKPOINT` is the other. Between them a read beside an open
    /// write transaction is safe and sees the committed state (ADR 0050). Readers take this
    /// shared for a whole question; a commit and a checkpoint take it alone.
    readers: RwLock<()>,
}

impl DatabaseOwner {
    /// The guard every write is taken under, and the one place a graph opened for reading
    /// refuses one. Refused here rather than by the engine mid-statement: a transaction
    /// that cannot write should not be opened at all.
    pub(super) fn write_guard(&self) -> Result<MutexGuard<'_, ()>, RepositoryError> {
        if self.access.read_only() {
            return Err(RepositoryError::Storage(
                "this graph was opened for reading only; nothing may be written through it"
                    .to_string(),
            ));
        }
        self.write_lock.lock().map_err(|_| {
            RepositoryError::Storage("knowledge graph write lock is poisoned".to_string())
        })
    }

    /// Held for a commit and a checkpoint: no reader of this process is inside the engine
    /// while pages move.
    pub(super) fn readers_kept_out(&self) -> RwLockWriteGuard<'_, ()> {
        self.readers.write().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct LadybugDatabaseManager {
    pub(super) owner: DatabaseOwner,
    database_path: PathBuf,
    pub(super) schema_version: u64,
}

/// A reader's hold on a graph another thread of this process writes to: while it lives, no
/// commit and no checkpoint of that graph runs. Taken for one whole question.
pub struct ReadingHold<'a> {
    _shared: RwLockReadGuard<'a, ()>,
}

impl LadybugDatabaseManager {
    /// Holds commits and checkpoints of this graph off for as long as the hold lives — what a
    /// reader in the writer's own process takes before it asks anything.
    pub fn reading(&self) -> ReadingHold<'_> {
        ReadingHold {
            _shared: self
                .owner
                .readers
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        }
    }

    /// The graph of one context. Each context has a database of its own, which is what makes
    /// the boundary a fact rather than a discipline: a query cannot reach another context's
    /// files because they are not in the database it runs against.
    pub fn open_workspace(workspace: &WorkspaceName) -> Result<Arc<Self>, RepositoryError> {
        Self::open(workspace_database_path(workspace)?)
    }

    /// The graph of one context for a command that may not write to it at all.
    ///
    /// Unlike `open_workspace_for_reading`, no opening for writing is tried first, which is
    /// the whole of the difference: a write open creates the database of a context nobody
    /// has indexed, migrates one an older build wrote, and folds the log into the database
    /// when it closes. A report over every context may do none of those, so it takes the
    /// narrower opening and accepts what that costs — a graph whose schema is not this
    /// build's is reported rather than migrated, and what a writer has committed but not
    /// checkpointed is not read.
    pub fn open_workspace_read_only(
        workspace: &WorkspaceName,
    ) -> Result<Arc<Self>, RepositoryError> {
        Self::open_read_only(workspace_database_path(workspace)?)
    }

    pub fn open(database_path: PathBuf) -> Result<Arc<Self>, RepositoryError> {
        Self::open_with(database_path, GraphAccess::ReadWrite)
    }

    /// The read-only half of `open_workspace_for_reading`, named on its own because the
    /// engine's lock is held per process: only a test that opens both halves itself can
    /// show what a read-only opening does and does not allow.
    pub(super) fn open_read_only(database_path: PathBuf) -> Result<Arc<Self>, RepositoryError> {
        Self::open_with(database_path, GraphAccess::ReadOnly)
    }

    fn open_with(
        database_path: PathBuf,
        access: GraphAccess,
    ) -> Result<Arc<Self>, RepositoryError> {
        let database_path = normalized_database_path(database_path)?;
        let key = (database_path.clone(), access);
        let mut managers = database_managers().lock().map_err(|_| {
            RepositoryError::Storage("database manager registry is poisoned".to_string())
        })?;
        managers.retain(|_, manager| manager.strong_count() > 0);
        if let Some(manager) = managers.get(&key).and_then(Weak::upgrade) {
            return Ok(manager);
        }

        let manager = Arc::new(Self::open_new(database_path, access)?);
        managers.insert(key, Arc::downgrade(&manager));
        Ok(manager)
    }

    fn open_new(database_path: PathBuf, access: GraphAccess) -> Result<Self, RepositoryError> {
        let parent = database_path.parent().ok_or_else(|| {
            RepositoryError::Storage(format!(
                "database path {} has no parent directory",
                database_path.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            RepositoryError::Storage(format!(
                "create database directory {}: {error}",
                parent.display()
            ))
        })?;
        let database = Database::new(
            &database_path,
            EngineTuning::production().apply(
                SystemConfig::default()
                    .enable_multi_writes(false)
                    .read_only(access.read_only()),
            ),
        )
        .map_err(|error| open_failure(&database_path, &error))?;
        let connection = Connection::new(&database).map_err(|error| {
            RepositoryError::Storage(format!("open LadybugDB connection: {error}"))
        })?;
        // A reader migrates nothing: it states the schema it found and refuses a database
        // this build would have had to change before reading it.
        let schema_version = match access {
            GraphAccess::ReadWrite => migration::apply(&connection)?,
            GraphAccess::ReadOnly => migration::require_current_schema(&connection)?,
        };
        drop(connection);

        Ok(Self {
            owner: DatabaseOwner {
                database,
                access,
                write_lock: Mutex::new(()),
                readers: RwLock::new(()),
            },
            database_path,
            schema_version,
        })
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn repository(self: &Arc<Self>) -> LadybugGraphRepository {
        LadybugGraphRepository::new(Arc::clone(self))
    }
}

/// The write-ahead log the engine keeps beside `database`, which is the file a recovery
/// moves aside. Named in one place, because the recogniser, the recovery and every message
/// about either have to mean the same file.
pub(super) fn write_ahead_log_of(database: &Path) -> PathBuf {
    beside(database, ".wal")
}

/// The log a checkpoint is folding into `database`: the engine renames the write-ahead log
/// to this when a checkpoint starts and removes it when the checkpoint is done, so it is
/// there only after a writer stopped inside one (ADR 0060).
pub(super) fn checkpoint_log_of(database: &Path) -> PathBuf {
    beside(database, ".wal.checkpoint")
}

/// The pages a checkpoint wrote before copying them into `database`. They mean something
/// only together with the checkpoint's log, and the engine deletes them on its next open
/// when that log is gone.
pub(super) fn shadow_pages_of(database: &Path) -> PathBuf {
    beside(database, ".shadow")
}

/// The log a refusal to open `database` is about: the write-ahead log while it is there,
/// and an interrupted checkpoint's when only that one is.
pub(super) fn refused_log_of(database: &Path) -> PathBuf {
    let ordinary = write_ahead_log_of(database);
    let checkpoint = checkpoint_log_of(database);
    if !ordinary.exists() && checkpoint.exists() {
        checkpoint
    } else {
        ordinary
    }
}

fn beside(database: &Path, suffix: &str) -> PathBuf {
    let mut file = database.as_os_str().to_os_string();
    file.push(suffix);
    PathBuf::from(file)
}

/// What the engine's refusal to open a database means, as a category.
///
/// Two wordings mean the same state, and both were produced by this engine from a database
/// whose log was left the way a stopped writer leaves one: a log extended past its last
/// record reads as `Corrupted wal file. Read out invalid WAL record type.`, and one that
/// ends inside a record as `Checksum verification failed, the WAL file is corrupted.`
/// `read_only_access_tests` provokes both against the engine this build links, so a new
/// wording fails a test instead of quietly becoming an ordinary storage error.
///
/// The lock refusal is the other category worth naming: a database another process holds
/// for writing is not broken, it is busy, and what an operator does about the two could not
/// be more different.
///
/// The engine's text never leaves this function except in the failure nobody recognised,
/// where it is the only thing a reader has to go on.
pub(super) fn open_failure(
    database_path: &Path,
    error: &impl std::fmt::Display,
) -> RepositoryError {
    let reported = error.to_string();
    let lowercase = reported.to_lowercase();
    if lowercase.contains("wal") && lowercase.contains("corrupt") {
        return RepositoryError::GraphLogUnreadable {
            log: refused_log_of(database_path),
        };
    }
    if lowercase.contains("could not set lock") {
        return RepositoryError::GraphBusy;
    }
    RepositoryError::Storage(format!("open LadybugDB: {reported}"))
}

/// The managers this process already holds, keyed by the database and the access it was
/// opened with. Two openings of one database differ in what they may do, so a caller that
/// asked to write is never handed the reader.
type ManagerKey = (PathBuf, GraphAccess);

fn database_managers() -> &'static Mutex<HashMap<ManagerKey, Weak<LadybugDatabaseManager>>> {
    static MANAGERS: OnceLock<Mutex<HashMap<ManagerKey, Weak<LadybugDatabaseManager>>>> =
        OnceLock::new();
    MANAGERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What a graph written before contexts existed is called. It is never opened: nothing
/// records which of its files belong where, and a boundary cannot be drawn through a graph
/// after the fact.
const CONTEXT_FREE_DATABASE_FILE: &str = "graph.lbug";

/// `graph-<context>.lbug` next to the policy file. The name is derived from the context's
/// own identity, which refused anything that is not a plain name, so nothing here has to
/// escape or clean up a path.
pub fn workspace_database_path(workspace: &WorkspaceName) -> Result<PathBuf, RepositoryError> {
    let directory = app_directories::data_directory()
        .map_err(|error| RepositoryError::Storage(error.to_string()))?;
    refuse_context_free_graph(&directory)?;
    Ok(directory.join(format!("graph-{workspace}.lbug")))
}

/// A graph from before contexts is refused rather than adopted. Adopting it would put files
/// nobody assigned to a context inside one, which is the single mistake this boundary
/// exists to prevent; the operator is told what to do instead of being guessed at.
fn refuse_context_free_graph(directory: &Path) -> Result<(), RepositoryError> {
    match context_free_graph_in(directory) {
        None => Ok(()),
        Some(legacy) => Err(RepositoryError::Storage(context_free_graph_reason(&legacy))),
    }
}

/// The same question asked without refusing anything, for a report that has to name this
/// once rather than once per context.
///
/// The file belongs to the data directory, not to any context: every context derives its
/// path from that directory, so one file there stops all of them for one reason. `status`
/// printing it per context said one machine-wide condition four times on a machine with
/// four contexts and nothing at all about the contexts themselves.
pub fn context_free_graph() -> Result<Option<PathBuf>, RepositoryError> {
    let directory = app_directories::data_directory()
        .map_err(|error| RepositoryError::Storage(error.to_string()))?;
    Ok(context_free_graph_in(&directory))
}

fn context_free_graph_in(directory: &Path) -> Option<PathBuf> {
    let legacy = directory.join(CONTEXT_FREE_DATABASE_FILE);
    legacy.exists().then_some(legacy)
}

/// What such a file means and what to do about it, spelled once. The refusal above and the
/// report both say it, and a second spelling would be a second rule.
pub fn context_free_graph_reason(legacy: &Path) -> String {
    format!(
        "{} was written before contexts existed and holds files no context claims; move it aside or delete it and index again",
        legacy.display()
    )
}

fn normalized_database_path(database_path: PathBuf) -> Result<PathBuf, RepositoryError> {
    let parent = database_path.parent().ok_or_else(|| {
        RepositoryError::Storage(format!(
            "database path {} has no parent directory",
            database_path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        RepositoryError::Storage(format!(
            "create database directory {}: {error}",
            parent.display()
        ))
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|error| {
        RepositoryError::Storage(format!(
            "canonicalize database directory {}: {error}",
            parent.display()
        ))
    })?;
    let filename = database_path.file_name().ok_or_else(|| {
        RepositoryError::Storage(format!(
            "database path {} has no filename",
            database_path.display()
        ))
    })?;
    Ok(canonical_parent.join(filename))
}
