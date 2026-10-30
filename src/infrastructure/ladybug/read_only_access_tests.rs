// What a graph opened for reading allows and refuses.
//
// Only half of this can be shown from one process: the engine's file lock is held per
// process, so a second opening from here is never refused the way a second Crosskit is.
// What a running writer does to a second command is checked from the outside, in
// `tests/cli_concurrent_read.rs`. What is here is the opening itself — that it reads, that
// it writes nothing, and that it refuses a database this build would have had to change
// before reading it.
use lbug::Connection;

use super::LadybugDatabaseManager;
use super::migration;
use super::test_support::{TestDatabase, count, create_v9_fixture, evidence_commit};
use crate::application::{RepositoryError, SourceBatchWriter};

/// A database with one committed source, closed the way a finished command closes it.
fn written_and_closed(test_name: &str) -> TestDatabase {
    let database = TestDatabase::new(test_name);
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    manager
        .repository()
        .commit_batch(vec![evidence_commit("file:1", "/files/one.txt")])
        .unwrap();
    database
}

#[test]
fn a_graph_opened_for_reading_answers_what_the_writer_committed() {
    let database = written_and_closed("read-only-reads");
    let reader = LadybugDatabaseManager::open_read_only(database.path().to_path_buf()).unwrap();

    assert_eq!(count(&reader, "MATCH (:File) RETURN COUNT(*);"), 1);
    assert_eq!(count(&reader, "MATCH (:ContentChunk) RETURN COUNT(*);"), 1);
}

/// The refusal is ours and it happens before a transaction is opened, not inside one: a
/// caller that cannot write should not have begun.
#[test]
fn a_graph_opened_for_reading_refuses_every_write() {
    let database = written_and_closed("read-only-refuses-writes");
    let reader = LadybugDatabaseManager::open_read_only(database.path().to_path_buf()).unwrap();

    let refused = reader
        .repository()
        .commit_batch(vec![evidence_commit("file:2", "/files/two.txt")])
        .unwrap_err();

    match refused {
        RepositoryError::Storage(message) => assert!(
            message.contains("opened for reading only"),
            "the refusal must say why: {message}"
        ),
        other => panic!("expected a storage refusal, received {other:?}"),
    }
    assert_eq!(
        count(&reader, "MATCH (:File) RETURN COUNT(*);"),
        1,
        "nothing may have been written"
    );
}

/// A reader migrates nothing, so a database an older build left behind is refused instead
/// of being read through a schema it was not written with. The refusal names both versions,
/// because what an operator does about it is to let a writing command open it once.
#[test]
fn a_reader_refuses_a_database_it_would_have_had_to_migrate() {
    let database = TestDatabase::new("read-only-older-schema");
    create_v9_fixture(database.path());

    let Err(refused) = LadybugDatabaseManager::open_read_only(database.path().to_path_buf()) else {
        panic!("a database at schema 9 must not be read by a build that writes 12");
    };

    let message = refused.to_string();
    assert!(message.contains(" 9 "), "{message}");
    assert!(
        message.contains(&migration::LATEST_SCHEMA_VERSION.to_string()),
        "{message}"
    );
}

/// The same question asked of the rule itself: a database this build prepared is at the
/// version this build reads, and a database with no schema at all is refused rather than
/// read as if it were empty.
#[test]
fn the_schema_a_reader_requires_is_the_one_this_build_writes() {
    let database = written_and_closed("read-only-schema-current");
    let manager = LadybugDatabaseManager::open_read_only(database.path().to_path_buf()).unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();
    assert_eq!(
        migration::require_current_schema(&connection).unwrap(),
        migration::LATEST_SCHEMA_VERSION
    );

    let empty = TestDatabase::new("read-only-schema-absent");
    let database = super::test_support::open_fixture_database(empty.path());
    let connection = Connection::new(&database).unwrap();
    let refused = migration::require_current_schema(&connection).unwrap_err();
    assert!(
        refused.to_string().contains("schema version 0"),
        "a database nobody prepared records nothing: {refused}"
    );
}
