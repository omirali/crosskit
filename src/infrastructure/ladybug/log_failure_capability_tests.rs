// The two ways this engine refuses a write-ahead log, against the engine this build links.
//
// The category `GraphLogUnreadable` is decided by the engine's words, the way an exhausted
// page cache is, and for the same reason: the text is the only signal the engine gives. So
// it has to be provoked rather than remembered. A wording this build no longer produces
// fails a test here instead of quietly turning a context that needs recovering into an
// ordinary storage error with no recipe.
//
// Both shapes are what a stopped writer leaves. The file the engine is writing is longer
// than the records it has finished — a copy taken with that tail reads as a record type
// that does not exist — and a file that ends inside a record fails its checksum. Neither is
// fabricated corruption: the bytes come from this program's own writing, and only where the
// log ends differs.
use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::LadybugDatabaseManager;
use super::test_support::{TestDatabase, evidence_commit};
use crate::application::{RepositoryError, SourceBatchWriter};

/// A database with enough committed batches for its log to hold more than the schema, kept
/// open so the log is still beside it.
struct LiveDatabase {
    database: TestDatabase,
    /// Held, not used: while this is alive the database is open, so its log is still on
    /// disk to be copied. Dropping it closes the database, which checkpoints and removes
    /// the log — and there would be nothing left to take a copy of.
    _manager: Arc<LadybugDatabaseManager>,
}

fn live(name: &str) -> LiveDatabase {
    let database = TestDatabase::new(name);
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for batch in 0..10 {
        let commits = (0..20)
            .map(|index| {
                evidence_commit(
                    &format!("file:{batch}-{index}"),
                    &format!("/files/{batch}-{index}.txt"),
                )
            })
            .collect();
        repository.commit_batch(commits).unwrap();
    }
    LiveDatabase {
        database,
        _manager: manager,
    }
}

impl LiveDatabase {
    fn log(&self) -> PathBuf {
        PathBuf::from(format!("{}.wal", self.database.path().display()))
    }

    /// The bytes on disk right now, under a name of their own: what a process killed at
    /// this moment would leave behind.
    fn copy(&self, name: &str) -> TestDatabase {
        let copy = TestDatabase::new(name);
        fs::copy(self.database.path(), copy.path()).unwrap();
        fs::copy(self.log(), format!("{}.wal", copy.path().display())).unwrap();
        copy
    }
}

fn log_of(database: &TestDatabase) -> PathBuf {
    PathBuf::from(format!("{}.wal", database.path().display()))
}

fn set_log_length(database: &TestDatabase, length: u64) {
    OpenOptions::new()
        .write(true)
        .open(log_of(database))
        .unwrap()
        .set_len(length)
        .unwrap();
}

fn open_failure(path: &Path) -> RepositoryError {
    match LadybugDatabaseManager::open(path.to_path_buf()) {
        Ok(_) => panic!("this database was expected not to open"),
        Err(error) => error,
    }
}

/// A log longer than the records in it: the shape every killed pass in the measurements
/// left. The engine reads past the last record and finds a record type that is not one.
#[test]
fn a_log_that_reaches_past_its_last_record_is_recognised_on_this_build() {
    let live = live("log-extended");
    let copy = live.copy("log-extended-copy");
    let length = log_of(&copy).metadata().unwrap().len();
    set_log_length(&copy, length + 64 * 1024);

    assert!(
        matches!(
            open_failure(copy.path()),
            RepositoryError::GraphLogUnreadable { .. }
        ),
        "a context left by a stopped writer must arrive as the category that has a repair, \
         not as a storage error carrying the engine's words"
    );
    drop(live);
}

/// A log that ends inside a record, which is the other way a stopped writer leaves one.
/// The engine says it differently — a failed checksum rather than an impossible record —
/// and it is the same state with the same repair.
#[test]
fn a_log_that_ends_inside_a_record_is_recognised_on_this_build() {
    let live = live("log-truncated");
    let copy = live.copy("log-truncated-copy");
    let length = log_of(&copy).metadata().unwrap().len();
    set_log_length(&copy, length * 9 / 10);

    assert!(
        matches!(
            open_failure(copy.path()),
            RepositoryError::GraphLogUnreadable { .. }
        ),
        "the second wording means the same state and must reach the same category"
    );
    drop(live);
}

/// The copy itself is not the failure: the same bytes without the deformed tail open, so
/// what the two tests above provoke is the log and nothing else about copying a database.
#[test]
fn the_same_database_with_its_log_untouched_opens() {
    let live = live("log-intact");
    let copy = live.copy("log-intact-copy");

    LadybugDatabaseManager::open(copy.path().to_path_buf())
        .expect("a database copied with a whole log opens");
    drop(live);
}

/// The category names the log, because moving that one file is the whole of the repair and
/// a message that named the database instead would send an operator to the wrong file.
#[test]
fn the_category_names_the_log_beside_the_database() {
    let live = live("log-named");
    let copy = live.copy("log-named-copy");
    let length = log_of(&copy).metadata().unwrap().len();
    set_log_length(&copy, length + 64 * 1024);

    match open_failure(copy.path()) {
        RepositoryError::GraphLogUnreadable { log } => {
            // Through the canonical directory, the way every path this program stores is:
            // on macOS a temp path resolves through /private.
            let expected = fs::canonicalize(log_of(&copy).parent().unwrap())
                .unwrap()
                .join(log_of(&copy).file_name().unwrap());
            assert_eq!(log, expected);
        }
        other => panic!("expected the log category, received {other:?}"),
    }
    drop(live);
}
