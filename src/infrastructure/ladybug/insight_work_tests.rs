// The job lifecycle against the real database: a claim excludes every other worker, a
// claim that outlives its worker returns to the queue, a failure costs an attempt and
// writes nothing, a job that has spent its attempts is retired and never handed out again,
// and a finished job cannot gain a second result.
//
// Every assertion reads the stored row. "The query did not return it" is not evidence that
// a boundary held, so a job that must not be taken is checked by what the database says
// about it, not by what a claim happened to hand back.
use std::sync::Arc;

use lbug::{Connection, Value};

use super::test_support::V1_DATA;
use super::test_support::{
    TestDatabase, assert_current_version, count, create_v9_fixture, evidence_commit,
    evidence_commit_revision, open_fixture_database, query_int64, query_string, queued_jobs,
    unanalysed_evidence_commit_revision,
};
use super::{LadybugDatabaseManager, insight_work_store, migration, repository};
use crate::application::JobScope;
use crate::application::{
    AnalysisReviewService, InsightHandler, InsightJobQueue, InsightQueueReader,
    InsightWorkerService, KnowledgeGraphRepository, RepositoryError,
};
use crate::domain::{
    ClaimedInsightJob, InsightFailureCategory, InsightFinding, InsightJobState, InsightLease,
    InsightProvenance, InsightResult, InsightRetryPolicy, RevisionSummary,
};

const KEPT: &str = "/data/docs/kept.md";
const JOB_A: &str = "insight-job:insight-outbox-v1:file:kept@hash:a";
const REVISION_A: &str = "file:kept@hash:a";

/// A lease that is already over the moment it is taken, which is what a worker killed
/// right after claiming leaves behind.
const SPENT: InsightLease = InsightLease::of_milliseconds(0);

/// Long enough that no test can reach its end by running slowly.
const HELD: InsightLease = InsightLease::of_milliseconds(600_000);

fn opened(test_name: &str) -> (TestDatabase, Arc<LadybugDatabaseManager>) {
    let database = TestDatabase::new(test_name);
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    (database, manager)
}

/// One indexed file with one revision, so its outbox holds exactly one queued job.
fn with_one_queued_job(
    test_name: &str,
) -> (
    TestDatabase,
    Arc<LadybugDatabaseManager>,
    repository::LadybugGraphRepository,
) {
    let (database, manager) = opened(test_name);
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit("file:kept", KEPT))
        .unwrap();
    (database, manager, repository)
}

fn state_of(manager: &LadybugDatabaseManager, job_id: &str) -> String {
    job_text(manager, job_id, "state")
}

fn attempts_of(manager: &LadybugDatabaseManager, job_id: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (job:InsightJob {id: $id}) RETURN job.attempts;",
        vec![("id", Value::String(job_id.to_string()))],
    )
}

fn job_text(manager: &LadybugDatabaseManager, job_id: &str, field: &str) -> String {
    query_string(
        manager,
        &format!("MATCH (job:InsightJob {{id: $id}}) RETURN job.{field};"),
        vec![("id", Value::String(job_id.to_string()))],
    )
}

fn job_number(manager: &LadybugDatabaseManager, job_id: &str, field: &str) -> i64 {
    query_int64(
        manager,
        &format!("MATCH (job:InsightJob {{id: $id}}) RETURN job.{field};"),
        vec![("id", Value::String(job_id.to_string()))],
    )
}

fn result_count(manager: &LadybugDatabaseManager) -> i64 {
    count(manager, "MATCH (:InsightResult) RETURN COUNT(*);")
}

fn result_link_count(manager: &LadybugDatabaseManager) -> i64 {
    count(
        manager,
        "MATCH (:InsightJob)-[:HAS_INSIGHT_RESULT]->(:InsightResult) RETURN COUNT(*);",
    )
}

/// Asserts that no trace of a result exists: neither the record nor the link to it.
fn assert_no_result(manager: &LadybugDatabaseManager) {
    assert_eq!(result_count(manager), 0, "no result may have been written");
    assert_eq!(result_link_count(manager), 0, "no result may be linked");
}

fn run_cypher(manager: &LadybugDatabaseManager, statement: &str) {
    let connection = Connection::new(&manager.owner.database).unwrap();
    connection.query(statement).unwrap();
}

/// A handler that always refuses, so a test can reach the failing half of the lifecycle
/// without waiting for a real failure to happen.
struct RefusingHandler;

impl InsightHandler for RefusingHandler {
    fn handle(&self, _job: &ClaimedInsightJob) -> Result<InsightFinding, InsightFailureCategory> {
        Err(InsightFailureCategory::Handler)
    }
}

/// A handler that always succeeds with the same answer, so a test about the lifecycle does
/// not depend on what the deterministic handler happens to compute.
struct ConstantHandler;

impl InsightHandler for ConstantHandler {
    fn handle(&self, _job: &ClaimedInsightJob) -> Result<InsightFinding, InsightFailureCategory> {
        Ok(constant_finding())
    }
}

/// One finding, so a test about the lifecycle does not depend on what any handler computes.
fn constant_finding() -> InsightFinding {
    InsightFinding::counted(
        RevisionSummary::new(1, 5),
        InsightProvenance::local("test-handler"),
    )
}

#[test]
fn a_second_worker_never_receives_a_job_the_first_has_claimed() {
    let (_database, manager, first) = with_one_queued_job("claim-exclusive");
    // A second repository over the same graph is a second worker: same database, its own
    // connection and its own transaction for every step.
    let second = manager.repository();

    let taken = first
        .claim_insight_jobs(1, HELD, JobScope::Everything)
        .unwrap();
    let denied = second
        .claim_insight_jobs(1, HELD, JobScope::Everything)
        .unwrap();

    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].id(), JOB_A);
    assert!(denied.is_empty(), "{denied:?}");
    // What the second worker received is not the evidence: the stored job is claimed once,
    // by one attempt, and holds one lease.
    assert_eq!(state_of(&manager, JOB_A), "claimed");
    assert_eq!(attempts_of(&manager, JOB_A), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (job:InsightJob) WHERE job.lease_expires_at_unix_ms > 0 RETURN COUNT(job);"
        ),
        1
    );
}

#[test]
fn two_workers_split_the_queue_instead_of_repeating_it() {
    let (_database, manager) = opened("claim-split");
    let first = manager.repository();
    let second = manager.repository();
    for index in 0..4 {
        first
            .commit_source(evidence_commit_revision(
                &format!("file:{index}"),
                &format!("/data/docs/{index}.md"),
                "hash:a",
                &format!("observation:{index}"),
                100 + index,
            ))
            .unwrap();
    }

    let mine = first
        .claim_insight_jobs(2, HELD, JobScope::Everything)
        .unwrap();
    let theirs = second
        .claim_insight_jobs(2, HELD, JobScope::Everything)
        .unwrap();

    let mut ids: Vec<&str> = mine
        .iter()
        .chain(theirs.iter())
        .map(|job| job.id())
        .collect();
    assert_eq!(ids.len(), 4);
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4, "no job may be handed to two workers");
    assert_eq!(
        count(
            &manager,
            "MATCH (job:InsightJob {state: 'claimed'}) WHERE job.attempts = 1 RETURN COUNT(job);"
        ),
        4
    );
}

#[test]
fn a_claim_that_outlived_its_worker_returns_to_the_queue_with_the_attempt_counted() {
    let database = TestDatabase::new("claim-abandoned");
    {
        let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
        let repository = manager.repository();
        repository
            .commit_source(evidence_commit("file:kept", KEPT))
            .unwrap();
        // The worker claims and then stops existing: nothing after this line runs, and the
        // lease it took is already over.
        assert_eq!(
            repository
                .claim_insight_jobs(1, SPENT, JobScope::Everything)
                .unwrap()
                .len(),
            1
        );
    }

    // Everything the next run has is what reached the disk.
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    assert_eq!(
        state_of(&manager, JOB_A),
        "claimed",
        "a killed worker leaves its claim behind"
    );

    let recovery = repository
        .recover_insight_jobs(InsightRetryPolicy::of_attempts(3))
        .unwrap();

    assert_eq!(recovery.returned_to_queue(), 1);
    assert_eq!(recovery.retired_dead(), 0);
    assert_eq!(state_of(&manager, JOB_A), "queued");
    assert_eq!(attempts_of(&manager, JOB_A), 1, "the attempt is counted");
    assert_eq!(
        job_text(&manager, JOB_A, "last_error_category"),
        "lease-expired"
    );
    assert_eq!(job_number(&manager, JOB_A, "claimed_at_unix_ms"), 0);
    assert_eq!(job_number(&manager, JOB_A, "lease_expires_at_unix_ms"), 0);
    assert_no_result(&manager);
}

#[test]
fn a_lease_that_still_holds_is_not_recovered() {
    let (_database, manager, repository) = with_one_queued_job("claim-held");
    repository
        .claim_insight_jobs(1, HELD, JobScope::Everything)
        .unwrap();

    let recovery = repository
        .recover_insight_jobs(InsightRetryPolicy::of_attempts(3))
        .unwrap();

    assert_eq!(recovery.returned_to_queue(), 0);
    assert_eq!(recovery.retired_dead(), 0);
    assert_eq!(state_of(&manager, JOB_A), "claimed");
}

#[test]
fn a_handler_that_fails_costs_an_attempt_and_writes_nothing() {
    let (_database, manager, repository) = with_one_queued_job("handler-failure");
    let handler = RefusingHandler;

    let report = InsightWorkerService::with_lifecycle(
        &repository,
        &handler,
        HELD,
        InsightRetryPolicy::of_attempts(3),
    )
    .execute(10, &anywhere())
    .unwrap();

    assert_eq!(report.claimed(), 1);
    assert_eq!(report.failed(), 1);
    assert_eq!(report.completed(), 0);
    assert_eq!(state_of(&manager, JOB_A), "failed");
    assert_eq!(attempts_of(&manager, JOB_A), 1);
    assert_eq!(job_text(&manager, JOB_A, "last_error_category"), "handler");
    assert_eq!(job_number(&manager, JOB_A, "lease_expires_at_unix_ms"), 0);
    assert_no_result(&manager);
}

#[test]
fn a_job_that_keeps_failing_is_retired_and_never_handed_out_again() {
    let (_database, manager, repository) = with_one_queued_job("poison-job");
    let handler = RefusingHandler;
    let worker = InsightWorkerService::with_lifecycle(
        &repository,
        &handler,
        HELD,
        InsightRetryPolicy::of_attempts(2),
    );

    // Two passes spend the two attempts the policy allows.
    for attempt in 1..=2 {
        let report = worker.execute(10, &anywhere()).unwrap();
        assert_eq!(report.failed(), 1, "pass {attempt} must work the job");
        assert_eq!(state_of(&manager, JOB_A), "failed");
        assert_eq!(attempts_of(&manager, JOB_A), attempt);
    }

    // The third pass finds nothing left to try and retires it instead.
    let retiring = worker.execute(10, &anywhere()).unwrap();

    assert_eq!(retiring.recovery().retired_dead(), 1);
    assert_eq!(retiring.recovery().returned_to_queue(), 0);
    assert_eq!(retiring.claimed(), 0, "a retired job is not worked");
    assert_eq!(state_of(&manager, JOB_A), "dead");
    assert_eq!(attempts_of(&manager, JOB_A), 2);
    assert_eq!(job_text(&manager, JOB_A, "last_error_category"), "handler");

    // And a later pass leaves it exactly where it is: dead is not returned on its own.
    let later = worker.execute(10, &anywhere()).unwrap();

    assert_eq!(later.claimed(), 0);
    assert_eq!(later.recovery().returned_to_queue(), 0);
    assert_eq!(later.recovery().retired_dead(), 0);
    assert_eq!(state_of(&manager, JOB_A), "dead");
    assert_eq!(attempts_of(&manager, JOB_A), 2);
    assert_no_result(&manager);
}

#[test]
fn a_claim_a_dying_worker_keeps_losing_is_retired_by_the_same_limit() {
    let (_database, manager, repository) = with_one_queued_job("poison-claim");
    let policy = InsightRetryPolicy::of_attempts(2);

    // Twice the worker takes the job and never comes back.
    for attempt in 1..=2 {
        repository
            .claim_insight_jobs(1, SPENT, JobScope::Everything)
            .unwrap();
        assert_eq!(attempts_of(&manager, JOB_A), attempt);
        repository.recover_insight_jobs(policy).unwrap();
    }

    assert_eq!(state_of(&manager, JOB_A), "dead");
    assert_eq!(
        job_text(&manager, JOB_A, "last_error_category"),
        "lease-expired"
    );
    assert!(
        repository
            .claim_insight_jobs(10, HELD, JobScope::Everything)
            .unwrap()
            .is_empty(),
        "a retired job is never claimed again"
    );
    assert_no_result(&manager);
}

#[test]
fn a_finished_job_is_never_worked_again_and_keeps_its_one_result() {
    let (_database, manager, repository) = with_one_queued_job("idempotent-pass");
    let handler = ConstantHandler;
    let worker = InsightWorkerService::with_lifecycle(
        &repository,
        &handler,
        HELD,
        InsightRetryPolicy::of_attempts(3),
    );

    let first = worker.execute(10, &anywhere()).unwrap();
    let second = worker.execute(10, &anywhere()).unwrap();

    assert_eq!(first.completed(), 1);
    assert_eq!(second.claimed(), 0, "a done job is not claimed again");
    assert_eq!(state_of(&manager, JOB_A), "done");
    assert_eq!(attempts_of(&manager, JOB_A), 1);
    assert_eq!(result_count(&manager), 1);
    assert_eq!(result_link_count(&manager), 1);
    assert_eq!(
        query_int64(
            &manager,
            "MATCH (:InsightJob {id: $id})-[:HAS_INSIGHT_RESULT]->(result:InsightResult) RETURN result.chunk_count;",
            vec![("id", Value::String(JOB_A.to_string()))],
        ),
        1
    );
}

/// Why the state machine is not what keeps a result unique. The second write is attempted
/// directly, as a worker that somehow ran twice would, and the database refuses it.
#[test]
fn a_second_result_for_one_job_is_refused_by_the_schema() {
    let (_database, manager, repository) = with_one_queued_job("idempotent-schema");
    let job = repository
        .claim_insight_jobs(1, HELD, JobScope::Everything)
        .unwrap()[0]
        .clone();
    repository
        .complete_insight_job(&job, constant_finding())
        .unwrap();

    let connection = Connection::new(&manager.owner.database).unwrap();
    // The same result again: refused on its primary key.
    let repeated = migration::in_transaction(&connection, |connection| {
        insight_work_store::complete_job(
            connection,
            &InsightResult::of(&job, constant_finding(), 900),
        )
    });
    // A differently named result for the same job: refused on the link's cardinality, so
    // no second result can reach the graph by renaming it either.
    let renamed = connection.query(
        "MATCH (job:InsightJob {id: 'insight-job:insight-outbox-v1:file:kept@hash:a'}) CREATE (other:InsightResult {id: 'insight-result:other', job_id: job.id, source_revision_id: 'file:kept@hash:a', input_content_hash: 'hash:a', chunk_count: 9, text_bytes: 9, produced_at_unix_ms: 900}) CREATE (job)-[:HAS_INSIGHT_RESULT]->(other);",
    );

    assert!(repeated.is_err(), "{repeated:?}");
    assert!(renamed.is_err(), "the link allows one result per job");
    assert_eq!(
        result_count(&manager),
        1,
        "the stored result is the first one"
    );
    assert_eq!(result_link_count(&manager), 1);
    assert_eq!(
        query_int64(
            &manager,
            "MATCH (result:InsightResult) RETURN result.chunk_count;",
            vec![],
        ),
        1,
        "the first result is left exactly as it was"
    );
}

#[test]
fn a_claim_takes_one_page_oldest_revision_first() {
    let (_database, manager) = opened("claim-page");
    let repository = manager.repository();
    // Queued in the reverse of their id order, so an id-ordered claim would be visible.
    for (index, queued_at) in (0..5).zip([500, 400, 300, 200, 100]) {
        repository
            .commit_source(evidence_commit_revision(
                &format!("file:{index}"),
                &format!("/data/docs/{index}.md"),
                "hash:a",
                &format!("observation:{index}"),
                queued_at,
            ))
            .unwrap();
    }

    let page = repository
        .claim_insight_jobs(2, HELD, JobScope::Everything)
        .unwrap();

    assert_eq!(
        page.iter().map(|job| job.id()).collect::<Vec<_>>(),
        vec![
            "insight-job:insight-outbox-v1:file:4@hash:a",
            "insight-job:insight-outbox-v1:file:3@hash:a",
        ],
        "the two oldest by the moment their revision was observed, in that order"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:InsightJob {state: 'claimed'}) RETURN COUNT(*);"
        ),
        2,
        "exactly one page left the queue"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:InsightJob {state: 'queued'}) RETURN COUNT(*);"
        ),
        3
    );
}

#[test]
fn a_worker_stops_at_the_limit_it_was_given() {
    let (_database, manager) = opened("worker-limit");
    let repository = manager.repository();
    for index in 0..4 {
        repository
            .commit_source(evidence_commit_revision(
                &format!("file:{index}"),
                &format!("/data/docs/{index}.md"),
                "hash:a",
                &format!("observation:{index}"),
                100 + index,
            ))
            .unwrap();
    }
    let handler = ConstantHandler;

    let report = InsightWorkerService::with_lifecycle(
        &repository,
        &handler,
        HELD,
        InsightRetryPolicy::of_attempts(3),
    )
    .execute(3, &anywhere())
    .unwrap();

    assert_eq!(report.completed(), 3);
    assert_eq!(result_count(&manager), 3);
    assert_eq!(
        count(
            &manager,
            "MATCH (:InsightJob {state: 'queued'}) RETURN COUNT(*);"
        ),
        1,
        "the fourth job was never touched"
    );
}

#[test]
fn reindexing_a_file_while_its_job_is_claimed_disturbs_neither() {
    let (_database, manager, repository) = with_one_queued_job("reindex-during-work");
    let job = repository
        .claim_insight_jobs(1, HELD, JobScope::Everything)
        .unwrap()[0]
        .clone();
    let lease = job_number(&manager, JOB_A, "lease_expires_at_unix_ms");
    let claimed_at = job_number(&manager, JOB_A, "claimed_at_unix_ms");

    // The same revision is seen again, and then the file changes and gains a new one.
    repository
        .commit_source(evidence_commit_revision(
            "file:kept",
            KEPT,
            "hash:a",
            "observation:second-look",
            300,
        ))
        .unwrap();
    repository
        .commit_source(evidence_commit_revision(
            "file:kept",
            KEPT,
            "hash:b",
            "observation:rewritten",
            400,
        ))
        .unwrap();

    // The claim the worker holds is untouched by either commit.
    assert_eq!(state_of(&manager, JOB_A), "claimed");
    assert_eq!(attempts_of(&manager, JOB_A), 1);
    assert_eq!(
        job_number(&manager, JOB_A, "claimed_at_unix_ms"),
        claimed_at
    );
    assert_eq!(
        job_number(&manager, JOB_A, "lease_expires_at_unix_ms"),
        lease
    );
    assert_eq!(
        job_number(&manager, JOB_A, "revision_observed_at_unix_ms"),
        100
    );
    // The new revision brought its own job, queued like any other.
    let job_b = "insight-job:insight-outbox-v1:file:kept@hash:b";
    assert_eq!(state_of(&manager, job_b), "queued");
    assert_eq!(attempts_of(&manager, job_b), 0);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_current_version(&manager, "file:kept@hash:b");

    // And the worker still finishes the job it was holding all along.
    repository
        .complete_insight_job(&job, constant_finding())
        .unwrap();

    assert_eq!(state_of(&manager, JOB_A), "done");
    assert_eq!(state_of(&manager, job_b), "queued");
    assert_eq!(result_count(&manager), 1);
    assert_eq!(
        query_string(
            &manager,
            "MATCH (result:InsightResult) RETURN result.source_revision_id;",
            vec![],
        ),
        REVISION_A,
        "the result belongs to the revision the job named, not to the newest one"
    );
}

#[test]
fn completing_a_job_that_is_no_longer_claimed_is_refused() {
    let (_database, manager, repository) = with_one_queued_job("stolen-claim");
    let job = repository
        .claim_insight_jobs(1, HELD, JobScope::Everything)
        .unwrap()[0]
        .clone();
    // The lease ran out and the queue took the job back while this worker was busy.
    run_cypher(
        &manager,
        "MATCH (job:InsightJob) SET job.state = 'queued', job.lease_expires_at_unix_ms = 0;",
    );

    let error = repository
        .complete_insight_job(&job, constant_finding())
        .unwrap_err();

    assert!(
        matches!(error, RepositoryError::InvariantViolation(_)),
        "{error:?}"
    );
    assert_eq!(state_of(&manager, JOB_A), "queued");
    assert_no_result(&manager);
}

#[test]
fn a_result_carries_counts_and_never_a_path_or_a_text() {
    let (_database, manager, repository) = with_one_queued_job("result-privacy");
    let handler = ConstantHandler;
    InsightWorkerService::with_lifecycle(
        &repository,
        &handler,
        HELD,
        InsightRetryPolicy::of_attempts(3),
    )
    .execute(1, &anywhere())
    .unwrap();

    for field in ["id", "job_id", "source_revision_id", "input_content_hash"] {
        let value = query_string(
            &manager,
            &format!("MATCH (result:InsightResult) RETURN result.{field};"),
            vec![],
        );
        assert!(
            !value.contains("/data") && !value.contains("kept.md") && !value.contains("hello"),
            "{field} leaks a path or the file's text: {value}"
        );
    }
    assert_eq!(
        query_int64(
            &manager,
            "MATCH (result:InsightResult) RETURN result.text_bytes;",
            vec![],
        ),
        5
    );
}

#[test]
fn the_summary_counts_every_state_the_queue_is_in() {
    let (_database, manager) = opened("state-summary");
    let repository = manager.repository();
    for index in 0..4 {
        repository
            .commit_source(evidence_commit_revision(
                &format!("file:{index}"),
                &format!("/data/docs/{index}.md"),
                "hash:a",
                &format!("observation:{index}"),
                100 + index,
            ))
            .unwrap();
    }
    run_cypher(
        &manager,
        "MATCH (job:InsightJob {source_revision_id: 'file:1@hash:a'}) SET job.state = 'claimed';",
    );
    run_cypher(
        &manager,
        "MATCH (job:InsightJob {source_revision_id: 'file:2@hash:a'}) SET job.state = 'failed';",
    );
    run_cypher(
        &manager,
        "MATCH (job:InsightJob {source_revision_id: 'file:3@hash:a'}) SET job.state = 'dead';",
    );

    let summary = repository.insight_jobs_by_state().unwrap();

    assert_eq!(
        summary.counts().collect::<Vec<_>>(),
        vec![
            (InsightJobState::Queued, 1),
            (InsightJobState::Claimed, 1),
            (InsightJobState::Done, 0),
            (InsightJobState::Failed, 1),
            (InsightJobState::Dead, 1),
            (InsightJobState::Skipped, 0),
        ]
    );
}

#[test]
fn a_state_this_program_never_wrote_is_refused_rather_than_counted() {
    let (_database, manager, repository) = with_one_queued_job("unknown-state");
    run_cypher(
        &manager,
        "MATCH (job:InsightJob) SET job.state = 'running';",
    );

    let error = repository.insight_jobs_by_state().unwrap_err();

    assert!(
        matches!(error, RepositoryError::InvariantViolation(_)),
        "{error:?}"
    );
}

#[test]
fn a_v9_database_gains_the_lifecycle_without_its_jobs_losing_anything() {
    let database = TestDatabase::new("v9-upgrade");
    create_v9_fixture(database.path());

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    assert_eq!(repository.schema_version().unwrap(), 18);
    assert_eq!(
        count(&manager, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
    // The two jobs the v8 backfill wrote are still queued, still take their turn by the
    // age of the revision they enrich, and now start with no attempt, no claim and no
    // failure behind them.
    assert_eq!(count(&manager, "MATCH (:InsightJob) RETURN COUNT(*);"), 2);
    for (job_id, observed_at) in [
        ("insight-job:insight-outbox-v1:file:1@hash:a", 100),
        ("insight-job:insight-outbox-v1:file:1@hash:b", 200),
    ] {
        assert_eq!(state_of(&manager, job_id), "queued");
        assert_eq!(
            job_number(&manager, job_id, "revision_observed_at_unix_ms"),
            observed_at
        );
        assert_eq!(attempts_of(&manager, job_id), 0);
        assert_eq!(job_number(&manager, job_id, "claimed_at_unix_ms"), 0);
        assert_eq!(job_number(&manager, job_id, "lease_expires_at_unix_ms"), 0);
        assert_eq!(job_text(&manager, job_id, "last_error_category"), "");
    }
    assert_no_result(&manager);
    drop(repository);
    drop(manager);

    // Reopening applies nothing twice and changes nothing.
    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    assert_eq!(reopened.repository().schema_version().unwrap(), 18);
    assert_eq!(
        count(&reopened, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
    assert_eq!(count(&reopened, "MATCH (:InsightJob) RETURN COUNT(*);"), 2);
}

#[test]
fn a_job_written_today_starts_with_the_same_blank_lifecycle() {
    let (_database, manager, _repository) = with_one_queued_job("fresh-job-lifecycle");

    assert_eq!(attempts_of(&manager, JOB_A), 0);
    assert_eq!(job_number(&manager, JOB_A, "claimed_at_unix_ms"), 0);
    assert_eq!(job_number(&manager, JOB_A, "lease_expires_at_unix_ms"), 0);
    assert_eq!(job_text(&manager, JOB_A, "last_error_category"), "");
}

#[test]
fn work_and_its_results_survive_reopen() {
    let database = TestDatabase::new("work-reopen");
    {
        let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
        let repository = manager.repository();
        repository
            .commit_source(evidence_commit("file:kept", KEPT))
            .unwrap();
        let handler = ConstantHandler;
        InsightWorkerService::with_lifecycle(
            &repository,
            &handler,
            HELD,
            InsightRetryPolicy::of_attempts(3),
        )
        .execute(10, &anywhere())
        .unwrap();
    }

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    assert_eq!(state_of(&reopened, JOB_A), "done");
    assert_eq!(attempts_of(&reopened, JOB_A), 1);
    assert_eq!(result_count(&reopened), 1);
    assert_eq!(result_link_count(&reopened), 1);
}

/// These tests are about the lifecycle of a job, not about the policy, so every path is one
/// this context still indexes.
fn anywhere() -> impl Fn(&crate::domain::NormalizedPath) -> bool {
    |_: &crate::domain::NormalizedPath| true
}

/// The rule the queue is ordered by, on the shape that broke it: a tree of documents an
/// automatic rule already covers, and beside them revisions nobody had decided about —
/// some older than the documents, some younger. Approving a mask queues those today, so
/// their rows are the last written; their turn is not last, because their work is as old
/// as the revisions are.
///
/// The order must be the revisions' ages, 100, 300, 400, 500 — not the order the rows
/// appeared in, which is 300, 400, 100, 500.
#[test]
fn approving_a_mask_gives_each_revision_the_turn_its_age_earns() {
    let (_database, manager) = opened("claim-order-after-approval");
    let repository = manager.repository();
    // Documents, covered from the first pass and queued with it.
    for (file_id, observed_at) in [("file:doc-a", 300), ("file:doc-b", 400)] {
        repository
            .commit_source(evidence_commit_revision(
                file_id,
                &format!("/tree/{file_id}.md"),
                "hash:a",
                &format!("observation:{file_id}"),
                observed_at,
            ))
            .unwrap();
    }
    // And two nobody had decided about, one older than the documents and one younger.
    for (file_id, observed_at) in [("file:old", 100), ("file:young", 500)] {
        repository
            .commit_source(unanalysed_evidence_commit_revision(
                file_id,
                &format!("/tree/{file_id}.py"),
                "hash:a",
                &format!("observation:{file_id}"),
                observed_at,
            ))
            .unwrap();
    }
    assert_eq!(queued_jobs(&repository), 2, "only the documents are queued");

    // The owner approves `**/*.py`, which reaches back over revisions that already exist.
    let queued = AnalysisReviewService
        .backfill(&repository, &|path| path.as_str().ends_with(".py"))
        .unwrap();
    assert_eq!(queued, 2);

    let page = repository
        .claim_insight_jobs(4, HELD, JobScope::Everything)
        .unwrap();

    assert_eq!(
        page.iter()
            .map(|job| job.source_revision_id())
            .collect::<Vec<_>>(),
        vec![
            "file:old@hash:a",
            "file:doc-a@hash:a",
            "file:doc-b@hash:a",
            "file:young@hash:a",
        ],
        "oldest revision first, whatever order the rows were written in"
    );
}

/// The same rule where the moment came from the wrong clock. A revision is named by its
/// content, so a pass that re-reads an unchanged file rebuilds it with the clock of that
/// read; the graph keeps the first one. A job first queued by that later pass — which is
/// what a mask approved between passes causes — must take the revision's age, not the
/// pass's.
#[test]
fn a_job_queued_by_a_later_pass_still_takes_the_age_of_its_revision() {
    let (_database, manager) = opened("claim-order-later-pass");
    let repository = manager.repository();
    // Seen long ago, by an installation whose rules did not cover it.
    repository
        .commit_source(unanalysed_evidence_commit_revision(
            "file:old",
            "/tree/old.py",
            "hash:a",
            "observation:old",
            100,
        ))
        .unwrap();
    // A document seen since, queued when it was seen.
    repository
        .commit_source(evidence_commit_revision(
            "file:doc",
            "/tree/doc.md",
            "hash:a",
            "observation:doc",
            500,
        ))
        .unwrap();
    // Today's pass re-reads the unchanged file, now covered: same revision, later clock.
    repository
        .commit_source(evidence_commit_revision(
            "file:old",
            "/tree/old.py",
            "hash:a",
            "observation:old-again",
            9_000,
        ))
        .unwrap();

    assert_eq!(
        job_number(
            &manager,
            "insight-job:insight-outbox-v1:file:old@hash:a",
            "revision_observed_at_unix_ms"
        ),
        100,
        "the job takes the moment the graph holds for its revision, not the pass's clock"
    );
    let page = repository
        .claim_insight_jobs(2, HELD, JobScope::Everything)
        .unwrap();
    assert_eq!(
        page.iter()
            .map(|job| job.source_revision_id())
            .collect::<Vec<_>>(),
        vec!["file:old@hash:a", "file:doc@hash:a"],
    );
}

/// The queue the owner already has. Before migration 13 a job could be stamped with the
/// clock of the pass that queued it rather than with the age of its revision, so the order
/// on a stored queue is wrong until the migration puts it back. Upgrading is what fixes it,
/// and this is the shape it fixes: a job whose revision the graph dates at 100 carrying
/// 9 000, ahead of a job whose revision is genuinely newer.
#[test]
fn upgrading_puts_a_queue_stamped_by_its_passes_back_in_the_order_of_its_revisions() {
    let database = TestDatabase::new("v12-queue-order");
    {
        let fixture = open_fixture_database(database.path());
        let connection = Connection::new(&fixture).unwrap();
        migration::apply_through(&connection, 1).unwrap();
        for statement in V1_DATA {
            connection.query(statement).unwrap();
        }
        // Revisions a and b were observed at 100 and 200; v8 queues a job for each.
        migration::apply_through(&connection, 12).unwrap();
        connection
            .query("MATCH (job:InsightJob {source_revision_id: 'file:1@hash:a'}) SET job.created_at_unix_ms = 9000;")
            .unwrap();
    }

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    assert_eq!(
        job_number(
            &manager,
            "insight-job:insight-outbox-v1:file:1@hash:a",
            "revision_observed_at_unix_ms"
        ),
        100,
        "the migration reads the moment back from the revision itself"
    );
    let page = repository
        .claim_insight_jobs(2, HELD, JobScope::Everything)
        .unwrap();
    assert_eq!(
        page.iter()
            .map(|job| job.source_revision_id())
            .collect::<Vec<_>>(),
        vec!["file:1@hash:a", "file:1@hash:b"],
        "and the stored queue comes out in the order of its revisions"
    );
}
