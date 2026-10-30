// The policy-driven walk shared by `index init` and `index reconcile`: every entry the
// walker offers is decided, described and, when allowed, read and handed to a sink.
use crate::domain::{
    AnalysisRequest, AnalysisRules, ExtractionProfile, FileKind, FileLocation, IndexPolicy,
    NormalizedPath, PolicyClass, PolicyRule, SourceCommit, generated_directory_marker,
};

use super::file_identity::FileIdentityHistory;
use super::initial_scan::IndexError;
use super::repository::RepositoryError;
use super::search_ports::SearchError;
use super::source_batch::{PublishedCounts, SourceBatchBuffer, SourceBatchLimits};
use super::source_walker::{
    Admission, ContentSkipReason, EvidenceRead, SourceEntry, SourceVisitor, SourceWalker,
};

/// Where a scan publishes the sources it has read, one bounded batch at a time. Storage
/// failures stop the scan.
pub trait SourceSink {
    fn commit_batch(&mut self, commits: Vec<SourceCommit>) -> Result<(), RepositoryError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReport {
    root: NormalizedPath,
    indexed_files: usize,
    indexed_chunks: usize,
    metadata_only_files: usize,
    content_skipped_files: usize,
    oversize_files: usize,
    no_text_files: usize,
    unstable_sources: usize,
    undated_sources: usize,
    excluded_paths: usize,
    generated_directories: Vec<PolicyRule>,
    unrepresentable_names: usize,
    failures: Vec<String>,
    search_index_failures: Vec<SearchError>,
}

impl ScanReport {
    pub(super) fn empty(root: NormalizedPath) -> Self {
        Self {
            root,
            indexed_files: 0,
            indexed_chunks: 0,
            metadata_only_files: 0,
            content_skipped_files: 0,
            oversize_files: 0,
            no_text_files: 0,
            unstable_sources: 0,
            undated_sources: 0,
            excluded_paths: 0,
            generated_directories: Vec::new(),
            unrepresentable_names: 0,
            failures: Vec::new(),
            search_index_failures: Vec::new(),
        }
    }

    pub fn root(&self) -> &NormalizedPath {
        &self.root
    }

    /// Files whose metadata and content were indexed.
    pub fn indexed_files(&self) -> usize {
        self.indexed_files
    }

    pub fn indexed_chunks(&self) -> usize {
        self.indexed_chunks
    }

    /// Every file this pass published, whatever the policy allowed of it: the ones whose
    /// content was indexed, the ones limited to metadata, the ones whose content could not
    /// be read and the ones that held no text. Each published source is counted in exactly
    /// one of the four, so this is how many files the graph knows from this pass — the
    /// number `status` reads back out of the graph afterwards.
    ///
    /// Unstable and excluded sources are not in it, because neither was published.
    pub fn published_files(&self) -> usize {
        self.indexed_files
            + self.metadata_only_files
            + self.content_skipped_files
            + self.no_text_files
    }

    /// Files the Metadata policy limited to name, size, type and dates.
    pub fn metadata_only_files(&self) -> usize {
        self.metadata_only_files
    }

    /// Included files whose content could not be indexed: symlinks, special files, binary
    /// or oversized content. Their location is still recorded.
    pub fn content_skipped_files(&self) -> usize {
        self.content_skipped_files
    }

    /// Of those, the ones larger than the policy reads. Their content was never opened.
    pub fn oversize_files(&self) -> usize {
        self.oversize_files
    }

    /// Files that were read whole and hold no text: empty files, and files with nothing in
    /// them but whitespace. Their version is published with no chunks, so a file whose text
    /// was deleted loses its documents on this pass rather than answering search with text
    /// that is no longer in it. Nothing could be extracted, so nothing was — and that is a
    /// count, not a failure anybody has to go and read.
    pub fn no_text_files(&self) -> usize {
        self.no_text_files
    }

    /// Included files that changed between being described and being read. Nothing about
    /// them is published; a later scan sees a stable file again.
    pub fn unstable_sources(&self) -> usize {
        self.unstable_sources
    }

    /// Sources whose filesystem records no creation time. Their identity is the kernel's
    /// device and inode pair alone, and that pair is recycled, so for these sources a newly
    /// created file cannot be told from the deleted one whose inode number it took. The
    /// protection is off for exactly this many sources, and saying so is the point of the
    /// counter.
    pub fn undated_sources(&self) -> usize {
        self.undated_sources
    }

    /// Entries the Exclude policy kept the scanner from inspecting; their paths are not
    /// reported.
    pub fn excluded_paths(&self) -> usize {
        self.excluded_paths
    }

    /// Directories skipped whole because they carry a marker saying a tool made them. They
    /// are counted apart from excluded paths: an exclusion is a rule somebody can read in
    /// the policy, while this one is the directory's own admission about itself.
    pub fn generated_directories(&self) -> usize {
        self.generated_directories.len()
    }

    /// Those same directories as Exclude rules, for the steps that run after the walk.
    ///
    /// A run has to add these to its policy before it decides anything about paths it did
    /// not visit. Skipping a directory and excluding it are the same statement — "we do not
    /// look here" — and the one place that treated them as two declared 727 files on disk
    /// missing.
    pub fn generated_directory_rules(&self) -> &[PolicyRule] {
        &self.generated_directories
    }

    /// Entries whose names are not valid UTF-8. They are excluded before any filesystem
    /// call and never named.
    pub fn unrepresentable_names(&self) -> usize {
        self.unrepresentable_names
    }

    pub fn failed_paths(&self) -> usize {
        self.failures.len()
    }

    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    /// What stopped the search index from being kept up to date during this scan.
    ///
    /// Apart from `failures`, and deliberately: a failed path means the walk cannot vouch
    /// for what is on disk, which makes a reconciliation incomplete, while an index that
    /// was not written means a derivative fell behind. The graph is whole either way.
    pub fn search_index_failures(&self) -> &[SearchError] {
        &self.search_index_failures
    }

    pub(super) fn record_search_index_failures(&mut self, failures: Vec<SearchError>) {
        self.search_index_failures = failures;
    }

    /// Credits one published source. Called only after the batch carrying it committed.
    pub(super) fn record_published(&mut self, counts: PublishedCounts) {
        self.indexed_files += counts.indexed_files;
        self.indexed_chunks += counts.indexed_chunks;
        self.metadata_only_files += counts.metadata_only_files;
        self.content_skipped_files += counts.content_skipped_files;
        self.oversize_files += counts.oversize_files;
        self.no_text_files += counts.no_text_files;
    }
}

/// Walks `root` under `policy` (already granted to the root) and publishes through `sink`
/// in bounded batches. Whatever was read before the walk ended is published either way:
/// a source the scan saw but never committed would keep its old sighting epoch, and a
/// later finalization could then declare it missing.
pub(super) fn scan_with_policy<Sink: SourceSink>(
    walker: &impl SourceWalker,
    root: &NormalizedPath,
    policy: &IndexPolicy,
    analysis_rules: &AnalysisRules,
    identities: &dyn FileIdentityHistory,
    sink: Sink,
) -> Result<ScanReport, IndexError> {
    let mut visitor = PolicyScan {
        policy,
        analysis_rules,
        identities,
        sink,
        report: ScanReport::empty(root.clone()),
        buffer: SourceBatchBuffer::new(SourceBatchLimits::production()),
    };
    let walked = walker.walk(root, &mut visitor);
    let flushed = visitor.flush();
    match (walked, flushed) {
        (Ok(()), Ok(())) => Ok(visitor.report),
        (Err(walk_error), Ok(())) => Err(walk_error),
        (Ok(()), Err(flush_error)) => Err(flush_error),
        (Err(walk_error), Err(flush_error)) => Err(IndexError::Source(format!(
            "{walk_error}; publishing the sources read so far also failed: {flush_error}"
        ))),
    }
}

/// Applies the policy to every entry the walker offers and persists what it allows.
struct PolicyScan<'a, Sink> {
    policy: &'a IndexPolicy,
    analysis_rules: &'a AnalysisRules,
    identities: &'a dyn FileIdentityHistory,
    sink: Sink,
    report: ScanReport,
    buffer: SourceBatchBuffer,
}

impl<Sink: SourceSink> PolicyScan<'_, Sink> {
    /// Which file this entry is, before anything is described under it. The kernel's
    /// device and inode pair is only a question here: the graph answers what it already
    /// holds under that pair, and the domain rule decides whether this observation is that
    /// file or a new one that was handed its recycled inode number.
    fn resolve_identity(&mut self, entry: &dyn SourceEntry) -> Result<String, IndexError> {
        let observed = entry.observed_identity()?;
        if !observed.has_birth_time() {
            self.report.undated_sources += 1;
        }
        let known = self
            .identities
            .known_identity(&observed.base_id())
            .map_err(IndexError::Repository)?;
        Ok(observed.resolve(known))
    }

    fn describe_only(&mut self, entry: &dyn SourceEntry) -> Result<Option<Published>, IndexError> {
        let file_id = self.resolve_identity(entry)?;
        let location = entry.location(&file_id)?;
        Ok(Some((
            SourceCommit::metadata_only(location),
            PublishedCounts::metadata_only(),
        )))
    }

    /// `None` means the source was unstable: its described location would not match the
    /// content that was read, so neither is published.
    fn describe_and_read(
        &mut self,
        entry: &dyn SourceEntry,
    ) -> Result<Option<Published>, IndexError> {
        let file_id = self.resolve_identity(entry)?;
        let location = entry.location(&file_id)?;
        // Decided from the described size, so a file larger than the policy reads is never
        // opened at all.
        if !content_is_readable(&location, self.policy.max_content_bytes()) {
            return Ok(Some((
                SourceCommit::metadata_only(location),
                PublishedCounts::oversize(),
            )));
        }

        match entry.read_evidence(&location, self.policy.max_content_bytes())? {
            EvidenceRead::Commit(evidence) => {
                // A version with no chunks is a source with nothing in it to extract: an
                // empty file, or one with nothing but whitespace. It is published like any
                // other, so the file stops being current at whatever version last held
                // text, and counted apart, because "read and holds nothing" is not the
                // same fact as "read and indexed".
                let counts = match evidence.chunks().len() {
                    0 => PublishedCounts::no_text(),
                    chunks => PublishedCounts::indexed(chunks),
                };
                // The rules are asked here, where the path is still in hand and before a
                // job could exist: a revision nobody asked to analyse never becomes a row.
                let analysis = match self.analysis_rules.cover(location.path()) {
                    true => AnalysisRequest::Requested,
                    false => AnalysisRequest::NotRequested,
                };
                SourceCommit::with_evidence(location, evidence, analysis)
                    .map(|commit| Some((commit, counts)))
                    .map_err(|error| IndexError::Source(error.to_string()))
            }
            EvidenceRead::Skipped(ContentSkipReason::ChangedWhileReading) => {
                self.report.unstable_sources += 1;
                Ok(None)
            }
            EvidenceRead::Skipped(_) => Ok(Some((
                SourceCommit::metadata_only(location),
                PublishedCounts::content_skipped(),
            ))),
        }
    }

    /// Buffers one source, publishing the batch before it would cross a limit and again
    /// once the limit is reached, so the buffer never holds more than one batch.
    fn buffer(&mut self, published: Published) -> Result<(), IndexError> {
        let (commit, counts) = published;
        if self.buffer.would_exceed(&commit) {
            self.flush()?;
        }
        self.buffer.push(commit, counts);
        if self.buffer.is_full() {
            self.flush()?;
        }
        Ok(())
    }

    /// Publishes the buffered sources in one transaction and only then credits them to
    /// the report: a batch that did not land is a batch the report never saw.
    fn flush(&mut self) -> Result<(), IndexError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let buffered = self.buffer.take();
        let mut commits = Vec::with_capacity(buffered.len());
        let mut published = Vec::with_capacity(buffered.len());
        for (commit, counts) in buffered {
            commits.push(commit);
            published.push(counts);
        }
        self.sink
            .commit_batch(commits)
            .map_err(IndexError::Repository)?;
        for counts in published {
            counts.apply(&mut self.report);
        }
        Ok(())
    }
}

/// One source ready to publish, with what its publication adds to the report.
type Published = (SourceCommit, PublishedCounts);

/// Only regular files a profile is willing to open are opened; the reader checks again,
/// because the file may grow in between.
///
/// The size question goes through the profile rather than being answered here, because the
/// answer is not the same for every kind of file: for text the file is the content, and for a
/// transcript the content is the conversation inside it.
fn content_is_readable(location: &FileLocation, max_content_bytes: u64) -> bool {
    location.kind() == FileKind::RegularFile
        && ExtractionProfile::of_path(location.path())
            .may_read(location.size_bytes(), max_content_bytes)
}

impl<Sink: SourceSink> SourceVisitor for PolicyScan<'_, Sink> {
    fn admit(&mut self, path: &NormalizedPath) -> Admission {
        if self.policy.decide(path).class() == PolicyClass::Exclude {
            self.report.excluded_paths += 1;
            return Admission::Skip;
        }
        Admission::Inspect
    }

    fn admit_directory(
        &mut self,
        path: &NormalizedPath,
        holds: &dyn Fn(&str) -> bool,
    ) -> Admission {
        if generated_directory_marker(holds).is_some() {
            self.report
                .generated_directories
                .push(IndexPolicy::generated_directory_rule(path));
            return Admission::Skip;
        }
        Admission::Inspect
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        let outcome = match self.policy.decide(entry.path()).class() {
            PolicyClass::Exclude => {
                self.report.excluded_paths += 1;
                return Ok(());
            }
            PolicyClass::Metadata => self.describe_only(entry),
            PolicyClass::Include => self.describe_and_read(entry),
        };

        match outcome {
            Ok(Some(published)) => self.buffer(published),
            Ok(None) => Ok(()),
            // A single unreadable entry is reported; a storage failure stops the scan.
            Err(IndexError::Source(message)) => {
                self.report
                    .failures
                    .push(format!("{}: {message}", entry.path()));
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn skip_unrepresentable_name(&mut self) {
        self.report.unrepresentable_names += 1;
    }

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        if self.policy.decide(path).class() == PolicyClass::Exclude {
            self.report
                .failures
                .push(format!("an excluded path could not be inspected: {error}"));
            return;
        }
        self.report.failures.push(format!("{path}: {error}"));
    }
}
