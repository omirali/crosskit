// Reconciliation runs against the real database: scope, policy, idempotency, crash,
// overlap and forgery safety of retiring unseen locations.
use lbug::{Connection, Value};

use super::LadybugDatabaseManager;
use super::test_support::{
    TestDatabase, assert_current_version, count, evidence_commit, metadata_commit, path,
    query_int64, run_status, status_of,
};
use super::{migration, reconcile_store};
use crate::application::{
    KnowledgeGraphRepository, ReconcileRepository, RepositoryError, RetirementReport,
};
use crate::domain::{NormalizedPath, ReconcileRun};

const ROOT: &str = "/data/docs";

fn all_visible(_path: &NormalizedPath) -> bool {
    true
}

/// Seeds one file with evidence and four metadata-only files, then re-observes only the
/// file with evidence in a new run.
fn seed(repository: &super::repository::LadybugGraphRepository) {
    repository
        .commit_source(evidence_commit("file:kept", "/data/docs/kept.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:gone", "/data/docs/nested/gone.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:sibling", "/data/docs-old/gone.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:cousin", "/data/documents/gone.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:other", "/other/gone.md"))
        .unwrap();
}

#[test]
fn a_complete_run_retires_only_unseen_locations_inside_the_root() {
    let database = TestDatabase::new("reconcile-scope");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    seed(&repository);

    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_observed_source(&run, evidence_commit("file:kept", "/data/docs/kept.md"))
        .unwrap();
    let retired = repository
        .finalize_reconcile_run(&run, &all_visible)
        .unwrap();

    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(status_of(&manager, "/data/docs/kept.md"), "present");
    assert_eq!(status_of(&manager, "/data/docs/nested/gone.md"), "missing");
    assert_eq!(status_of(&manager, "/data/docs-old/gone.md"), "present");
    assert_eq!(status_of(&manager, "/data/documents/gone.md"), "present");
    assert_eq!(status_of(&manager, "/other/gone.md"), "present");
    assert_eq!(run_status(&manager, &run), "completed");
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:ContentChunk) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        1
    );
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 5);
    assert_current_version(&manager, "file:kept@hash:a");
}

#[test]
fn a_repeated_complete_run_is_idempotent() {
    let database = TestDatabase::new("reconcile-idempotent");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    seed(&repository);
    for _ in 0..2 {
        let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
        repository
            .commit_observed_source(&run, evidence_commit("file:kept", "/data/docs/kept.md"))
            .unwrap();
        repository
            .finalize_reconcile_run(&run, &all_visible)
            .unwrap();
    }

    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_observed_source(&run, evidence_commit("file:kept", "/data/docs/kept.md"))
        .unwrap();
    let retired = repository
        .finalize_reconcile_run(&run, &all_visible)
        .unwrap();

    assert_eq!(retired.marked_missing(), 0);
    assert_eq!(status_of(&manager, "/data/docs/nested/gone.md"), "missing");
    assert_eq!(status_of(&manager, "/data/docs/kept.md"), "present");
    assert_eq!(
        count(
            &manager,
            "MATCH (:ReconcileRun {status: 'completed'}) RETURN COUNT(*);"
        ),
        3
    );
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 5);
}

#[test]
fn the_retirement_predicate_protects_paths_the_policy_now_hides() {
    let database = TestDatabase::new("reconcile-predicate");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    seed(&repository);

    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    let keep_nested = |candidate: &NormalizedPath| !candidate.as_str().contains("/nested/");
    let retired = repository
        .finalize_reconcile_run(&run, &keep_nested)
        .unwrap();

    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(status_of(&manager, "/data/docs/nested/gone.md"), "present");
    assert_eq!(status_of(&manager, "/data/docs/kept.md"), "missing");
}

#[test]
fn an_abandoned_run_changes_no_status_and_a_reopened_database_shows_no_false_missing() {
    let database = TestDatabase::new("reconcile-abandon");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    seed(&repository);

    let abandoned = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_observed_source(
            &abandoned,
            evidence_commit("file:kept", "/data/docs/kept.md"),
        )
        .unwrap();
    repository.abandon_reconcile_run(&abandoned).unwrap();
    assert_eq!(run_status(&manager, &abandoned), "incomplete");

    // A run that never finished at all, as after a crash, stays `running`.
    let crashed = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_observed_source(&crashed, evidence_commit("file:kept", "/data/docs/kept.md"))
        .unwrap();
    drop(repository);
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    assert_eq!(run_status(&reopened, &crashed), "running");
    assert_eq!(
        count(
            &reopened,
            "MATCH (:FileLocation {status: 'missing'}) RETURN COUNT(*);"
        ),
        0
    );

    // The next complete run is not misled by either unfinished run.
    let repository = reopened.repository();
    let next = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    assert!(next.sequence() > crashed.sequence());
    repository
        .commit_observed_source(&next, evidence_commit("file:kept", "/data/docs/kept.md"))
        .unwrap();
    repository
        .commit_observed_source(
            &next,
            metadata_commit("file:gone", "/data/docs/nested/gone.md"),
        )
        .unwrap();
    let retired = repository
        .finalize_reconcile_run(&next, &all_visible)
        .unwrap();
    assert_eq!(retired.marked_missing(), 0);
    assert_eq!(
        count(
            &reopened,
            "MATCH (:FileLocation {status: 'missing'}) RETURN COUNT(*);"
        ),
        0
    );
}

#[test]
fn overlapping_runs_never_retire_a_location_seen_after_they_started() {
    let database = TestDatabase::new("reconcile-overlap");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(metadata_commit("file:stable", "/data/docs/stable.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:removed", "/data/docs/removed.md"))
        .unwrap();

    // The older run sees `removed.md` before the newer run starts; the file then goes away.
    let first = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_observed_source(
            &first,
            metadata_commit("file:removed", "/data/docs/removed.md"),
        )
        .unwrap();
    let second = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    assert!(second.sequence() > first.sequence());
    assert_eq!(
        epoch_of(&manager, "/data/docs/removed.md"),
        first.sequence()
    );

    // Both runs see the stable file after the newer run started; only the newer run reaches
    // `fresh.md`, created after the older run passed.
    repository
        .commit_observed_source(
            &first,
            metadata_commit("file:stable", "/data/docs/stable.md"),
        )
        .unwrap();
    repository
        .commit_observed_source(
            &second,
            metadata_commit("file:stable", "/data/docs/stable.md"),
        )
        .unwrap();
    repository
        .commit_observed_source(
            &second,
            metadata_commit("file:fresh", "/data/docs/fresh.md"),
        )
        .unwrap();
    assert_eq!(
        epoch_of(&manager, "/data/docs/stable.md"),
        second.sequence()
    );

    let retired_by_first = repository
        .finalize_reconcile_run(&first, &all_visible)
        .unwrap();
    assert_eq!(
        retired_by_first.marked_missing(),
        0,
        "the older run must not retire what was seen after it started"
    );
    assert_eq!(status_of(&manager, "/data/docs/fresh.md"), "present");

    let retired_by_second = repository
        .finalize_reconcile_run(&second, &all_visible)
        .unwrap();
    assert_eq!(retired_by_second.marked_missing(), 1);
    assert_eq!(status_of(&manager, "/data/docs/removed.md"), "missing");
    assert_eq!(status_of(&manager, "/data/docs/stable.md"), "present");
    assert_eq!(status_of(&manager, "/data/docs/fresh.md"), "present");
}

#[test]
fn a_retired_location_returns_to_present_through_an_ordinary_commit() {
    let database = TestDatabase::new("reconcile-return");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(metadata_commit("file:back", "/data/docs/back.md"))
        .unwrap();
    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .finalize_reconcile_run(&run, &all_visible)
        .unwrap();
    assert_eq!(status_of(&manager, "/data/docs/back.md"), "missing");

    repository
        .commit_source(metadata_commit("file:back", "/data/docs/back.md"))
        .unwrap();

    assert_eq!(status_of(&manager, "/data/docs/back.md"), "present");
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
}

#[test]
fn run_sequences_are_durable_across_reopen() {
    let database = TestDatabase::new("reconcile-sequence");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let first = manager
        .repository()
        .begin_reconcile_run(&path(ROOT))
        .unwrap();
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let second = reopened
        .repository()
        .begin_reconcile_run(&path(ROOT))
        .unwrap();

    assert_eq!(first.sequence(), 1);
    assert_eq!(second.sequence(), 2);
    assert_ne!(first.id(), second.id());
    assert_eq!(
        query_int64(
            &reopened,
            "MATCH (run:ReconcileRun {id: $id}) RETURN run.sequence;",
            vec![("id", Value::String(second.id().to_string()))],
        ),
        2
    );
}

fn epoch_of(manager: &LadybugDatabaseManager, location_path: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (location:FileLocation {normalized_path: $path}) RETURN location.seen_in_epoch;",
        vec![("path", Value::String(location_path.to_string()))],
    )
}

#[test]
fn a_plain_index_commit_during_a_run_protects_the_location_from_that_run() {
    let database = TestDatabase::new("reconcile-init-overlap");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(metadata_commit("file:old", "/data/docs/old.md"))
        .unwrap();
    assert_eq!(epoch_of(&manager, "/data/docs/old.md"), 0);

    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    // `index init` sees a file the run never reaches and writes it without any run.
    repository
        .commit_source(metadata_commit("file:late", "/data/docs/late.md"))
        .unwrap();
    assert_eq!(epoch_of(&manager, "/data/docs/late.md"), run.sequence());
    let retired = repository
        .finalize_reconcile_run(&run, &all_visible)
        .unwrap();

    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(status_of(&manager, "/data/docs/late.md"), "present");
    assert_eq!(status_of(&manager, "/data/docs/old.md"), "missing");
}

#[test]
fn a_forged_run_cannot_write_retire_or_close_anything() {
    let database = TestDatabase::new("reconcile-forged");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(metadata_commit("file:other", "/other/gone.md"))
        .unwrap();
    let genuine = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    let forged = ReconcileRun::new(
        genuine.id(),
        genuine.sequence() + 5,
        path("/"),
        genuine.started_at_unix_ms(),
    )
    .unwrap();
    let unknown = ReconcileRun::new("reconcile-run:999", 999, path(ROOT), 1).unwrap();

    let retire = repository.finalize_reconcile_run(&forged, &all_visible);
    assert!(
        matches!(retire, Err(RepositoryError::InvariantViolation(_))),
        "{retire:?}"
    );
    let write = repository
        .commit_observed_source(&forged, metadata_commit("file:new", "/data/docs/new.md"));
    assert!(
        matches!(write, Err(RepositoryError::InvariantViolation(_))),
        "{write:?}"
    );
    let close = repository.abandon_reconcile_run(&forged);
    assert!(
        matches!(close, Err(RepositoryError::InvariantViolation(_))),
        "{close:?}"
    );
    let missing_run = repository.finalize_reconcile_run(&unknown, &all_visible);
    assert!(
        matches!(missing_run, Err(RepositoryError::InvariantViolation(_))),
        "{missing_run:?}"
    );

    assert_eq!(status_of(&manager, "/other/gone.md"), "present");
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(run_status(&manager, &genuine), "running");
    // The genuine run still works afterwards.
    repository
        .finalize_reconcile_run(&genuine, &all_visible)
        .unwrap();
    assert_eq!(run_status(&manager, &genuine), "completed");
}

#[test]
fn a_closed_run_rejects_further_writes_finalization_and_abandonment() {
    let database = TestDatabase::new("reconcile-closed");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(metadata_commit("file:gone", "/data/docs/gone.md"))
        .unwrap();
    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .finalize_reconcile_run(&run, &all_visible)
        .unwrap();
    assert_eq!(status_of(&manager, "/data/docs/gone.md"), "missing");

    // A file reappears through an ordinary commit; the stale run must not retire it again.
    repository
        .commit_source(metadata_commit("file:gone", "/data/docs/gone.md"))
        .unwrap();
    let again = repository.finalize_reconcile_run(&run, &all_visible);
    assert!(
        matches!(again, Err(RepositoryError::InvariantViolation(_))),
        "{again:?}"
    );
    let abandon = repository.abandon_reconcile_run(&run);
    assert!(
        matches!(abandon, Err(RepositoryError::InvariantViolation(_))),
        "{abandon:?}"
    );
    let write =
        repository.commit_observed_source(&run, metadata_commit("file:x", "/data/docs/x.md"));
    assert!(
        matches!(write, Err(RepositoryError::InvariantViolation(_))),
        "{write:?}"
    );

    assert_eq!(status_of(&manager, "/data/docs/gone.md"), "present");
    assert_eq!(run_status(&manager, &run), "completed");
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);

    let abandoned = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository.abandon_reconcile_run(&abandoned).unwrap();
    let late_finalize = repository.finalize_reconcile_run(&abandoned, &all_visible);
    assert!(matches!(
        late_finalize,
        Err(RepositoryError::InvariantViolation(_))
    ));
    assert_eq!(status_of(&manager, "/data/docs/gone.md"), "present");
}

/// Finalizes through the store with a tiny page so several keyset pages are exercised
/// inside one transaction.
fn finalize_in_pages(
    manager: &LadybugDatabaseManager,
    run: &ReconcileRun,
    page_size: usize,
) -> Result<RetirementReport, RepositoryError> {
    let connection = Connection::new(&manager.owner.database).unwrap();
    migration::in_transaction(&connection, |connection| {
        reconcile_store::finalize_run(connection, run, 2_000, &all_visible, page_size)
    })
}

#[test]
fn retirement_walks_unseen_locations_in_bounded_pages() {
    let database = TestDatabase::new("reconcile-pages");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for index in 0..5 {
        repository
            .commit_source(metadata_commit(
                &format!("file:{index}"),
                &format!("/data/docs/{index}.md"),
            ))
            .unwrap();
    }
    repository
        .commit_source(metadata_commit("file:kept", "/data/docs/kept.md"))
        .unwrap();
    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_observed_source(&run, metadata_commit("file:kept", "/data/docs/kept.md"))
        .unwrap();

    let retired = finalize_in_pages(&manager, &run, 2).unwrap();

    assert_eq!(retired.marked_missing(), 5);
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileLocation {status: 'missing'}) RETURN COUNT(*);"
        ),
        5
    );
    assert_eq!(status_of(&manager, "/data/docs/kept.md"), "present");
    assert_eq!(run_status(&manager, &run), "completed");
}

#[test]
fn a_failure_after_the_pages_rolls_back_every_retirement() {
    let database = TestDatabase::new("reconcile-page-rollback");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for index in 0..4 {
        repository
            .commit_source(metadata_commit(
                &format!("file:{index}"),
                &format!("/data/docs/{index}.md"),
            ))
            .unwrap();
    }
    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();

    // Two pages of retirements succeed, then the transaction fails before it commits, as
    // it would if closing the run or a later statement failed.
    let connection = Connection::new(&manager.owner.database).unwrap();
    let error = migration::in_transaction(&connection, |connection| {
        let retired = reconcile_store::finalize_run(connection, &run, 2_000, &all_visible, 2)?;
        assert_eq!(retired.marked_missing(), 4);
        Err::<usize, _>(RepositoryError::Storage(
            "injected failure after the last page".to_string(),
        ))
    })
    .unwrap_err();

    assert!(matches!(error, RepositoryError::Storage(_)), "{error:?}");
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileLocation {status: 'missing'}) RETURN COUNT(*);"
        ),
        0
    );
    assert_eq!(run_status(&manager, &run), "running");
    // The genuine finalization still works after the rollback.
    let retired = repository
        .finalize_reconcile_run(&run, &all_visible)
        .unwrap();
    assert_eq!(retired.marked_missing(), 4);
}

#[test]
fn a_retired_location_is_relocated_when_the_same_file_is_confirmed_present_elsewhere() {
    let database = TestDatabase::new("reconcile-relocated");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit("file:moved", "/data/docs/old-name.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:deleted", "/data/docs/deleted.md"))
        .unwrap();

    // The run sees the same file under its new path and a brand-new file under another.
    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_observed_source(
            &run,
            metadata_commit("file:moved", "/data/docs/new-name.md"),
        )
        .unwrap();
    repository
        .commit_observed_source(
            &run,
            metadata_commit("file:unrelated", "/data/docs/newcomer.md"),
        )
        .unwrap();
    let retired = repository
        .finalize_reconcile_run(&run, &all_visible)
        .unwrap();

    assert_eq!(retired.marked_missing(), 2);
    assert_eq!(retired.relocated(), 1);
    assert_eq!(status_of(&manager, "/data/docs/old-name.md"), "missing");
    assert_eq!(status_of(&manager, "/data/docs/new-name.md"), "present");
    assert_eq!(status_of(&manager, "/data/docs/deleted.md"), "missing");
    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:moved'})-[:LOCATED_AT]->(:FileLocation) RETURN COUNT(*);"
        ),
        2
    );
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 3);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_current_version(&manager, "file:moved@hash:a");

    let repeated = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_observed_source(
            &repeated,
            metadata_commit("file:moved", "/data/docs/new-name.md"),
        )
        .unwrap();
    repository
        .commit_observed_source(
            &repeated,
            metadata_commit("file:unrelated", "/data/docs/newcomer.md"),
        )
        .unwrap();
    let nothing = repository
        .finalize_reconcile_run(&repeated, &all_visible)
        .unwrap();
    assert_eq!(nothing, RetirementReport::default());
}

#[test]
fn relocation_needs_a_sibling_confirmed_in_this_epoch_not_a_stale_present() {
    let database = TestDatabase::new("reconcile-relocated-stale");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    // Two links of one file inside the root, both gone now; a third present record for
    // the same file lives outside the root and was never confirmed by this run.
    repository
        .commit_source(metadata_commit("file:linked", "/data/docs/first-link.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:linked", "/data/docs/second-link.md"))
        .unwrap();
    repository
        .commit_source(metadata_commit("file:linked", "/elsewhere/stale-link.md"))
        .unwrap();

    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    let retired = repository
        .finalize_reconcile_run(&run, &all_visible)
        .unwrap();

    assert_eq!(retired.marked_missing(), 2);
    assert_eq!(retired.relocated(), 0);
    assert_eq!(status_of(&manager, "/elsewhere/stale-link.md"), "present");
}

fn hides_hidden_paths(candidate: &NormalizedPath) -> bool {
    candidate.is_within(&path(ROOT)) && !candidate.as_str().contains("/hidden/")
}

#[test]
fn a_fresh_sibling_under_an_excluded_path_is_not_a_relocation_destination() {
    let database = TestDatabase::new("reconcile-relocated-hidden");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit("file:moved", "/data/docs/visible.md"))
        .unwrap();

    // A restrictive reconciliation starts; a permissive `index init` then records the same
    // file under a path this reconciliation excludes, fresh in the run's epoch.
    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_source(metadata_commit("file:moved", "/data/docs/hidden/moved.md"))
        .unwrap();
    assert_eq!(
        epoch_of(&manager, "/data/docs/hidden/moved.md"),
        run.sequence()
    );
    let retired = repository
        .finalize_reconcile_run(&run, &hides_hidden_paths)
        .unwrap();

    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(retired.relocated(), 0);
    assert_eq!(status_of(&manager, "/data/docs/visible.md"), "missing");
    assert_eq!(status_of(&manager, "/data/docs/hidden/moved.md"), "present");
    assert_current_version(&manager, "file:moved@hash:a");
}

#[test]
fn a_fresh_sibling_outside_the_root_is_not_a_relocation_destination() {
    let database = TestDatabase::new("reconcile-relocated-outside");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(metadata_commit("file:moved", "/data/docs/inside.md"))
        .unwrap();

    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    repository
        .commit_source(metadata_commit("file:moved", "/elsewhere/outside.md"))
        .unwrap();
    assert_eq!(epoch_of(&manager, "/elsewhere/outside.md"), run.sequence());
    let retired = repository
        .finalize_reconcile_run(&run, &hides_hidden_paths)
        .unwrap();

    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(retired.relocated(), 0);
    assert_eq!(status_of(&manager, "/data/docs/inside.md"), "missing");
    assert_eq!(status_of(&manager, "/elsewhere/outside.md"), "present");
}

#[test]
fn the_sibling_scan_reaches_a_visible_destination_on_a_later_page() {
    let database = TestDatabase::new("reconcile-relocated-pages");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(metadata_commit("file:linked", "/data/docs/gone.md"))
        .unwrap();

    // Five fresh siblings sort before the visible one, so with a page of two the visible
    // destination is only reached on the third page.
    let run = repository.begin_reconcile_run(&path(ROOT)).unwrap();
    for index in 0..5 {
        repository
            .commit_observed_source(
                &run,
                metadata_commit("file:linked", &format!("/data/docs/hidden/link-{index}.md")),
            )
            .unwrap();
    }
    repository
        .commit_observed_source(
            &run,
            metadata_commit("file:linked", "/data/docs/visible-link.md"),
        )
        .unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();
    let retired = migration::in_transaction(&connection, |connection| {
        reconcile_store::finalize_run(connection, &run, 2_000, &hides_hidden_paths, 2)
    })
    .unwrap();

    assert_eq!(retired.marked_missing(), 1);
    assert_eq!(retired.relocated(), 1);
    assert_eq!(status_of(&manager, "/data/docs/gone.md"), "missing");
    assert_eq!(run_status(&manager, &run), "completed");
}
