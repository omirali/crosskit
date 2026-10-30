// How a failure from the engine crosses the boundary. LadybugDB reports an exhausted page
// cache in text that names its own internals, and it ends the transaction itself when that
// happens; neither may reach a reader, and neither may be mistaken for a failed rollback.
use lbug::{Connection, Database, SystemConfig};

use super::store_support::{no_active_transaction, storage_error};
use super::test_support::{TestDatabase, count, evidence_commit};
use super::{LadybugDatabaseManager, migration};
use crate::application::{RepositoryError, SourceBatchWriter};

/// The text LadybugDB produces when its buffer pool cannot free a page, as Stage 2P
/// recorded it from a real refusal.
const ENGINE_OUT_OF_MEMORY: &str = "Query execution failed: Buffer manager exception: Unable to allocate memory! The buffer pool is full and no memory could be freed!";

/// What it says when the transaction is already gone, which is what it leaves behind after
/// the refusal above.
const ENGINE_NO_TRANSACTION: &str = "Query execution failed: No active transaction for ROLLBACK.";

#[test]
fn running_out_of_memory_becomes_a_category_and_never_repeats_the_engine() {
    let error = storage_error("content chunk batch insert", ENGINE_OUT_OF_MEMORY);

    assert_eq!(
        error,
        RepositoryError::StorageExhausted("content chunk batch insert")
    );
    let shown = error.to_string();
    assert!(
        !shown.contains("Buffer manager") && !shown.contains("buffer pool"),
        "the engine's own words must not reach a reader: {shown}"
    );
    assert_eq!(
        shown, "knowledge graph storage ran out of memory during content chunk batch insert",
        "the operation is this program's own word for it and belongs in the message"
    );
}

#[test]
fn any_other_failure_keeps_its_context_and_stays_a_storage_error() {
    let error = storage_error(
        "current version set",
        "Query execution failed: Binder exception",
    );

    assert!(
        matches!(&error, RepositoryError::Storage(message) if message.starts_with("current version set: ")),
        "{error:?}"
    );
}

#[test]
fn a_transaction_the_engine_already_ended_is_not_a_failed_rollback() {
    assert!(no_active_transaction(&ENGINE_NO_TRANSACTION));
    assert!(!no_active_transaction(&ENGINE_OUT_OF_MEMORY));
}

/// A rollback that genuinely fails is reported as such, and the failure that started it
/// stays readable inside it rather than being replaced by it.
#[test]
fn a_rollback_that_fails_keeps_the_failure_that_caused_it() {
    let error = RepositoryError::RollbackFailed(Box::new(RepositoryError::StorageExhausted(
        "content chunk batch insert",
    )));

    assert_eq!(
        error.to_string(),
        "knowledge graph transaction could not be rolled back after: knowledge graph storage ran out of memory during content chunk batch insert"
    );
}

/// The whole path, on a real database: a batch that fails inside its transaction leaves
/// nothing behind, and the caller is told in a category rather than in the engine's words.
#[test]
fn a_failed_transaction_leaves_no_row_and_no_engine_text() {
    let database = TestDatabase::new("storage-failure");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();
    manager
        .repository()
        .commit_batch(vec![evidence_commit("file:kept", "/data/docs/kept.md")])
        .unwrap();
    let before = count(&manager, "MATCH (file:File) RETURN COUNT(*);");

    let error = migration::in_transaction(&connection, |connection| {
        connection
            .query("CREATE (:File {id: 'file:doomed'});")
            .map_err(|error| storage_error("probe insert", error))?;
        Err::<(), _>(RepositoryError::StorageExhausted("probe insert"))
    })
    .unwrap_err();

    assert_eq!(error, RepositoryError::StorageExhausted("probe insert"));
    assert_eq!(
        count(&manager, "MATCH (file:File) RETURN COUNT(*);"),
        before,
        "a failed transaction must leave the graph as it was"
    );
}

/// A page cache too small to hold even the schema, which makes the refusal cheap and
/// certain. Everything else here works on remembered text; this one proves the recogniser
/// against the engine this build links, because the wording is LadybugDB's to change and a
/// changed wording would silently turn an exhausted cache back into an ordinary storage
/// error. If a future engine fits the schema into this pool, the test fails rather than
/// passing without proving anything.
#[test]
fn an_exhausted_page_cache_is_recognised_on_this_build() {
    const TOO_SMALL_FOR_THE_SCHEMA: u64 = 4 * 1024 * 1024;
    let database = TestDatabase::new("exhausted-page-cache");
    let engine = Database::new(
        database.path(),
        SystemConfig::default()
            .enable_multi_writes(false)
            .buffer_pool_size(TOO_SMALL_FOR_THE_SCHEMA),
    )
    .unwrap();
    let connection = Connection::new(&engine).unwrap();

    let error = migration::apply(&connection).expect_err(
        "a 4 MiB page cache cannot hold this schema; an engine that now fits it leaves this test proving nothing, so the recogniser needs another way to be exercised",
    );

    assert!(
        matches!(error, RepositoryError::StorageExhausted(_)),
        "an exhausted page cache must arrive as its own category, not as a storage error carrying the engine's words: {error:?}"
    );
}
