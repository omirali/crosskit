// What the two current-state pointer writes must leave on a real database. Stage 2N
// changed how each of them finds its two endpoints, so the rules they carry are proven
// here against the schema's own cardinality rather than against the plan they were chosen
// for. The rules that hold for a whole batch of sources, including an older retry and a
// hard link, are proven in `set_based_tests`.
use lbug::{Connection, Value};

use super::current_state_statements::{
    SET_CURRENT_OBSERVATION, SET_CURRENT_OBSERVATION_FOR_FILE, SET_CURRENT_VERSION,
    SET_CURRENT_VERSION_FOR_FILE,
};
use super::current_state_store::{CurrentCandidate, advance_current_state_batch};
use super::observation_store::ObservationOrderKey;
use super::test_support::{
    TestDatabase, count, evidence_commit_at, metadata_commit, query_int64, query_string,
};
use super::{LadybugDatabaseManager, migration};
use crate::application::{RepositoryError, SourceBatchWriter};
use crate::domain::SourceCommit;

/// A file observed twice inside one batch, a file seen once, and a file with no evidence
/// at all: the three shapes the pointer writes have to tell apart.
fn current_state_batch() -> Vec<SourceCommit> {
    vec![
        metadata_commit("file:plain", "/data/docs/plain.bin"),
        evidence_commit_at(
            "file:notes",
            "/data/docs/notes.md",
            "hash:a",
            "observation:notes:1",
            100,
        ),
        evidence_commit_at(
            "file:notes",
            "/data/docs/notes.md",
            "hash:b",
            "observation:notes:2",
            200,
        ),
        evidence_commit_at(
            "file:other",
            "/data/docs/other.md",
            "hash:c",
            "observation:other:1",
            120,
        ),
    ]
}

fn seeded(test_name: &str) -> (TestDatabase, std::sync::Arc<LadybugDatabaseManager>) {
    let database = TestDatabase::new(test_name);
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    manager
        .repository()
        .commit_batch(current_state_batch())
        .unwrap();
    (database, manager)
}

fn pointer_count(manager: &LadybugDatabaseManager, file_id: &str, pointer: &str) -> i64 {
    query_int64(
        manager,
        &format!("MATCH (:File {{id: $file_id}})-[:{pointer}]->() RETURN COUNT(*);"),
        vec![("file_id", Value::String(file_id.to_string()))],
    )
}

#[test]
fn an_observed_file_has_exactly_one_of_each_pointer_and_an_unobserved_file_has_none() {
    let (_database, manager) = seeded("current-state-cardinality");

    for file_id in ["file:notes", "file:other"] {
        for pointer in ["CURRENT_VERSION", "CURRENT_OBSERVATION"] {
            assert_eq!(
                pointer_count(&manager, file_id, pointer),
                1,
                "{file_id} must have exactly one {pointer}"
            );
        }
    }
    for pointer in ["CURRENT_VERSION", "CURRENT_OBSERVATION"] {
        assert_eq!(
            pointer_count(&manager, "file:plain", pointer),
            0,
            "a file with no evidence must have no {pointer}"
        );
    }
    // Both pointers of every file name the same revision, reached through the observation.
    assert_eq!(
        count(
            &manager,
            "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion)<-[:OBSERVES]-(observation:FileObservation)<-[:CURRENT_OBSERVATION]-(file) RETURN COUNT(*);"
        ),
        2,
        "each observed file's two pointers must agree on one revision"
    );
    assert_eq!(
        query_string(
            &manager,
            "MATCH (:File {id: 'file:notes'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
            vec![],
        ),
        "observation:notes:2",
        "the newer of two observations in one batch wins the current state"
    );
}

/// Two observations of one file at the same instant are ordered only by the sequence the
/// batch allocated, in input order. The winner is therefore the later input row.
#[test]
fn a_tie_on_observed_at_is_decided_by_the_durable_sequence() {
    let database = TestDatabase::new("current-state-tie");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    manager
        .repository()
        .commit_batch(vec![
            evidence_commit_at(
                "file:tie",
                "/data/docs/tie.md",
                "hash:first",
                "observation:tie:first",
                700,
            ),
            evidence_commit_at(
                "file:tie",
                "/data/docs/tie.md",
                "hash:second",
                "observation:tie:second",
                700,
            ),
        ])
        .unwrap();

    assert_eq!(
        query_string(
            &manager,
            "MATCH (:File {id: 'file:tie'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
            vec![],
        ),
        "observation:tie:second"
    );
    assert_eq!(
        query_string(
            &manager,
            "MATCH (:File {id: 'file:tie'})-[:CURRENT_VERSION]->(version:FileVersion) RETURN version.id;",
            vec![],
        ),
        "file:tie@hash:second"
    );
}

/// The removals and the two creates are one transaction. When the second create cannot
/// process every row, the first one must not survive: a file would otherwise be left with
/// a current version and no current observation, which no reader could reconcile.
#[test]
fn a_failure_between_the_pointer_writes_leaves_neither_pointer() {
    let (_database, manager) = seeded("current-state-lone-pointer");
    let before = (
        query_string(
            &manager,
            "MATCH (:File {id: 'file:notes'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
            vec![],
        ),
        query_string(
            &manager,
            "MATCH (:File {id: 'file:other'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
            vec![],
        ),
    );
    let connection = Connection::new(&manager.owner.database).unwrap();

    // Both candidates are newer than what is stored, so both pass the filter and reach the
    // writes. The second names an observation that does not exist, so the observation
    // create processes one row of two while the version create has already processed both.
    let error = migration::in_transaction(&connection, |connection| {
        advance_current_state_batch(
            connection,
            &[
                CurrentCandidate {
                    file_id: "file:notes".to_string(),
                    version_id: "file:notes@hash:a".to_string(),
                    observation_id: "observation:notes:1".to_string(),
                    key: ObservationOrderKey::new(900, 900),
                },
                CurrentCandidate {
                    file_id: "file:other".to_string(),
                    version_id: "file:other@hash:c".to_string(),
                    observation_id: "observation:other:missing".to_string(),
                    key: ObservationOrderKey::new(900, 901),
                },
            ],
        )
    })
    .unwrap_err();

    // Named, so the test keeps proving the case it is here for: the failure has to fall
    // between the two creates, not before the first one.
    assert!(
        matches!(&error, RepositoryError::InvariantViolation(message)
            if message.contains("current observation batch set")),
        "{error:?}"
    );
    for file_id in ["file:notes", "file:other"] {
        assert_eq!(
            pointer_count(&manager, file_id, "CURRENT_VERSION"),
            pointer_count(&manager, file_id, "CURRENT_OBSERVATION"),
            "{file_id} must not be left with one pointer of the pair"
        );
        assert_eq!(
            pointer_count(&manager, file_id, "CURRENT_VERSION"),
            1,
            "{file_id} keeps the current state it had before the failed write"
        );
    }
    assert_eq!(
        (
            query_string(
                &manager,
                "MATCH (:File {id: 'file:notes'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
                vec![],
            ),
            query_string(
                &manager,
                "MATCH (:File {id: 'file:other'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
                vec![],
            ),
        ),
        before,
        "a failed advance must leave the committed current state exactly as it was"
    );
}

/// The guard against a second pointer is the schema's ONE_ONE cardinality and nothing
/// else: no code checks for an existing pointer before creating one. Reaching the two
/// endpoints through separate `MATCH` clauses did not change what may be written, so the
/// database must still refuse the second create and leave the graph untouched. Both write
/// paths are covered: the batch statements and the per-file ones of the fallback.
#[test]
fn a_second_pointer_for_one_file_is_refused_by_the_schema() {
    let (_database, manager) = seeded("current-state-one-one");
    let connection = Connection::new(&manager.owner.database).unwrap();
    let before = (
        pointer_count(&manager, "file:notes", "CURRENT_VERSION"),
        pointer_count(&manager, "file:notes", "CURRENT_OBSERVATION"),
    );
    // A revision and an observation of the same file that are not the current ones.
    let row = Value::Struct(vec![
        (
            "file_id".to_string(),
            Value::String("file:notes".to_string()),
        ),
        (
            "version_id".to_string(),
            Value::String("file:notes@hash:a".to_string()),
        ),
        (
            "observation_id".to_string(),
            Value::String("observation:notes:1".to_string()),
        ),
    ]);
    let rows = Value::List(lbug::LogicalType::from(&row), vec![row]);

    for (name, query) in [
        ("current version set", SET_CURRENT_VERSION),
        ("current observation set", SET_CURRENT_OBSERVATION),
    ] {
        let mut statement = connection.prepare(query).unwrap();
        let refused = connection.execute(&mut statement, vec![("rows", rows.clone())]);

        assert!(
            refused.is_err(),
            "{name}: a second pointer for one file must be refused by the schema"
        );
    }
    for (name, query, key) in [
        (
            "current version set, one file",
            SET_CURRENT_VERSION_FOR_FILE,
            ("version_id", "file:notes@hash:a"),
        ),
        (
            "current observation set, one file",
            SET_CURRENT_OBSERVATION_FOR_FILE,
            ("observation_id", "observation:notes:1"),
        ),
    ] {
        let mut statement = connection.prepare(query).unwrap();
        let refused = connection.execute(
            &mut statement,
            vec![
                ("file_id", Value::String("file:notes".to_string())),
                (key.0, Value::String(key.1.to_string())),
            ],
        );

        assert!(
            refused.is_err(),
            "{name}: a second pointer for one file must be refused by the schema"
        );
    }
    assert_eq!(
        (
            pointer_count(&manager, "file:notes", "CURRENT_VERSION"),
            pointer_count(&manager, "file:notes", "CURRENT_OBSERVATION"),
        ),
        before,
        "a refused create must leave the current state as it was"
    );
}
