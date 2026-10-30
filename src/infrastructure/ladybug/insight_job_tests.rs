// The insight outbox against the real database: one queued job per revision, written in
// the same transaction as the evidence, never duplicated, never reset, rolled back with
// the commit on conflict and backfilled for revisions that predate schema v8.
use lbug::{Connection, Value};

use super::LadybugDatabaseManager;
use super::test_support::{
    TestDatabase, assert_current_version, count, create_v7_fixture, evidence_commit,
    evidence_commit_revision, metadata_commit, open_fixture_database, query_int64, query_string,
    queued_jobs,
};
use super::{insight_job_store, migration};
use crate::application::{KnowledgeGraphRepository, RepositoryError};

const KEPT: &str = "/data/docs/kept.md";
const JOB_A: &str = "insight-job:insight-outbox-v1:file:kept@hash:a";

fn job_count(manager: &LadybugDatabaseManager) -> i64 {
    count(manager, "MATCH (:InsightJob) RETURN COUNT(*);")
}

fn link_count(manager: &LadybugDatabaseManager) -> i64 {
    count(
        manager,
        "MATCH (:FileVersion)-[:REQUESTS_INSIGHT]->(:InsightJob) RETURN COUNT(*);",
    )
}

/// Jobs whose identity fields agree with the revision they are linked to.
fn consistent_link_count(manager: &LadybugDatabaseManager) -> i64 {
    count(
        manager,
        "MATCH (version:FileVersion)-[:REQUESTS_INSIGHT]->(job:InsightJob) WHERE job.source_revision_id = version.id AND job.input_content_hash = version.content_hash AND job.revision_observed_at_unix_ms = version.created_at_unix_ms RETURN COUNT(*);",
    )
}

fn job_text(manager: &LadybugDatabaseManager, job_id: &str, field: &str) -> String {
    query_string(
        manager,
        &format!("MATCH (job:InsightJob {{id: $id}}) RETURN job.{field};"),
        vec![("id", Value::String(job_id.to_string()))],
    )
}

/// The moment a job takes its turn by: the age of the revision it enriches.
fn job_revision_observed_at(manager: &LadybugDatabaseManager, job_id: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (job:InsightJob {id: $id}) RETURN job.revision_observed_at_unix_ms;",
        vec![("id", Value::String(job_id.to_string()))],
    )
}

fn run_cypher(manager: &LadybugDatabaseManager, statement: &str) {
    let connection = Connection::new(&manager.owner.database).unwrap();
    connection.query(statement).unwrap();
}

#[test]
fn an_evidence_commit_creates_one_queued_job_linked_to_its_revision() {
    let database = TestDatabase::new("outbox-commit");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    repository
        .commit_source(evidence_commit("file:kept", KEPT))
        .unwrap();

    assert_eq!(job_count(&manager), 1);
    assert_eq!(link_count(&manager), 1);
    assert_eq!(consistent_link_count(&manager), 1);
    assert_eq!(
        job_text(&manager, JOB_A, "source_revision_id"),
        "file:kept@hash:a"
    );
    assert_eq!(job_text(&manager, JOB_A, "input_content_hash"), "hash:a");
    assert_eq!(
        job_text(&manager, JOB_A, "pipeline_version"),
        "insight-outbox-v1"
    );
    assert_eq!(job_text(&manager, JOB_A, "state"), "queued");
    assert_eq!(job_revision_observed_at(&manager, JOB_A), 100);
    assert_eq!(queued_jobs(&repository), 1);
    // The job knows the revision and its hash, never where the file lives or what it says.
    for field in [
        "id",
        "source_revision_id",
        "input_content_hash",
        "pipeline_version",
    ] {
        let value = job_text(&manager, JOB_A, field);
        assert!(
            !value.contains("/data") && !value.contains("kept.md") && !value.contains("hello"),
            "{field} leaks path or text: {value}"
        );
    }
}

#[test]
fn a_metadata_only_commit_creates_no_job() {
    let database = TestDatabase::new("outbox-metadata");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    repository
        .commit_source(metadata_commit("file:plain", "/data/docs/plain.bin"))
        .unwrap();

    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(job_count(&manager), 0);
    assert_eq!(queued_jobs(&repository), 0);
}

#[test]
fn a_repeated_observation_neither_duplicates_nor_resets_the_job() {
    let database = TestDatabase::new("outbox-repeat");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit("file:kept", KEPT))
        .unwrap();
    // A future worker has taken the job; the outbox owns its state and time from here on.
    run_cypher(
        &manager,
        "MATCH (job:InsightJob) SET job.state = 'claimed', job.revision_observed_at_unix_ms = 5;",
    );

    repository
        .commit_source(evidence_commit_revision(
            "file:kept",
            KEPT,
            "hash:a",
            "observation:second-look",
            300,
        ))
        .unwrap();

    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        2
    );
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(job_count(&manager), 1);
    assert_eq!(link_count(&manager), 1);
    assert_eq!(job_text(&manager, JOB_A, "state"), "claimed");
    assert_eq!(job_revision_observed_at(&manager, JOB_A), 5);
    assert_eq!(queued_jobs(&repository), 0);
}

#[test]
fn a_new_revision_creates_a_new_job() {
    let database = TestDatabase::new("outbox-revision");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit("file:kept", KEPT))
        .unwrap();

    repository
        .commit_source(evidence_commit_revision(
            "file:kept",
            KEPT,
            "hash:b",
            "observation:rewritten",
            200,
        ))
        .unwrap();

    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(job_count(&manager), 2);
    assert_eq!(link_count(&manager), 2);
    assert_eq!(consistent_link_count(&manager), 2);
    assert_eq!(
        job_text(
            &manager,
            "insight-job:insight-outbox-v1:file:kept@hash:b",
            "input_content_hash"
        ),
        "hash:b"
    );
    assert_eq!(queued_jobs(&repository), 2);
    assert_current_version(&manager, "file:kept@hash:b");
}

#[test]
fn a_conflicting_pre_created_job_rolls_back_the_whole_commit() {
    let database = TestDatabase::new("outbox-conflict");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    run_cypher(
        &manager,
        "CREATE (:InsightJob {id: 'insight-job:insight-outbox-v1:file:kept@hash:a', source_revision_id: 'file:kept@hash:a', input_content_hash: 'hash:other', pipeline_version: 'insight-outbox-v1', state: 'queued', revision_observed_at_unix_ms: 1});",
    );

    let error = repository
        .commit_source(evidence_commit("file:kept", KEPT))
        .unwrap_err();

    assert!(
        matches!(error, RepositoryError::InvariantViolation(_)),
        "{error:?}"
    );
    for table in [
        "File",
        "FileLocation",
        "FileVersion",
        "FileObservation",
        "ObservationOrder",
        "ContentChunk",
    ] {
        assert_eq!(
            count(&manager, &format!("MATCH (:{table}) RETURN COUNT(*);")),
            0,
            "{table} must be rolled back with the commit"
        );
    }
    assert_eq!(job_count(&manager), 1);
    assert_eq!(link_count(&manager), 0);
    assert_eq!(
        job_text(&manager, JOB_A, "input_content_hash"),
        "hash:other",
        "the conflicting job itself is left as it was"
    );
}

#[test]
fn jobs_and_links_survive_reopen() {
    let database = TestDatabase::new("outbox-reopen");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    manager
        .repository()
        .commit_source(evidence_commit("file:kept", KEPT))
        .unwrap();
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    assert_eq!(job_count(&reopened), 1);
    assert_eq!(consistent_link_count(&reopened), 1);
    assert_eq!(queued_jobs(&reopened.repository()), 1);
    assert_eq!(job_text(&reopened, JOB_A, "state"), "queued");
}

#[test]
fn a_v7_database_gains_a_queued_job_for_every_existing_revision() {
    let database = TestDatabase::new("v7-upgrade");
    create_v7_fixture(database.path());

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    assert_eq!(repository.schema_version().unwrap(), 18);
    assert_eq!(
        count(&manager, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
    assert_eq!(job_count(&manager), 2);
    assert_eq!(consistent_link_count(&manager), 2);
    assert_eq!(queued_jobs(&repository), 2);
    // Evidence and current state are exactly what the v7 database held.
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 0);
    assert_current_version(&manager, "file:1@hash:b");
    drop(repository);
    drop(manager);

    // Reopening applies nothing twice and creates nothing new.
    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    assert_eq!(reopened.repository().schema_version().unwrap(), 18);
    assert_eq!(
        count(&reopened, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
    assert_eq!(job_count(&reopened), 2);
    assert_eq!(link_count(&reopened), 2);
}

/// The migration-8 backfill, exercised at the schema it belongs to. It writes the outbox
/// row as version 8 shaped it, so it cannot be run against a current database: a migration
/// writes the schema of its own moment.
#[test]
fn the_backfill_walks_revisions_in_bounded_pages_and_leaves_existing_jobs_alone() {
    let database = TestDatabase::new("outbox-backfill-pages");
    let fixture = open_fixture_database(database.path());
    let connection = Connection::new(&fixture).unwrap();
    migration::apply_through(&connection, 7).unwrap();
    for index in 0..5 {
        connection
            .query(&format!(
                "CREATE (:File {{id: 'file:{index}'}}); CREATE (:FileVersion {{id: 'file:{index}@hash:a', file_id: 'file:{index}', content_hash: 'hash:a', created_at_unix_ms: {}}});",
                100 + index
            ))
            .unwrap();
    }
    // Applying v8 backfills all five, which is the starting point rather than the subject.
    migration::apply_through(&connection, 8).unwrap();
    assert_eq!(v8_job_count(&connection), 5);
    // Three revisions lose their job, one of the remaining two is already claimed.
    connection
        .query("MATCH (job:InsightJob) WHERE job.source_revision_id <> 'file:0@hash:a' AND job.source_revision_id <> 'file:1@hash:a' DETACH DELETE job;")
        .unwrap();
    connection
        .query("MATCH (job:InsightJob {source_revision_id: 'file:0@hash:a'}) SET job.state = 'claimed';")
        .unwrap();
    assert_eq!(v8_job_count(&connection), 2);

    for _ in 0..2 {
        migration::in_transaction(&connection, |connection| {
            insight_job_store::backfill_insight_jobs(connection, 2)
        })
        .unwrap();

        assert_eq!(v8_job_count(&connection), 5);
        assert_eq!(
            v8_count(
                &connection,
                "MATCH (:FileVersion)-[:REQUESTS_INSIGHT]->(:InsightJob) RETURN COUNT(*);"
            ),
            5,
            "every job is linked to its revision exactly once"
        );
        assert_eq!(
            v8_count(
                &connection,
                "MATCH (version:FileVersion)-[:REQUESTS_INSIGHT]->(job:InsightJob) WHERE job.source_revision_id = version.id AND job.input_content_hash = version.content_hash AND job.created_at_unix_ms = version.created_at_unix_ms RETURN COUNT(*);"
            ),
            5,
            "each job agrees with its revision, down to the moment the revision was observed"
        );
        assert_eq!(
            v8_count(
                &connection,
                "MATCH (:InsightJob {state: 'claimed'}) RETURN COUNT(*);"
            ),
            1,
            "an existing job keeps the state its owner gave it"
        );
        assert_eq!(
            v8_count(
                &connection,
                "MATCH (:InsightJob {state: 'queued'}) RETURN COUNT(*);"
            ),
            4
        );
    }
}

/// A count read straight from a legacy-schema connection, which no repository may open.
fn v8_count(connection: &Connection<'_>, query: &str) -> i64 {
    let mut statement = connection.prepare(query).unwrap();
    match connection
        .execute(&mut statement, vec![])
        .unwrap()
        .next()
        .unwrap()
        .as_slice()
    {
        [Value::Int64(value)] => *value,
        values => panic!("expected a single INT64, received {values:?}"),
    }
}

fn v8_job_count(connection: &Connection<'_>) -> i64 {
    v8_count(connection, "MATCH (:InsightJob) RETURN COUNT(*);")
}
