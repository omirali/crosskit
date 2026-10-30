// Behaviour of the descriptor-relative walker under the scope policy, observed through a
// recording repository on a real temporary directory tree.
use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use super::DescriptorRelativeWalker;
use super::plain_text_evidence::{chunk_bytes, read_plain_text_evidence};
use super::unix_walk;
use crate::application::{
    Admission, ContentSkipReason, EvidenceRead, FileIdentityHistory, IndexError,
    InitialScanService, RepositoryError, ScanReport, SourceBatchWriter, SourceEntry, SourceVisitor,
    SourceWalker,
};
use crate::domain::{
    ChunkScript, EvidenceCommit, FileKind, FileLocation, IndexPolicy, KnownIdentity,
    NormalizedPath, ObservedFileIdentity, PathPattern, PolicyClass, PolicyRule, ReconcileRun,
    RuleOrigin, SourceCommit,
};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn test_directory(test_name: &str) -> PathBuf {
    let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "crosskit-walker-{test_name}-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

#[derive(Default)]
struct RecordingRepository {
    commits: RefCell<Vec<SourceCommit>>,
}

/// These tests drive a real filesystem against an empty graph, so every identity they meet
/// is one the graph has never seen and is published under the kernel's own pair.
impl FileIdentityHistory for RecordingRepository {
    fn known_identity(&self, _base_id: &str) -> Result<KnownIdentity, RepositoryError> {
        Ok(KnownIdentity::Unseen)
    }
}

impl SourceBatchWriter for RecordingRepository {
    fn commit_batch(&self, commits: Vec<SourceCommit>) -> Result<(), RepositoryError> {
        self.commits.borrow_mut().extend(commits);
        Ok(())
    }

    fn commit_observed_batch(
        &self,
        _run: &ReconcileRun,
        _commits: Vec<SourceCommit>,
    ) -> Result<(), RepositoryError> {
        unreachable!("these tests drive the initial scan, which has no run")
    }
}

fn policy(rules: &[(PolicyClass, &str)]) -> IndexPolicy {
    IndexPolicy::new(
        rules
            .iter()
            .map(|(class, pattern)| {
                PolicyRule::new(
                    *class,
                    PathPattern::parse(pattern).unwrap(),
                    RuleOrigin::Configuration,
                )
            })
            .collect(),
        100 * 1024 * 1024,
    )
}

/// Resolving the root is the command's job now, so these tests do it the same way before
/// handing the already resolved root to the use case.
fn resolved(walker: &impl SourceWalker, root: &Path) -> NormalizedPath {
    walker.resolve_root(root).unwrap()
}

fn scan(root: &Path, policy: &IndexPolicy) -> (ScanReport, Vec<SourceCommit>) {
    let repository = RecordingRepository::default();
    let report = InitialScanService::new(
        &DescriptorRelativeWalker,
        &repository,
        policy,
        &analysing_everything(),
    )
    .execute(&resolved(&DescriptorRelativeWalker, root))
    .unwrap();
    (report, repository.commits.into_inner())
}

fn commit_named<'a>(commits: &'a [SourceCommit], name: &str) -> &'a SourceCommit {
    commits
        .iter()
        .find(|commit| commit.location().display_name() == name)
        .unwrap_or_else(|| panic!("no commit for {name}"))
}

fn deny_all_access(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
}

fn restore_access(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn scanner_indexes_text_and_records_metadata_for_binary_files() {
    let root = test_directory("mixed");
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    fs::write(root.join("image.bin"), [0_u8, 1, 2]).unwrap();
    fs::write(root.join(".env"), "API_KEY=secret").unwrap();
    fs::write(root.join("target").join("generated.rs"), "not indexed").unwrap();

    let (report, commits) = scan(
        &root,
        &policy(&[
            (PolicyClass::Exclude, "**/target/**"),
            (PolicyClass::Exclude, "**/.env"),
        ]),
    );

    assert_eq!(report.indexed_files(), 1);
    assert_eq!(report.indexed_chunks(), 1);
    assert_eq!(report.content_skipped_files(), 1);
    assert_eq!(report.excluded_paths(), 2);
    assert!(report.failures().is_empty(), "{:?}", report.failures());
    assert!(commit_named(&commits, "notes.md").evidence().is_some());
    assert!(commit_named(&commits, "image.bin").evidence().is_none());
    assert_eq!(commits.len(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn excluded_entries_are_never_opened_or_described() {
    let root = test_directory("excluded");
    let secret = root.join("secret.env");
    let vault = root.join("node_modules");
    fs::write(&secret, "TOKEN=1").unwrap();
    fs::create_dir(&vault).unwrap();
    fs::write(vault.join("inner.txt"), "inside").unwrap();
    // Any attempt to open or list these would fail loudly and land in the failures.
    deny_all_access(&secret);
    deny_all_access(&vault);

    let (report, commits) = scan(
        &root,
        &policy(&[
            (PolicyClass::Exclude, "**/*.env"),
            (PolicyClass::Exclude, "**/node_modules/**"),
        ]),
    );

    assert_eq!(report.excluded_paths(), 2);
    assert!(report.failures().is_empty(), "{:?}", report.failures());
    assert!(commits.is_empty());
    restore_access(&vault);
    restore_access(&secret);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn metadata_only_files_are_described_without_being_opened() {
    let root = test_directory("metadata");
    let photo = root.join("photo.jpg");
    fs::write(&photo, "not really a jpeg but unreadable anyway").unwrap();
    deny_all_access(&photo);

    let (report, commits) = scan(&root, &policy(&[(PolicyClass::Metadata, "**/*.jpg")]));

    assert_eq!(report.metadata_only_files(), 1);
    assert_eq!(report.indexed_files(), 0);
    assert!(report.failures().is_empty(), "{:?}", report.failures());
    let location = commit_named(&commits, "photo.jpg").location();
    assert_eq!(location.kind(), FileKind::RegularFile);
    assert_eq!(location.size_bytes(), 39);
    assert!(commit_named(&commits, "photo.jpg").evidence().is_none());
    restore_access(&photo);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn symlinks_are_recorded_as_metadata_and_never_followed() {
    let base = test_directory("symlinks");
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let secret = outside.join("secret.txt");
    fs::write(&secret, "outside root").unwrap();
    symlink(&outside, root.join("link-dir")).unwrap();
    symlink(&secret, root.join("link-file")).unwrap();
    // Following either link would have to read through this and fail.
    deny_all_access(&secret);

    let (report, commits) = scan(&root, &policy(&[]));

    assert_eq!(report.indexed_files(), 0);
    assert_eq!(report.content_skipped_files(), 2);
    assert!(report.failures().is_empty(), "{:?}", report.failures());
    assert_eq!(commits.len(), 2);
    for commit in &commits {
        assert_eq!(commit.location().kind(), FileKind::Symlink);
        assert!(commit.evidence().is_none());
        assert!(!commit.location().path().as_str().contains("outside"));
    }
    restore_access(&secret);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_symlinked_root_is_resolved_before_the_policy_applies() {
    let base = test_directory("root-link");
    let real = base.join("real-secrets");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("notes.md"), "hidden").unwrap();
    let link = base.join("innocent-link");
    symlink(&real, &link).unwrap();
    let canonical_real = fs::canonicalize(&real).unwrap();

    let (report, commits) = scan(
        &link,
        &policy(&[(PolicyClass::Exclude, &canonical_real.to_string_lossy())]),
    );

    assert_eq!(report.root().as_str(), canonical_real.to_string_lossy());
    assert_eq!(report.excluded_paths(), 1);
    assert!(commits.is_empty());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn descriptor_relative_open_rejects_a_replaced_directory() {
    let root = test_directory("replaced");
    let original_directory = root.join("inside");
    let parked_directory = root.join("inside-before-replacement");
    let outside_directory = root.join("outside");
    fs::create_dir(&original_directory).unwrap();
    fs::create_dir(&outside_directory).unwrap();
    fs::write(outside_directory.join("secret.txt"), "outside root").unwrap();
    fs::rename(&original_directory, &parked_directory).unwrap();
    symlink(&outside_directory, &original_directory).unwrap();

    assert!(super::unix_walk::refuses_replaced_directory(
        &root,
        std::ffi::OsStr::new("inside")
    ));
    fs::remove_file(&original_directory).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_fifo_is_recorded_as_special_without_being_opened() {
    use std::os::unix::ffi::OsStrExt;

    let root = test_directory("fifo");
    let fifo = root.join("events.fifo");
    let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);

    let (report, commits) = scan(&root, &policy(&[]));

    assert_eq!(report.indexed_files(), 0);
    assert_eq!(report.content_skipped_files(), 1);
    assert!(report.failures().is_empty(), "{:?}", report.failures());
    assert_eq!(
        commit_named(&commits, "events.fifo").location().kind(),
        FileKind::Special
    );
    fs::remove_dir_all(root).unwrap();
}

/// Records every visitor call so a test can prove which entries reached which stage.
#[derive(Default)]
struct ProbeVisitor {
    admitted: Vec<String>,
    unrepresentable: usize,
    failures: Vec<String>,
}

impl SourceVisitor for ProbeVisitor {
    fn admit(&mut self, path: &NormalizedPath) -> Admission {
        self.admitted.push(path.as_str().to_string());
        Admission::Inspect
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        panic!("no entry was expected to exist: {}", entry.path());
    }

    fn skip_unrepresentable_name(&mut self) {
        self.unrepresentable += 1;
    }

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        self.failures.push(format!("{path}: {error}"));
    }
}

/// APFS refuses to create non-UTF-8 names, so the names are injected into the directory
/// step directly: a UTF-8 name that does not exist proves the step reaches `lstat`,
/// while the two non-UTF-8 names must be skipped before it.
#[test]
fn non_utf8_names_are_skipped_before_any_filesystem_call() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = test_directory("non-utf8-names");
    let root_path = DescriptorRelativeWalker.resolve_root(&root).unwrap();
    let directory = unix_walk::open_root_directory(&root_path).unwrap();
    let names = vec![
        OsString::from_vec(b"bad-\xFE.txt".to_vec()),
        OsString::from_vec(b"bad-\xFF.txt".to_vec()),
        OsString::from("missing.txt"),
    ];
    let mut visitor = ProbeVisitor::default();
    let mut pending = Vec::new();

    unix_walk::visit_directory_entries(&directory, &root_path, names, &mut visitor, &mut pending)
        .unwrap();

    assert_eq!(visitor.unrepresentable, 2);
    assert_eq!(
        visitor.admitted,
        [root_path.join("missing.txt").unwrap().as_str().to_string()]
    );
    assert_eq!(visitor.failures.len(), 1, "{:?}", visitor.failures);
    assert!(visitor.failures[0].contains("missing.txt"));
    assert!(
        visitor
            .admitted
            .iter()
            .all(|path| !path.contains('\u{FFFD}'))
    );
    assert!(pending.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn files_with_non_utf8_names_leave_no_trace_in_the_scan() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let root = test_directory("non-utf8-files");
    let names: [&[u8]; 2] = [b"bad-\xFE.txt", b"bad-\xFF.txt"];
    for name in names {
        let path = root.join(OsStr::from_bytes(name));
        fs::write(&path, "secret").unwrap();
        deny_all_access(&path);
    }

    let (report, commits) = scan(&root, &policy(&[]));

    assert_eq!(report.unrepresentable_names(), 2);
    assert_eq!(report.indexed_files(), 0);
    assert!(report.failures().is_empty(), "{:?}", report.failures());
    assert!(commits.is_empty());
    for name in names {
        restore_access(&root.join(OsStr::from_bytes(name)));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_file_rewritten_between_stat_and_open_is_not_published() {
    assert_eq!(
        read_after_rewrite_between_stat_and_open(rewrite_with_longer_body),
        [EvidenceRead::Skipped(
            ContentSkipReason::ChangedWhileReading
        )]
    );
}

#[test]
fn a_same_size_rewrite_within_one_millisecond_between_stat_and_open_is_not_published() {
    assert_eq!(
        read_after_rewrite_between_stat_and_open(rewrite_same_size_within_the_same_millisecond),
        [EvidenceRead::Skipped(
            ContentSkipReason::ChangedWhileReading
        )]
    );
}

#[test]
fn an_untouched_file_between_stat_and_open_is_published() {
    let outcomes = read_after_rewrite_between_stat_and_open(|_path| {});
    assert!(matches!(outcomes.as_slice(), [EvidenceRead::Commit(_)]));
}

/// How a test mutates a file inside a race window.
type Rewrite = fn(&Path);

fn rewrite_with_longer_body(path: &Path) {
    fs::write(path, "rewritten after lstat with a longer body").unwrap();
}

/// Replaces the five-byte body with another five-byte body and moves the modification
/// time to a different nanosecond inside the same millisecond, so size and the
/// millisecond timestamp stay equal while the exact fingerprint differs.
fn rewrite_same_size_within_the_same_millisecond(path: &Path) {
    let original = fs::metadata(path).unwrap().modified().unwrap();
    assert_eq!(fs::read(path).unwrap().len(), "fresh".len());
    fs::write(path, "fresh").unwrap();

    let since_epoch = original.duration_since(UNIX_EPOCH).unwrap();
    let whole_milliseconds = Duration::from_millis(u64::try_from(since_epoch.as_millis()).unwrap());
    let nanoseconds_in_millisecond = u64::from(since_epoch.subsec_nanos() % 1_000_000);
    let shifted = UNIX_EPOCH
        + whole_milliseconds
        + Duration::from_nanos((nanoseconds_in_millisecond + 100_000) % 1_000_000);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(shifted)
        .unwrap();

    let updated = fs::metadata(path).unwrap().modified().unwrap();
    assert_ne!(
        updated, original,
        "the filesystem must keep sub-millisecond mtime"
    );
    assert_eq!(
        updated.duration_since(UNIX_EPOCH).unwrap().as_millis(),
        since_epoch.as_millis(),
        "the rewrite must stay inside the original millisecond"
    );
}

fn read_after_rewrite_between_stat_and_open(rewrite: Rewrite) -> Vec<EvidenceRead> {
    let root = test_directory("stat-open-window");
    fs::write(root.join("volatile.txt"), "first").unwrap();
    let root_path = DescriptorRelativeWalker.resolve_root(&root).unwrap();
    let mut visitor = ReadAfterRewrite {
        target: root.join("volatile.txt"),
        rewrite,
        outcomes: Vec::new(),
    };

    DescriptorRelativeWalker
        .walk(&root_path, &mut visitor)
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    visitor.outcomes
}

/// Rewrites the target between `location()` and `read_evidence()`, which is exactly the
/// window between the walker's `lstat` and its `openat`.
struct ReadAfterRewrite {
    target: PathBuf,
    rewrite: Rewrite,
    outcomes: Vec<EvidenceRead>,
}

impl SourceVisitor for ReadAfterRewrite {
    fn admit(&mut self, _path: &NormalizedPath) -> Admission {
        Admission::Inspect
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        let location = entry.location(&entry.observed_identity()?.base_id())?;
        rewrite_after_describe(&self.target, &location, self.rewrite);
        self.outcomes
            .push(entry.read_evidence(&location, u64::MAX)?);
        Ok(())
    }

    fn skip_unrepresentable_name(&mut self) {}

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        panic!("unexpected failure at {path}: {error}");
    }
}

fn rewrite_after_describe(target: &Path, location: &FileLocation, rewrite: Rewrite) {
    if target
        .file_name()
        .is_some_and(|name| name == location.display_name())
    {
        rewrite(target);
    }
}

#[test]
fn a_file_rewritten_between_open_and_read_is_not_published() {
    assert_eq!(
        read_after_rewrite_between_open_and_read(rewrite_with_longer_body),
        EvidenceRead::Skipped(ContentSkipReason::ChangedWhileReading)
    );
}

#[test]
fn a_same_size_rewrite_within_one_millisecond_between_open_and_read_is_not_published() {
    assert_eq!(
        read_after_rewrite_between_open_and_read(rewrite_same_size_within_the_same_millisecond),
        EvidenceRead::Skipped(ContentSkipReason::ChangedWhileReading)
    );
}

fn read_after_rewrite_between_open_and_read(rewrite: Rewrite) -> EvidenceRead {
    let root = test_directory("open-read-window");
    let path = root.join("volatile.txt");
    fs::write(&path, "first").unwrap();
    let mut file = fs::File::open(&path).unwrap();
    let before = file.metadata().unwrap();
    rewrite(&path);

    let outcome = read_plain_text_evidence(&mut file, &before, "file:test", u64::MAX).unwrap();
    fs::remove_dir_all(root).unwrap();
    outcome
}

#[test]
fn unstable_sources_are_counted_and_leave_no_commit() {
    let root = test_directory("unstable-report");
    fs::write(root.join("stable.md"), "stays the same").unwrap();
    fs::write(root.join("volatile.txt"), "first").unwrap();
    let repository = RecordingRepository::default();
    let policy = policy(&[]);
    let walker = RewritingWalker {
        target: root.join("volatile.txt"),
    };

    let resolved_root = resolved(&walker, &root);
    let report = InitialScanService::new(&walker, &repository, &policy, &analysing_everything())
        .execute(&resolved_root)
        .unwrap();

    assert_eq!(report.indexed_files(), 1);
    assert_eq!(report.unstable_sources(), 1);
    assert_eq!(report.content_skipped_files(), 0);
    assert!(report.failures().is_empty(), "{:?}", report.failures());
    let commits = repository.commits.into_inner();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].location().display_name(), "stable.md");
    fs::remove_dir_all(root).unwrap();
}

/// The real walker, with `target` rewritten right after the use case described it, so
/// the use case observes a source that changed between `lstat` and `openat`.
struct RewritingWalker {
    target: PathBuf,
}

impl SourceWalker for RewritingWalker {
    fn resolve_root(&self, root: &Path) -> Result<NormalizedPath, IndexError> {
        DescriptorRelativeWalker.resolve_root(root)
    }

    fn walk(
        &self,
        root: &NormalizedPath,
        visitor: &mut dyn SourceVisitor,
    ) -> Result<(), IndexError> {
        let mut rewriting = RewritingVisitor {
            inner: visitor,
            target: &self.target,
        };
        DescriptorRelativeWalker.walk(root, &mut rewriting)
    }
}

struct RewritingVisitor<'a> {
    inner: &'a mut dyn SourceVisitor,
    target: &'a Path,
}

impl SourceVisitor for RewritingVisitor<'_> {
    fn admit(&mut self, path: &NormalizedPath) -> Admission {
        self.inner.admit(path)
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        let rewriting = RewritingEntry {
            inner: entry,
            target: self.target,
        };
        self.inner.visit_entry(&rewriting)
    }

    fn skip_unrepresentable_name(&mut self) {
        self.inner.skip_unrepresentable_name();
    }

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        self.inner.record_failure(path, error);
    }
}

struct RewritingEntry<'a> {
    inner: &'a dyn SourceEntry,
    target: &'a Path,
}

impl SourceEntry for RewritingEntry<'_> {
    fn path(&self) -> &NormalizedPath {
        self.inner.path()
    }

    fn observed_identity(&self) -> Result<ObservedFileIdentity, IndexError> {
        self.inner.observed_identity()
    }

    fn location(&self, file_id: &str) -> Result<FileLocation, IndexError> {
        let location = self.inner.location(file_id)?;
        rewrite_after_describe(self.target, &location, rewrite_with_longer_body);
        Ok(location)
    }

    fn read_evidence(
        &self,
        expected: &FileLocation,
        max_content_bytes: u64,
    ) -> Result<EvidenceRead, IndexError> {
        self.inner.read_evidence(expected, max_content_bytes)
    }
}

/// Reads the entry under tampered locations that keep the file id but change one other
/// field, then under the genuine one; records the outcome of every attempt.
struct TamperedLocationVisitor {
    root: NormalizedPath,
    rejected: Vec<String>,
    genuine: Vec<EvidenceRead>,
}

impl SourceVisitor for TamperedLocationVisitor {
    fn admit(&mut self, _path: &NormalizedPath) -> Admission {
        Admission::Inspect
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        let genuine = entry.location(&entry.observed_identity()?.base_id())?;
        let tampered = [
            relocated(&genuine, self.root.join("other.txt").unwrap()),
            resized(&genuine, genuine.size_bytes() + 1),
            retimed(&genuine, genuine.modified_at_unix_ms() + 1),
        ];
        for location in &tampered {
            match entry.read_evidence(location, u64::MAX) {
                Err(IndexError::Source(message)) => self.rejected.push(message),
                other => panic!(
                    "tampered location {} was accepted: {other:?}",
                    location.id()
                ),
            }
        }
        self.genuine.push(entry.read_evidence(&genuine, u64::MAX)?);
        Ok(())
    }

    fn skip_unrepresentable_name(&mut self) {}

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        panic!("unexpected failure at {path}: {error}");
    }
}

fn relocated(location: &FileLocation, path: NormalizedPath) -> FileLocation {
    FileLocation::observed(
        location.file_id(),
        path,
        location.kind(),
        location.size_bytes(),
        location.modified_at_unix_ms(),
        location.first_seen_at_unix_ms(),
    )
    .unwrap()
}

fn resized(location: &FileLocation, size_bytes: u64) -> FileLocation {
    FileLocation::observed(
        location.file_id(),
        location.path().clone(),
        location.kind(),
        size_bytes,
        location.modified_at_unix_ms(),
        location.first_seen_at_unix_ms(),
    )
    .unwrap()
}

fn retimed(location: &FileLocation, modified_at_unix_ms: i64) -> FileLocation {
    FileLocation::observed(
        location.file_id(),
        location.path().clone(),
        location.kind(),
        location.size_bytes(),
        modified_at_unix_ms,
        location.first_seen_at_unix_ms(),
    )
    .unwrap()
}

#[test]
fn evidence_is_read_only_for_the_location_the_entry_described() {
    let root = test_directory("tampered-location");
    fs::write(root.join("notes.md"), "Crosskit indexes this note.").unwrap();
    let root_path = DescriptorRelativeWalker.resolve_root(&root).unwrap();
    let mut visitor = TamperedLocationVisitor {
        root: root_path.clone(),
        rejected: Vec::new(),
        genuine: Vec::new(),
    };

    DescriptorRelativeWalker
        .walk(&root_path, &mut visitor)
        .unwrap();

    assert_eq!(visitor.rejected.len(), 3, "{:?}", visitor.rejected);
    assert!(
        visitor
            .rejected
            .iter()
            .all(|message| message.contains("is not the description of entry")),
        "{:?}",
        visitor.rejected
    );
    assert!(matches!(
        visitor.genuine.as_slice(),
        [EvidenceRead::Commit(_)]
    ));
    fs::remove_dir_all(root).unwrap();
}

/// The streaming cut must reproduce what a whole-file cut produced: chunks that follow one
/// another without a gap, cover every byte exactly once, and reassemble into the original.
/// The file is larger than one read block, so boundaries fall inside the window rather than
/// at its edge.
#[test]
fn streamed_chunks_cover_the_source_exactly() {
    let root = test_directory("streamed-coverage");
    let path = root.join("long.txt");
    let line = "lorem ipsum dolor sit amet consectetur adipiscing elit
";
    let body = line.repeat(20_000);
    fs::write(&path, &body).unwrap();
    let mut file = fs::File::open(&path).unwrap();
    let before = file.metadata().unwrap();

    let outcome = read_plain_text_evidence(&mut file, &before, "file:test", u64::MAX).unwrap();

    let EvidenceRead::Commit(commit) = outcome else {
        panic!("a long text file must be published, received {outcome:?}");
    };
    let chunks = commit.chunks();
    assert!(
        chunks.len() > 10,
        "expected many chunks, got {}",
        chunks.len()
    );
    let mut expected_start = 0;
    let mut reassembled = String::new();
    for (ordinal, chunk) in chunks.iter().enumerate() {
        assert_eq!(
            chunk.start_byte(),
            expected_start,
            "chunk {ordinal} starts after a gap"
        );
        assert_eq!(
            chunk.end_byte() - chunk.start_byte(),
            chunk.text().len() as u64,
            "chunk {ordinal} offsets disagree with its text"
        );
        assert_eq!(chunk.ordinal(), ordinal as u32);
        expected_start = chunk.end_byte();
        reassembled.push_str(chunk.text());
    }
    assert_eq!(
        expected_start as usize,
        body.len(),
        "the chunks must reach the end"
    );
    assert_eq!(
        reassembled, body,
        "the chunks must reassemble into the source"
    );
    fs::remove_dir_all(root).unwrap();
}

/// A character split across two read blocks belongs to the content, not to a refusal: the
/// file is multi-byte throughout and longer than one block, so the split is certain.
#[test]
fn a_character_split_across_read_blocks_is_carried_over() {
    let root = test_directory("streamed-multibyte");
    let path = root.join("multibyte.txt");
    // Three bytes per character, so no block boundary can land between characters evenly.
    let body = "словарь языка ".repeat(40_000);
    fs::write(&path, &body).unwrap();
    let mut file = fs::File::open(&path).unwrap();
    let before = file.metadata().unwrap();

    let outcome = read_plain_text_evidence(&mut file, &before, "file:test", u64::MAX).unwrap();

    let EvidenceRead::Commit(commit) = outcome else {
        panic!("a multi-byte text file must be published, received {outcome:?}");
    };
    let reassembled: String = commit.chunks().iter().map(|chunk| chunk.text()).collect();
    assert_eq!(reassembled, body);
    assert!(
        commit
            .chunks()
            .iter()
            .all(|chunk| chunk.script() == ChunkScript::Cyrillic),
        "every chunk of a Cyrillic file must say so, and say it about its own text"
    );
    fs::remove_dir_all(root).unwrap();
}

/// The spaces that are not one byte wide. Every one of them is ordinary in Russian text:
/// `U+00A0` comes out of every word processor, and the rest out of typesetting.
const WIDE_SPACES: [char; 6] = [
    '\u{00a0}', '\u{2009}', '\u{200a}', '\u{202f}', '\u{3000}', '\u{2028}',
];

/// The cut ends a chunk after the last whitespace inside it. A search answers with the byte
/// the whitespace *starts* at, and the chunk has to end after its last byte: `U+200A` is
/// three bytes, so ending one byte past its start lands inside the character and the slice
/// that follows brings the process down — which is what it did on the owner's tree.
///
/// What makes this test the one that would have caught it is not that the text is Unicode:
/// a corpus of Russian prose passed throughout, because prose has an ordinary space every
/// few words and the last whitespace of a chunk is almost never a wide one. It is the
/// position. The wide space is placed so that it is the last whitespace before the cut, at
/// the boundary, one byte short of it and one byte past it — the third pushing the cut back
/// off the boundary onto an earlier space, which is the case the first two do not reach.
#[test]
fn a_wide_space_at_the_chunk_boundary_is_cut_after_it_and_never_inside_it() {
    let root = test_directory("wide-space-boundary");
    let chunk = chunk_bytes();
    for space in WIDE_SPACES {
        for (position, ends_at) in [
            ("on the boundary", chunk),
            ("one byte before the boundary", chunk - 1),
            ("one byte past the boundary", chunk + 1),
        ] {
            let case = format!("{space:?} {position}");
            let (body, early_space_ends_at) = text_with_a_wide_space_ending_at(space, ends_at);
            let name = format!("{:04x}-{ends_at}.txt", space as u32);
            let commit = published_commit(&root.join(name), &case, &body);

            // Past the boundary, the cut cannot use that space at all: ending the chunk
            // after it would overrun the chunk size. It falls back to the space before it.
            let expected_end = if ends_at > chunk {
                early_space_ends_at
            } else {
                ends_at
            };
            let first = &commit.chunks()[0];
            assert_eq!(
                first.end_byte() as usize,
                expected_end,
                "{case}: the chunk must end where the space ends"
            );
            assert!(
                first.text().ends_with(space),
                "{case}: the chunk must hold the whole space, not its first byte"
            );
            assert_eq!(
                chunks_reassembled(&commit),
                body,
                "{case}: the chunks must still cover the source exactly"
            );
            assert!(
                first.end_byte() as usize <= chunk,
                "{case}: a chunk may not grow past the chunk size"
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}

/// `space` placed so that its last byte is at `ends_at`, with nothing else whitespace after
/// it, and one more wide space early on for the cut to fall back to. Returns the text and
/// where that earlier space ends.
fn text_with_a_wide_space_ending_at(space: char, ends_at: usize) -> (String, usize) {
    const EARLY: usize = 64;
    let width = space.len_utf8();
    let mut body = String::with_capacity(ends_at * 2);
    body.push_str(&"a".repeat(EARLY));
    body.push(space);
    let early_space_ends_at = EARLY + width;
    // No whitespace between the two spaces, so the one being placed is the last before the
    // cut and the early one is what the cut falls back to.
    body.push_str(&"b".repeat(ends_at - width - early_space_ends_at));
    body.push(space);
    assert_eq!(
        body.len(),
        ends_at,
        "the space must end exactly at {ends_at}"
    );
    // Long enough past the cut that the chunk is never the whole file, and with no
    // whitespace at all so no later space can be chosen instead.
    body.push_str(&"c".repeat(ends_at));
    (body, early_space_ends_at)
}

/// A source with no text in it at all. It used to panic on the first cut when the spaces
/// were wide, and before that it was a failed path — a list somebody has to go and read
/// about a file that asks nobody to look.
///
/// It is now published like any other source, with no chunks. That is not a detail of
/// bookkeeping: publishing the version is what makes a file whose text was deleted stop
/// being current at the version that still had it, so its documents leave the search index
/// on this pass. Skipping the file would have left them answering.
///
/// An empty file is the same statement with no bytes, and has always answered this way.
#[test]
fn a_source_with_no_text_in_it_is_published_with_no_chunks() {
    let root = test_directory("no-text");
    let mut bodies: Vec<(String, String)> = WIDE_SPACES
        .iter()
        .map(|space| {
            (
                format!("spaces-{:04x}", *space as u32),
                space.to_string().repeat(chunk_bytes()),
            )
        })
        .collect();
    bodies.push(("empty".to_string(), String::new()));
    bodies.push(("ascii-blank".to_string(), "  \n\t\n ".to_string()));

    for (name, body) in bodies {
        let path = root.join(format!("{name}.txt"));
        fs::write(&path, &body).unwrap();
        let mut file = fs::File::open(&path).unwrap();
        let before = file.metadata().unwrap();

        let outcome = read_plain_text_evidence(&mut file, &before, "file:test", u64::MAX)
            .unwrap_or_else(|error| panic!("{name}: nothing to extract is not a failure: {error}"));

        let EvidenceRead::Commit(commit) = outcome else {
            panic!("{name}: a source with nothing in it is still published: {outcome:?}");
        };
        assert!(
            commit.chunks().is_empty(),
            "{name}: there is nothing for a chunk to hold"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

/// A blank block longer than a chunk, with real text on both sides of it. The chunk that
/// falls inside the block holds nothing but whitespace, and refusing such a chunk refused
/// the whole document — a Markdown file with a wide gap in it indexed nothing at all.
#[test]
fn a_blank_block_longer_than_a_chunk_does_not_cost_the_document_around_it() {
    let root = test_directory("blank-block");
    let path = root.join("gap.md");
    let body = format!(
        "{}{}{}",
        "начало текста ".repeat(8),
        " ".repeat(chunk_bytes() * 2),
        "конец текста"
    );
    fs::write(&path, &body).unwrap();
    let mut file = fs::File::open(&path).unwrap();
    let before = file.metadata().unwrap();

    let outcome = read_plain_text_evidence(&mut file, &before, "file:test", u64::MAX)
        .unwrap_or_else(|error| panic!("a gap is not a reason to refuse a document: {error}"));

    let EvidenceRead::Commit(commit) = outcome else {
        panic!("the document must be published, received {outcome:?}");
    };
    assert_eq!(
        chunks_reassembled(&commit),
        body,
        "the chunks must still cover the source exactly, gap included"
    );
    assert!(
        commit
            .chunks()
            .iter()
            .any(|chunk| chunk.text().trim().is_empty()),
        "the case only bites when one chunk falls wholly inside the gap"
    );
    fs::remove_dir_all(root).unwrap();
}

/// `body` written to `path` and read back as evidence, or a failure naming the case.
fn published_commit(path: &Path, case: &str, body: &str) -> EvidenceCommit {
    fs::write(path, body).unwrap();
    let mut file = fs::File::open(path).unwrap();
    let before = file.metadata().unwrap();
    let outcome = read_plain_text_evidence(&mut file, &before, "file:test", u64::MAX)
        .unwrap_or_else(|error| panic!("{case}: the source must be published, received {error}"));
    let EvidenceRead::Commit(commit) = outcome else {
        panic!("{case}: the source must be published, received {outcome:?}");
    };
    commit
}

fn chunks_reassembled(commit: &EvidenceCommit) -> String {
    commit.chunks().iter().map(|chunk| chunk.text()).collect()
}

/// The content limit is a boundary, not an approximation: a source of exactly the limit is
/// read, and one byte more is published by its metadata alone, without its content ever
/// being opened. The oversize count names the reason, apart from content that was read and
/// refused for being binary or not UTF-8.
#[test]
fn a_source_at_the_content_limit_is_read_and_one_byte_more_is_not() {
    const LIMIT: u64 = 64 * 1024;
    let root = test_directory("content-limit-boundary");
    fs::write(root.join("exactly.txt"), "a".repeat(LIMIT as usize)).unwrap();
    fs::write(root.join("one-more.txt"), "a".repeat(LIMIT as usize + 1)).unwrap();
    let repository = RecordingRepository::default();
    let policy = IndexPolicy::new(Vec::new(), LIMIT);

    let report = InitialScanService::new(
        &DescriptorRelativeWalker,
        &repository,
        &policy,
        &analysing_everything(),
    )
    .execute(&resolved(&DescriptorRelativeWalker, &root))
    .unwrap();

    assert_eq!(report.indexed_files(), 1, "the file at the limit is read");
    assert_eq!(
        report.oversize_files(),
        1,
        "the file past the limit is counted as oversize"
    );
    assert_eq!(
        report.content_skipped_files(),
        1,
        "and as content that was not indexed"
    );
    assert!(report.failures().is_empty(), "{:?}", report.failures());

    let published = repository.commits.into_inner();
    let oversize = published
        .iter()
        .find(|commit| commit.location().display_name() == "one-more.txt")
        .expect("the oversize source is still published by its metadata");
    assert!(
        oversize.evidence().is_none(),
        "no content of an oversize source may be published"
    );
    let at_limit = published
        .iter()
        .find(|commit| commit.location().display_name() == "exactly.txt")
        .expect("the source at the limit is published");
    assert!(at_limit.evidence().is_some(), "its content is indexed");
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
