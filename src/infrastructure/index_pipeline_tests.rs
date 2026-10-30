// End-to-end: the descriptor-relative walker feeding the Ladybug repository through the
// policy-aware initial scan.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use lbug::Value;

use super::filesystem::DescriptorRelativeWalker;
use super::ladybug::LadybugDatabaseManager;
use super::ladybug::test_support::{TestDatabase, count, query_int64, query_string};
use crate::application::{
    Admission, IncompleteReason, IndexError, InitialScanService, ReconcileCompletion,
    ReconcileReport, ReconcileService, ScanReport, SourceEntry, SourceVisitor, SourceWalker,
};
use crate::domain::{
    IndexPolicy, NormalizedPath, PathPattern, PolicyClass, PolicyRule, RuleOrigin,
};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

pub(super) fn test_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "crosskit-pipeline-{test_name}-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn scan(
    manager: &std::sync::Arc<LadybugDatabaseManager>,
    root: &Path,
    policy: &IndexPolicy,
) -> ScanReport {
    let repository = manager.repository();
    InitialScanService::new(
        &DescriptorRelativeWalker,
        &repository,
        policy,
        &analysing_everything(),
    )
    .execute(&resolved(root))
    .unwrap()
}

/// Resolving the root is the command's job now, so these tests do it the same way.
fn resolved(root: &Path) -> NormalizedPath {
    DescriptorRelativeWalker.resolve_root(root).unwrap()
}

fn location_count_at(manager: &LadybugDatabaseManager, normalized_path: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (:File)-[:LOCATED_AT]->(:FileLocation {normalized_path: $normalized_path, status: 'present'}) RETURN COUNT(*);",
        vec![(
            "normalized_path",
            Value::String(normalized_path.to_string()),
        )],
    )
}

#[test]
fn an_included_plain_text_file_creates_location_and_evidence_graph() {
    let database = TestDatabase::new("pipeline-included");
    let root = test_directory("included");
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    let report = scan(
        &manager,
        &root,
        &IndexPolicy::new(vec![], 100 * 1024 * 1024),
    );

    assert_eq!(report.indexed_files(), 1);
    assert_eq!(report.indexed_chunks(), 1);
    assert!(report.failures().is_empty(), "{:?}", report.failures());
    let notes_path = report.root().join("notes.md").unwrap();
    assert_eq!(location_count_at(&manager, notes_path.as_str()), 1);
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
            "MATCH (:FileVersion)-[:REQUESTS_INSIGHT]->(:InsightJob {state: 'queued'}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (file:File)-[:LOCATED_AT]->(:FileLocation), (file)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rescanning_the_same_directory_does_not_duplicate_locations() {
    let database = TestDatabase::new("pipeline-rescan");
    let root = test_directory("rescan");
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(vec![], 100 * 1024 * 1024);

    scan(&manager, &root, &policy);
    let report = scan(&manager, &root, &policy);

    assert_eq!(report.indexed_files(), 1);
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:LOCATED_AT]->(:FileLocation) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:ContentChunk) RETURN COUNT(*);"), 1);
    // Every scan is a new observation of the same content.
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (location:FileLocation) WHERE location.last_seen_at_unix_ms >= location.first_seen_at_unix_ms RETURN COUNT(*);"
        ),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_only_files_reach_the_graph_without_evidence() {
    let database = TestDatabase::new("pipeline-metadata");
    let root = test_directory("metadata");
    fs::write(root.join("photo.jpg"), "pretend image").unwrap();
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(
        vec![PolicyRule::new(
            PolicyClass::Metadata,
            PathPattern::parse("**/*.jpg").unwrap(),
            RuleOrigin::Configuration,
        )],
        100 * 1024 * 1024,
    );

    let report = scan(&manager, &root, &policy);

    assert_eq!(report.indexed_files(), 1);
    assert_eq!(report.metadata_only_files(), 1);
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 2);
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 2);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:LOCATED_AT]->(:FileLocation {display_name: 'photo.jpg'}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (file:File)-[:LOCATED_AT]->(:FileLocation {display_name: 'photo.jpg'}), (file)-[:HAS_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        0
    );
    let notes_version = query_int64(
        &manager,
        "MATCH (file:File)-[:LOCATED_AT]->(:FileLocation {display_name: 'notes.md'}), (file)-[:CURRENT_VERSION]->(version:FileVersion) RETURN COUNT(version);",
        vec![],
    );
    assert_eq!(notes_version, 1);
    fs::remove_dir_all(root).unwrap();
}

fn reconcile(
    manager: &std::sync::Arc<LadybugDatabaseManager>,
    root: &Path,
    policy: &IndexPolicy,
) -> ReconcileReport {
    let repository = manager.repository();
    ReconcileService::new(
        &DescriptorRelativeWalker,
        &repository,
        policy,
        &analysing_everything(),
    )
    .execute(&resolved(root))
    .unwrap()
}

fn location_status(manager: &LadybugDatabaseManager, display_name: &str) -> String {
    query_string(
        manager,
        "MATCH (location:FileLocation {display_name: $name}) RETURN location.status;",
        vec![("name", Value::String(display_name.to_string()))],
    )
}

#[test]
fn reconcile_retires_a_moved_away_file_and_keeps_its_evidence() {
    let database = TestDatabase::new("pipeline-reconcile-delete");
    let base = test_directory("reconcile-delete");
    let root = base.join("root");
    let parking = base.join("parking");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&parking).unwrap();
    fs::write(root.join("kept.md"), "Crosskit keeps this note.").unwrap();
    fs::write(root.join("gone.md"), "Crosskit indexed this note once.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &policy);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    // Moving the file out of the root keeps its inode, so it can come back as the same file.
    fs::rename(root.join("gone.md"), parking.join("gone.md")).unwrap();

    let report = reconcile(&manager, &root, &policy);

    assert_eq!(
        *report.completion(),
        ReconcileCompletion::Completed {
            marked_missing: 1,
            relocated: 0,
        }
    );
    assert_eq!(location_status(&manager, "gone.md"), "missing");
    assert_eq!(location_status(&manager, "kept.md"), "present");
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 2);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    assert_eq!(count(&manager, "MATCH (:ContentChunk) RETURN COUNT(*);"), 2);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:CURRENT_OBSERVATION]->(:FileObservation) RETURN COUNT(*);"
        ),
        2
    );

    let repeated = reconcile(&manager, &root, &policy);
    assert_eq!(
        *repeated.completion(),
        ReconcileCompletion::Completed {
            marked_missing: 0,
            relocated: 0,
        }
    );
    assert_eq!(location_status(&manager, "gone.md"), "missing");

    fs::rename(parking.join("gone.md"), root.join("gone.md")).unwrap();
    scan(&manager, &root, &policy);
    assert_eq!(location_status(&manager, "gone.md"), "present");
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 2);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 2);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn reconcile_with_a_walk_failure_is_incomplete_and_changes_no_status() {
    use std::os::unix::fs::PermissionsExt;

    let database = TestDatabase::new("pipeline-reconcile-failure");
    let root = test_directory("reconcile-failure");
    fs::write(root.join("gone.md"), "Crosskit indexed this note once.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &policy);
    fs::remove_file(root.join("gone.md")).unwrap();
    let locked = root.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let report = reconcile(&manager, &root, &policy);

    assert!(matches!(
        report.completion(),
        ReconcileCompletion::Incomplete(reasons) if reasons == &[IncompleteReason::FailedPaths(1)]
    ));
    assert_eq!(location_status(&manager, "gone.md"), "present");
    assert_eq!(
        count(
            &manager,
            "MATCH (:ReconcileRun {status: 'incomplete'}) RETURN COUNT(*);"
        ),
        1
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reconcile_does_not_touch_locations_hidden_by_a_new_exclude_rule() {
    let database = TestDatabase::new("pipeline-reconcile-exclude");
    let root = test_directory("reconcile-exclude");
    fs::write(root.join("secret.env"), "TOKEN=1").unwrap();
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let permissive = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &permissive);
    assert_eq!(location_status(&manager, "secret.env"), "present");

    let restrictive = IndexPolicy::new(
        vec![PolicyRule::new(
            PolicyClass::Exclude,
            PathPattern::parse("**/*.env").unwrap(),
            RuleOrigin::Configuration,
        )],
        100 * 1024 * 1024,
    );
    let report = reconcile(&manager, &root, &restrictive);

    assert_eq!(
        *report.completion(),
        ReconcileCompletion::Completed {
            marked_missing: 0,
            relocated: 0,
        }
    );
    assert_eq!(report.scan().excluded_paths(), 1);
    assert_eq!(location_status(&manager, "secret.env"), "present");
    assert_eq!(location_status(&manager, "notes.md"), "present");
    fs::remove_dir_all(root).unwrap();
}

/// The real walker, with an `index init` of the same root under `init_policy` interleaved
/// at the moment the reconciliation visits `trigger`. The root listing was already taken,
/// so a file created in `late_file` is indexed by `index init` but never reached by the
/// reconciliation.
struct InterleavedInitWalker<'a> {
    manager: &'a std::sync::Arc<LadybugDatabaseManager>,
    init_policy: &'a IndexPolicy,
    root: PathBuf,
    trigger: &'static str,
    late_file: Option<&'static str>,
}

impl SourceWalker for InterleavedInitWalker<'_> {
    fn resolve_root(&self, root: &Path) -> Result<NormalizedPath, IndexError> {
        DescriptorRelativeWalker.resolve_root(root)
    }

    fn walk(
        &self,
        root: &NormalizedPath,
        visitor: &mut dyn SourceVisitor,
    ) -> Result<(), IndexError> {
        let mut interleaving = InterleavingVisitor {
            inner: visitor,
            walker: self,
        };
        DescriptorRelativeWalker.walk(root, &mut interleaving)
    }
}

struct InterleavingVisitor<'a> {
    inner: &'a mut dyn SourceVisitor,
    walker: &'a InterleavedInitWalker<'a>,
}

impl SourceVisitor for InterleavingVisitor<'_> {
    fn admit(&mut self, path: &NormalizedPath) -> Admission {
        self.inner.admit(path)
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        if entry.path().file_name() == self.walker.trigger {
            if let Some(late_file) = self.walker.late_file {
                fs::write(
                    self.walker.root.join(late_file),
                    "created while the reconciliation was running",
                )
                .unwrap();
            }
            let repository = self.walker.manager.repository();
            InitialScanService::new(
                &DescriptorRelativeWalker,
                &repository,
                self.walker.init_policy,
                &analysing_everything(),
            )
            .execute(&resolved(&self.walker.root))
            .unwrap();
        }
        self.inner.visit_entry(entry)
    }

    fn skip_unrepresentable_name(&mut self) {
        self.inner.skip_unrepresentable_name();
    }

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        self.inner.record_failure(path, error);
    }
}

#[test]
fn an_index_init_running_during_a_reconciliation_never_gets_its_files_retired() {
    let database = TestDatabase::new("pipeline-reconcile-interleaved-init");
    let root = test_directory("reconcile-interleaved-init");
    fs::write(root.join("kept.md"), "Crosskit keeps this note.").unwrap();
    fs::write(root.join("gone.md"), "Crosskit indexed this note once.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &policy);
    fs::remove_file(root.join("gone.md")).unwrap();
    let walker = InterleavedInitWalker {
        manager: &manager,
        init_policy: &policy,
        root: root.clone(),
        trigger: "kept.md",
        late_file: Some("zzz-late.md"),
    };

    let repository = manager.repository();
    let report = ReconcileService::new(&walker, &repository, &policy, &analysing_everything())
        .execute(&resolved(&root))
        .unwrap();

    assert_eq!(
        *report.completion(),
        ReconcileCompletion::Completed {
            marked_missing: 1,
            relocated: 0,
        }
    );
    // The reconciliation itself never saw the late file.
    assert_eq!(report.scan().indexed_files(), 1);
    assert_eq!(location_status(&manager, "zzz-late.md"), "present");
    assert_eq!(location_status(&manager, "kept.md"), "present");
    assert_eq!(location_status(&manager, "gone.md"), "missing");
    fs::remove_dir_all(root).unwrap();
}

fn completed(marked_missing: usize, relocated: usize) -> ReconcileCompletion {
    ReconcileCompletion::Completed {
        marked_missing,
        relocated,
    }
}

fn located_paths_of(manager: &LadybugDatabaseManager, display_name: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (file:File)-[:LOCATED_AT]->(:FileLocation {display_name: $name}), (file)-[:LOCATED_AT]->(sibling:FileLocation) RETURN COUNT(DISTINCT sibling);",
        vec![("name", Value::String(display_name.to_string()))],
    )
}

#[test]
fn a_renamed_file_is_retired_as_relocated_without_duplicating_its_evidence() {
    let database = TestDatabase::new("pipeline-relocated-rename");
    let root = test_directory("relocated-rename");
    fs::create_dir_all(root.join("archive")).unwrap();
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &policy);
    fs::rename(
        root.join("notes.md"),
        root.join("archive").join("renamed.md"),
    )
    .unwrap();

    let report = reconcile(&manager, &root, &policy);

    assert_eq!(*report.completion(), completed(1, 1));
    assert_eq!(location_status(&manager, "notes.md"), "missing");
    assert_eq!(location_status(&manager, "renamed.md"), "present");
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 1);
    assert_eq!(located_paths_of(&manager, "renamed.md"), 2);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:ContentChunk) RETURN COUNT(*);"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:HAS_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        1
    );
    // Each scan records one observation of the same version; nothing about the version,
    // its chunks or the current pointers is duplicated or changed.
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:CURRENT_OBSERVATION]->(:FileObservation) RETURN COUNT(*);"
        ),
        1
    );

    let repeated = reconcile(&manager, &root, &policy);
    assert_eq!(*repeated.completion(), completed(0, 0));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_unrelated_new_file_does_not_make_a_deletion_look_relocated() {
    let database = TestDatabase::new("pipeline-relocated-unrelated");
    let root = test_directory("relocated-unrelated");
    fs::write(root.join("old.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &policy);
    fs::remove_file(root.join("old.md")).unwrap();
    // Same content and name-like path, but a different filesystem object.
    fs::write(root.join("new.md"), "Crosskit indexes this note.").unwrap();

    let report = reconcile(&manager, &root, &policy);

    assert_eq!(*report.completion(), completed(1, 0));
    assert_eq!(location_status(&manager, "old.md"), "missing");
    assert_eq!(location_status(&manager, "new.md"), "present");
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_removed_hard_link_counts_as_relocated_without_any_rename_fact() {
    let database = TestDatabase::new("pipeline-relocated-hard-link");
    let root = test_directory("relocated-hard-link");
    fs::write(root.join("first.md"), "Crosskit indexes this note.").unwrap();
    fs::hard_link(root.join("first.md"), root.join("second.md")).unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &policy);
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 1);
    assert_eq!(located_paths_of(&manager, "first.md"), 2);
    fs::remove_file(root.join("first.md")).unwrap();

    let report = reconcile(&manager, &root, &policy);

    // The identity is still present under `second.md`; the graph says exactly that and
    // nothing about which link was the "original".
    assert_eq!(*report.completion(), completed(1, 1));
    assert_eq!(location_status(&manager, "first.md"), "missing");
    assert_eq!(location_status(&manager, "second.md"), "present");
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 1);
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);
    assert_eq!(located_paths_of(&manager, "second.md"), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_file_moved_under_an_excluded_path_is_retired_but_not_reported_relocated() {
    let database = TestDatabase::new("pipeline-relocated-exclude");
    let root = test_directory("relocated-exclude");
    fs::create_dir_all(root.join("hidden")).unwrap();
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let permissive = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &permissive);
    fs::rename(root.join("notes.md"), root.join("hidden").join("notes.md")).unwrap();
    let restrictive = IndexPolicy::new(
        vec![PolicyRule::new(
            PolicyClass::Exclude,
            PathPattern::parse("**/hidden/**").unwrap(),
            RuleOrigin::Configuration,
        )],
        100 * 1024 * 1024,
    );

    let report = reconcile(&manager, &root, &restrictive);

    // The hidden path was never inspected, so it cannot serve as evidence of relocation.
    assert_eq!(*report.completion(), completed(1, 0));
    assert_eq!(report.scan().excluded_paths(), 1);
    assert_eq!(location_status(&manager, "notes.md"), "missing");
    assert_eq!(count(&manager, "MATCH (:FileLocation) RETURN COUNT(*);"), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_incomplete_reconcile_reports_no_relocation_and_changes_no_status() {
    use std::os::unix::fs::PermissionsExt;

    let database = TestDatabase::new("pipeline-relocated-incomplete");
    let root = test_directory("relocated-incomplete");
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let policy = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &policy);
    fs::rename(root.join("notes.md"), root.join("renamed.md")).unwrap();
    let locked = root.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let report = reconcile(&manager, &root, &policy);

    assert!(matches!(
        report.completion(),
        ReconcileCompletion::Incomplete(reasons) if reasons == &[IncompleteReason::FailedPaths(1)]
    ));
    assert_eq!(location_status(&manager, "notes.md"), "present");
    assert_eq!(location_status(&manager, "renamed.md"), "present");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_permissive_init_during_a_restrictive_reconcile_cannot_prove_relocation_into_a_hidden_path() {
    let database = TestDatabase::new("pipeline-relocated-hidden-init");
    let root = test_directory("relocated-hidden-init");
    fs::create_dir_all(root.join("hidden")).unwrap();
    fs::write(root.join("kept.md"), "Crosskit keeps this note.").unwrap();
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let permissive = IndexPolicy::new(vec![], 100 * 1024 * 1024);
    scan(&manager, &root, &permissive);
    // The same inode moves under a path the reconciliation's policy hides.
    fs::rename(root.join("notes.md"), root.join("hidden").join("notes.md")).unwrap();
    let restrictive = IndexPolicy::new(
        vec![PolicyRule::new(
            PolicyClass::Exclude,
            PathPattern::parse("**/hidden/**").unwrap(),
            RuleOrigin::Configuration,
        )],
        100 * 1024 * 1024,
    );
    // While the restrictive reconciliation runs, a permissive `index init` records the
    // hidden path fresh in the run's epoch.
    let walker = InterleavedInitWalker {
        manager: &manager,
        init_policy: &permissive,
        root: root.clone(),
        trigger: "kept.md",
        late_file: None,
    };

    let repository = manager.repository();
    let report = ReconcileService::new(&walker, &repository, &restrictive, &analysing_everything())
        .execute(&resolved(&root))
        .unwrap();

    assert_eq!(*report.completion(), completed(1, 0));
    assert_eq!(report.scan().excluded_paths(), 1);
    assert_eq!(
        query_int64(
            &manager,
            "MATCH (location:FileLocation {display_name: 'notes.md', status: 'present'}) RETURN COUNT(*);",
            vec![],
        ),
        1
    );
    assert_eq!(
        query_int64(
            &manager,
            "MATCH (location:FileLocation {display_name: 'notes.md', status: 'missing'}) RETURN COUNT(*);",
            vec![],
        ),
        1
    );
    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 2);
    fs::remove_dir_all(root).unwrap();
}

/// These tests are about the walk, the policy or the pipeline, not about which revisions are
/// worth analysing, so every path is one the installation analyses.
fn analysing_everything() -> crate::domain::AnalysisRules {
    crate::domain::AnalysisRules::new(
        vec![crate::domain::PathPattern::parse("**/*").unwrap()],
        Vec::new(),
    )
}
