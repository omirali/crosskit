// Retention of closed reconciliation runs against the real database: per-root keep,
// protection of running and legacy runs, preview safety, paging, rollback and everything
// the pass must leave untouched.
use lbug::{Connection, Value};

use super::LadybugDatabaseManager;
use super::repository::LadybugGraphRepository;
use super::test_support::{
    TestDatabase, assert_current_version, count, create_v6_fixture, evidence_commit,
    metadata_commit, path, run_status, status_of,
};
use super::{migration, run_retention_store};
use crate::application::{
    KnowledgeGraphRepository, ReconcileRepository, ReconcileRunMaintenance, RepositoryError,
};
use crate::domain::{ReconcileRun, RetentionMode, RunRetention, RunRetentionReport};

const DOCS: &str = "/data/docs";
const NOTES: &str = "/data/notes";

fn completed_run(repository: &LadybugGraphRepository, root: &str) -> ReconcileRun {
    let run = repository.begin_reconcile_run(&path(root)).unwrap();
    repository
        .finalize_reconcile_run(&run, &|_path| true)
        .unwrap();
    run
}

fn incomplete_run(repository: &LadybugGraphRepository, root: &str) -> ReconcileRun {
    let run = repository.begin_reconcile_run(&path(root)).unwrap();
    repository.abandon_reconcile_run(&run).unwrap();
    run
}

fn remaining_run_ids(manager: &LadybugDatabaseManager) -> Vec<String> {
    let connection = Connection::new(&manager.owner.database).unwrap();
    let mut statement = connection
        .prepare("MATCH (run:ReconcileRun) RETURN run.id ORDER BY run.sequence;")
        .unwrap();
    let result = connection.execute(&mut statement, vec![]).unwrap();
    result
        .map(|row| match row.as_slice() {
            [Value::String(id)] => id.clone(),
            values => panic!("expected a run id, received {values:?}"),
        })
        .collect()
}

fn epoch_sequence(manager: &LadybugDatabaseManager) -> i64 {
    count(
        manager,
        "MATCH (epoch:ReconcileEpoch {id: 'current'}) RETURN epoch.sequence;",
    )
}

fn observed_edges(manager: &LadybugDatabaseManager) -> i64 {
    count(
        manager,
        "MATCH (:ReconcileRun)-[:OBSERVED]->(:FileLocation) RETURN COUNT(*);",
    )
}

/// Prunes through the store with a tiny page so several keyset pages of roots and runs
/// are exercised inside one transaction.
fn prune_in_pages(
    manager: &LadybugDatabaseManager,
    retention: RunRetention,
    mode: RetentionMode,
    page_size: usize,
) -> Result<RunRetentionReport, RepositoryError> {
    let connection = Connection::new(&manager.owner.database).unwrap();
    migration::in_transaction(&connection, |connection| {
        run_retention_store::prune_closed_runs(connection, retention, mode, page_size)
    })
}

#[test]
fn retention_keeps_the_newest_runs_of_each_root_independently() {
    let database = TestDatabase::new("retention-roots");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for _ in 0..3 {
        completed_run(&repository, DOCS);
    }
    let notes = completed_run(&repository, NOTES);
    let newest_docs = completed_run(&repository, DOCS);

    let report = repository
        .apply_run_retention(RunRetention::keep_newest(2))
        .unwrap();

    assert_eq!(report.mode(), RetentionMode::Apply);
    assert_eq!(report.eligible_closed_runs(), 5);
    assert_eq!(report.retained_runs(), 3);
    assert_eq!(report.removed_runs(), 2);
    assert_eq!(report.protected_running_runs(), 0);
    assert_eq!(report.protected_legacy_runs(), 0);
    // Docs keeps its two newest; the single notes run is never pushed out by docs activity.
    assert_eq!(
        remaining_run_ids(&manager),
        ["reconcile-run:3", notes.id(), newest_docs.id()]
    );
}

#[test]
fn completed_and_incomplete_runs_are_eligible_while_running_runs_are_protected() {
    let database = TestDatabase::new("retention-statuses");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    completed_run(&repository, DOCS);
    incomplete_run(&repository, DOCS);
    completed_run(&repository, DOCS);
    let newest_closed = incomplete_run(&repository, DOCS);
    let running = repository.begin_reconcile_run(&path(DOCS)).unwrap();

    let report = repository
        .apply_run_retention(RunRetention::keep_newest(1))
        .unwrap();

    assert_eq!(report.eligible_closed_runs(), 4);
    assert_eq!(report.retained_runs(), 1);
    assert_eq!(report.removed_runs(), 3);
    assert_eq!(report.protected_running_runs(), 1);
    assert_eq!(
        remaining_run_ids(&manager),
        [newest_closed.id(), running.id()]
    );
    assert_eq!(run_status(&manager, &running), "running");
    // The protected run is still a working run.
    repository
        .finalize_reconcile_run(&running, &|_path| true)
        .unwrap();
    assert_eq!(run_status(&manager, &running), "completed");
}

#[test]
fn legacy_observed_runs_stay_together_with_their_evidence() {
    let database = TestDatabase::new("retention-legacy");
    create_v6_fixture(database.path());
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    completed_run(&repository, "/files");
    completed_run(&repository, "/files");
    assert_eq!(observed_edges(&manager), 1);

    let preview = repository
        .preview_run_retention(RunRetention::keep_newest(0))
        .unwrap();
    let applied = repository
        .apply_run_retention(RunRetention::keep_newest(0))
        .unwrap();

    for report in [preview, applied] {
        assert_eq!(report.protected_legacy_runs(), 1);
        assert_eq!(report.eligible_closed_runs(), 2);
        assert_eq!(report.removed_runs(), 2);
        assert_eq!(report.retained_runs(), 0);
    }
    assert_eq!(remaining_run_ids(&manager), ["reconcile-run:3"]);
    assert_eq!(run_status(&manager, &legacy_run()), "completed");
    assert_eq!(observed_edges(&manager), 1);
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(&manager, "MATCH (:SchemaMigration) RETURN COUNT(*);"),
        18
    );
}

/// The run `create_v6_fixture` wrote, as a domain value for status lookups.
fn legacy_run() -> ReconcileRun {
    ReconcileRun::new("reconcile-run:3", 3, path("/files"), 1).unwrap()
}

#[test]
fn keep_zero_removes_every_eligible_run_and_nothing_protected() {
    let database = TestDatabase::new("retention-keep-zero");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    completed_run(&repository, DOCS);
    completed_run(&repository, DOCS);
    incomplete_run(&repository, NOTES);
    let running = repository.begin_reconcile_run(&path(NOTES)).unwrap();

    let report = repository
        .apply_run_retention(RunRetention::keep_newest(0))
        .unwrap();

    assert_eq!(report.eligible_closed_runs(), 3);
    assert_eq!(report.retained_runs(), 0);
    assert_eq!(report.removed_runs(), 3);
    assert_eq!(report.protected_running_runs(), 1);
    assert_eq!(remaining_run_ids(&manager), [running.id()]);
}

#[test]
fn a_preview_counts_what_an_apply_would_remove_and_writes_nothing() {
    let database = TestDatabase::new("retention-preview");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for _ in 0..3 {
        completed_run(&repository, DOCS);
    }
    let before = remaining_run_ids(&manager);

    let first = repository
        .preview_run_retention(RunRetention::keep_newest(1))
        .unwrap();
    let second = repository
        .preview_run_retention(RunRetention::keep_newest(1))
        .unwrap();

    assert_eq!(first.mode(), RetentionMode::Preview);
    assert_eq!(first.eligible_closed_runs(), 3);
    assert_eq!(first.retained_runs(), 1);
    assert_eq!(first.removed_runs(), 2);
    assert_eq!(second, first);
    assert_eq!(remaining_run_ids(&manager), before);
    assert_eq!(epoch_sequence(&manager), 3);
}

#[test]
fn apply_removes_exactly_the_expected_runs_and_a_second_apply_removes_none() {
    let database = TestDatabase::new("retention-idempotent");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for _ in 0..4 {
        completed_run(&repository, DOCS);
    }

    let first = repository
        .apply_run_retention(RunRetention::keep_newest(2))
        .unwrap();
    assert_eq!(first.removed_runs(), 2);
    assert_eq!(first.retained_runs(), 2);
    assert_eq!(
        remaining_run_ids(&manager),
        ["reconcile-run:3", "reconcile-run:4"]
    );

    let second = repository
        .apply_run_retention(RunRetention::keep_newest(2))
        .unwrap();
    assert_eq!(second.removed_runs(), 0);
    assert_eq!(second.retained_runs(), 2);
    assert_eq!(second.eligible_closed_runs(), 2);
    assert_eq!(
        remaining_run_ids(&manager),
        ["reconcile-run:3", "reconcile-run:4"]
    );
}

#[test]
fn pruning_walks_roots_and_runs_in_bounded_pages() {
    let database = TestDatabase::new("retention-pages");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let roots = ["/data/a", "/data/b", "/data/c"];
    // Interleaved, so every root's runs are spread across the sequence.
    for _ in 0..5 {
        for root in roots {
            completed_run(&repository, root);
        }
    }

    let report = prune_in_pages(
        &manager,
        RunRetention::keep_newest(1),
        RetentionMode::Apply,
        2,
    )
    .unwrap();

    assert_eq!(report.eligible_closed_runs(), 15);
    assert_eq!(report.retained_runs(), 3);
    assert_eq!(report.removed_runs(), 12);
    assert_eq!(
        remaining_run_ids(&manager),
        ["reconcile-run:13", "reconcile-run:14", "reconcile-run:15"]
    );
}

#[test]
fn a_failure_after_the_pages_rolls_back_every_removal() {
    let database = TestDatabase::new("retention-rollback");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for _ in 0..5 {
        completed_run(&repository, DOCS);
    }
    let before = remaining_run_ids(&manager);

    // Three pages of removals succeed, then the transaction fails before it commits.
    let connection = Connection::new(&manager.owner.database).unwrap();
    let error = migration::in_transaction(&connection, |connection| {
        let report = run_retention_store::prune_closed_runs(
            connection,
            RunRetention::keep_newest(0),
            RetentionMode::Apply,
            2,
        )?;
        assert_eq!(report.removed_runs(), 5);
        Err::<usize, _>(RepositoryError::Storage(
            "injected failure after the last page".to_string(),
        ))
    })
    .unwrap_err();

    assert!(matches!(error, RepositoryError::Storage(_)), "{error:?}");
    assert_eq!(remaining_run_ids(&manager), before);
    // The genuine pass still works after the rollback.
    let report = repository
        .apply_run_retention(RunRetention::keep_newest(0))
        .unwrap();
    assert_eq!(report.removed_runs(), 5);
    assert!(remaining_run_ids(&manager).is_empty());
}

#[test]
fn cleanup_leaves_locations_evidence_chunks_and_current_state_untouched() {
    let database = TestDatabase::new("retention-untouched");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit("file:kept", "/data/docs/kept.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:gone", "/data/docs/gone.md"))
        .unwrap();
    for _ in 0..2 {
        let run = repository.begin_reconcile_run(&path(DOCS)).unwrap();
        repository
            .commit_observed_source(&run, evidence_commit("file:kept", "/data/docs/kept.md"))
            .unwrap();
        repository
            .finalize_reconcile_run(&run, &|_path| true)
            .unwrap();
    }
    let graph_before = graph_snapshot(&manager);
    assert_eq!(status_of(&manager, "/data/docs/gone.md"), "missing");

    let report = repository
        .apply_run_retention(RunRetention::keep_newest(0))
        .unwrap();

    assert_eq!(report.removed_runs(), 2);
    assert!(remaining_run_ids(&manager).is_empty());
    assert_eq!(graph_snapshot(&manager), graph_before);
    assert_eq!(status_of(&manager, "/data/docs/kept.md"), "present");
    assert_eq!(status_of(&manager, "/data/docs/gone.md"), "missing");
    assert_current_version(&manager, "file:kept@hash:a");
}

/// Everything outside `ReconcileRun` that a retention pass must leave as it found it.
fn graph_snapshot(manager: &LadybugDatabaseManager) -> Vec<(&'static str, i64)> {
    [
        ("files", "MATCH (:File) RETURN COUNT(*);"),
        ("locations", "MATCH (:FileLocation) RETURN COUNT(*);"),
        (
            "present locations",
            "MATCH (:FileLocation {status: 'present'}) RETURN COUNT(*);",
        ),
        (
            "kept epoch",
            "MATCH (location:FileLocation {normalized_path: '/data/docs/kept.md'}) RETURN location.seen_in_epoch;",
        ),
        (
            "gone epoch",
            "MATCH (location:FileLocation {normalized_path: '/data/docs/gone.md'}) RETURN location.seen_in_epoch;",
        ),
        (
            "located_at",
            "MATCH (:File)-[:LOCATED_AT]->(:FileLocation) RETURN COUNT(*);",
        ),
        ("versions", "MATCH (:FileVersion) RETURN COUNT(*);"),
        ("observations", "MATCH (:FileObservation) RETURN COUNT(*);"),
        ("chunks", "MATCH (:ContentChunk) RETURN COUNT(*);"),
        (
            "current versions",
            "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);",
        ),
        (
            "current observations",
            "MATCH (:File)-[:CURRENT_OBSERVATION]->(:FileObservation) RETURN COUNT(*);",
        ),
        (
            "epoch",
            "MATCH (epoch:ReconcileEpoch {id: 'current'}) RETURN epoch.sequence;",
        ),
    ]
    .into_iter()
    .map(|(name, query)| (name, count(manager, query)))
    .collect()
}

#[test]
fn cleanup_then_reopen_then_a_new_run_keeps_the_epoch_and_the_next_sequence() {
    let database = TestDatabase::new("retention-epoch");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for _ in 0..3 {
        completed_run(&repository, DOCS);
    }
    let report = repository
        .apply_run_retention(RunRetention::keep_newest(0))
        .unwrap();
    assert_eq!(report.removed_runs(), 3);
    assert!(remaining_run_ids(&manager).is_empty());
    assert_eq!(epoch_sequence(&manager), 3);
    drop(repository);
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = reopened.repository();
    let next = completed_run(&repository, DOCS);
    let after = repository.begin_reconcile_run(&path(NOTES)).unwrap();

    assert_eq!(next.sequence(), 4);
    assert_eq!(next.id(), "reconcile-run:4");
    assert_eq!(after.sequence(), 5);
    assert_eq!(epoch_sequence(&reopened), 5);
    assert_eq!(
        remaining_run_ids(&reopened),
        ["reconcile-run:4", "reconcile-run:5"]
    );
}
