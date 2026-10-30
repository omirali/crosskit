// Reading the graph directly between measured runs: how much history it holds and whether
// it still satisfies the invariants the earlier stages established.
use std::path::Path;

use lbug::{Connection, Database, LogicalType, SystemConfig, Value};

/// How much history the database carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct History {
    pub file_versions: usize,
    pub observations: usize,
    pub observation_orders: usize,
    pub locations: usize,
    pub insight_jobs: usize,
}

pub fn history(database_path: &Path) -> History {
    let database = open(database_path, 0);
    let connection = Connection::new(&database).unwrap();
    History {
        file_versions: count(&connection, "MATCH (:FileVersion) RETURN COUNT(*);"),
        observations: count(&connection, "MATCH (:FileObservation) RETURN COUNT(*);"),
        observation_orders: count(&connection, "MATCH (:ObservationOrder) RETURN COUNT(*);"),
        locations: count(&connection, "MATCH (:FileLocation) RETURN COUNT(*);"),
        insight_jobs: count(&connection, "MATCH (:InsightJob) RETURN COUNT(*);"),
    }
}

/// The invariants of stages 2G and 2I, checked again after the diagnostic passes.
pub fn assert_invariants(database_path: &Path) {
    let database = open(database_path, 0);
    let connection = Connection::new(&database).unwrap();
    for (what, query) in [
        (
            "revision without chunks",
            "MATCH (version:FileVersion) WHERE NOT EXISTS { MATCH (version)-[:HAS_CHUNK]->(:ContentChunk) } RETURN COUNT(*);",
        ),
        (
            "revision without its outbox entry",
            "MATCH (version:FileVersion) WHERE NOT EXISTS { MATCH (version)-[:REQUESTS_INSIGHT]->(:InsightJob) } RETURN COUNT(*);",
        ),
        (
            "observation without its file",
            "MATCH (observation:FileObservation) WHERE NOT EXISTS { MATCH (:File)-[:HAS_OBSERVATION]->(observation) } RETURN COUNT(*);",
        ),
        (
            "observation without its revision",
            "MATCH (observation:FileObservation) WHERE NOT EXISTS { MATCH (observation)-[:OBSERVES]->(:FileVersion) } RETURN COUNT(*);",
        ),
        (
            "order without its observation",
            "MATCH (observation_order:ObservationOrder) WHERE NOT EXISTS { MATCH (observation_order)-[:ORDERS]->(:FileObservation) } RETURN COUNT(*);",
        ),
        (
            "run left running",
            "MATCH (:ReconcileRun {status: 'running'}) RETURN COUNT(*);",
        ),
    ] {
        assert_eq!(count(&connection, query), 0, "{what}");
    }
    let files_with_current = count(
        &connection,
        "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);",
    );
    let agreeing = count(
        &connection,
        "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion)<-[:OBSERVES]-(observation:FileObservation)<-[:CURRENT_OBSERVATION]-(file) RETURN COUNT(*);",
    );
    assert_eq!(
        files_with_current, agreeing,
        "both current pointers must name the same revision"
    );
    let duplicate_jobs = count(
        &connection,
        "MATCH (version:FileVersion)-[:REQUESTS_INSIGHT]->(job:InsightJob) WITH version, COUNT(job) AS jobs WHERE jobs > 1 RETURN COUNT(*);",
    );
    assert_eq!(duplicate_jobs, 0, "a revision must have one outbox entry");
}

pub fn open(database_path: &Path, buffer_pool_bytes: u64) -> Database {
    Database::new(
        database_path,
        SystemConfig::default().buffer_pool_size(buffer_pool_bytes),
    )
    .unwrap()
}

pub fn count(connection: &Connection<'_>, query: &str) -> usize {
    let mut result = connection.query(query).unwrap();
    match result.next().unwrap().as_slice() {
        [Value::Int64(value)] => usize::try_from(*value).unwrap(),
        values => panic!("expected one count, received {values:?}"),
    }
}

/// The identifiers a batch of the real pipeline would look up at once.
pub fn sample_ids(connection: &Connection<'_>, query: &str, limit: usize) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("{} LIMIT {limit};", query.trim_end_matches(';')))
        .unwrap();
    connection
        .execute(&mut statement, vec![])
        .unwrap()
        .map(|row| match row.as_slice() {
            [Value::String(id)] => id.clone(),
            values => panic!("expected one id, received {values:?}"),
        })
        .collect()
}

pub fn id_list(ids: &[String]) -> Value {
    Value::List(
        LogicalType::String,
        ids.iter().map(|id| Value::String(id.clone())).collect(),
    )
}
