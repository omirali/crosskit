// The evidence base of ADR 0007: that this build exposes query plans, and that the two
// plan shapes the decision rests on are the ones the engine still chooses. If a future
// LadybugDB stops exposing plans, or plans a point lookup as a scan, these tests fail and
// the conclusion drawn from them has to be revisited rather than silently inherited.
use lbug::{Connection, Value};

use super::LadybugDatabaseManager;
use super::current_state_statements::{
    CANDIDATE_CURRENT_ORDER_BY_OBSERVATION_ID, CURRENT_ORDER_LOOKUP, REMOVE_CURRENT_OBSERVATION,
    REMOVE_CURRENT_VERSION, REPLACED_SET_CURRENT_OBSERVATION, REPLACED_SET_CURRENT_VERSION,
    SET_CURRENT_OBSERVATION, SET_CURRENT_OBSERVATION_FOR_FILE, SET_CURRENT_VERSION,
    SET_CURRENT_VERSION_FOR_FILE,
};
use super::insight_work_store;
use super::query_plan::{
    has_scan, operators, primary_key_lookups, primary_key_scans, relationship_scans, table_scans,
};
use super::test_support::{TestDatabase, evidence_commit};
use crate::application::SourceBatchWriter;

/// The two statements Stage 2N changed, named as the store runs them.
const SET_SHAPES: &[(&str, &str)] = &[
    ("current version set", SET_CURRENT_VERSION),
    ("current observation set", SET_CURRENT_OBSERVATION),
];

/// The same two writes as the sequential fallback runs them: one file at a time, named by
/// scalar parameters. The path is rare, but it writes the same pointers.
const SCALAR_SET_SHAPES: &[(&str, &str)] = &[
    (
        "current version set, one file",
        SET_CURRENT_VERSION_FOR_FILE,
    ),
    (
        "current observation set, one file",
        SET_CURRENT_OBSERVATION_FOR_FILE,
    ),
];

/// The shapes Stage 2N replaced, kept here so the reason for replacing them stays
/// checkable rather than remembered: one comma-separated pattern per pointer write.
const REPLACED_SET_SHAPES: &[(&str, &str)] = &[
    ("replaced current version set", REPLACED_SET_CURRENT_VERSION),
    (
        "replaced current observation set",
        REPLACED_SET_CURRENT_OBSERVATION,
    ),
];

/// A point lookup by primary key, the shape that stays independent of table size.
const POINT_LOOKUP: &str =
    "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}) RETURN observation.id;";

fn plan(connection: &Connection<'_>, query: &str) -> String {
    plan_with(connection, "EXPLAIN", query, parameters(query))
}

fn profiled_plan(connection: &Connection<'_>, query: &str, clear_pointers: bool) -> String {
    connection.query("BEGIN TRANSACTION;").unwrap();
    if clear_pointers {
        connection
            .query("MATCH (:File)-[current:CURRENT_VERSION]->(:FileVersion) DELETE current;")
            .unwrap();
        connection
            .query(
                "MATCH (:File)-[current:CURRENT_OBSERVATION]->(:FileObservation) DELETE current;",
            )
            .unwrap();
    }
    let plan = plan_with(connection, "PROFILE", query, parameters(query));
    connection.query("ROLLBACK;").unwrap();
    plan
}

fn plan_with(
    connection: &Connection<'_>,
    keyword: &str,
    query: &str,
    parameters: Vec<(&'static str, Value)>,
) -> String {
    let mut statement = connection.prepare(&format!("{keyword} {query}")).unwrap();
    let mut result = connection.execute(&mut statement, parameters).unwrap();
    match result.next().as_deref() {
        Some([Value::String(plan)]) => plan.clone(),
        other => panic!("a plan was expected, received {other:?}"),
    }
}

fn parameters(query: &str) -> Vec<(&'static str, Value)> {
    if query.contains("$file_id") {
        return [
            ("file_id", "file:a"),
            ("version_id", "file:a@hash:a"),
            ("observation_id", "observation:file:a"),
        ]
        .into_iter()
        .filter(|(name, _)| query.contains(&format!("${name}")))
        .map(|(name, id)| (name, Value::String(id.to_string())))
        .collect();
    }
    if query.contains("$ids") {
        return vec![(
            "ids",
            Value::List(
                lbug::LogicalType::String,
                vec![Value::String("file:a".to_string())],
            ),
        )];
    }
    let row = Value::Struct(vec![
        ("file_id".to_string(), Value::String("file:a".to_string())),
        (
            "version_id".to_string(),
            Value::String("file:a@hash:a".to_string()),
        ),
        (
            "observation_id".to_string(),
            Value::String("observation:file:a".to_string()),
        ),
    ]);
    vec![(
        "rows",
        Value::List(lbug::LogicalType::from(&row), vec![row]),
    )]
}

fn rows(connection: &Connection<'_>, query: &str) -> Vec<Vec<Value>> {
    let mut statement = connection.prepare(query).unwrap();
    connection
        .execute(&mut statement, parameters(query))
        .unwrap()
        .collect()
}

fn database() -> (TestDatabase, std::sync::Arc<LadybugDatabaseManager>) {
    let database = TestDatabase::new("explain-capability");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    manager
        .repository()
        .commit_batch(vec![evidence_commit("file:a", "/data/docs/a.md")])
        .unwrap();
    (database, manager)
}

#[test]
fn both_plan_keywords_return_a_plan() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for keyword in ["EXPLAIN", "PROFILE"] {
        let mut statement = connection
            .prepare(&format!("{keyword} MATCH (file:File) RETURN file.id;"))
            .unwrap();
        let mut result = connection.execute(&mut statement, vec![]).unwrap();
        match result.next().as_deref() {
            Some([Value::String(plan)]) => assert!(
                plan.contains("Physical Plan"),
                "{keyword} must describe a plan, received {plan:?}"
            ),
            other => panic!("{keyword} returned no plan: {other:?}"),
        }
    }
}

#[test]
fn a_point_lookup_is_planned_as_a_primary_key_lookup() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let plan = plan(&connection, POINT_LOOKUP);

    assert!(
        primary_key_lookups(&plan) > 0,
        "a lookup by primary key must not become a scan"
    );
    assert_eq!(
        table_scans(&plan),
        0,
        "a lookup by primary key must not scan a node table"
    );
}

/// The observation ADR 0007 records: this traversal is planned as scans of the history
/// tables, which is why its cost grows with the number of stored observations.
#[test]
fn the_current_state_traversal_is_planned_as_scans() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let plan = plan(&connection, CURRENT_ORDER_LOOKUP);

    assert!(
        table_scans(&plan) > 0,
        "ADR 0007 records this traversal as scanning; the plan no longer does"
    );
    assert!(
        plan.contains("ObservationOrder"),
        "the scan covers the observation order table"
    );
    assert!(
        relationship_scans(&plan) > 0,
        "the traversal scans its relationship tables"
    );
    assert!(
        primary_key_lookups(&plan) == 0,
        "ADR 0007 records that this traversal does not use the primary key"
    );
}

/// Every batch statement the store runs has both an EXPLAIN and a PROFILE plan. PROFILE
/// executes writes, so each profile runs in a transaction that is rolled back; the test
/// never changes the seeded graph while proving the capability.
#[test]
fn every_current_state_statement_has_explain_and_profile_plans() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();
    let statements = [
        ("current order lookup", CURRENT_ORDER_LOOKUP, false),
        ("current version removal", REMOVE_CURRENT_VERSION, false),
        (
            "current observation removal",
            REMOVE_CURRENT_OBSERVATION,
            false,
        ),
        (SET_SHAPES[0].0, SET_SHAPES[0].1, true),
        (SET_SHAPES[1].0, SET_SHAPES[1].1, true),
    ];

    for (name, query, clear_pointers) in statements {
        for plan in [
            plan(&connection, query),
            profiled_plan(&connection, query, clear_pointers),
        ] {
            assert!(
                plan.contains("Physical Plan"),
                "{name}: the engine must return its physical plan"
            );
        }
    }
}

/// The removals still delete by traversing the pointer they remove, which is a scan of
/// both the file table and the relationship table. Stage 2N did not touch them, so the
/// 2M record for these two statements stands.
#[test]
fn current_state_removals_are_scanning_plans() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in [
        ("current version removal", REMOVE_CURRENT_VERSION),
        ("current observation removal", REMOVE_CURRENT_OBSERVATION),
    ] {
        let plan = plan(&connection, query);
        assert!(
            table_scans(&plan) > 0 && relationship_scans(&plan) > 0,
            "{name}: the 2M record says this statement scans nodes and relationships"
        );
        assert_eq!(
            primary_key_lookups(&plan),
            0,
            "{name}: the statement gained a primary-key lookup"
        );
    }
}

/// What Stage 2N rests on: each batch pointer write reaches both of its endpoints by
/// primary key and scans nothing. If a future planner decides otherwise this fails, and
/// the decision is revisited rather than silently inherited.
#[test]
fn current_state_pointer_creation_uses_two_primary_key_lookups_without_scans() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in SET_SHAPES {
        let plan = plan(&connection, query);
        assert_eq!(
            primary_key_lookups(&plan),
            2,
            "{name}: both endpoints must be found by primary key"
        );
        assert!(
            !has_scan(&plan),
            "{name}: the statement must not scan a node or relationship table"
        );
    }
}

/// Why the statements are written the way they are: one comma-separated pattern joins two
/// node scans instead of looking both endpoints up by key. The replaced shapes are kept
/// pinned so the difference remains demonstrable on this build.
#[test]
fn the_set_shapes_that_were_replaced_still_scan() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in REPLACED_SET_SHAPES {
        let plan = plan(&connection, query);
        assert!(
            table_scans(&plan) > 0,
            "{name}: recorded as scanning a node table, but the plan no longer does"
        );
        assert_eq!(
            primary_key_lookups(&plan),
            0,
            "{name}: the replaced shape now uses a primary-key lookup, so the comparison it is kept for no longer holds"
        );
    }
}

/// Looking the order up by its observation identity does not turn the whole operation
/// into a point lookup: the current-observation traversal still scans. It also cannot be
/// substituted directly, because it would hide a broken ORDERS relationship.
#[test]
fn candidate_order_lookup_by_observation_identity_is_still_scanning() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let plan = plan(&connection, CANDIDATE_CURRENT_ORDER_BY_OBSERVATION_ID);
    assert_eq!(
        primary_key_lookups(&plan),
        1,
        "the order itself is reached by id, while the current traversal remains a scan"
    );
    assert!(
        table_scans(&plan) > 0 && relationship_scans(&plan) > 0,
        "the candidate must not be recorded as a full point lookup while it scans nodes and relationships"
    );
}

/// The order and observation sharing an id is useful only as an additional lookup. It
/// cannot replace the `ORDERS` traversal: this deliberately malformed test graph proves
/// that the candidate would otherwise accept an order that is no longer linked to the
/// current observation.
#[test]
fn candidate_order_lookup_would_hide_a_missing_orders_relationship() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();
    connection
        .query(
            "MATCH (:ObservationOrder {id: 'observation:file:a'})-[orders:ORDERS]->(:FileObservation {id: 'observation:file:a'}) DELETE orders;",
        )
        .unwrap();

    assert!(
        rows(&connection, CURRENT_ORDER_LOOKUP).is_empty(),
        "the exact lookup must expose a missing ORDERS relationship"
    );
    assert_eq!(
        rows(&connection, CANDIDATE_CURRENT_ORDER_BY_OBSERVATION_ID).len(),
        1,
        "the identity candidate hides the broken relationship and is not a safe substitution"
    );
}

/// The relationship write ADR 0007 names as the cause of the memory growth.
#[test]
fn the_observation_link_merge_is_planned_as_scans() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();
    let mut statement = connection
        .prepare("EXPLAIN UNWIND $rows AS row MATCH (file:File {id: row.file_id}), (observation:FileObservation {id: row.id}) MERGE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);")
        .unwrap();
    let row = Value::Struct(vec![
        ("id".to_string(), Value::String("probe".to_string())),
        ("file_id".to_string(), Value::String("probe".to_string())),
    ]);
    let rows = Value::List(lbug::LogicalType::from(&row), vec![row]);
    let mut result = connection
        .execute(&mut statement, vec![("rows", rows)])
        .unwrap();

    let plan = match result.next().as_deref() {
        Some([Value::String(plan)]) => plan.clone(),
        other => panic!("a plan was expected, received {other:?}"),
    };

    assert!(
        table_scans(&plan) > 0 && relationship_scans(&plan) > 0,
        "ADR 0007 records this MERGE as scanning both node and relationship tables"
    );
    assert_eq!(
        primary_key_lookups(&plan) + primary_key_scans(&plan),
        0,
        "ADR 0007 records that this MERGE does not use the primary keys it was given"
    );
}

/// Why Stage 2N left the sequential fallback's text alone. Its statements name their two
/// keys with scalar parameters, and the planner then reaches both by primary key without a
/// table scan whichever way the pattern is written: the separate-`MATCH` form produces the
/// identical plan, operator for operator. The batch operator `QUERY_PRIMARY_KEY_LOOKUP`
/// does not appear on this path at all, so the batch assertion cannot be reused here.
#[test]
fn per_file_pointer_creation_reaches_both_keys_by_primary_key_and_ignores_the_pattern_form() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for ((name, query), separate_match) in SCALAR_SET_SHAPES.iter().zip([
        "MATCH (file:File {id: $file_id}) MATCH (version:FileVersion {id: $version_id}) CREATE (file)-[:CURRENT_VERSION]->(version);",
        "MATCH (file:File {id: $file_id}) MATCH (observation:FileObservation {id: $observation_id}) CREATE (file)-[:CURRENT_OBSERVATION]->(observation);",
    ]) {
        let plan = plan(&connection, query);
        assert_eq!(
            primary_key_scans(&plan),
            2,
            "{name}: both keys must be reached by primary key"
        );
        assert!(
            !has_scan(&plan),
            "{name}: neither a node nor a relationship table may be scanned"
        );
        assert_eq!(
            primary_key_lookups(&plan),
            0,
            "{name}: this path does not use the batch lookup operator, so it cannot be held to the batch assertion"
        );
        assert_eq!(
            operators(&plan),
            operators(&self::plan(&connection, separate_match)),
            "{name}: separate MATCH clauses must still give the identical plan, or the reason for leaving this text alone is gone"
        );
    }
}

/// The same evidence for the other end of the queue (ADR 0031). The sweep that brings the
/// whole queue to the rules walks it keyset by job id, and it must cost what a claim costs:
/// one table, no edge, and a page the engine bounds rather than an ordered copy of the
/// queue it then cuts. A sweep is asked for once over a backlog of a hundred thousand jobs,
/// so the shape of its page is the whole of what it costs.
#[test]
fn a_sweep_reads_one_bounded_page_of_the_queue_and_walks_no_edge() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let page = plan_with(
        &connection,
        "EXPLAIN",
        insight_work_store::QUEUED_PAGE_BY_ID,
        vec![
            ("state", Value::String("queued".to_string())),
            ("after", Value::String(String::new())),
            ("limit", Value::Int64(2)),
        ],
    );

    assert_eq!(
        table_scans(&page),
        1,
        "a sweep reads one table, the queue's own: {:?}",
        operators(&page)
    );
    assert_eq!(
        relationship_scans(&page),
        0,
        "a sweep walks no edge of the graph: {:?}",
        operators(&page)
    );
    assert!(
        operators(&page)
            .iter()
            .any(|operator| operator.starts_with("TOP_K")),
        "the page must be bounded in the engine, not sorted whole and then cut: {:?}",
        operators(&page)
    );
}

/// The evidence behind the claim's order (ADR 0030). A claim orders the queue by the
/// moment its revision was observed, and that moment is a property of the job, copied once
/// when the job was written. The alternative — reading it from `FileVersion` at claim
/// time — is the honest shape and was rejected on this plan, so the plan is the record.
///
/// The bound is the other half: `LIMIT` reaches the planner as a top-k, so the engine
/// keeps a heap of the page and never an ordered copy of the queue.
#[test]
fn a_claim_orders_one_bounded_page_over_the_queue_and_never_joins_the_revisions() {
    let (_database, manager) = database();
    let connection = Connection::new(&manager.owner.database).unwrap();
    let claim_parameters = vec![
        ("state", Value::String("queued".to_string())),
        ("limit", Value::Int64(2)),
    ];

    let page = plan_with(
        &connection,
        "EXPLAIN",
        insight_work_store::CLAIM_PAGE_BY_OLDEST_REVISION,
        claim_parameters.clone(),
    );
    assert_eq!(
        table_scans(&page),
        1,
        "a claim reads one table, the queue's own"
    );
    assert_eq!(
        relationship_scans(&page),
        0,
        "a claim walks no edge of the graph"
    );
    assert!(
        operators(&page)
            .iter()
            .any(|operator| operator.starts_with("TOP_K[")),
        "the page must be bounded in the engine, not sorted whole and then cut: {:?}",
        operators(&page)
    );

    let joined = plan_with(
        &connection,
        "EXPLAIN",
        insight_work_store::CLAIM_PAGE_BY_JOINED_REVISION,
        claim_parameters,
    );
    assert_eq!(
        table_scans(&joined),
        2,
        "reading the moment from FileVersion scans every revision the graph holds, which is why the job carries it: {:?}",
        operators(&joined)
    );
}
