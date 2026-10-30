// The read workload the benchmark measures in its own process, so a buffer pool size can
// be chosen per run and the resident memory of that run belongs to nothing else.
//
// It repeats the lookups the set-based stores perform for every batch of a reconcile: the
// current-state traversal, the observation and order lookups and the location lookups.
// It writes nothing.
use std::path::Path;
use std::time::Instant;

use lbug::{Connection, Value};

use super::graph::{count, id_list, open, sample_ids};
use super::statements::CURRENT_ORDER_LOOKUP;

/// One batch of the real pipeline.
const BATCH: usize = 100;

/// The query shapes a reconcile issues per batch, named for the report.
pub const PROBED_QUERIES: &[(&str, &str)] = &[
    // The statement the set-based store issues, not a copy of it.
    ("current-state traversal", CURRENT_ORDER_LOOKUP),
    (
        // The shape the stores used before the batch became set-based: one file at a
        // time, named by its primary key.
        "current-state traversal, one file",
        "MATCH (file:File {id: $id})-[:CURRENT_OBSERVATION]->(:FileObservation)<-[:ORDERS]-(observation_order:ObservationOrder) RETURN file.id, observation_order.observed_at_unix_ms, observation_order.sequence;",
    ),
    (
        "observation lookup",
        "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}) RETURN observation.id, observation.file_id, observation.file_version_id, observation.observed_at_unix_ms;",
    ),
    (
        "observation order lookup",
        "UNWIND $ids AS id MATCH (observation_order:ObservationOrder {id: id}) RETURN observation_order.id, observation_order.sequence;",
    ),
    (
        "observation sequence maximum",
        "MATCH (observation_order:ObservationOrder) RETURN MAX(observation_order.sequence);",
    ),
    (
        "location lookup",
        "UNWIND $ids AS id MATCH (location:FileLocation {id: id}) RETURN location.id, location.file_id, location.last_seen_at_unix_ms;",
    ),
];

/// How the probe links the observations it writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Links {
    /// Only the nodes, so the cost of the links can be read off by difference.
    None,
    /// What the store does today: both endpoints by primary key, then `CREATE`.
    Create,
    /// What it did before: one comma-separated pattern and `MERGE`.
    Merge,
}

impl Links {
    pub fn from_env(value: Option<&str>) -> Self {
        match value {
            Some("create") => Self::Create,
            Some("merge") => Self::Merge,
            _ => Self::None,
        }
    }

    fn statements(self) -> &'static [&'static str] {
        match self {
            Self::None => &[],
            Self::Create => CREATE_LINKS,
            Self::Merge => MERGE_LINKS,
        }
    }
}

/// The linking statements the store runs today, driven by a list of observation ids.
const CREATE_LINKS: &[&str] = &[
    "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}) MATCH (file:File {id: observation.file_id}) CREATE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);",
    "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}) MATCH (version:FileVersion {id: observation.file_version_id}) CREATE (observation)-[:OBSERVES]->(version) RETURN COUNT(*);",
    "UNWIND $ids AS id MATCH (observation_order:ObservationOrder {id: id}) MATCH (observation:FileObservation {id: id}) CREATE (observation_order)-[:ORDERS]->(observation) RETURN COUNT(*);",
];

/// The shape that was replaced, kept so one measurement shows the difference.
const MERGE_LINKS: &[&str] = &[
    "UNWIND $rows AS row MATCH (file:File {id: row.file_id}), (observation:FileObservation {id: row.id}) MERGE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);",
    "UNWIND $rows AS row MATCH (observation:FileObservation {id: row.id}), (version:FileVersion {id: row.file_version_id}) MERGE (observation)-[:OBSERVES]->(version) RETURN COUNT(*);",
    "UNWIND $rows AS row MATCH (observation_order:ObservationOrder {id: row.id}), (observation:FileObservation {id: row.id}) MERGE (observation_order)-[:ORDERS]->(observation) RETURN COUNT(*);",
];

/// The statements whose plan matters but which must not run: they would write.
pub const LINK_STATEMENTS: &[(&str, &str)] = &[
    (
        "HAS_OBSERVATION merge",
        "UNWIND $rows AS row MATCH (file:File {id: row.file_id}), (observation:FileObservation {id: row.id}) MERGE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);",
    ),
    (
        "OBSERVES merge",
        "UNWIND $rows AS row MATCH (observation:FileObservation {id: row.id}), (version:FileVersion {id: row.file_version_id}) MERGE (observation)-[:OBSERVES]->(version) RETURN COUNT(*);",
    ),
    (
        // The shape the store used before the batch became set-based: scalar parameters,
        // one relationship at a time.
        "HAS_OBSERVATION merge, one row",
        "MATCH (file:File {id: $file_id}), (observation:FileObservation {id: $observation_id}) MERGE (file)-[:HAS_OBSERVATION]->(observation);",
    ),
    (
        "ORDERS merge",
        "UNWIND $rows AS row MATCH (observation_order:ObservationOrder {id: row.id}), (observation:FileObservation {id: row.id}) MERGE (observation_order)-[:ORDERS]->(observation) RETURN COUNT(*);",
    ),
];

/// Prints the plan LadybugDB chose for each link statement, without running any of them.
pub fn link_plans(database_path: &Path) {
    let database = open(database_path, 0);
    let connection = Connection::new(&database).unwrap();
    let row = Value::Struct(vec![
        ("id".to_string(), Value::String("probe".to_string())),
        ("file_id".to_string(), Value::String("probe".to_string())),
        (
            "file_version_id".to_string(),
            Value::String("probe".to_string()),
        ),
    ]);
    let rows = Value::List(lbug::LogicalType::from(&row), vec![row]);
    for (name, query) in LINK_STATEMENTS {
        let mut statement = connection.prepare(&format!("EXPLAIN {query}")).unwrap();
        let parameters = if query.contains("$rows") {
            vec![("rows", rows.clone())]
        } else {
            vec![
                ("file_id", Value::String("probe".to_string())),
                ("observation_id", Value::String("probe".to_string())),
            ]
        };
        let mut result = connection.execute(&mut statement, parameters).unwrap();
        if let Some(values) = result.next()
            && let [Value::String(plan)] = values.as_slice()
        {
            println!("plan-begin|{name}");
            println!("{plan}");
            println!("plan-end|{name}");
        }
    }
}

/// Writes as one reconcile pass does: `batches` batches of a hundred observations and
/// their orders, each batch in its own transaction, through the same static statements
/// the stores use. It runs on a copy of the database, never on the measured one, and
/// exists only to tell a growing buffer pool apart from a query that materializes with
/// the size of the tables it touches.
pub fn write_probe(database_path: &Path, buffer_pool_bytes: u64, batches: usize, links: Links) {
    let database = open(database_path, buffer_pool_bytes);
    let connection = Connection::new(&database).unwrap();
    let observations = count(&connection, "MATCH (:FileObservation) RETURN COUNT(*);");
    println!("worker: buffer pool {buffer_pool_bytes} bytes, {observations} observations");
    let started = Instant::now();

    // The endpoints the links need; the probe owns them and touches nothing else.
    connection
        .query("MERGE (:File {id: 'probe:file'});")
        .unwrap();
    connection
        .query("CREATE (:FileVersion {id: 'probe:version', file_id: 'probe:file', content_hash: 'probe', created_at_unix_ms: 1});")
        .unwrap();

    for batch in 0..batches {
        connection.query("BEGIN TRANSACTION;").unwrap();
        let rows: Vec<Value> = (0..100)
            .map(|index| {
                let id = format!("probe:{batch}:{index}");
                Value::Struct(vec![
                    ("id".to_string(), Value::String(id)),
                    (
                        "file_id".to_string(),
                        Value::String("probe:file".to_string()),
                    ),
                    (
                        "file_version_id".to_string(),
                        Value::String("probe:version".to_string()),
                    ),
                    (
                        "observed_at_unix_ms".to_string(),
                        Value::Int64(1_000 + batch as i64),
                    ),
                    (
                        "sequence".to_string(),
                        Value::Int64((batch * 100 + index) as i64),
                    ),
                ])
            })
            .collect();
        let list = Value::List(lbug::LogicalType::from(&rows[0]), rows);
        let inserts = [
            "UNWIND $rows AS row CREATE (observation:FileObservation {id: row.id, file_id: row.file_id, file_version_id: row.file_version_id, observed_at_unix_ms: row.observed_at_unix_ms});",
            "UNWIND $rows AS row CREATE (observation_order:ObservationOrder {id: row.id, file_id: row.file_id, observed_at_unix_ms: row.observed_at_unix_ms, sequence: row.sequence});",
        ];
        let ids = Value::List(
            lbug::LogicalType::String,
            (0..100)
                .map(|index| Value::String(format!("probe:{batch}:{index}")))
                .collect(),
        );
        for query in inserts
            .iter()
            .copied()
            .chain(links.statements().iter().copied())
        {
            let mut statement = connection.prepare(query).unwrap();
            let parameters = if query.contains("$ids") {
                vec![("ids", ids.clone())]
            } else {
                vec![("rows", list.clone())]
            };
            connection.execute(&mut statement, parameters).unwrap();
        }
        connection.query("COMMIT;").unwrap();
    }

    println!(
        "write-probe|{batches}|{:.1}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

/// Runs the read workload `rounds` times over the database, with the given buffer pool
/// size (`0` asks LadybugDB for its own default). Prints one line per query shape with
/// the time the engine reported, and a plan for each shape when asked.
pub fn run(database_path: &Path, buffer_pool_bytes: u64, rounds: usize, print_plans: bool) {
    let database = open(database_path, buffer_pool_bytes);
    let connection = Connection::new(&database).unwrap();
    println!(
        "worker: buffer pool {} bytes, {} observations",
        buffer_pool_bytes,
        count(&connection, "MATCH (:FileObservation) RETURN COUNT(*);")
    );

    let file_ids = sample_ids(&connection, "MATCH (file:File) RETURN file.id", BATCH);
    let observation_ids = sample_ids(
        &connection,
        "MATCH (observation:FileObservation) RETURN observation.id",
        BATCH,
    );
    let location_ids = sample_ids(
        &connection,
        "MATCH (location:FileLocation) RETURN location.id",
        BATCH,
    );

    for (name, query) in PROBED_QUERIES {
        let ids: &[String] = match *name {
            "observation lookup" | "observation order lookup" => &observation_ids,
            "location lookup" => &location_ids,
            _ => &file_ids,
        };
        let started = Instant::now();
        let mut tuples = 0;
        let mut execution_ms = 0.0;
        // The batched shapes answer for a hundred identifiers at once; the single-file
        // shape answers for one, so it is issued once per identifier of the same batch.
        let single = query.contains("$id") && !query.contains("$ids");
        for _ in 0..rounds {
            let mut statement = connection.prepare(query).unwrap();
            let calls: Vec<&[String]> = if single {
                ids.iter().map(std::slice::from_ref).collect()
            } else {
                vec![ids]
            };
            for call in calls {
                let mut result = connection
                    .execute(&mut statement, parameters(query, call))
                    .unwrap();
                execution_ms += result.get_execution_time();
                tuples += result.by_ref().count();
            }
        }
        println!(
            "query|{name}|{:.1}|{:.1}|{tuples}",
            started.elapsed().as_secs_f64() * 1000.0,
            execution_ms
        );
        if print_plans {
            print_plan(&connection, name, query, ids);
        }
    }
}

fn parameters(query: &str, ids: &[String]) -> Vec<(&'static str, Value)> {
    if query.contains("$ids") {
        return vec![("ids", id_list(ids))];
    }
    if query.contains("$id") {
        return vec![("id", Value::String(ids[0].clone()))];
    }
    Vec::new()
}

/// The physical plan LadybugDB chose, as it prints it.
fn print_plan(connection: &Connection<'_>, name: &str, query: &str, ids: &[String]) {
    let mut statement = connection.prepare(&format!("EXPLAIN {query}")).unwrap();
    let mut result = connection
        .execute(&mut statement, parameters(query, ids))
        .unwrap();
    if let Some(row) = result.next()
        && let [Value::String(plan)] = row.as_slice()
    {
        println!("plan-begin|{name}");
        println!("{plan}");
        println!("plan-end|{name}");
    }
}
