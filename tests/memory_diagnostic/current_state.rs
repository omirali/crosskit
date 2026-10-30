// Isolated current-state workloads for the 2M capability spike. The fixture is built
// from the same synthetic corpus as the scale profile, but each measured worker gets a
// copy of it so a write cannot affect another sample.
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use lbug::{Connection, LogicalType, Value};

use super::corpus::CURRENT_STATE_SOURCE_IDENTITIES;
use super::graph::{count, id_list, open};
use super::statements::{
    CANDIDATE_CURRENT_ORDER_BY_OBSERVATION_ID, CURRENT_ORDER_LOOKUP, REMOVE_CURRENT_OBSERVATION,
    REMOVE_CURRENT_VERSION, REPLACED_SET_CURRENT_OBSERVATION, REPLACED_SET_CURRENT_VERSION,
    SET_CURRENT_OBSERVATION, SET_CURRENT_VERSION,
};

/// One production-sized `UNWIND` page, matching the batch writer.
const BATCH: usize = 100;
/// These incoming observations must be newer than every timestamp emitted by the fixture
/// corpus and its `index init` run.
const CANDIDATE_OBSERVED_AT: i64 = 9_000_000_000_000;
const CANDIDATE_SEQUENCE: i64 = 9_000_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fixture {
    Fresh,
    Reconcile,
}

impl Fixture {
    fn name(self) -> &'static str {
        match self {
            Self::Fresh => "fresh-like",
            Self::Reconcile => "reconcile-like",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    OrderLookup,
    PointerRemovals,
    PointerCreation,
    ReplacedPointerCreation,
    Sequence,
    CandidateOrderLookup,
}

/// A worker workload, including its fixture precondition. Every workload named for a
/// production statement runs the text the store runs; the replaced and candidate forms
/// stay here, not in production, so a comparison cannot become an implementation switch.
#[derive(Debug, Clone, Copy)]
pub struct Workload {
    label: &'static str,
    fixture: Fixture,
    operation: Operation,
    environment: &'static str,
}

impl Workload {
    pub fn label(self) -> &'static str {
        self.label
    }

    pub fn fixture(self) -> Fixture {
        self.fixture
    }

    pub fn environment(self) -> &'static str {
        self.environment
    }
}

pub const WORKLOADS: &[Workload] = &[
    Workload {
        label: "fresh-like: current-order lookup",
        fixture: Fixture::Fresh,
        operation: Operation::OrderLookup,
        environment: "fresh-order-lookup",
    },
    Workload {
        label: "fresh-like: pointer creation (two primary-key MATCH)",
        fixture: Fixture::Fresh,
        operation: Operation::PointerCreation,
        environment: "fresh-pointer-creation",
    },
    // The form Stage 2N replaced, on the same fixture as the one that replaced it, so the
    // two numbers of the report are comparable and neither can be read as the other.
    Workload {
        label: "fresh-like: replaced pointer creation (comma-separated MATCH)",
        fixture: Fixture::Fresh,
        operation: Operation::ReplacedPointerCreation,
        environment: "fresh-replaced-pointer-creation",
    },
    Workload {
        label: "fresh-like: current-state sequence (two primary-key MATCH)",
        fixture: Fixture::Fresh,
        operation: Operation::Sequence,
        environment: "fresh-sequence",
    },
    Workload {
        label: "reconcile-like: current-order lookup",
        fixture: Fixture::Reconcile,
        operation: Operation::OrderLookup,
        environment: "reconcile-order-lookup",
    },
    Workload {
        label: "reconcile-like: pointer removals (rolled back)",
        fixture: Fixture::Reconcile,
        operation: Operation::PointerRemovals,
        environment: "reconcile-pointer-removals",
    },
    Workload {
        label: "reconcile-like: current-state sequence (two primary-key MATCH)",
        fixture: Fixture::Reconcile,
        operation: Operation::Sequence,
        environment: "reconcile-sequence",
    },
    Workload {
        label: "reconcile-like: candidate order lookup by observation id",
        fixture: Fixture::Reconcile,
        operation: Operation::CandidateOrderLookup,
        environment: "reconcile-candidate-order-lookup",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    file_id: String,
    version_id: String,
    observation_id: String,
}

impl Candidate {
    fn injected(file_id: String) -> Self {
        Self {
            version_id: format!("current-state-candidate-version:{file_id}"),
            observation_id: format!("current-state-candidate-observation:{file_id}"),
            file_id,
        }
    }
}

/// Creates the current-state slice of schema v8 directly. It has exactly the four node
/// kinds and six relationships the current-state statements touch; no corpus text, chunks,
/// or application worker state can affect this isolated measurement.
pub fn prepare_fixture(database_path: &Path, fixture: Fixture) {
    let database = open(database_path, 0);
    let connection = Connection::new(&database)
        .unwrap_or_else(|_| panic!("the current-state diagnostic could not open its fixture"));
    for statement in FIXTURE_SCHEMA {
        run_query(&connection, statement, vec![]);
    }
    let seed = fixture_candidates();
    assert_eq!(
        seed.len(),
        CURRENT_STATE_SOURCE_IDENTITIES,
        "the fixture must contain the scale source identity count"
    );
    insert_candidates(&connection, &seed, 1, 1, true);
    assert_eq!(
        source_candidates(&connection).len(),
        CURRENT_STATE_SOURCE_IDENTITIES,
        "the fresh fixture must persist every file, version and observation"
    );

    match fixture {
        Fixture::Fresh => {
            assert_pointer_absence(&connection);
        }
        Fixture::Reconcile => {
            set_pointers(
                &connection,
                SET_CURRENT_VERSION,
                SET_CURRENT_OBSERVATION,
                &seed,
            );
            let incoming: Vec<_> = seed
                .into_iter()
                .map(|candidate| Candidate::injected(candidate.file_id))
                .collect();
            insert_candidates(
                &connection,
                &incoming,
                CANDIDATE_OBSERVED_AT,
                CANDIDATE_SEQUENCE,
                false,
            );
            assert_current_state(&connection);
            assert_candidates_are_newer(&connection, &incoming);
        }
    }
}

const FIXTURE_SCHEMA: &[&str] = &[
    "CREATE NODE TABLE File(id STRING PRIMARY KEY);",
    "CREATE NODE TABLE FileVersion(id STRING PRIMARY KEY, file_id STRING, content_hash STRING, created_at_unix_ms INT64);",
    "CREATE NODE TABLE FileObservation(id STRING PRIMARY KEY, file_id STRING, file_version_id STRING, observed_at_unix_ms INT64);",
    "CREATE NODE TABLE ObservationOrder(id STRING PRIMARY KEY, file_id STRING, observed_at_unix_ms INT64, sequence INT64);",
    "CREATE REL TABLE HAS_VERSION(FROM File TO FileVersion ONE_MANY);",
    "CREATE REL TABLE CURRENT_VERSION(FROM File TO FileVersion ONE_ONE);",
    "CREATE REL TABLE HAS_OBSERVATION(FROM File TO FileObservation ONE_MANY);",
    "CREATE REL TABLE OBSERVES(FROM FileObservation TO FileVersion MANY_ONE);",
    "CREATE REL TABLE ORDERS(FROM ObservationOrder TO FileObservation ONE_ONE);",
    "CREATE REL TABLE CURRENT_OBSERVATION(FROM File TO FileObservation ONE_ONE);",
];

/// Executes one isolated workload. The parent gives every invocation a private copy of a
/// prepared fixture and measures that process with `/usr/bin/time -l`.
pub fn run_worker(database_path: &Path, environment: &str) {
    let workload = workload(environment);
    let database = open(database_path, 0);
    let connection = Connection::new(&database).unwrap_or_else(|_| {
        panic!("the current-state diagnostic worker could not open its database")
    });
    let candidates = candidates_for(&connection, workload.fixture);
    assert_eq!(
        candidates.len(),
        CURRENT_STATE_SOURCE_IDENTITIES,
        "the workload must operate on every scale source identity"
    );
    assert_fixture_before(&connection, workload.fixture);

    let started = Instant::now();
    match workload.operation {
        Operation::OrderLookup => {
            let found = lookup(&connection, CURRENT_ORDER_LOOKUP, &candidates);
            let expected = usize::from(workload.fixture == Fixture::Reconcile)
                * CURRENT_STATE_SOURCE_IDENTITIES;
            assert_eq!(
                found.len(),
                expected,
                "the lookup must see the fixture state"
            );
        }
        Operation::PointerRemovals => {
            run_query(&connection, "BEGIN TRANSACTION;", vec![]);
            remove_pointers(&connection, &candidates);
            // The removal pair, measured in its production transaction shape. It rolls
            // back so the postcondition can prove no lone pointer escaped.
            run_query(&connection, "ROLLBACK;", vec![]);
        }
        Operation::PointerCreation => {
            run_query(&connection, "BEGIN TRANSACTION;", vec![]);
            set_pointers(
                &connection,
                SET_CURRENT_VERSION,
                SET_CURRENT_OBSERVATION,
                &candidates,
            );
            run_query(&connection, "COMMIT;", vec![]);
        }
        Operation::ReplacedPointerCreation => {
            run_query(&connection, "BEGIN TRANSACTION;", vec![]);
            set_pointers(
                &connection,
                REPLACED_SET_CURRENT_VERSION,
                REPLACED_SET_CURRENT_OBSERVATION,
                &candidates,
            );
            run_query(&connection, "COMMIT;", vec![]);
        }
        Operation::Sequence => {
            run_current_state_sequence(&connection, workload.fixture, &candidates);
        }
        Operation::CandidateOrderLookup => {
            let found = lookup(
                &connection,
                CANDIDATE_CURRENT_ORDER_BY_OBSERVATION_ID,
                &candidates,
            );
            assert_eq!(
                found.len(),
                CURRENT_STATE_SOURCE_IDENTITIES,
                "the candidate lookup must answer for every current observation"
            );
        }
    }
    let elapsed = started.elapsed();

    assert_workload_after(&connection, workload, &candidates);
    println!(
        "current-state-workload|{}|{}|{:.1}",
        workload.fixture.name(),
        workload.label,
        elapsed.as_secs_f64() * 1_000.0
    );
}

fn workload(environment: &str) -> Workload {
    WORKLOADS
        .iter()
        .copied()
        .find(|workload| workload.environment == environment)
        .unwrap_or_else(|| panic!("the current-state diagnostic received an unknown workload"))
}

fn assert_fixture_before(connection: &Connection<'_>, fixture: Fixture) {
    match fixture {
        Fixture::Fresh => assert_pointer_absence(connection),
        Fixture::Reconcile => assert_current_state(connection),
    }
}

fn assert_workload_after(
    connection: &Connection<'_>,
    workload: Workload,
    candidates: &[Candidate],
) {
    match workload.operation {
        Operation::PointerCreation | Operation::ReplacedPointerCreation | Operation::Sequence => {
            assert_current_state(connection);
            assert_current_targets(connection, candidates);
            if workload.fixture == Fixture::Reconcile {
                assert_committed_order_is_the_candidate(connection, candidates);
            }
        }
        Operation::PointerRemovals => {
            // The transaction rollback is the relevant guarantee for the destructive half
            // of replacement: no committed graph contains only one current pointer.
            assert_current_state(connection);
            assert_candidates_are_newer(connection, candidates);
        }
        Operation::OrderLookup | Operation::CandidateOrderLookup => {
            assert_fixture_before(connection, workload.fixture);
        }
    }
}

fn run_current_state_sequence(
    connection: &Connection<'_>,
    fixture: Fixture,
    candidates: &[Candidate],
) {
    run_query(connection, "BEGIN TRANSACTION;", vec![]);
    let current = lookup(connection, CURRENT_ORDER_LOOKUP, candidates);
    match fixture {
        Fixture::Fresh => assert!(current.is_empty(), "fresh-like has no current pointers"),
        Fixture::Reconcile => assert_candidates_are_newer_than(&current, candidates),
    }
    remove_pointers(connection, candidates);
    set_pointers(
        connection,
        SET_CURRENT_VERSION,
        SET_CURRENT_OBSERVATION,
        candidates,
    );
    run_query(connection, "COMMIT;", vec![]);
}

fn remove_pointers(connection: &Connection<'_>, candidates: &[Candidate]) {
    for batch in candidates.chunks(BATCH) {
        run_query(
            connection,
            REMOVE_CURRENT_VERSION,
            vec![("rows", candidate_values(batch))],
        );
        run_query(
            connection,
            REMOVE_CURRENT_OBSERVATION,
            vec![("rows", candidate_values(batch))],
        );
    }
}

fn set_pointers(
    connection: &Connection<'_>,
    version_query: &str,
    observation_query: &str,
    candidates: &[Candidate],
) {
    for batch in candidates.chunks(BATCH) {
        assert_count(
            run_query(
                connection,
                version_query,
                vec![("rows", candidate_values(batch))],
            ),
            batch.len(),
            "current-version pointer creation",
        );
        assert_count(
            run_query(
                connection,
                observation_query,
                vec![("rows", candidate_values(batch))],
            ),
            batch.len(),
            "current-observation pointer creation",
        );
    }
}

fn lookup(
    connection: &Connection<'_>,
    query: &str,
    candidates: &[Candidate],
) -> HashMap<String, (i64, i64)> {
    let mut found = HashMap::with_capacity(candidates.len());
    for batch in candidates.chunks(BATCH) {
        for row in run_query(
            connection,
            query,
            vec![(
                "ids",
                id_list(
                    &batch
                        .iter()
                        .map(|candidate| candidate.file_id.clone())
                        .collect::<Vec<_>>(),
                ),
            )],
        ) {
            match row.as_slice() {
                [
                    Value::String(file_id),
                    Value::Int64(observed_at),
                    Value::Int64(sequence),
                ] => {
                    assert!(
                        found
                            .insert(file_id.clone(), (*observed_at, *sequence))
                            .is_none(),
                        "a file must have at most one current order"
                    );
                }
                _ => panic!("the current-state lookup returned an invalid row"),
            }
        }
    }
    found
}

fn source_candidates(connection: &Connection<'_>) -> Vec<Candidate> {
    let rows = run_query(
        connection,
        "MATCH (file:File)-[:HAS_VERSION]->(version:FileVersion)<-[:OBSERVES]-(observation:FileObservation) RETURN file.id, version.id, observation.id;",
        vec![],
    );
    let mut candidates = Vec::with_capacity(rows.len());
    let mut files = HashSet::with_capacity(rows.len());
    for row in rows {
        match row.as_slice() {
            [
                Value::String(file_id),
                Value::String(version_id),
                Value::String(observation_id),
            ] => {
                assert!(
                    files.insert(file_id.clone()),
                    "the fresh fixture must have one observation per file"
                );
                candidates.push(Candidate {
                    file_id: file_id.clone(),
                    version_id: version_id.clone(),
                    observation_id: observation_id.clone(),
                });
            }
            _ => panic!("the fixture did not return file, version and observation identities"),
        }
    }
    candidates
}

fn candidates_for(connection: &Connection<'_>, fixture: Fixture) -> Vec<Candidate> {
    match fixture {
        Fixture::Fresh => source_candidates(connection),
        Fixture::Reconcile => {
            let rows = run_query(connection, "MATCH (file:File) RETURN file.id;", vec![]);
            rows.into_iter()
                .map(|row| match row.as_slice() {
                    [Value::String(file_id)] => Candidate::injected(file_id.clone()),
                    _ => panic!("the fixture did not return a file identity"),
                })
                .collect()
        }
    }
}

fn insert_candidates(
    connection: &Connection<'_>,
    candidates: &[Candidate],
    observed_at: i64,
    sequence: i64,
    create_files: bool,
) {
    for batch in candidates.chunks(BATCH) {
        let rows = candidate_values_at(batch, observed_at, sequence);
        if create_files {
            assert_count(
                run_query(
                    connection,
                    "UNWIND $rows AS row CREATE (file:File {id: row.file_id}) RETURN COUNT(file);",
                    vec![("rows", rows.clone())],
                ),
                batch.len(),
                "fixture file creation",
            );
        }
        for (what, query) in [
            (
                "fixture version creation",
                "UNWIND $rows AS row CREATE (version:FileVersion {id: row.version_id, file_id: row.file_id, content_hash: row.version_id, created_at_unix_ms: row.observed_at_unix_ms}) RETURN COUNT(version);",
            ),
            (
                "fixture observation creation",
                "UNWIND $rows AS row CREATE (observation:FileObservation {id: row.observation_id, file_id: row.file_id, file_version_id: row.version_id, observed_at_unix_ms: row.observed_at_unix_ms}) RETURN COUNT(observation);",
            ),
            (
                "fixture order creation",
                "UNWIND $rows AS row CREATE (observation_order:ObservationOrder {id: row.observation_id, file_id: row.file_id, observed_at_unix_ms: row.observed_at_unix_ms, sequence: row.sequence}) RETURN COUNT(observation_order);",
            ),
            (
                "fixture file-version link",
                "UNWIND $rows AS row MATCH (file:File {id: row.file_id}) MATCH (version:FileVersion {id: row.version_id}) CREATE (file)-[:HAS_VERSION]->(version) RETURN COUNT(*);",
            ),
            (
                "fixture file-observation link",
                "UNWIND $rows AS row MATCH (file:File {id: row.file_id}) MATCH (observation:FileObservation {id: row.observation_id}) CREATE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);",
            ),
            (
                "fixture observation-version link",
                "UNWIND $rows AS row MATCH (observation:FileObservation {id: row.observation_id}) MATCH (version:FileVersion {id: row.version_id}) CREATE (observation)-[:OBSERVES]->(version) RETURN COUNT(*);",
            ),
            (
                "fixture order-observation link",
                "UNWIND $rows AS row MATCH (observation_order:ObservationOrder {id: row.observation_id}) MATCH (observation:FileObservation {id: row.observation_id}) CREATE (observation_order)-[:ORDERS]->(observation) RETURN COUNT(*);",
            ),
        ] {
            assert_count(
                run_query(connection, query, vec![("rows", rows.clone())]),
                batch.len(),
                what,
            );
        }
    }
}

fn candidate_values(candidates: &[Candidate]) -> Value {
    candidate_values_at(candidates, CANDIDATE_OBSERVED_AT, CANDIDATE_SEQUENCE)
}

fn candidate_values_at(candidates: &[Candidate], observed_at: i64, sequence: i64) -> Value {
    let rows: Vec<_> = candidates
        .iter()
        .map(|candidate| {
            Value::Struct(vec![
                (
                    "file_id".to_string(),
                    Value::String(candidate.file_id.clone()),
                ),
                (
                    "version_id".to_string(),
                    Value::String(candidate.version_id.clone()),
                ),
                (
                    "observation_id".to_string(),
                    Value::String(candidate.observation_id.clone()),
                ),
                ("observed_at_unix_ms".to_string(), Value::Int64(observed_at)),
                ("sequence".to_string(), Value::Int64(sequence)),
            ])
        })
        .collect();
    Value::List(LogicalType::from(&rows[0]), rows)
}

fn fixture_candidates() -> Vec<Candidate> {
    (0..CURRENT_STATE_SOURCE_IDENTITIES)
        .map(|index| Candidate {
            file_id: format!("current-state-file:{index}"),
            version_id: format!("current-state-version:{index}"),
            observation_id: format!("current-state-observation:{index}"),
        })
        .collect()
}

fn assert_pointer_absence(connection: &Connection<'_>) {
    assert_eq!(
        count(
            connection,
            "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        0,
        "fresh-like starts without current-version pointers"
    );
    assert_eq!(
        count(
            connection,
            "MATCH (:File)-[:CURRENT_OBSERVATION]->(:FileObservation) RETURN COUNT(*);",
        ),
        0,
        "fresh-like starts without current-observation pointers"
    );
}

fn assert_current_state(connection: &Connection<'_>) {
    let versions = count(
        connection,
        "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);",
    );
    let observations = count(
        connection,
        "MATCH (:File)-[:CURRENT_OBSERVATION]->(:FileObservation) RETURN COUNT(*);",
    );
    let agreeing = count(
        connection,
        "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion)<-[:OBSERVES]-(observation:FileObservation)<-[:CURRENT_OBSERVATION]-(file) RETURN COUNT(*);",
    );
    assert_eq!(versions, CURRENT_STATE_SOURCE_IDENTITIES);
    assert_eq!(observations, CURRENT_STATE_SOURCE_IDENTITIES);
    assert_eq!(agreeing, CURRENT_STATE_SOURCE_IDENTITIES);
}

fn assert_current_targets(connection: &Connection<'_>, candidates: &[Candidate]) {
    let rows = run_query(
        connection,
        "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion), (file)-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN file.id, version.id, observation.id;",
        vec![],
    );
    let actual: HashMap<_, _> = rows
        .into_iter()
        .map(|row| match row.as_slice() {
            [
                Value::String(file_id),
                Value::String(version_id),
                Value::String(observation_id),
            ] => (
                file_id.clone(),
                (version_id.clone(), observation_id.clone()),
            ),
            _ => panic!("the current-state assertion returned an invalid pointer row"),
        })
        .collect();
    assert_eq!(actual.len(), candidates.len());
    for candidate in candidates {
        assert_eq!(
            actual.get(&candidate.file_id),
            Some(&(
                candidate.version_id.clone(),
                candidate.observation_id.clone()
            )),
            "the current pointers must name the winning observation and its version"
        );
    }
}

fn assert_candidates_are_newer(connection: &Connection<'_>, candidates: &[Candidate]) {
    assert_candidates_are_newer_than(
        &lookup(connection, CURRENT_ORDER_LOOKUP, candidates),
        candidates,
    );
}

fn assert_candidates_are_newer_than(
    current: &HashMap<String, (i64, i64)>,
    candidates: &[Candidate],
) {
    assert_eq!(current.len(), candidates.len());
    for candidate in candidates {
        let stored = current
            .get(&candidate.file_id)
            .unwrap_or_else(|| panic!("a reconcile fixture file is missing its current order"));
        assert!(
            (CANDIDATE_OBSERVED_AT, CANDIDATE_SEQUENCE) > *stored,
            "the incoming candidate must be newer by observed_at and sequence"
        );
    }
}

/// The committed pointer carries the candidate's order, so the reconcile fixture's older
/// order no longer stands: the measured write moved the current state forward and left it
/// readable through `ORDERS`.
///
/// This says nothing about a retry. The rule that an older candidate is filtered before
/// any removal or set lives in the store, which this diagnostic deliberately does not
/// call, and is proven by `retrying_an_older_commit_does_not_move_current_version_backwards`
/// and `a_single_observation_can_be_retried_without_moving_its_sequence`.
fn assert_committed_order_is_the_candidate(connection: &Connection<'_>, candidates: &[Candidate]) {
    let current = lookup(connection, CURRENT_ORDER_LOOKUP, candidates);
    for candidate in candidates {
        let stored = current
            .get(&candidate.file_id)
            .unwrap_or_else(|| panic!("the committed current pointer must retain an order"));
        assert_eq!(*stored, (CANDIDATE_OBSERVED_AT, CANDIDATE_SEQUENCE));
    }
}

fn assert_count(rows: Vec<Vec<Value>>, expected: usize, context: &str) {
    match rows.as_slice() {
        [values] => match values.as_slice() {
            [Value::Int64(actual)] => assert_eq!(*actual as usize, expected, "{context}"),
            _ => panic!("the current-state write did not return its processed row count"),
        },
        _ => panic!("the current-state write did not return its processed row count"),
    }
}

fn run_query(
    connection: &Connection<'_>,
    query: &str,
    parameters: Vec<(&str, Value)>,
) -> Vec<Vec<Value>> {
    let mut statement = connection
        .prepare(query)
        .unwrap_or_else(|_| panic!("the current-state diagnostic could not prepare a statement"));
    connection
        .execute(&mut statement, parameters)
        .unwrap_or_else(|_| panic!("the current-state diagnostic statement did not execute"))
        .collect()
}
