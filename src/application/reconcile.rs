// Use case: walk a root again under the policy and retire the locations that a complete,
// trustworthy walk did not see.
use crate::domain::{AnalysisRules, IndexPolicy, NormalizedPath, PolicyClass, ReconcileRun};

use super::file_identity::FileIdentityHistory;
use super::initial_scan::IndexError;
use super::policy_scan::{ScanReport, scan_with_policy};
use super::repository::{ReconcileRepository, RepositoryError};
use super::search_indexing::{IndexingSink, SearchIndexUpdate};
use super::source_batch::SourceBatchWriter;
use super::source_walker::SourceWalker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    run_id: String,
    scan: ScanReport,
    completion: ReconcileCompletion,
}

impl ReconcileReport {
    pub(super) fn new(run_id: &str, scan: ScanReport, completion: ReconcileCompletion) -> Self {
        Self {
            run_id: run_id.to_string(),
            scan,
            completion,
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn scan(&self) -> &ScanReport {
        &self.scan
    }

    pub fn completion(&self) -> &ReconcileCompletion {
        &self.completion
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileCompletion {
    /// The walk was complete and the unseen locations in scope were marked missing.
    /// `relocated` counts those whose file identity is still present under another path.
    Completed {
        marked_missing: usize,
        relocated: usize,
    },
    /// The walk cannot vouch for what is on disk, so no status changed.
    Incomplete(Vec<IncompleteReason>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncompleteReason {
    FailedPaths(usize),
    UnstableSources(usize),
    UnrepresentableNames(usize),
}

pub struct ReconcileService<'a, Walker, Repository> {
    walker: &'a Walker,
    repository: &'a Repository,
    policy: &'a IndexPolicy,
    analysis_rules: &'a AnalysisRules,
    search: Option<&'a SearchIndexUpdate<'a>>,
}

impl<'a, Walker, Repository> ReconcileService<'a, Walker, Repository>
where
    Walker: SourceWalker,
    Repository: ReconcileRepository + SourceBatchWriter + FileIdentityHistory,
{
    pub fn new(
        walker: &'a Walker,
        repository: &'a Repository,
        policy: &'a IndexPolicy,
        analysis_rules: &'a AnalysisRules,
    ) -> Self {
        Self {
            walker,
            repository,
            policy,
            analysis_rules,
            search: None,
        }
    }

    /// Also keeps the search index of this context up to date, batch by batch, after each
    /// batch has been committed to the graph.
    pub fn keeping_search_index(mut self, search: &'a SearchIndexUpdate<'a>) -> Self {
        self.search = Some(search);
        self
    }

    /// `root` is already resolved, so the directory whose context was checked is the
    /// directory that is walked.
    pub fn execute(&self, root: &NormalizedPath) -> Result<ReconcileReport, IndexError> {
        let policy = self.policy.with_scan_root(root);
        let run = self
            .repository
            .begin_reconcile_run(root)
            .map_err(IndexError::Repository)?;

        // Each batch is published together with the fact that this run observed it.
        let scan = match scan_with_policy(
            self.walker,
            root,
            &policy,
            self.analysis_rules,
            self.repository,
            IndexingSink::new(
                |commits| self.repository.commit_observed_batch(&run, commits),
                self.search,
            ),
        ) {
            Ok(mut scan) => {
                if let Some(search) = self.search {
                    search.finish();
                    scan.record_search_index_failures(search.failures());
                }
                scan
            }
            // The run is left on record as incomplete; if even that fails, both errors
            // are reported together.
            Err(error) => {
                return Err(match self.repository.abandon_reconcile_run(&run) {
                    Ok(()) => error,
                    Err(abandon_error) => {
                        IndexError::Repository(RepositoryError::Storage(format!(
                            "{error}; marking run {} incomplete also failed: {abandon_error}",
                            run.id()
                        )))
                    }
                });
            }
        };

        let completion = self.finish(&run, root, &policy, &scan)?;
        Ok(ReconcileReport::new(run.id(), scan, completion))
    }

    fn finish(
        &self,
        run: &ReconcileRun,
        root: &NormalizedPath,
        policy: &IndexPolicy,
        scan: &ScanReport,
    ) -> Result<ReconcileCompletion, IndexError> {
        let reasons = incomplete_reasons(scan);
        if !reasons.is_empty() {
            self.repository
                .abandon_reconcile_run(run)
                .map_err(IndexError::Repository)?;
            return Ok(ReconcileCompletion::Incomplete(reasons));
        }

        // Only paths inside the root that the policy lets this run see may be retired or
        // serve as relocation destinations: a path the policy excludes was never
        // inspected, so neither its absence nor its presence proves anything.
        //
        // The policy asked here is the run's own, which is the configured one plus what the
        // walk learned from the tree. A directory that carried a marker was skipped exactly
        // as an excluded one was, so it has to answer this question exactly as an excluded
        // one does; without its rule the walk's silence about a directory it deliberately
        // did not enter read as proof that its files were gone.
        let policy = policy.with_discovered(scan.generated_directory_rules());
        let is_visible = |path: &NormalizedPath| {
            path.is_within(root) && policy.decide(path).class() != PolicyClass::Exclude
        };
        let retirement = self
            .repository
            .finalize_reconcile_run(run, &is_visible)
            .map_err(IndexError::Repository)?;
        Ok(ReconcileCompletion::Completed {
            marked_missing: retirement.marked_missing(),
            relocated: retirement.relocated(),
        })
    }
}

fn incomplete_reasons(scan: &ScanReport) -> Vec<IncompleteReason> {
    let mut reasons = Vec::new();
    if scan.failed_paths() > 0 {
        reasons.push(IncompleteReason::FailedPaths(scan.failed_paths()));
    }
    if scan.unstable_sources() > 0 {
        reasons.push(IncompleteReason::UnstableSources(scan.unstable_sources()));
    }
    if scan.unrepresentable_names() > 0 {
        reasons.push(IncompleteReason::UnrepresentableNames(
            scan.unrepresentable_names(),
        ));
    }
    reasons
}

#[cfg(test)]
mod tests {
    use crate::domain::AnalysisRules;

    /// These tests are about the walk and the policy, not about which revisions are worth
    /// analysing, so every path is one the installation analyses.
    fn analysing_everything() -> AnalysisRules {
        AnalysisRules::new(vec![PathPattern::parse("**/*").unwrap()], Vec::new())
    }

    use std::cell::RefCell;
    use std::path::Path;

    use super::{IncompleteReason, ReconcileCompletion, ReconcileService};
    use crate::application::{
        Admission, CommitResult, ContentSkipReason, EvidenceRead, FileIdentityHistory, IndexError,
        ReconcileRepository, RepositoryError, RetirementReport, SourceBatchWriter, SourceEntry,
        SourceVisitor, SourceWalker,
    };
    use crate::domain::{
        EvidenceCommit, FileKind, FileLocation, FileObservation, FileVersion, IndexPolicy,
        KnownIdentity, NormalizedPath, ObservedFileIdentity, PathPattern, PolicyClass, PolicyRule,
        ReconcileRun, RuleOrigin, SourceCommit,
    };

    const ROOT: &str = "/scan";

    /// What a fake entry does when read.
    #[derive(Clone, Copy)]
    enum Behaviour {
        Text,
        Unstable,
        Failing,
    }

    struct FakeWalker {
        entries: Vec<(&'static str, Behaviour)>,
        unrepresentable_names: usize,
        directory_failure: bool,
    }

    impl SourceWalker for FakeWalker {
        fn resolve_root(&self, _root: &Path) -> Result<NormalizedPath, IndexError> {
            Ok(NormalizedPath::from_absolute(ROOT).unwrap())
        }

        fn walk(
            &self,
            root: &NormalizedPath,
            visitor: &mut dyn SourceVisitor,
        ) -> Result<(), IndexError> {
            if visitor.admit(root) == Admission::Skip {
                return Ok(());
            }
            for _ in 0..self.unrepresentable_names {
                visitor.skip_unrepresentable_name();
            }
            if self.directory_failure {
                visitor.record_failure(
                    &root.join("locked").unwrap(),
                    IndexError::Source("permission denied".to_string()),
                );
            }
            for (name, behaviour) in &self.entries {
                let path = root.join(name).unwrap();
                if visitor.admit(&path) == Admission::Skip {
                    continue;
                }
                visitor.visit_entry(&FakeEntry {
                    path,
                    behaviour: *behaviour,
                })?;
            }
            Ok(())
        }
    }

    struct FakeEntry {
        path: NormalizedPath,
        behaviour: Behaviour,
    }

    impl SourceEntry for FakeEntry {
        fn path(&self) -> &NormalizedPath {
            &self.path
        }

        fn observed_identity(&self) -> Result<ObservedFileIdentity, IndexError> {
            if matches!(self.behaviour, Behaviour::Failing) {
                return Err(IndexError::Source("stat failed".to_string()));
            }
            Ok(ObservedFileIdentity::distinct_for_test(self.path.as_str()))
        }

        fn location(&self, file_id: &str) -> Result<FileLocation, IndexError> {
            if matches!(self.behaviour, Behaviour::Failing) {
                return Err(IndexError::Source("stat failed".to_string()));
            }
            FileLocation::observed(
                file_id,
                self.path.clone(),
                FileKind::RegularFile,
                5,
                100,
                200,
            )
            .map_err(|error| IndexError::Source(error.to_string()))
        }

        fn read_evidence(
            &self,
            expected: &FileLocation,
            _max_content_bytes: u64,
        ) -> Result<EvidenceRead, IndexError> {
            if matches!(self.behaviour, Behaviour::Unstable) {
                return Ok(EvidenceRead::Skipped(
                    ContentSkipReason::ChangedWhileReading,
                ));
            }
            let version = FileVersion::new(
                format!("{}@hash", expected.file_id()),
                expected.file_id(),
                "hash",
                100,
            )
            .unwrap();
            let observation =
                FileObservation::new("observation", expected.file_id(), version.id(), 100).unwrap();
            Ok(EvidenceRead::Commit(
                EvidenceCommit::new(version, observation, vec![]).unwrap(),
            ))
        }
    }

    #[derive(Default)]
    struct RecordingRepository {
        observed_batches: RefCell<Vec<Vec<String>>>,
        finalized_with: RefCell<Option<Vec<(String, bool)>>>,
        abandoned: RefCell<bool>,
        fail_commits: bool,
    }

    impl RecordingRepository {
        /// Candidate paths a real store would offer to the predicate during finalization.
        const CANDIDATES: [&'static str; 4] = [
            "/scan/gone.md",
            "/scan/secret.env",
            "/scan-old/gone.md",
            "/elsewhere/gone.md",
        ];

        /// Every observed path in the order the scan published it.
        fn observed(&self) -> Vec<String> {
            self.observed_batches
                .borrow()
                .iter()
                .flatten()
                .cloned()
                .collect()
        }
    }

    impl FileIdentityHistory for RecordingRepository {
        fn known_identity(&self, _base_id: &str) -> Result<KnownIdentity, RepositoryError> {
            Ok(KnownIdentity::Unseen)
        }
    }

    impl SourceBatchWriter for RecordingRepository {
        fn commit_batch(&self, _commits: Vec<SourceCommit>) -> Result<(), RepositoryError> {
            unreachable!("a reconcile publishes only on behalf of its run")
        }

        /// Records the batch as a whole, so a test can see how the scan grouped sources
        /// and that every batch landed before finalization.
        fn commit_observed_batch(
            &self,
            run: &ReconcileRun,
            commits: Vec<SourceCommit>,
        ) -> Result<(), RepositoryError> {
            if self.fail_commits {
                return Err(RepositoryError::Storage("disk full".to_string()));
            }
            assert_eq!(run.id(), "reconcile-run:1");
            assert!(
                self.finalized_with.borrow().is_none(),
                "every observed source must be published before finalization"
            );
            self.observed_batches.borrow_mut().push(
                commits
                    .iter()
                    .map(|commit| commit.location().path().as_str().to_string())
                    .collect(),
            );
            Ok(())
        }
    }

    impl ReconcileRepository for RecordingRepository {
        fn begin_reconcile_run(
            &self,
            root: &NormalizedPath,
        ) -> Result<ReconcileRun, RepositoryError> {
            Ok(ReconcileRun::new("reconcile-run:1", 1, root.clone(), 1_000).unwrap())
        }

        fn commit_observed_source(
            &self,
            run: &ReconcileRun,
            commit: SourceCommit,
        ) -> Result<CommitResult, RepositoryError> {
            self.commit_observed_batch(run, vec![commit])?;
            Ok(CommitResult::new(0))
        }

        fn finalize_reconcile_run(
            &self,
            _run: &ReconcileRun,
            is_visible: &dyn Fn(&NormalizedPath) -> bool,
        ) -> Result<RetirementReport, RepositoryError> {
            let decisions: Vec<(String, bool)> = Self::CANDIDATES
                .iter()
                .map(|path| {
                    let path = NormalizedPath::from_absolute(path).unwrap();
                    (path.as_str().to_string(), is_visible(&path))
                })
                .collect();
            let mut report = RetirementReport::default();
            for _ in decisions.iter().filter(|(_, retire)| *retire) {
                report.record(false);
            }
            *self.finalized_with.borrow_mut() = Some(decisions);
            Ok(report)
        }

        fn abandon_reconcile_run(&self, _run: &ReconcileRun) -> Result<(), RepositoryError> {
            *self.abandoned.borrow_mut() = true;
            Ok(())
        }
    }

    fn policy() -> IndexPolicy {
        IndexPolicy::new(
            vec![PolicyRule::new(
                PolicyClass::Exclude,
                PathPattern::parse("**/*.env").unwrap(),
                RuleOrigin::Configuration,
            )],
            1_024,
        )
    }

    fn walker(entries: Vec<(&'static str, Behaviour)>) -> FakeWalker {
        FakeWalker {
            entries,
            unrepresentable_names: 0,
            directory_failure: false,
        }
    }

    #[test]
    fn a_complete_run_retires_only_unseen_paths_inside_the_root_that_policy_still_allows() {
        let walker = walker(vec![("kept.md", Behaviour::Text)]);
        let repository = RecordingRepository::default();
        let policy = policy();

        let report = ReconcileService::new(&walker, &repository, &policy, &analysing_everything())
            .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
            .unwrap();

        assert_eq!(report.run_id(), "reconcile-run:1");
        assert_eq!(repository.observed(), ["/scan/kept.md"]);
        assert_eq!(
            *report.completion(),
            ReconcileCompletion::Completed {
                marked_missing: 1,
                relocated: 0,
            }
        );
        let decisions = repository.finalized_with.borrow().clone().unwrap();
        assert_eq!(
            decisions,
            [
                ("/scan/gone.md".to_string(), true),
                ("/scan/secret.env".to_string(), false),
                ("/scan-old/gone.md".to_string(), false),
                ("/elsewhere/gone.md".to_string(), false),
            ]
        );
        assert!(!*repository.abandoned.borrow());
    }

    #[test]
    fn an_entry_failure_makes_the_run_incomplete_without_retiring_anything() {
        let walker = walker(vec![
            ("kept.md", Behaviour::Text),
            ("broken.md", Behaviour::Failing),
        ]);
        let repository = RecordingRepository::default();
        let policy = policy();

        let report = ReconcileService::new(&walker, &repository, &policy, &analysing_everything())
            .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
            .unwrap();

        assert_eq!(
            *report.completion(),
            ReconcileCompletion::Incomplete(vec![IncompleteReason::FailedPaths(1)])
        );
        assert!(repository.finalized_with.borrow().is_none());
        assert!(*repository.abandoned.borrow());
        assert_eq!(repository.observed(), ["/scan/kept.md"]);
    }

    #[test]
    fn a_directory_failure_makes_the_run_incomplete() {
        let mut walker = walker(vec![("kept.md", Behaviour::Text)]);
        walker.directory_failure = true;
        let repository = RecordingRepository::default();
        let policy = policy();

        let report = ReconcileService::new(&walker, &repository, &policy, &analysing_everything())
            .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
            .unwrap();

        assert!(matches!(
            report.completion(),
            ReconcileCompletion::Incomplete(reasons) if reasons == &[IncompleteReason::FailedPaths(1)]
        ));
        assert!(repository.finalized_with.borrow().is_none());
    }

    #[test]
    fn unstable_sources_and_unrepresentable_names_block_finalization() {
        let mut walker = walker(vec![("volatile.md", Behaviour::Unstable)]);
        walker.unrepresentable_names = 2;
        let repository = RecordingRepository::default();
        let policy = policy();

        let report = ReconcileService::new(&walker, &repository, &policy, &analysing_everything())
            .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
            .unwrap();

        assert_eq!(
            *report.completion(),
            ReconcileCompletion::Incomplete(vec![
                IncompleteReason::UnstableSources(1),
                IncompleteReason::UnrepresentableNames(2),
            ])
        );
        assert!(repository.finalized_with.borrow().is_none());
        assert!(*repository.abandoned.borrow());
    }

    #[test]
    fn a_storage_failure_abandons_the_run_and_propagates() {
        let walker = walker(vec![("kept.md", Behaviour::Text)]);
        let repository = RecordingRepository {
            fail_commits: true,
            ..RecordingRepository::default()
        };
        let policy = policy();

        let error = ReconcileService::new(&walker, &repository, &policy, &analysing_everything())
            .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
            .unwrap_err();

        assert_eq!(
            error,
            IndexError::Repository(RepositoryError::Storage("disk full".to_string()))
        );
        assert!(*repository.abandoned.borrow());
        assert!(repository.finalized_with.borrow().is_none());
    }
}
