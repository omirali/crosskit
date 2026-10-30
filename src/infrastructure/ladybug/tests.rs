// Behavioural tests of the Ladybug evidence store: they drive the real database through
// `LadybugGraphRepository` and inspect the resulting graph.
use std::path::Path;

use lbug::{Connection, Value};

use super::LadybugDatabaseManager;
use super::migration;
use super::test_support::{
    TestDatabase, V1_DATA, assert_current_version, count, create_v6_fixture, open_fixture_database,
    query_int64,
};
use crate::application::{KnowledgeGraphRepository, ReconcileRepository, RepositoryError};
use crate::domain::{
    AnalysisRequest, ChunkScript, ContentChunk, EvidenceCommit, FileKind, FileLocation,
    FileObservation, FileVersion, NormalizedPath, SourceCommit,
};

const LOCATION_PATH: &str = "/files/one.txt";

fn version(id: &str, content_hash: &str) -> FileVersion {
    version_at(id, content_hash, 1_700_000_000_000)
}

fn version_at(id: &str, content_hash: &str, created_at_unix_ms: i64) -> FileVersion {
    FileVersion::new(id, "file:1", content_hash, created_at_unix_ms).unwrap()
}

fn chunk(id: &str, version: &FileVersion) -> ContentChunk {
    chunk_at(id, version, 1_700_000_000_000)
}

fn chunk_at(id: &str, version: &FileVersion, created_at_unix_ms: i64) -> ContentChunk {
    ContentChunk::new(
        id,
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
        created_at_unix_ms,
    )
    .unwrap()
}

fn observation(id: &str, version: &FileVersion, observed_at_unix_ms: i64) -> FileObservation {
    FileObservation::new(id, version.file_id(), version.id(), observed_at_unix_ms).unwrap()
}

fn location(file_id: &str, path: &str, kind: FileKind, observed_at_unix_ms: i64) -> FileLocation {
    let path = NormalizedPath::from_absolute(path).unwrap();
    FileLocation::observed(file_id, path, kind, 5, 100, observed_at_unix_ms).unwrap()
}

fn evidence(
    version: &FileVersion,
    observation: FileObservation,
    chunks: Vec<ContentChunk>,
) -> EvidenceCommit {
    EvidenceCommit::new(version.clone(), observation, chunks).unwrap()
}

/// Evidence for `version` observed at the shared fixture path.
fn commit(
    version: &FileVersion,
    observation: FileObservation,
    chunks: Vec<ContentChunk>,
) -> SourceCommit {
    SourceCommit::with_evidence(
        location(
            version.file_id(),
            LOCATION_PATH,
            FileKind::RegularFile,
            1_000,
        ),
        evidence(version, observation, chunks),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

fn metadata_commit(file_id: &str, path: &str, observed_at_unix_ms: i64) -> SourceCommit {
    SourceCommit::metadata_only(location(
        file_id,
        path,
        FileKind::RegularFile,
        observed_at_unix_ms,
    ))
}

fn location_count_with_status(manager: &LadybugDatabaseManager, status: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (location:FileLocation {status: $status}) RETURN COUNT(*);",
        vec![("status", Value::String(status.to_string()))],
    )
}

fn sequence_of(manager: &LadybugDatabaseManager, observation_id: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (observation_order:ObservationOrder {id: $id}) RETURN observation_order.sequence;",
        vec![("id", Value::String(observation_id.to_string()))],
    )
}

fn location_seen_period(manager: &LadybugDatabaseManager, location_id: &str) -> (i64, i64) {
    let first_seen = query_int64(
        manager,
        "MATCH (location:FileLocation {id: $id}) RETURN location.first_seen_at_unix_ms;",
        vec![("id", Value::String(location_id.to_string()))],
    );
    let last_seen = query_int64(
        manager,
        "MATCH (location:FileLocation {id: $id}) RETURN location.last_seen_at_unix_ms;",
        vec![("id", Value::String(location_id.to_string()))],
    );
    (first_seen, last_seen)
}

/// A database written by the first release: hand-written v1 DDL and a v1 marker only.
fn create_v1_fixture(path: &Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).unwrap();
    let legacy_schema = [
        "CREATE NODE TABLE SchemaMigration(version INT64 PRIMARY KEY, name STRING, applied_at_unix_ms INT64);",
        "CREATE NODE TABLE File(id STRING PRIMARY KEY);",
        "CREATE NODE TABLE FileVersion(id STRING PRIMARY KEY, file_id STRING, content_hash STRING, created_at_unix_ms INT64);",
        "CREATE NODE TABLE ContentChunk(id STRING PRIMARY KEY, source_revision_id STRING, ordinal INT64, start_byte INT64, end_byte INT64, text_hash STRING, text STRING, language STRING, extraction_method STRING, extractor_id STRING, extractor_version STRING, config_hash STRING, input_content_hash STRING, created_at_unix_ms INT64);",
        "CREATE NODE TABLE Extractor(id STRING PRIMARY KEY, extractor_id STRING, extractor_version STRING);",
        "CREATE REL TABLE HAS_VERSION(FROM File TO FileVersion ONE_MANY);",
        "CREATE REL TABLE CURRENT_VERSION(FROM File TO FileVersion ONE_ONE);",
        "CREATE REL TABLE HAS_CHUNK(FROM FileVersion TO ContentChunk ONE_MANY);",
        "CREATE REL TABLE GENERATED_BY(FROM ContentChunk TO Extractor MANY_ONE);",
    ];
    for statement in legacy_schema.iter().chain(V1_DATA) {
        connection.query(statement).unwrap();
    }
    connection
        .query("CREATE (:SchemaMigration {version: 1, name: 'evidence-foundation', applied_at_unix_ms: 1});")
        .unwrap();
}

/// A database migrated by the previous release up to v4, with v1 data present before the
/// observation backfill ran.
fn create_v4_fixture(path: &Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).unwrap();
    migration::apply_through(&connection, 1).unwrap();
    for statement in V1_DATA {
        connection.query(statement).unwrap();
    }
    migration::apply_through(&connection, 4).unwrap();
}

/// A database written by the previous release at schema v5: legacy evidence plus one
/// present location for the current version.
fn create_v5_fixture(path: &Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).unwrap();
    migration::apply_through(&connection, 1).unwrap();
    for statement in V1_DATA {
        connection.query(statement).unwrap();
    }
    migration::apply_through(&connection, 5).unwrap();
    for statement in [
        "CREATE (:FileLocation {id: 'file:1@/files/one.txt', file_id: 'file:1', normalized_path: '/files/one.txt', display_name: 'one.txt', kind: 'file', size_bytes: 5, modified_at_unix_ms: 100, first_seen_at_unix_ms: 1000, last_seen_at_unix_ms: 1000, status: 'present'});",
        "MATCH (file:File {id: 'file:1'}), (location:FileLocation {id: 'file:1@/files/one.txt'}) CREATE (file)-[:LOCATED_AT]->(location);",
    ] {
        connection.query(statement).unwrap();
    }
}

fn assert_legacy_fixture_upgraded(manager: &std::sync::Arc<LadybugDatabaseManager>) {
    assert_eq!(manager.repository().schema_version().unwrap(), 18);
    assert_eq!(
        count(manager, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
    assert_eq!(count(manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(
        count(manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_eq!(count(manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 0);
    assert_current_version(manager, "file:1@hash:b");
    // Every legacy revision gained its queued insight job during the upgrade.
    assert_eq!(count(manager, "MATCH (:InsightJob) RETURN COUNT(*);"), 2);
    assert_eq!(
        count(
            manager,
            "MATCH (:FileVersion)-[:REQUESTS_INSIGHT]->(:InsightJob) RETURN COUNT(*);"
        ),
        2
    );
}

#[test]
fn opens_a_v1_fixture_and_applies_all_migrations() {
    let database = TestDatabase::new("v1-upgrade");
    create_v1_fixture(database.path());

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    assert_legacy_fixture_upgraded(&manager);

    let version = version_at("file:1@hash:a", "hash:a", 100);
    repository
        .commit_source(commit(
            &version,
            observation("observation:old-a", &version, 100),
            vec![chunk("chunk:upgrade", &version)],
        ))
        .unwrap();
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:HAS_OBSERVATION]->(:FileObservation) RETURN COUNT(*);"
        ),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileObservation)-[:OBSERVES]->(:FileVersion) RETURN COUNT(*);"
        ),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:1'})-[:LOCATED_AT]->(:FileLocation) RETURN COUNT(*);"
        ),
        1
    );
    assert_current_version(&manager, "file:1@hash:b");
}

#[test]
fn a_v4_database_upgrades_to_v5_without_touching_evidence_or_current_state() {
    let database = TestDatabase::new("v4-upgrade");
    create_v4_fixture(database.path());

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    assert_legacy_fixture_upgraded(&manager);

    repository
        .commit_source(metadata_commit("file:1", LOCATION_PATH, 1_000))
        .unwrap();
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:1'})-[:LOCATED_AT]->(:FileLocation) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_current_version(&manager, "file:1@hash:b");
}

#[test]
fn a_v5_database_upgrades_to_latest_keeping_locations_evidence_and_current_state() {
    let database = TestDatabase::new("v5-upgrade");
    create_v5_fixture(database.path());

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    assert_eq!(manager.repository().schema_version().unwrap(), 18);
    assert_eq!(
        count(&manager, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:1'})-[:LOCATED_AT]->(:FileLocation {normalized_path: '/files/one.txt', status: 'present', first_seen_at_unix_ms: 1000}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_eq!(count(&manager, "MATCH (:ReconcileRun) RETURN COUNT(*);"), 0);
    // Locations written before the epoch existed default to epoch 0, older than any run.
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileLocation {id: 'file:1@/files/one.txt', seen_in_epoch: 0}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:ReconcileEpoch {id: 'current', sequence: 0}) RETURN COUNT(*);"
        ),
        1
    );
    assert_current_version(&manager, "file:1@hash:b");

    // The upgraded location keeps its identity and first_seen on the next observation.
    manager
        .repository()
        .commit_source(metadata_commit("file:1", LOCATION_PATH, 2_000))
        .unwrap();
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(
        location_seen_period(&manager, "file:1@/files/one.txt"),
        (1_000, 2_000)
    );
}

#[test]
fn a_v6_database_upgrades_to_v7_continuing_the_epoch_from_its_runs() {
    let database = TestDatabase::new("v6-upgrade");
    create_v6_fixture(database.path());

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    assert_eq!(repository.schema_version().unwrap(), 18);
    assert_eq!(
        count(&manager, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:ReconcileEpoch {id: 'current', sequence: 3}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:1'})-[:LOCATED_AT]->(:FileLocation {id: 'file:1@/files/one.txt', status: 'present', seen_in_epoch: 0, first_seen_at_unix_ms: 1000}) RETURN COUNT(*);"
        ),
        1
    );
    // The historical model's data is kept as applied, it is simply no longer written.
    assert_eq!(
        count(
            &manager,
            "MATCH (:ReconcileRun)-[:OBSERVED]->(:FileLocation) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_current_version(&manager, "file:1@hash:b");

    let run = repository
        .begin_reconcile_run(&NormalizedPath::from_absolute("/files").unwrap())
        .unwrap();
    assert_eq!(run.sequence(), 4);
    assert_eq!(run.id(), "reconcile-run:4");
    assert_eq!(count(&manager, "MATCH (:ReconcileRun) RETURN COUNT(*);"), 2);

    // The legacy location was never seen under the epoch model, so a complete run that
    // does not find it retires it while evidence and current state stay untouched.
    let retired = repository
        .finalize_reconcile_run(&run, &|_path| true)
        .unwrap();
    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(retired.relocated(), 0);
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileLocation {status: 'missing'}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_current_version(&manager, "file:1@hash:b");
}

#[test]
fn migration_is_idempotent_after_reopen() {
    let database = TestDatabase::new("migration");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    assert_eq!(manager.repository().schema_version().unwrap(), 18);
    assert_eq!(
        count(&manager, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    assert_eq!(reopened.repository().schema_version().unwrap(), 18);
    assert_eq!(
        count(&reopened, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
}

#[test]
fn metadata_only_commit_creates_file_and_location_but_no_evidence() {
    let database = TestDatabase::new("metadata-only");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    let result = repository
        .commit_source(metadata_commit("file:9", "/files/photo.jpg", 1_000))
        .unwrap();

    assert_eq!(result.chunk_count(), 0);
    assert_eq!(
        count(&manager, "MATCH (:File {id: 'file:9'}) RETURN COUNT(*);"),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:9'})-[:LOCATED_AT]->(:FileLocation {normalized_path: '/files/photo.jpg', display_name: 'photo.jpg', kind: 'file', status: 'present'}) RETURN COUNT(*);"
        ),
        1
    );
    for table in [
        "FileVersion",
        "ContentChunk",
        "FileObservation",
        "ObservationOrder",
    ] {
        assert_eq!(
            count(&manager, &format!("MATCH (:{table}) RETURN COUNT(*);")),
            0,
            "{table}"
        );
    }
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        0
    );
}

#[test]
fn included_commit_creates_location_and_evidence_graph_together() {
    let database = TestDatabase::new("included");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let version = version("file:1@hash:a", "hash:a");

    let result = repository
        .commit_source(commit(
            &version,
            observation("observation:1", &version, 100),
            vec![chunk("chunk:1", &version)],
        ))
        .unwrap();

    assert_eq!(result.chunk_count(), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:1'})-[:LOCATED_AT]->(:FileLocation {normalized_path: '/files/one.txt', status: 'present'}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileVersion)-[:HAS_CHUNK]->(:ContentChunk) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:ContentChunk)-[:GENERATED_BY]->(:Extractor) RETURN COUNT(*);"
        ),
        1
    );
    assert_current_version(&manager, "file:1@hash:a");
}

#[test]
fn committing_the_same_source_twice_keeps_one_record_per_identity() {
    let database = TestDatabase::new("idempotency");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let version = version("file:1@hash:a", "hash:a");
    repository
        .commit_source(commit(
            &version,
            observation("observation:1", &version, 100),
            vec![chunk("chunk:1", &version)],
        ))
        .unwrap();

    let repeated_version = version_at("file:1@hash:a", "hash:a", 1_700_000_000_001);
    let repeated = SourceCommit::with_evidence(
        location("file:1", LOCATION_PATH, FileKind::RegularFile, 2_000),
        evidence(
            &repeated_version,
            observation("observation:1", &repeated_version, 100),
            vec![chunk_at("chunk:1", &repeated_version, 1_700_000_000_001)],
        ),
        AnalysisRequest::Requested,
    )
    .unwrap();
    repository.commit_source(repeated).unwrap();

    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:LOCATED_AT]->(:FileLocation) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        location_seen_period(&manager, "file:1@/files/one.txt"),
        (1_000, 2_000)
    );
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:ContentChunk) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_eq!(
        count(&manager, "MATCH (:ObservationOrder) RETURN COUNT(*);"),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:HAS_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        1
    );
    assert_current_version(&manager, "file:1@hash:a");
}

#[test]
fn a_new_file_at_the_same_path_retires_the_previous_location() {
    let database = TestDatabase::new("retire-location");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    repository
        .commit_source(metadata_commit("file:1", LOCATION_PATH, 1_000))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:2", LOCATION_PATH, 2_000))
        .unwrap();

    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 2);
    assert_eq!(location_count_with_status(&manager, "present"), 1);
    assert_eq!(location_count_with_status(&manager, "missing"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:2'})-[:LOCATED_AT]->(:FileLocation {status: 'present'}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:1'})-[:LOCATED_AT]->(:FileLocation {status: 'missing'}) RETURN COUNT(*);"
        ),
        1
    );
}

/// The case the batch must still get right now that it asks about a path only for the
/// locations arriving at one: a file that goes away, has its path taken, and then comes
/// back to it. The returning location is already stored, so it is not an insert; it is
/// `missing` in the graph and published as `present`, which is what makes it a claim.
/// Without that second rule the path would end up holding two present locations.
#[test]
fn a_location_returning_to_a_path_retires_whatever_took_it_meanwhile() {
    let database = TestDatabase::new("returning-location");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    repository
        .commit_source(metadata_commit("file:1", LOCATION_PATH, 1_000))
        .unwrap();
    // file:2 takes the path, which retires file:1's location.
    repository
        .commit_source(metadata_commit("file:2", LOCATION_PATH, 2_000))
        .unwrap();
    // file:1 comes back to the same path: its location is stored and missing.
    repository
        .commit_source(metadata_commit("file:1", LOCATION_PATH, 3_000))
        .unwrap();

    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 2);
    assert_eq!(
        location_count_with_status(&manager, "present"),
        1,
        "one path holds one file at a time, whichever of them arrived last"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:1'})-[:LOCATED_AT]->(:FileLocation {status: 'present'}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:2'})-[:LOCATED_AT]->(:FileLocation {status: 'missing'}) RETURN COUNT(*);"
        ),
        1
    );
    // The link of a location the graph already held is written once and never again.
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:LOCATED_AT]->(:FileLocation) RETURN COUNT(*);"
        ),
        2
    );
}

#[test]
fn a_location_with_a_different_kind_is_rejected_and_rolled_back() {
    let database = TestDatabase::new("location-kind-conflict");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(metadata_commit("file:1", LOCATION_PATH, 1_000))
        .unwrap();

    let error = repository
        .commit_source(SourceCommit::metadata_only(location(
            "file:1",
            LOCATION_PATH,
            FileKind::Symlink,
            2_000,
        )))
        .unwrap_err();

    assert!(matches!(error, RepositoryError::InvariantViolation(_)));
    assert_eq!(
        location_seen_period(&manager, "file:1@/files/one.txt"),
        (1_000, 1_000)
    );
}

#[test]
fn retrying_an_observation_reuses_its_durable_sequence() {
    let database = TestDatabase::new("retry-sequence");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let version = version("file:1@hash:a", "hash:a");
    for observation_id in [
        "observation:first",
        "observation:second",
        "observation:first",
    ] {
        repository
            .commit_source(commit(
                &version,
                observation(observation_id, &version, 100),
                vec![chunk("chunk:1", &version)],
            ))
            .unwrap();
    }

    assert_eq!(
        count(&manager, "MATCH (:ObservationOrder) RETURN COUNT(*);"),
        2
    );
    assert_eq!(sequence_of(&manager, "observation:first"), 0);
    assert_eq!(sequence_of(&manager, "observation:second"), 1);
}

#[test]
fn retrying_an_older_commit_does_not_move_current_version_backwards() {
    let database = TestDatabase::new("current-order");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let version_a = version_at("file:1@hash:a", "hash:a", 100);
    let version_b = version_at("file:1@hash:b", "hash:b", 200);

    for version in [&version_a, &version_b] {
        let chunk_id = format!("chunk:{}", version.id());
        repository
            .commit_source(commit(
                version,
                observation(
                    &format!("observation:{}", version.id()),
                    version,
                    version.created_at_unix_ms(),
                ),
                vec![chunk(&chunk_id, version)],
            ))
            .unwrap();
    }
    assert_current_version(&manager, "file:1@hash:b");

    let retried_a = version_at("file:1@hash:a", "hash:a", 300);
    repository
        .commit_source(commit(
            &retried_a,
            observation("observation:retry-a", &retried_a, 100),
            vec![chunk_at("chunk:file:1@hash:a", &retried_a, 300)],
        ))
        .unwrap();
    assert_current_version(&manager, "file:1@hash:b");

    repository
        .commit_source(commit(
            &version_a,
            observation("observation:new-a", &version_a, 300),
            vec![chunk("chunk:file:1@hash:a", &version_a)],
        ))
        .unwrap();
    assert_current_version(&manager, "file:1@hash:a");

    repository
        .commit_source(commit(
            &version_b,
            observation("observation:file:1@hash:b", &version_b, 200),
            vec![chunk("chunk:file:1@hash:b", &version_b)],
        ))
        .unwrap();
    assert_current_version(&manager, "file:1@hash:a");

    repository
        .commit_source(commit(
            &version_b,
            observation("observation:tie-b", &version_b, 300),
            vec![chunk("chunk:file:1@hash:b", &version_b)],
        ))
        .unwrap();
    assert_current_version(&manager, "file:1@hash:b");

    repository
        .commit_source(commit(
            &version_b,
            observation("observation:file:1@hash:b", &version_b, 200),
            vec![chunk("chunk:file:1@hash:b", &version_b)],
        ))
        .unwrap();
    assert_current_version(&manager, "file:1@hash:b");
}

#[test]
fn durable_sequence_advances_after_reopen_at_the_same_timestamp() {
    let database = TestDatabase::new("durable-observation-sequence");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let version_a = version("file:1@hash:a", "hash:a");
    for ordinal in 0..=50 {
        repository
            .commit_source(commit(
                &version_a,
                observation(&format!("observation:a:{ordinal}"), &version_a, 1_000),
                vec![chunk("chunk:file:1@hash:a", &version_a)],
            ))
            .unwrap();
    }
    drop(repository);
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let version_b = version("file:1@hash:b", "hash:b");
    reopened
        .repository()
        .commit_source(commit(
            &version_b,
            observation("observation:b:after-reopen", &version_b, 1_000),
            vec![chunk("chunk:file:1@hash:b", &version_b)],
        ))
        .unwrap();
    assert_eq!(sequence_of(&reopened, "observation:b:after-reopen"), 51);
    assert_current_version(&reopened, "file:1@hash:b");
}

#[test]
fn repository_managers_are_shared_per_database_path() {
    let database = TestDatabase::new("manager");
    let first = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let second = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

#[test]
fn evidence_and_current_version_survive_reopen() {
    let database = TestDatabase::new("reopen-evidence");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let version = version("file:1@hash:a", "hash:a");
    manager
        .repository()
        .commit_source(commit(
            &version,
            observation("observation:reopen", &version, 100),
            vec![chunk("chunk:reopen", &version)],
        ))
        .unwrap();
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    assert_eq!(
        count(&reopened, "MATCH (:FileLocation) RETURN COUNT(*);"),
        1
    );
    assert_eq!(count(&reopened, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(&reopened, "MATCH (:ContentChunk) RETURN COUNT(*);"),
        1
    );
    assert_eq!(
        count(
            &reopened,
            "MATCH (:FileVersion)-[:HAS_CHUNK]->(:ContentChunk) RETURN COUNT(*);"
        ),
        1
    );
    assert_current_version(&reopened, "file:1@hash:a");
}

#[test]
fn failed_commit_rolls_back_location_and_evidence_together() {
    let database = TestDatabase::new("rollback");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let first_version = version("file:1@hash:a", "hash:a");
    repository
        .commit_source(commit(
            &first_version,
            observation("observation:stable", &first_version, 100),
            vec![chunk("chunk:stable", &first_version)],
        ))
        .unwrap();

    // A different file at a new path whose chunk id collides with stored immutable data.
    let conflicting_version = FileVersion::new("file:2@hash:b", "file:2", "hash:b", 200).unwrap();
    let conflicting = SourceCommit::with_evidence(
        location(
            "file:2",
            "/files/conflict.txt",
            FileKind::RegularFile,
            2_000,
        ),
        evidence(
            &conflicting_version,
            observation("observation:conflict", &conflicting_version, 200),
            vec![chunk("chunk:stable", &conflicting_version)],
        ),
        AnalysisRequest::Requested,
    )
    .unwrap();
    let error = repository.commit_source(conflicting).unwrap_err();

    assert!(error.to_string().contains("invariant violation"));
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_eq!(
        count(&manager, "MATCH (:ObservationOrder) RETURN COUNT(*);"),
        1
    );
    assert_current_version(&manager, "file:1@hash:a");
}

#[test]
fn conflicting_file_version_is_rejected_and_rolled_back() {
    let database = TestDatabase::new("file-version-conflict");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let stored = version("file:1@hash:a", "hash:a");
    repository
        .commit_source(commit(
            &stored,
            observation("observation:stored", &stored, 100),
            vec![chunk("chunk:stored", &stored)],
        ))
        .unwrap();

    let rewritten = version("file:1@hash:a", "hash:rewritten");
    let error = repository
        .commit_source(commit(
            &rewritten,
            observation("observation:rewritten", &rewritten, 200),
            vec![],
        ))
        .unwrap_err();

    assert!(matches!(error, RepositoryError::InvariantViolation(_)));
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileVersion {id: 'file:1@hash:a', content_hash: 'hash:a'}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_current_version(&manager, "file:1@hash:a");
}

#[test]
fn conflicting_observation_is_rejected_and_rolled_back() {
    let database = TestDatabase::new("observation-conflict");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let version = version("file:1@hash:a", "hash:a");
    repository
        .commit_source(commit(
            &version,
            observation("observation:1", &version, 100),
            vec![chunk("chunk:1", &version)],
        ))
        .unwrap();

    let error = repository
        .commit_source(commit(
            &version,
            observation("observation:1", &version, 200),
            vec![],
        ))
        .unwrap_err();

    assert!(matches!(error, RepositoryError::InvariantViolation(_)));
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileObservation {id: 'observation:1', observed_at_unix_ms: 100}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(&manager, "MATCH (:ObservationOrder) RETURN COUNT(*);"),
        1
    );
    assert_eq!(sequence_of(&manager, "observation:1"), 0);
}
