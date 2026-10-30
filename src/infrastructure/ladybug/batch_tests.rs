// Bounded source batches against the real database: a batch is one transaction, so its
// sources become visible together, a conflict anywhere rolls the whole batch back, and a
// replayed batch changes nothing.
use super::LadybugDatabaseManager;
use super::test_support::{
    TestDatabase, assert_current_version, count, evidence_commit, evidence_commit_revision,
    located, metadata_commit, path, query_string, queued_jobs, status_of,
};
use crate::application::{ReconcileRepository, RepositoryError, SourceBatchWriter};
use crate::domain::{
    AnalysisRequest, EvidenceCommit, FileObservation, FileVersion, ReconcileRun, SourceCommit,
};

const ROOT: &str = "/data/docs";

fn batch(file_ids: &[&str]) -> Vec<SourceCommit> {
    file_ids
        .iter()
        .map(|file_id| evidence_commit(file_id, &format!("/data/docs/{file_id}.md")))
        .collect()
}

fn count_of(manager: &LadybugDatabaseManager, label: &str) -> i64 {
    count(manager, &format!("MATCH (:{label}) RETURN COUNT(*);"))
}

/// Everything one published source must leave behind, for every source of a batch.
fn assert_fully_published(manager: &LadybugDatabaseManager, file_ids: &[&str]) {
    for file_id in file_ids {
        let version_id = format!("{file_id}@hash:a");
        assert_eq!(
            count(
                manager,
                &format!(
                    "MATCH (file:File {{id: '{file_id}'}})-[:LOCATED_AT]->(:FileLocation {{status: 'present'}}) RETURN COUNT(*);"
                )
            ),
            1,
            "{file_id} must have its location"
        );
        assert_eq!(
            count(
                manager,
                &format!(
                    "MATCH (version:FileVersion {{id: '{version_id}'}})-[:HAS_CHUNK]->(:ContentChunk) RETURN COUNT(*);"
                )
            ),
            1,
            "{file_id} must have its chunk"
        );
        assert_eq!(
            count(
                manager,
                &format!(
                    "MATCH (:File {{id: '{file_id}'}})-[:CURRENT_VERSION]->(:FileVersion {{id: '{version_id}'}}) RETURN COUNT(*);"
                )
            ),
            1,
            "{file_id} must have current state"
        );
        assert_eq!(
            count(
                manager,
                &format!(
                    "MATCH (:FileVersion {{id: '{version_id}'}})-[:REQUESTS_INSIGHT]->(:InsightJob {{state: 'queued'}}) RETURN COUNT(*);"
                )
            ),
            1,
            "{file_id} must have its queued insight job"
        );
    }
}

#[test]
fn a_batch_publishes_every_source_whole_and_survives_reopen() {
    let database = TestDatabase::new("batch-commit");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let file_ids = ["file:a", "file:b", "file:c"];

    manager.repository().commit_batch(batch(&file_ids)).unwrap();

    assert_fully_published(&manager, &file_ids);
    assert_eq!(count_of(&manager, "FileLocation"), 3);
    assert_eq!(count_of(&manager, "FileVersion"), 3);
    assert_eq!(count_of(&manager, "InsightJob"), 3);
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    assert_fully_published(&reopened, &file_ids);
    assert_eq!(queued_jobs(&reopened.repository()), 3);
    assert_eq!(count_of(&reopened, "ContentChunk"), 3);
}

#[test]
fn a_conflict_in_one_source_rolls_back_its_whole_batch_and_spares_the_previous_one() {
    let database = TestDatabase::new("batch-rollback");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let published = ["file:first", "file:second"];
    repository.commit_batch(batch(&published)).unwrap();

    // The third source claims a revision id that already exists with another content
    // hash, which the version store must refuse.
    let doomed = vec![
        evidence_commit("file:third", "/data/docs/file:third.md"),
        claims_a_stored_revision_for_another_file(),
    ];

    let error = repository.commit_batch(doomed).unwrap_err();

    assert!(
        matches!(error, RepositoryError::InvariantViolation(_)),
        "{error:?}"
    );
    // The earlier batch is untouched and the failed batch left nothing at all, not even
    // its first, perfectly valid source.
    assert_fully_published(&manager, &published);
    assert_eq!(count_of(&manager, "FileLocation"), 2);
    assert_eq!(count_of(&manager, "FileVersion"), 2);
    assert_eq!(count_of(&manager, "InsightJob"), 2);
    assert_eq!(count_of(&manager, "ContentChunk"), 2);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:third'}) RETURN COUNT(*);"
        ),
        0,
        "the valid source of the failed batch must be rolled back with it"
    );
}

/// A source whose revision id is already stored for a different file: the version store
/// refuses it, because a revision id is immutable.
fn claims_a_stored_revision_for_another_file() -> SourceCommit {
    let version = FileVersion::new("file:first@hash:a", "file:other", "hash:a", 100).unwrap();
    let observation =
        FileObservation::new("observation:conflict", "file:other", version.id(), 100).unwrap();
    SourceCommit::with_evidence(
        located("file:other", "/data/docs/other.md"),
        EvidenceCommit::new(version, observation, vec![]).unwrap(),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

#[test]
fn replaying_a_batch_duplicates_nothing() {
    let database = TestDatabase::new("batch-retry");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let file_ids = ["file:a", "file:b"];

    for _ in 0..3 {
        repository.commit_batch(batch(&file_ids)).unwrap();
    }

    assert_fully_published(&manager, &file_ids);
    assert_eq!(count_of(&manager, "File"), 2);
    assert_eq!(count_of(&manager, "FileLocation"), 2);
    assert_eq!(count_of(&manager, "FileVersion"), 2);
    assert_eq!(count_of(&manager, "ContentChunk"), 2);
    assert_eq!(count_of(&manager, "InsightJob"), 2);
    // Each replay is a fresh sighting, so observations grow while identities do not.
    assert_eq!(count_of(&manager, "FileObservation"), 2);
    assert_eq!(queued_jobs(&repository), 2);
}

#[test]
fn an_observed_batch_keeps_epoch_and_observation_order() {
    let database = TestDatabase::new("batch-observed");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![metadata_commit("file:gone", "/data/docs/gone.md")])
        .unwrap();

    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    // Two revisions of one file inside a single batch: the later observation must win.
    repository
        .commit_observed_batch(
            &run,
            vec![
                evidence_commit_revision(
                    "file:kept",
                    "/data/docs/kept.md",
                    "hash:a",
                    "observation:first",
                    100,
                ),
                evidence_commit_revision(
                    "file:kept",
                    "/data/docs/kept.md",
                    "hash:b",
                    "observation:second",
                    200,
                ),
            ],
        )
        .unwrap();

    assert_current_version(&manager, "file:kept@hash:b");
    assert_eq!(
        count(
            &manager,
            "MATCH (location:FileLocation {normalized_path: '/data/docs/kept.md'}) WHERE location.seen_in_epoch = 1 RETURN COUNT(*);"
        ),
        1,
        "every source of the batch is stamped with the run's epoch"
    );
    assert_eq!(
        query_string(
            &manager,
            "MATCH (:File {id: 'file:kept'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
            vec![],
        ),
        "observation:second"
    );

    let retired = repository
        .finalize_reconcile_run(&run, &|_path| true)
        .unwrap();
    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(status_of(&manager, "/data/docs/gone.md"), "missing");
    assert_eq!(status_of(&manager, "/data/docs/kept.md"), "present");
}

#[test]
fn a_forged_run_cannot_publish_a_single_source_of_its_batch() {
    let database = TestDatabase::new("batch-forged-run");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let genuine = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    let forged = ReconcileRun::new(
        genuine.id(),
        genuine.sequence() + 5,
        path("/"),
        genuine.started_at_unix_ms(),
    )
    .unwrap();

    let error = repository
        .commit_observed_batch(&forged, batch(&["file:a", "file:b"]))
        .unwrap_err();

    assert!(
        matches!(error, RepositoryError::InvariantViolation(_)),
        "{error:?}"
    );
    assert_eq!(count_of(&manager, "FileLocation"), 0);
    assert_eq!(count_of(&manager, "InsightJob"), 0);
}

#[test]
fn a_plain_batch_during_a_run_still_protects_its_locations_from_that_run() {
    let database = TestDatabase::new("batch-init-overlap");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![metadata_commit("file:old", "/data/docs/old.md")])
        .unwrap();

    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    // A parallel `index init` publishes a batch the run will never reach.
    repository
        .commit_batch(vec![
            metadata_commit("file:late-a", "/data/docs/late-a.md"),
            metadata_commit("file:late-b", "/data/docs/late-b.md"),
        ])
        .unwrap();
    assert_eq!(
        count(
            &manager,
            &format!(
                "MATCH (location:FileLocation) WHERE location.seen_in_epoch = {} RETURN COUNT(*);",
                run.sequence()
            )
        ),
        2,
        "every source of the batch carries the epoch current at its transaction"
    );

    let retired = repository
        .finalize_reconcile_run(&run, &|_path| true)
        .unwrap();

    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(status_of(&manager, "/data/docs/old.md"), "missing");
    assert_eq!(status_of(&manager, "/data/docs/late-a.md"), "present");
    assert_eq!(status_of(&manager, "/data/docs/late-b.md"), "present");
}

#[test]
fn the_single_source_path_is_a_batch_of_one() {
    let database = TestDatabase::new("batch-of-one");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    let result = crate::application::KnowledgeGraphRepository::commit_source(
        &repository,
        evidence_commit("file:only", "/data/docs/only.md"),
    )
    .unwrap();

    assert_eq!(result.chunk_count(), 1);
    assert_fully_published(&manager, &["file:only"]);
    assert_eq!(count_of(&manager, "InsightJob"), 1);
    assert_eq!(
        query_string(
            &manager,
            "MATCH (job:InsightJob) RETURN job.source_revision_id;",
            vec![],
        ),
        "file:only@hash:a"
    );
}
