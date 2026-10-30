// What the database does with the two ways of linking a location, and with finding a
// location by its path. Held as tests, because the choice between them is the difference
// between a pass that costs the tree it walks and one that costs the whole graph.
//
// Stage 2R established the rule for chunks: `MERGE` looks a relationship up by scanning the
// relationship table even when both endpoints are already reached by primary key, so a link
// that is new is written with `CREATE` and only rows that were already stored pay for a
// repair. Locations never got that treatment — one `MERGE` ran for every row of every batch
// — and for locations "already stored" is not the rare case, it is every file of every
// reconcile. Stage 18 measured what that costs: five identical passes of ten thousand files
// into one graph took 28.5s, 60.2s, 111.8s, 179.8s and 269.0s.
use lbug::{Connection, LogicalType, Value};

use super::location_store::{
    LOCATED_AT_CREATE, REPLACED_LOCATION_LINK_SHAPE, RETIRE_OTHER_LOCATIONS_AT_PATH,
};
use super::query_plan::{primary_key_lookups, relationship_scans, table_scans};
use super::test_support::{TestDatabase, evidence_commit};
use super::{LadybugDatabaseManager, repository};
use crate::application::SourceBatchWriter;

const FILE: &str = "file:a";
const PATH: &str = "/data/docs/a.md";

fn seeded() -> (
    TestDatabase,
    std::sync::Arc<LadybugDatabaseManager>,
    repository::LadybugGraphRepository,
) {
    let database = TestDatabase::new("location-link-capability");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![evidence_commit(FILE, PATH)])
        .unwrap();
    (database, manager, repository)
}

/// The row shape every one of these statements takes: the location's own id, the file it
/// belongs to and the path it sits at.
fn rows() -> Value {
    let row = Value::Struct(vec![
        ("id".to_string(), Value::String(format!("{FILE}@{PATH}"))),
        ("file_id".to_string(), Value::String(FILE.to_string())),
        (
            "normalized_path".to_string(),
            Value::String(PATH.to_string()),
        ),
    ]);
    Value::List(LogicalType::from(&row), vec![row])
}

fn plan(connection: &Connection<'_>, query: &str) -> String {
    let mut statement = connection.prepare(&format!("EXPLAIN {query}")).unwrap();
    let mut result = connection
        .execute(
            &mut statement,
            vec![
                ("rows", rows()),
                (
                    "ids",
                    Value::List(
                        LogicalType::String,
                        vec![Value::String(format!("{FILE}@{PATH}"))],
                    ),
                ),
                ("present", Value::String("present".to_string())),
                ("missing", Value::String("missing".to_string())),
            ],
        )
        .unwrap();
    match result.next().as_deref() {
        Some([Value::String(plan)]) => plan.clone(),
        other => panic!("a plan was expected, received {other:?}"),
    }
}

#[test]
fn linking_a_new_location_reaches_both_endpoints_by_primary_key_and_scans_nothing() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let plan = plan(&connection, LOCATED_AT_CREATE);
    assert_eq!(
        primary_key_lookups(&plan),
        2,
        "both endpoints must be found by primary key:\n{plan}"
    );
    assert_eq!(
        table_scans(&plan),
        0,
        "no node table may be scanned:\n{plan}"
    );
    assert_eq!(
        relationship_scans(&plan),
        0,
        "the relationship table may not be scanned:\n{plan}"
    );
}

/// Why the link is no longer written this way. It reaches both endpoints by key and still
/// scans the relationship table to look for an edge, and that scan grows with every
/// location the graph has ever held.
#[test]
fn the_merge_this_replaced_still_scans_the_relationship_table() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let plan = plan(&connection, REPLACED_LOCATION_LINK_SHAPE);
    assert!(
        relationship_scans(&plan) > 0,
        "recorded as scanning the relationship table, but the plan no longer does:\n{plan}"
    );
}

/// The retirement reaches `FileLocation` by a path, which is not its primary key, so it
/// scans the node table. That cost is why it is asked only about rows that can displace
/// something — a location becoming present at a path — and never about the locations a
/// reconcile merely sees again.
#[test]
fn finding_a_location_by_path_scans_the_node_table() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let plan = plan(&connection, RETIRE_OTHER_LOCATIONS_AT_PATH);
    assert!(
        table_scans(&plan) > 0,
        "a path is not a primary key, so this must still scan:\n{plan}"
    );
}

/// The node upserts and links a batch writes, and what each must cost. Every one of these
/// is run for rows the batch has already established are new, so none of them may scan:
/// what they scan grows with the graph, and the batch does not.
const NEW_ROW_STATEMENTS: &[(&str, &str)] = &[
    (
        "file insert",
        "UNWIND $rows AS row CREATE (file:File {id: row.file_id, first_seen_at_unix_ms: 1}) RETURN COUNT(file);",
    ),
    ("located_at create", LOCATED_AT_CREATE),
];

/// What a batch asks by primary key before it writes. These decide which rows are new, so
/// they must reach the key rather than scan for it.
const LOOKUP_STATEMENTS: &[(&str, &str)] = &[
    (
        "file lookup",
        "UNWIND $ids AS id MATCH (file:File {id: id}) RETURN file.id;",
    ),
    (
        "location lookup",
        "UNWIND $ids AS id MATCH (location:FileLocation {id: id}) RETURN location.id;",
    ),
];

/// The node `MERGE` this replaced. `id` is the primary key of `File` and the planner still
/// scans the table to answer the question `MERGE` asks, and that scan was paid for every
/// file of every batch of every pass — including a pass over a tree where nothing changed.
const REPLACED_FILE_UPSERT: &str = "UNWIND $rows AS row MERGE (file:File {id: row.file_id}) ON CREATE SET file.first_seen_at_unix_ms = 1 RETURN COUNT(file);";

#[test]
fn nothing_a_batch_writes_for_a_new_row_scans() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in NEW_ROW_STATEMENTS {
        let plan = plan(&connection, query);
        assert_eq!(
            table_scans(&plan),
            0,
            "{name}: no node table may be scanned:\n{plan}"
        );
        assert_eq!(
            relationship_scans(&plan),
            0,
            "{name}: the relationship table may not be scanned:\n{plan}"
        );
    }
}

#[test]
fn what_a_batch_asks_before_writing_reaches_the_primary_key() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in LOOKUP_STATEMENTS {
        let plan = plan(&connection, query);
        assert_eq!(
            primary_key_lookups(&plan),
            1,
            "{name}: the row must be found by primary key:\n{plan}"
        );
        assert_eq!(
            table_scans(&plan),
            0,
            "{name}: no node table may be scanned:\n{plan}"
        );
    }
}

/// Why the file upsert is no longer a `MERGE`.
#[test]
fn the_file_merge_this_replaced_still_scans_its_own_primary_key_table() {
    let (_database, manager, _repository) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let plan = plan(&connection, REPLACED_FILE_UPSERT);
    assert!(
        table_scans(&plan) > 0,
        "recorded as scanning, but the plan no longer does:\n{plan}"
    );
}
