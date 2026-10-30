// What the database does with the two ways of linking an observation, held as tests so the
// choice cannot quietly stop being true: the statements the store runs must reach both
// endpoints by primary key, and the shapes they replaced must still be the scanning ones.
use lbug::{Connection, LogicalType, Value};

use super::observation_store::NEW_LINK_STATEMENTS;
use super::query_plan::{primary_key_lookups, relationship_scans, table_scans};
use super::test_support::{TestDatabase, count, evidence_commit};
use super::{LadybugDatabaseManager, repository};
use crate::application::SourceBatchWriter;

const OBSERVATION: &str = "observation:file:a";

/// The shapes that were measured to scan, each with what it demonstrates. They fail for
/// different reasons, and the reason is the point: the comma-separated pattern does not use
/// the keys it was given at all, while `MERGE` uses both and still scans the relationship
/// table to find an edge that cannot be there.
struct ScanningShape {
    name: &'static str,
    query: &'static str,
    primary_key_lookups: usize,
    relationship_scans: usize,
}

const SCANNING_SHAPES: &[ScanningShape] = &[
    ScanningShape {
        name: "one comma-separated pattern",
        query: "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}), (file:File {id: observation.file_id}) CREATE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);",
        primary_key_lookups: 0,
        relationship_scans: 0,
    },
    ScanningShape {
        name: "merge instead of create",
        query: "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}) MATCH (file:File {id: observation.file_id}) MERGE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);",
        primary_key_lookups: 2,
        relationship_scans: 1,
    },
];

fn seeded() -> (
    TestDatabase,
    std::sync::Arc<LadybugDatabaseManager>,
    repository::LadybugGraphRepository,
) {
    let database = TestDatabase::new("link-capability");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![evidence_commit("file:a", "/data/docs/a.md")])
        .unwrap();
    (database, manager, repository)
}

fn ids() -> Value {
    Value::List(
        LogicalType::String,
        vec![Value::String(OBSERVATION.to_string())],
    )
}

fn plan(connection: &Connection<'_>, query: &str) -> String {
    let mut statement = connection.prepare(&format!("EXPLAIN {query}")).unwrap();
    let mut result = connection
        .execute(&mut statement, vec![("ids", ids())])
        .unwrap();
    match result.next().as_deref() {
        Some([Value::String(plan)]) => plan.clone(),
        other => panic!("a plan was expected, received {other:?}"),
    }
}

#[test]
fn every_statement_that_links_a_new_observation_reaches_both_endpoints_by_primary_key() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (context, query) in NEW_LINK_STATEMENTS {
        let plan = plan(&connection, query);
        assert_eq!(
            primary_key_lookups(&plan),
            2,
            "{context}: both endpoints must be found by primary key"
        );
        assert_eq!(
            table_scans(&plan),
            0,
            "{context}: no node table may be scanned"
        );
        assert_eq!(
            relationship_scans(&plan),
            0,
            "{context}: the relationship table may not be scanned"
        );
    }
}

/// Why the statements are written the way they are. One comma-separated pattern joins two
/// node scans; `MERGE` looks the relationship up by scanning its table even when both
/// endpoints are already found by key. Either one brings back a cost that grows with the
/// stored history.
#[test]
fn the_shapes_that_were_replaced_still_scan() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for shape in SCANNING_SHAPES {
        let plan = plan(&connection, shape.query);
        assert!(
            table_scans(&plan) > 0,
            "{}: recorded as scanning node tables, but the plan no longer does",
            shape.name
        );
        assert_eq!(
            relationship_scans(&plan),
            shape.relationship_scans,
            "{}: the relationship scan this shape was replaced for changed",
            shape.name
        );
        assert_eq!(
            primary_key_lookups(&plan),
            shape.primary_key_lookups,
            "{}: how many endpoints this shape still reaches by key changed",
            shape.name
        );
    }
}

#[test]
fn creating_a_link_twice_is_refused_and_so_can_never_stand_in_for_idempotency() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (context, query) in NEW_LINK_STATEMENTS {
        let before = count(&manager, "MATCH ()-[]->() RETURN COUNT(*);");
        let mut statement = connection.prepare(query).unwrap();
        let repeated = connection.execute(&mut statement, vec![("ids", ids())]);

        assert!(
            repeated.is_err(),
            "{context}: a second create of the same link must be refused, so a retry can never be made idempotent by repeating it"
        );
        assert_eq!(
            count(&manager, "MATCH ()-[]->() RETURN COUNT(*);"),
            before,
            "{context}: a refused create must leave the graph as it was"
        );
    }
}

/// The cardinality the schema declares, confirmed against the database rather than read
/// off the migration: an observation belongs to one file, observes one revision, and is
/// ordered by one order. Each of the three new `CREATE` statements relies on exactly one
/// of these, so all three are covered here.
#[test]
fn every_observation_link_allows_at_most_one_edge() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();
    connection
        .query("CREATE (:File {id: 'file:other'});")
        .unwrap();
    connection
        .query("CREATE (:FileVersion {id: 'file:other@hash', file_id: 'file:other', content_hash: 'hash', created_at_unix_ms: 1});")
        .unwrap();
    connection
        .query("CREATE (:ObservationOrder {id: 'order:other', file_id: 'file:other', observed_at_unix_ms: 1, sequence: 99});")
        .unwrap();

    let second_owner = connection.query(
        "MATCH (file:File {id: 'file:other'}), (observation:FileObservation {id: 'observation:file:a'}) CREATE (file)-[:HAS_OBSERVATION]->(observation);",
    );
    let second_version = connection.query(
        "MATCH (observation:FileObservation {id: 'observation:file:a'}), (version:FileVersion {id: 'file:other@hash'}) CREATE (observation)-[:OBSERVES]->(version);",
    );
    let second_order = connection.query(
        "MATCH (observation_order:ObservationOrder {id: 'order:other'}), (observation:FileObservation {id: 'observation:file:a'}) CREATE (observation_order)-[:ORDERS]->(observation);",
    );

    assert!(
        second_owner.is_err(),
        "an observation belongs to one file only"
    );
    assert!(
        second_version.is_err(),
        "an observation observes one revision only"
    );
    assert!(
        second_order.is_err(),
        "an observation is ordered by one order only"
    );
    for kind in ["HAS_OBSERVATION", "OBSERVES", "ORDERS"] {
        assert_eq!(
            count(
                &manager,
                &format!("MATCH ()-[:{kind}]->() RETURN COUNT(*);")
            ),
            1,
            "{kind} must still have exactly one edge"
        );
    }
}
