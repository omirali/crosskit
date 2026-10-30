// Use case: index one explicitly requested directory under the configured scope policy.
use std::fmt;

use crate::domain::{AnalysisRules, IndexPolicy, NormalizedPath};

use super::file_identity::FileIdentityHistory;
use super::policy_scan::{ScanReport, scan_with_policy};
use super::repository::RepositoryError;
use super::search_indexing::{IndexingSink, SearchIndexUpdate};
use super::source_batch::SourceBatchWriter;
use super::source_walker::SourceWalker;

pub struct InitialScanService<'a, Walker, Repository> {
    walker: &'a Walker,
    repository: &'a Repository,
    policy: &'a IndexPolicy,
    analysis_rules: &'a AnalysisRules,
    search: Option<&'a SearchIndexUpdate<'a>>,
}

impl<'a, Walker, Repository> InitialScanService<'a, Walker, Repository>
where
    Walker: SourceWalker,
    Repository: SourceBatchWriter + FileIdentityHistory,
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
    /// directory that is walked: resolving it a second time here could resolve to another
    /// one.
    pub fn execute(&self, root: &NormalizedPath) -> Result<ScanReport, IndexError> {
        let policy = self.policy.with_scan_root(root);
        // Each batch is published as a plain commit, without any run bookkeeping.
        let mut report = scan_with_policy(
            self.walker,
            root,
            &policy,
            self.analysis_rules,
            self.repository,
            IndexingSink::new(|commits| self.repository.commit_batch(commits), self.search),
        )?;
        if let Some(search) = self.search {
            search.finish();
            report.record_search_index_failures(search.failures());
        }
        Ok(report)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexError {
    Source(String),
    /// A directory the operating system would not let this process open or list
    /// (`EPERM`): on macOS, a place behind privacy protection that the terminal was not
    /// given access to. Kept apart from `Source` by its kind, never by its message, so a
    /// command that has no use for such places can count them instead of naming each one;
    /// it is said the same way as any other source failure.
    NotPermitted(String),
    Repository(RepositoryError),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(message) | Self::NotPermitted(message) => {
                write!(f, "source scan failed: {message}")
            }
            Self::Repository(error) => write!(f, "index commit failed: {error}"),
        }
    }
}

impl std::error::Error for IndexError {}

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

    use super::{IndexError, InitialScanService};
    use crate::application::{
        Admission, ContentSkipReason, EvidenceRead, FileIdentityHistory, RepositoryError,
        SourceBatchWriter, SourceEntry, SourceVisitor, SourceWalker,
    };
    use crate::domain::{
        ChunkScript, ContentChunk, EvidenceCommit, FileKind, FileLocation, FileObservation,
        FileVersion, IndexPolicy, KnownIdentity, NormalizedPath, ObservedFileIdentity, PathPattern,
        PolicyClass, PolicyRule, ReconcileRun, RuleOrigin, SourceCommit,
    };

    const ROOT: &str = "/scan";
    /// The entry whose filesystem records no creation time.
    const UNDATED: &str = "undated.md";

    /// An in-memory source tree that records which entries were described or opened.
    struct FakeWalker {
        entries: Vec<(&'static str, FileKind)>,
        described: RefCell<Vec<String>>,
        opened: RefCell<Vec<String>>,
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
            visitor.skip_unrepresentable_name();
            for (name, kind) in &self.entries {
                let path = root.join(name).unwrap();
                if visitor.admit(&path) == Admission::Skip {
                    continue;
                }
                let entry = FakeEntry {
                    walker: self,
                    path,
                    kind: *kind,
                };
                visitor.visit_entry(&entry)?;
            }
            Ok(())
        }
    }

    struct FakeEntry<'a> {
        walker: &'a FakeWalker,
        path: NormalizedPath,
        kind: FileKind,
    }

    impl SourceEntry for FakeEntry<'_> {
        fn path(&self) -> &NormalizedPath {
            &self.path
        }

        /// One name stands for a filesystem that records no creation time, which is the
        /// case the scan has to report rather than pass over.
        fn observed_identity(&self) -> Result<ObservedFileIdentity, IndexError> {
            if self.path.file_name() == UNDATED {
                return Ok(ObservedFileIdentity::new(1, 7, None));
            }
            Ok(ObservedFileIdentity::distinct_for_test(self.path.as_str()))
        }

        fn location(&self, file_id: &str) -> Result<FileLocation, IndexError> {
            self.walker
                .described
                .borrow_mut()
                .push(self.path.as_str().to_string());
            FileLocation::observed(file_id, self.path.clone(), self.kind, 5, 100, 200)
                .map_err(|error| IndexError::Source(error.to_string()))
        }

        fn read_evidence(
            &self,
            expected: &FileLocation,
            _max_content_bytes: u64,
        ) -> Result<EvidenceRead, IndexError> {
            self.walker
                .opened
                .borrow_mut()
                .push(self.path.as_str().to_string());
            if self.path.file_name() == "image.bin" {
                return Ok(EvidenceRead::Skipped(ContentSkipReason::Binary));
            }
            if self.path.file_name() == "volatile.log" {
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
            // One chunk, because these tests say "indexed" about this file. A commit with
            // no chunks is a source with nothing in it, which is a different report line.
            let chunk = ContentChunk::new(
                format!("{}:chunk:0", version.id()),
                version.id(),
                0,
                0,
                4,
                "chunk-hash",
                "text",
                ChunkScript::Latin,
                "plain-text",
                "plain-text",
                "1",
                "config-hash",
                version.content_hash(),
                100,
            )
            .unwrap();
            Ok(EvidenceRead::Commit(
                EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
            ))
        }
    }

    #[derive(Default)]
    struct RecordingRepository {
        /// Every batch as it was published, so both order and grouping stay visible.
        batches: RefCell<Vec<Vec<SourceCommit>>>,
    }

    impl RecordingRepository {
        fn commits(&self) -> Vec<SourceCommit> {
            self.batches.borrow().iter().flatten().cloned().collect()
        }
    }

    /// Every fake repository answers the same way: the graph has never seen the identity,
    /// so the scan publishes under the kernel's own pair and these tests stay about the
    /// policy rather than about identity branching.
    impl FileIdentityHistory for RecordingRepository {
        fn known_identity(&self, _base_id: &str) -> Result<KnownIdentity, RepositoryError> {
            Ok(KnownIdentity::Unseen)
        }
    }

    impl SourceBatchWriter for RecordingRepository {
        fn commit_batch(&self, commits: Vec<SourceCommit>) -> Result<(), RepositoryError> {
            self.batches.borrow_mut().push(commits);
            Ok(())
        }

        fn commit_observed_batch(
            &self,
            _run: &ReconcileRun,
            _commits: Vec<SourceCommit>,
        ) -> Result<(), RepositoryError> {
            unreachable!("the initial scan never publishes on behalf of a run")
        }
    }

    struct FailingRepository;

    impl FileIdentityHistory for FailingRepository {
        fn known_identity(&self, _base_id: &str) -> Result<KnownIdentity, RepositoryError> {
            Ok(KnownIdentity::Unseen)
        }
    }

    impl SourceBatchWriter for FailingRepository {
        fn commit_batch(&self, _commits: Vec<SourceCommit>) -> Result<(), RepositoryError> {
            Err(RepositoryError::Storage("disk full".to_string()))
        }

        fn commit_observed_batch(
            &self,
            _run: &ReconcileRun,
            _commits: Vec<SourceCommit>,
        ) -> Result<(), RepositoryError> {
            unreachable!("the initial scan never publishes on behalf of a run")
        }
    }

    fn rule(class: PolicyClass, pattern: &str) -> PolicyRule {
        PolicyRule::new(
            class,
            PathPattern::parse(pattern).unwrap(),
            RuleOrigin::Configuration,
        )
    }

    fn walker(entries: Vec<(&'static str, FileKind)>) -> FakeWalker {
        FakeWalker {
            entries,
            described: RefCell::new(Vec::new()),
            opened: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn policy_decides_before_metadata_or_content_is_touched() {
        let walker = walker(vec![
            ("notes.md", FileKind::RegularFile),
            ("secret.env", FileKind::RegularFile),
            ("photo.jpg", FileKind::RegularFile),
            ("image.bin", FileKind::RegularFile),
            ("volatile.log", FileKind::RegularFile),
            ("link", FileKind::Symlink),
        ]);
        let repository = RecordingRepository::default();
        let policy = IndexPolicy::new(
            vec![
                rule(PolicyClass::Exclude, "**/*.env"),
                rule(PolicyClass::Metadata, "/scan/photo.jpg"),
            ],
            1_024,
        );

        let report =
            InitialScanService::new(&walker, &repository, &policy, &analysing_everything())
                .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
                .unwrap();

        assert_eq!(report.indexed_files(), 1);
        assert_eq!(report.metadata_only_files(), 1);
        assert_eq!(report.content_skipped_files(), 2);
        assert_eq!(report.unstable_sources(), 1);
        assert_eq!(report.excluded_paths(), 1);
        assert_eq!(report.unrepresentable_names(), 1);
        assert!(report.failures().is_empty());
        assert_eq!(
            *walker.described.borrow(),
            [
                "/scan/notes.md",
                "/scan/photo.jpg",
                "/scan/image.bin",
                "/scan/volatile.log",
                "/scan/link"
            ]
        );
        assert_eq!(
            *walker.opened.borrow(),
            ["/scan/notes.md", "/scan/image.bin", "/scan/volatile.log"]
        );

        let commits = repository.commits();
        let evidence_for = |name: &str| {
            commits
                .iter()
                .find(|commit| commit.location().display_name() == name)
                .map(|commit| commit.evidence().is_some())
        };
        assert_eq!(evidence_for("notes.md"), Some(true));
        assert_eq!(evidence_for("photo.jpg"), Some(false));
        assert_eq!(evidence_for("image.bin"), Some(false));
        assert_eq!(evidence_for("link"), Some(false));
        assert_eq!(evidence_for("secret.env"), None);
        assert_eq!(evidence_for("volatile.log"), None);
    }

    /// Where the filesystem records no creation time, a file that takes a deleted file's
    /// inode number cannot be told from it. The scan says how many such sources it met, so
    /// the gap is stated instead of being silently left open.
    #[test]
    fn sources_the_filesystem_gives_no_creation_time_are_counted() {
        let walker = walker(vec![
            ("notes.md", FileKind::RegularFile),
            (UNDATED, FileKind::RegularFile),
        ]);
        let repository = RecordingRepository::default();
        let policy = IndexPolicy::new(vec![], 1_024);

        let report =
            InitialScanService::new(&walker, &repository, &policy, &analysing_everything())
                .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
                .unwrap();

        assert_eq!(report.indexed_files(), 2);
        assert_eq!(report.undated_sources(), 1);
    }

    #[test]
    fn an_excluded_root_is_never_inspected() {
        let walker = walker(vec![("notes.md", FileKind::RegularFile)]);
        let repository = RecordingRepository::default();
        let policy = IndexPolicy::new(vec![rule(PolicyClass::Exclude, "/scan")], 1_024);

        let report =
            InitialScanService::new(&walker, &repository, &policy, &analysing_everything())
                .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
                .unwrap();

        assert_eq!(report.excluded_paths(), 1);
        assert!(walker.described.borrow().is_empty());
        assert!(repository.commits().is_empty());
    }

    #[test]
    fn a_storage_failure_stops_the_scan() {
        let walker = walker(vec![("notes.md", FileKind::RegularFile)]);
        let policy = IndexPolicy::new(vec![], 1_024);

        let error = InitialScanService::new(
            &walker,
            &FailingRepository,
            &policy,
            &analysing_everything(),
        )
        .execute(&NormalizedPath::from_absolute(ROOT).unwrap())
        .unwrap_err();

        assert_eq!(
            error,
            IndexError::Repository(RepositoryError::Storage("disk full".to_string()))
        );
    }
}
