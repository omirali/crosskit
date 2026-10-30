// Why the index cannot reuse what current-state already read.
//
// Both look at the same moment — the state of the current pointers before a batch writes —
// so the question came up in Stage 19 whether they are one question asked twice. They are
// not, and the difference is held here rather than explained in prose, because it is the
// reason the indexing path pays for a reading of its own.
//
// current-state compares **observations**: which sighting of a file is the newest. The
// index compares **versions**: whether the text of the file changed. A pass over a tree
// where nothing changed produces a new observation for every file it reads — the id carries
// the time of the sighting and a nonce, so it is never the same twice — while the version,
// which is derived from the content, stays exactly as it was. Every one of those files wins
// the current-state comparison and moves its pointers, and not one of them has a document
// in the index that needs rewriting.
use lbug::{Connection, LogicalType, Value};

use super::test_support::{TestDatabase, evidence_commit, evidence_commit_revision};
use super::{LadybugDatabaseManager, repository};
use crate::application::SourceBatchWriter;

/// The order lookup current-state runs, with the current version read in the same
/// traversal. It is not in production: reading it here would do nothing for the index
/// unless what the transaction read came back out to the indexing path, which means
/// changing what committing a batch returns. Kept as a measured statement of what such a
/// change could save — one query per batch instead of two — so the option stays checkable
/// rather than remembered.
const ORDER_AND_VERSION: &str = "UNWIND $ids AS id MATCH (file:File {id: id})-[:CURRENT_OBSERVATION]->(:FileObservation)<-[:ORDERS]-(observation_order:ObservationOrder) OPTIONAL MATCH (file)-[:CURRENT_VERSION]->(version:FileVersion) RETURN file.id, observation_order.observed_at_unix_ms, observation_order.sequence, version.id;";

const FILE: &str = "file:a";
const PATH: &str = "/data/a.md";

fn seeded() -> (
    TestDatabase,
    std::sync::Arc<LadybugDatabaseManager>,
    repository::LadybugGraphRepository,
) {
    let database = TestDatabase::new("current-version-probe");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![evidence_commit(FILE, PATH)])
        .unwrap();
    (database, manager, repository)
}

/// The claim: a file whose content did not change still moves its current pointers, because
/// the sighting is new even though the version is not. So "current-state moved this file" is
/// not an answer to "does the index have to rewrite this file".
#[test]
fn current_state_moves_for_a_file_whose_version_did_not_change() {
    let (_database, manager, repository) = seeded();

    // The same content, seen again: production derives the observation id from the time of
    // the sighting and a nonce, so a second sighting is a second observation.
    repository
        .commit_batch(vec![evidence_commit_revision(
            FILE,
            PATH,
            "hash:a",
            "observation:file:a:second",
            200,
        )])
        .unwrap();

    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        2,
        "seeing the same file again records a second observation"
    );
    assert_eq!(
        count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"),
        1,
        "the content did not change, so there is still one version"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:a'})-[:CURRENT_OBSERVATION]->(:FileObservation {id: 'observation:file:a:second'}) RETURN COUNT(*);"
        ),
        1,
        "current-state moved the observation pointer, so this file won its comparison"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:a'})-[:CURRENT_VERSION]->(:FileVersion {id: 'file:a@hash:a'}) RETURN COUNT(*);"
        ),
        1,
        "and the current version is the one it already was: nothing for the index to write"
    );
}

/// The saving that a change to the batch's return value could make: the order lookup can
/// read the current version in the same traversal, so one query would answer both questions.
#[test]
fn the_order_lookup_can_read_the_current_version_in_the_same_query() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let mut statement = connection.prepare(ORDER_AND_VERSION).unwrap();
    let mut result = connection
        .execute(
            &mut statement,
            vec![(
                "ids",
                Value::List(LogicalType::String, vec![Value::String(FILE.to_string())]),
            )],
        )
        .unwrap();

    match result.next().as_deref() {
        Some(
            [
                Value::String(file),
                Value::Int64(_),
                Value::Int64(_),
                Value::String(version),
            ],
        ) => {
            assert_eq!(file, FILE);
            assert_eq!(
                version, "file:a@hash:a",
                "the current version comes back beside the order it was asked for"
            );
        }
        other => panic!("expected a file, an order and a version, received {other:?}"),
    }
}

fn count(manager: &std::sync::Arc<LadybugDatabaseManager>, query: &str) -> i64 {
    let connection = Connection::new(&manager.owner.database).unwrap();
    let mut result = connection.query(query).unwrap();
    match result.next().unwrap().as_slice() {
        [Value::Int64(value)] => *value,
        values => panic!("expected one count, received {values:?}"),
    }
}
