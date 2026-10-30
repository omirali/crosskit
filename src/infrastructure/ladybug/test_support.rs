// Shared fixtures for tests that open a real Ladybug database.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lbug::{Connection, Database, SystemConfig, Value};

use super::LadybugDatabaseManager;
use super::migration;
use super::repository::LadybugGraphRepository;
use crate::application::InsightQueueReader;
use crate::domain::{
    AnalysisRequest, ChunkScript, ContentChunk, EvidenceCommit, FileKind, FileLocation,
    FileObservation, FileVersion, InsightJobState, NormalizedPath, ReconcileRun, SourceCommit,
};

static NEXT_TEST_DATABASE: AtomicU64 = AtomicU64::new(0);

/// A database path in the temp directory, removed together with its WAL on drop. Declare
/// it before the manager and repository so they close before the files disappear.
pub struct TestDatabase {
    path: PathBuf,
}

impl TestDatabase {
    pub fn new(test_name: &str) -> Self {
        let sequence = NEXT_TEST_DATABASE.fetch_add(1, Ordering::Relaxed);
        Self {
            path: std::env::temp_dir().join(format!(
                "crosskit-{test_name}-{}-{sequence}.lbug",
                std::process::id()
            )),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        for database_file in [
            self.path.clone(),
            PathBuf::from(format!("{}.wal", self.path.display())),
        ] {
            match std::fs::remove_file(&database_file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!(
                    "failed to remove test database {}: {error}",
                    database_file.display()
                ),
            }
        }
    }
}

pub fn query_int64(
    manager: &LadybugDatabaseManager,
    query: &str,
    parameters: Vec<(&str, Value)>,
) -> i64 {
    let connection = Connection::new(&manager.owner.database).unwrap();
    let mut statement = connection.prepare(query).unwrap();
    let mut result = connection.execute(&mut statement, parameters).unwrap();
    match result.next().unwrap().as_slice() {
        [Value::Int64(value)] => *value,
        values => panic!("expected a single INT64, received {values:?}"),
    }
}

pub fn query_string(
    manager: &LadybugDatabaseManager,
    query: &str,
    parameters: Vec<(&str, Value)>,
) -> String {
    let connection = Connection::new(&manager.owner.database).unwrap();
    let mut statement = connection.prepare(query).unwrap();
    let mut result = connection.execute(&mut statement, parameters).unwrap();
    match result.next().unwrap().as_slice() {
        [Value::String(value)] => value.clone(),
        values => panic!("expected a single STRING, received {values:?}"),
    }
}

pub fn count(manager: &LadybugDatabaseManager, query: &str) -> i64 {
    query_int64(manager, query, vec![])
}

/// How many jobs still wait in the outbox, read the way the `index jobs` summary reads it.
pub fn queued_jobs(repository: &LadybugGraphRepository) -> usize {
    repository
        .insight_jobs_by_state()
        .unwrap()
        .count(InsightJobState::Queued)
}

/// A consistent current state points CURRENT_VERSION and CURRENT_OBSERVATION of the same
/// file at a version and at an observation of exactly that version.
pub fn assert_current_version(manager: &LadybugDatabaseManager, version_id: &str) {
    assert_eq!(
        query_int64(
            manager,
            "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion {id: $version_id}) RETURN COUNT(*);",
            vec![("version_id", Value::String(version_id.to_string()))],
        ),
        1,
        "CURRENT_VERSION must point at {version_id}"
    );
    assert_eq!(
        count(
            manager,
            "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion)<-[:OBSERVES]-(:FileObservation)<-[:CURRENT_OBSERVATION]-(file) RETURN COUNT(*);"
        ),
        1,
        "CURRENT_OBSERVATION must observe the current version"
    );
}

pub fn path(path: &str) -> NormalizedPath {
    NormalizedPath::from_absolute(path).unwrap()
}

pub fn located(file_id: &str, location_path: &str) -> FileLocation {
    FileLocation::observed(
        file_id,
        path(location_path),
        FileKind::RegularFile,
        5,
        100,
        1_000,
    )
    .unwrap()
}

pub fn metadata_commit(file_id: &str, location_path: &str) -> SourceCommit {
    SourceCommit::metadata_only(located(file_id, location_path))
}

/// A located file with one version, one observation and one chunk.
pub fn evidence_commit(file_id: &str, location_path: &str) -> SourceCommit {
    evidence_commit_revision(
        file_id,
        location_path,
        "hash:a",
        &format!("observation:{file_id}"),
        100,
    )
}

/// The same shape with an explicit content hash, observation id and time, for a second
/// revision of a file or a repeated observation of the same one.
pub fn evidence_commit_revision(
    file_id: &str,
    location_path: &str,
    content_hash: &str,
    observation_id: &str,
    observed_at_unix_ms: i64,
) -> SourceCommit {
    commit_revision(
        file_id,
        location_path,
        content_hash,
        observation_id,
        observed_at_unix_ms,
        AnalysisRequest::Requested,
        "hello",
    )
}

/// A revision whose one chunk says `text`, for a test that needs the words a quote is found
/// in. The observation is named after the revision, so two revisions of one file are two
/// observations.
pub fn evidence_commit_saying(
    file_id: &str,
    location_path: &str,
    content_hash: &str,
    observed_at_unix_ms: i64,
    text: &str,
) -> SourceCommit {
    commit_revision(
        file_id,
        location_path,
        content_hash,
        &format!("observation:{file_id}@{content_hash}"),
        observed_at_unix_ms,
        AnalysisRequest::Requested,
        text,
    )
}

/// A revision whose one chunk has no byte range, the way a reply of a conversation is stored:
/// it was extracted out of JSON, and its size is its text.
pub fn evidence_commit_without_range(
    file_id: &str,
    location_path: &str,
    content_hash: &str,
    text: &str,
) -> SourceCommit {
    let version = FileVersion::new(
        format!("{file_id}@{content_hash}"),
        file_id,
        content_hash,
        100,
    )
    .unwrap();
    let observation =
        FileObservation::new(format!("observation:{file_id}"), file_id, version.id(), 100).unwrap();
    let chunk = ContentChunk::new(
        format!("chunk:{}", version.id()),
        version.id(),
        0,
        0,
        0,
        format!("text:{text}"),
        text,
        ChunkScript::Cyrillic,
        "conversation",
        "conversation",
        "1",
        "config:1",
        version.content_hash(),
        100,
    )
    .unwrap();
    SourceCommit::with_evidence(
        located(file_id, location_path),
        EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

/// The same revision published by an installation whose rules do not cover it: the
/// evidence lands and the outbox stays empty. This is what every file looks like before
/// somebody approves the mask that asks for it.
pub fn unanalysed_evidence_commit_revision(
    file_id: &str,
    location_path: &str,
    content_hash: &str,
    observation_id: &str,
    observed_at_unix_ms: i64,
) -> SourceCommit {
    commit_revision(
        file_id,
        location_path,
        content_hash,
        observation_id,
        observed_at_unix_ms,
        AnalysisRequest::NotRequested,
        "hello",
    )
}

fn commit_revision(
    file_id: &str,
    location_path: &str,
    content_hash: &str,
    observation_id: &str,
    observed_at_unix_ms: i64,
    analysis: AnalysisRequest,
    text: &str,
) -> SourceCommit {
    let version = FileVersion::new(
        format!("{file_id}@{content_hash}"),
        file_id,
        content_hash,
        observed_at_unix_ms,
    )
    .unwrap();
    let observation =
        FileObservation::new(observation_id, file_id, version.id(), observed_at_unix_ms).unwrap();
    let chunk = ContentChunk::new(
        format!("chunk:{}", version.id()),
        version.id(),
        0,
        0,
        text.len() as u64,
        format!("text:{text}"),
        text,
        ChunkScript::Latin,
        "plain-text",
        "plain-text",
        "1",
        "config:1",
        version.content_hash(),
        observed_at_unix_ms,
    )
    .unwrap();
    SourceCommit::with_evidence(
        located(file_id, location_path),
        EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
        analysis,
    )
    .unwrap()
}

pub fn status_of(manager: &LadybugDatabaseManager, location_path: &str) -> String {
    query_string(
        manager,
        "MATCH (location:FileLocation {normalized_path: $path}) RETURN location.status;",
        vec![("path", Value::String(location_path.to_string()))],
    )
}

pub fn run_status(manager: &LadybugDatabaseManager, run: &ReconcileRun) -> String {
    query_string(
        manager,
        "MATCH (run:ReconcileRun {id: $id}) RETURN run.status;",
        vec![("id", Value::String(run.id().to_string()))],
    )
}

/// Legacy data: file:1 with versions a and b, b being current.
pub const V1_DATA: &[&str] = &[
    "CREATE (:File {id: 'file:1'});",
    "CREATE (:FileVersion {id: 'file:1@hash:a', file_id: 'file:1', content_hash: 'hash:a', created_at_unix_ms: 100});",
    "CREATE (:FileVersion {id: 'file:1@hash:b', file_id: 'file:1', content_hash: 'hash:b', created_at_unix_ms: 200});",
    "MATCH (file:File {id: 'file:1'}), (version:FileVersion {id: 'file:1@hash:a'}) CREATE (file)-[:HAS_VERSION]->(version);",
    "MATCH (file:File {id: 'file:1'}), (version:FileVersion {id: 'file:1@hash:b'}) CREATE (file)-[:HAS_VERSION]->(version), (file)-[:CURRENT_VERSION]->(version);",
];

pub fn open_fixture_database(path: &Path) -> Database {
    Database::new(path, SystemConfig::default().enable_multi_writes(false)).unwrap()
}

/// A database written by the first reconcile slice at schema v6: legacy evidence, one
/// present location, a completed run and the OBSERVED edge that model used to write.
pub fn create_v6_fixture(path: &Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).unwrap();
    migration::apply_through(&connection, 1).unwrap();
    for statement in V1_DATA {
        connection.query(statement).unwrap();
    }
    migration::apply_through(&connection, 6).unwrap();
    for statement in [
        "CREATE (:FileLocation {id: 'file:1@/files/one.txt', file_id: 'file:1', normalized_path: '/files/one.txt', display_name: 'one.txt', kind: 'file', size_bytes: 5, modified_at_unix_ms: 100, first_seen_at_unix_ms: 1000, last_seen_at_unix_ms: 1000, status: 'present'});",
        "MATCH (file:File {id: 'file:1'}), (location:FileLocation {id: 'file:1@/files/one.txt'}) CREATE (file)-[:LOCATED_AT]->(location);",
        "CREATE (:ReconcileRun {id: 'reconcile-run:3', sequence: 3, root: '/files', status: 'completed', started_at_unix_ms: 1, finished_at_unix_ms: 2});",
        "MATCH (run:ReconcileRun {id: 'reconcile-run:3'}), (location:FileLocation {id: 'file:1@/files/one.txt'}) CREATE (run)-[:OBSERVED]->(location);",
    ] {
        connection.query(statement).unwrap();
    }
}

/// A database written by the previous release at schema v7: legacy evidence with two
/// revisions and no insight jobs yet.
pub fn create_v7_fixture(path: &Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).unwrap();
    migration::apply_through(&connection, 1).unwrap();
    for statement in V1_DATA {
        connection.query(statement).unwrap();
    }
    migration::apply_through(&connection, 7).unwrap();
}

/// A database written by the previous release at schema v9: legacy evidence with two
/// revisions and the queued jobs the v8 backfill gave them, but nothing a worker needs.
pub fn create_v9_fixture(path: &Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).unwrap();
    migration::apply_through(&connection, 1).unwrap();
    for statement in V1_DATA {
        connection.query(statement).unwrap();
    }
    migration::apply_through(&connection, 9).unwrap();
}

/// A located file with one version, one observation and one chunk, at an explicit path
/// and with an explicit observation id: the shape a hard link or a repeated sighting has.
pub fn evidence_commit_at(
    file_id: &str,
    location_path: &str,
    content_hash: &str,
    observation_id: &str,
    observed_at_unix_ms: i64,
) -> SourceCommit {
    let version = FileVersion::new(
        format!("{file_id}@{content_hash}"),
        file_id,
        content_hash,
        observed_at_unix_ms,
    )
    .unwrap();
    let observation =
        FileObservation::new(observation_id, file_id, version.id(), observed_at_unix_ms).unwrap();
    let chunk = ContentChunk::new(
        format!("chunk:{}", version.id()),
        version.id(),
        0,
        0,
        5,
        "text:hello",
        "hello",
        ChunkScript::Latin,
        "plain-text",
        "plain-text",
        "1",
        "config:1",
        version.content_hash(),
        observed_at_unix_ms,
    )
    .unwrap();
    SourceCommit::with_evidence(
        located(file_id, location_path),
        EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

/// Every row of the graph that a batch may write, normalized and sorted, so two databases
/// built by different paths can be compared exactly.
pub fn graph_snapshot(manager: &LadybugDatabaseManager) -> Vec<String> {
    let queries = [
        "MATCH (file:File) RETURN 'file|' + file.id ORDER BY file.id;",
        "MATCH (location:FileLocation) RETURN 'location|' + location.id + '|' + location.normalized_path + '|' + location.status + '|' + CAST(location.size_bytes AS STRING) + '|' + CAST(location.first_seen_at_unix_ms AS STRING) + '|' + CAST(location.last_seen_at_unix_ms AS STRING) + '|' + CAST(location.seen_in_epoch AS STRING) ORDER BY location.id;",
        "MATCH (version:FileVersion) RETURN 'version|' + version.id + '|' + version.file_id + '|' + version.content_hash ORDER BY version.id;",
        "MATCH (chunk:ContentChunk) RETURN 'chunk|' + chunk.id + '|' + chunk.source_revision_id + '|' + chunk.text_hash ORDER BY chunk.id;",
        "MATCH (observation:FileObservation) RETURN 'observation|' + observation.id + '|' + observation.file_id + '|' + observation.file_version_id + '|' + CAST(observation.observed_at_unix_ms AS STRING) ORDER BY observation.id;",
        "MATCH (observation_order:ObservationOrder) RETURN 'order|' + observation_order.id + '|' + CAST(observation_order.sequence AS STRING) ORDER BY observation_order.id;",
        "MATCH (job:InsightJob) RETURN 'job|' + job.id + '|' + job.source_revision_id + '|' + job.state + '|' + CAST(job.revision_observed_at_unix_ms AS STRING) ORDER BY job.id;",
        "MATCH (extractor:Extractor) RETURN 'extractor|' + extractor.id ORDER BY extractor.id;",
        "MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation) RETURN 'located_at|' + file.id + '|' + location.id ORDER BY file.id, location.id;",
        "MATCH (file:File)-[:HAS_VERSION]->(version:FileVersion) RETURN 'has_version|' + file.id + '|' + version.id ORDER BY file.id, version.id;",
        "MATCH (version:FileVersion)-[:HAS_CHUNK]->(chunk:ContentChunk) RETURN 'has_chunk|' + version.id + '|' + chunk.id ORDER BY version.id, chunk.id;",
        "MATCH (chunk:ContentChunk)-[:GENERATED_BY]->(extractor:Extractor) RETURN 'generated_by|' + chunk.id + '|' + extractor.id ORDER BY chunk.id;",
        "MATCH (file:File)-[:HAS_OBSERVATION]->(observation:FileObservation) RETURN 'has_observation|' + file.id + '|' + observation.id ORDER BY file.id, observation.id;",
        "MATCH (observation:FileObservation)-[:OBSERVES]->(version:FileVersion) RETURN 'observes|' + observation.id + '|' + version.id ORDER BY observation.id;",
        "MATCH (observation_order:ObservationOrder)-[:ORDERS]->(observation:FileObservation) RETURN 'orders|' + observation_order.id + '|' + observation.id ORDER BY observation_order.id;",
        "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion) RETURN 'current_version|' + file.id + '|' + version.id ORDER BY file.id;",
        "MATCH (file:File)-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN 'current_observation|' + file.id + '|' + observation.id ORDER BY file.id;",
        "MATCH (version:FileVersion)-[:REQUESTS_INSIGHT]->(job:InsightJob) RETURN 'requests_insight|' + version.id + '|' + job.id ORDER BY version.id;",
    ];
    let connection = Connection::new(&manager.owner.database).unwrap();
    let mut rows = Vec::new();
    for query in queries {
        let mut statement = connection.prepare(query).unwrap();
        let result = connection.execute(&mut statement, vec![]).unwrap();
        for row in result {
            match row.as_slice() {
                [Value::String(value)] => rows.push(value.clone()),
                values => panic!("expected one string, received {values:?}"),
            }
        }
    }
    rows
}
