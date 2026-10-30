// Bounded batches of prepared sources: the unit in which a scan publishes what it read.
//
// One batch is one write transaction. Several sources therefore become visible together,
// but a source is never split across batches, so it is still published whole or not at
// all. The limits are internal constants, not configuration: they bound how much work one
// transaction carries and how much a scan holds in memory, so memory never grows with the
// size of the root.
use crate::domain::{ReconcileRun, SourceCommit};

use super::policy_scan::ScanReport;
use super::repository::RepositoryError;

/// How much one transaction may carry. The two bounds answer different questions: the
/// count bounds how many writes a transaction performs, and the payload bounds how much a
/// scan holds in memory while it fills one, since chunk text dominates the size of a
/// prepared source. Together they keep the memory of a scan independent of the size of the
/// tree it walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceBatchLimits {
    sources: usize,
    payload_bytes: usize,
}

impl SourceBatchLimits {
    pub const fn new(sources: usize, payload_bytes: usize) -> Self {
        Self {
            sources,
            payload_bytes,
        }
    }

    /// The limits every scan uses. One compile-time value, chosen by measurement: see
    /// ADR 0006. Tests construct other limits directly, and a running Crosskit reads
    /// nothing about batching from its environment.
    #[cfg(not(feature = "batch-limit-benchmark"))]
    pub fn production() -> Self {
        Self::new(PRODUCTION_SOURCES, PRODUCTION_PAYLOAD_BYTES)
    }

    /// The same, for a binary built by the batch limit benchmark: the limits come from
    /// the environment of the compiler, not of the running program, so a measured binary
    /// still has exactly one fixed pair of limits. A value that is missing keeps the
    /// production one; a value that cannot be used is refused rather than silently
    /// replaced, because a benchmark that measured something other than it printed would
    /// be worse than no measurement.
    #[cfg(feature = "batch-limit-benchmark")]
    pub fn production() -> Self {
        Self::new(
            compiled_in(
                "CROSSKIT_BATCH_LIMIT_SOURCES",
                option_env!("CROSSKIT_BATCH_LIMIT_SOURCES"),
                PRODUCTION_SOURCES,
            ),
            compiled_in(
                "CROSSKIT_BATCH_LIMIT_PAYLOAD_MIB",
                option_env!("CROSSKIT_BATCH_LIMIT_PAYLOAD_MIB"),
                0,
            )
            .checked_mul(1024 * 1024)
            .expect("the payload limit in MiB overflows a byte count"),
        )
    }

    /// At most this many sources are published by one transaction.
    pub fn sources(&self) -> usize {
        self.sources
    }

    /// At most this much chunk text is carried by one transaction. A single source larger
    /// than this is published alone rather than waiting for company it would never get.
    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }
}

const PRODUCTION_SOURCES: usize = 100;
const PRODUCTION_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

/// One limit as the compiler saw it. `fallback` of zero means the production payload,
/// which is expressed in bytes rather than MiB.
#[cfg(feature = "batch-limit-benchmark")]
fn compiled_in(name: &str, value: Option<&str>, fallback: usize) -> usize {
    let Some(value) = value else {
        return if fallback == 0 {
            PRODUCTION_PAYLOAD_BYTES / (1024 * 1024)
        } else {
            fallback
        };
    };
    match value.parse::<usize>() {
        Ok(parsed) if parsed > 0 => parsed,
        _ => panic!("{name} must be a whole number above zero, received {value:?}"),
    }
}

/// Publishes prepared sources in bounded transactions. Narrow on purpose: it grants no
/// read access and no run bookkeeping, only the right to publish what a scan has read.
pub trait SourceBatchWriter {
    /// Publishes `commits` in one transaction, in the given order. Either every source in
    /// the batch becomes visible or none of them does.
    fn commit_batch(&self, commits: Vec<SourceCommit>) -> Result<(), RepositoryError>;

    /// The same, on behalf of `run`, whose identity and `running` status are verified
    /// inside that transaction before anything is written.
    fn commit_observed_batch(
        &self,
        run: &ReconcileRun,
        commits: Vec<SourceCommit>,
    ) -> Result<(), RepositoryError>;
}

/// What the report gains when one buffered source is published. The counters are applied
/// only after the batch lands, so a report never claims a source that no transaction
/// committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PublishedCounts {
    pub(super) indexed_files: usize,
    pub(super) indexed_chunks: usize,
    pub(super) metadata_only_files: usize,
    pub(super) content_skipped_files: usize,
    pub(super) oversize_files: usize,
    pub(super) no_text_files: usize,
}

impl PublishedCounts {
    /// Metadata and content were indexed.
    pub(super) fn indexed(chunks: usize) -> Self {
        Self {
            indexed_files: 1,
            indexed_chunks: chunks,
            ..Self::none()
        }
    }

    /// Nothing counted. Every constructor names only what it adds.
    fn none() -> Self {
        Self {
            indexed_files: 0,
            indexed_chunks: 0,
            metadata_only_files: 0,
            content_skipped_files: 0,
            oversize_files: 0,
            no_text_files: 0,
        }
    }

    /// The policy allowed metadata only.
    pub(super) fn metadata_only() -> Self {
        Self {
            metadata_only_files: 1,
            ..Self::none()
        }
    }

    /// Content was allowed but could not be indexed; the location is still published.
    pub(super) fn content_skipped() -> Self {
        Self {
            content_skipped_files: 1,
            ..Self::none()
        }
    }

    /// The whole content was read and holds no text. The version is published with no
    /// chunks, so this is not skipped content: it is content that says there is nothing in
    /// it. Counted apart from the binary and the oversized, which is what a reader needs —
    /// "nothing in it" is not "something this extractor cannot read".
    pub(super) fn no_text() -> Self {
        Self {
            no_text_files: 1,
            ..Self::none()
        }
    }

    /// The file is larger than the policy reads. Its content is never opened, and it is
    /// counted apart from content that was read and refused: a reader needs to tell "too
    /// big to index" from "not text".
    pub(super) fn oversize() -> Self {
        Self {
            content_skipped_files: 1,
            oversize_files: 1,
            ..Self::none()
        }
    }

    pub(super) fn apply(self, report: &mut ScanReport) {
        report.record_published(self);
    }
}

/// Sources read but not yet published, together with what their publication will report.
pub(super) struct SourceBatchBuffer {
    limits: SourceBatchLimits,
    buffered: Vec<(SourceCommit, PublishedCounts)>,
    payload_bytes: usize,
}

impl SourceBatchBuffer {
    pub(super) fn new(limits: SourceBatchLimits) -> Self {
        Self {
            limits,
            buffered: Vec::new(),
            payload_bytes: 0,
        }
    }

    /// Whether the buffer must be published before `commit` joins it. A source that is
    /// alone larger than the payload limit still joins an empty buffer, and `is_full` then
    /// asks for it to be published by itself.
    pub(super) fn would_exceed(&self, commit: &SourceCommit) -> bool {
        self.buffered.len() + 1 > self.limits.sources()
            || self.payload_bytes + payload_bytes(commit) > self.limits.payload_bytes()
    }

    pub(super) fn push(&mut self, commit: SourceCommit, published: PublishedCounts) {
        self.payload_bytes += payload_bytes(&commit);
        self.buffered.push((commit, published));
    }

    /// Whether the buffer has reached a limit and must be published now.
    pub(super) fn is_full(&self) -> bool {
        self.buffered.len() >= self.limits.sources()
            || self.payload_bytes >= self.limits.payload_bytes()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.buffered.is_empty()
    }

    /// Takes everything buffered, in the order the walker offered it.
    pub(super) fn take(&mut self) -> Vec<(SourceCommit, PublishedCounts)> {
        self.payload_bytes = 0;
        std::mem::take(&mut self.buffered)
    }
}

/// The chunk text one source carries. Metadata-only sources cost nothing to publish.
fn payload_bytes(commit: &SourceCommit) -> usize {
    commit.evidence().map_or(0, |evidence| {
        evidence
            .chunks()
            .iter()
            .map(|chunk| chunk.text().len())
            .sum()
    })
}

#[cfg(test)]
mod tests {
    use crate::domain::AnalysisRequest;

    use super::{PublishedCounts, SourceBatchBuffer, SourceBatchLimits};
    use crate::domain::{
        ChunkScript, ContentChunk, EvidenceCommit, FileKind, FileLocation, FileObservation,
        FileVersion, NormalizedPath, SourceCommit,
    };

    fn location(index: usize) -> FileLocation {
        FileLocation::observed(
            format!("file:{index}"),
            NormalizedPath::from_absolute(&format!("/scan/{index}.md")).unwrap(),
            FileKind::RegularFile,
            5,
            100,
            200,
        )
        .unwrap()
    }

    fn metadata_commit(index: usize) -> SourceCommit {
        SourceCommit::metadata_only(location(index))
    }

    /// A source whose single chunk carries `text_bytes` of text.
    fn evidence_commit(index: usize, text_bytes: usize) -> SourceCommit {
        let file_id = format!("file:{index}");
        let version = FileVersion::new(format!("{file_id}@hash"), &file_id, "hash", 100).unwrap();
        let observation =
            FileObservation::new(format!("observation:{index}"), &file_id, version.id(), 100)
                .unwrap();
        let text = "x".repeat(text_bytes);
        let chunk = ContentChunk::new(
            format!("chunk:{index}"),
            version.id(),
            0,
            0,
            text_bytes as u64,
            "text-hash",
            text,
            ChunkScript::Latin,
            "plain-text",
            "plain-text",
            "1",
            "config:1",
            version.content_hash(),
            100,
        )
        .unwrap();
        SourceCommit::with_evidence(
            location(index),
            EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
            AnalysisRequest::Requested,
        )
        .unwrap()
    }

    /// Limits small enough to reach in a test, and far apart so neither bound can be
    /// mistaken for the other.
    fn limits() -> SourceBatchLimits {
        SourceBatchLimits::new(4, 1_024)
    }

    #[test]
    fn the_buffer_fills_up_to_the_source_limit_and_keeps_the_walker_order() {
        let limits = limits();
        let mut buffer = SourceBatchBuffer::new(limits);
        assert!(buffer.is_empty());

        for index in 0..limits.sources() {
            assert!(!buffer.would_exceed(&metadata_commit(index)));
            buffer.push(metadata_commit(index), PublishedCounts::metadata_only());
            assert_eq!(buffer.is_full(), index + 1 == limits.sources());
        }
        assert!(buffer.would_exceed(&metadata_commit(limits.sources())));

        let taken = buffer.take();
        assert_eq!(taken.len(), limits.sources());
        for (index, (commit, _)) in taken.iter().enumerate() {
            assert_eq!(commit.location().file_id(), format!("file:{index}"));
        }
        assert!(buffer.is_empty());
        assert!(!buffer.is_full());
    }

    #[test]
    fn the_source_limit_alone_can_fill_a_buffer_that_carries_no_payload() {
        let limits = limits();
        let mut buffer = SourceBatchBuffer::new(limits);

        // Metadata-only sources carry no chunk text at all, so only the count can fill it.
        for index in 0..limits.sources() {
            buffer.push(metadata_commit(index), PublishedCounts::metadata_only());
        }

        assert!(buffer.is_full());
        assert_eq!(buffer.take().len(), limits.sources());
    }

    #[test]
    fn the_payload_limit_alone_can_fill_a_buffer_that_is_far_from_the_count() {
        let limits = limits();
        let mut buffer = SourceBatchBuffer::new(limits);

        buffer.push(
            evidence_commit(0, limits.payload_bytes() / 2),
            PublishedCounts::indexed(1),
        );
        assert!(!buffer.is_full(), "half the payload is not full");
        let second = evidence_commit(1, limits.payload_bytes() / 2);
        assert!(!buffer.would_exceed(&second));
        buffer.push(second, PublishedCounts::indexed(1));

        // Two sources of four, and the buffer is full on payload alone.
        assert!(buffer.is_full());
        assert!(buffer.would_exceed(&evidence_commit(2, 1)));
        assert_eq!(buffer.take().len(), 2);
        assert!(!buffer.is_full());
    }

    #[test]
    fn a_source_larger_than_the_payload_limit_asks_to_be_published_alone() {
        let limits = limits();
        let oversized = evidence_commit(0, limits.payload_bytes() + 1);
        let mut buffer = SourceBatchBuffer::new(limits);

        // It cannot join company, and an empty buffer still accepts it.
        assert!(buffer.would_exceed(&oversized));
        buffer.push(oversized, PublishedCounts::indexed(1));
        assert!(
            buffer.is_full(),
            "an oversized source must be published at once"
        );
        assert_eq!(buffer.take().len(), 1);
    }

    #[test]
    fn published_counts_describe_each_kind_of_source() {
        assert_eq!(
            PublishedCounts::indexed(3),
            PublishedCounts {
                indexed_files: 1,
                indexed_chunks: 3,
                ..PublishedCounts::none()
            }
        );
        assert_eq!(
            PublishedCounts::metadata_only(),
            PublishedCounts {
                metadata_only_files: 1,
                ..PublishedCounts::none()
            }
        );
        assert_eq!(
            PublishedCounts::content_skipped(),
            PublishedCounts {
                content_skipped_files: 1,
                ..PublishedCounts::none()
            }
        );
        // A file larger than the limit is counted twice over: as content that was skipped,
        // and as the reason it was.
        assert_eq!(
            PublishedCounts::oversize(),
            PublishedCounts {
                content_skipped_files: 1,
                oversize_files: 1,
                ..PublishedCounts::none()
            }
        );
        // And so is a file with nothing in it to extract, for the same reason.
        // A source with nothing in it is published, not skipped: its version lands with no
        // chunks, so it belongs to none of the counters above.
        assert_eq!(
            PublishedCounts::no_text(),
            PublishedCounts {
                no_text_files: 1,
                ..PublishedCounts::none()
            }
        );
    }
}

#[cfg(test)]
mod scan_tests {
    //! The buffer as the shared scan path uses it: grouping, order, the final flush and
    //! counters that appear only after a batch lands.
    use crate::domain::{AnalysisRules, PathPattern};
    use std::cell::RefCell;
    use std::path::Path;

    use super::SourceBatchLimits;
    use crate::application::policy_scan::{ScanReport, SourceSink, scan_with_policy};
    use crate::application::{
        Admission, EvidenceRead, FileIdentityHistory, IndexError, RepositoryError, SourceEntry,
        SourceVisitor, SourceWalker,
    };
    use crate::domain::{
        ChunkScript, ContentChunk, EvidenceCommit, FileKind, FileLocation, FileObservation,
        FileVersion, IndexPolicy, KnownIdentity, NormalizedPath, ObservedFileIdentity,
        SourceCommit,
    };

    const ROOT: &str = "/scan";

    /// The scan path uses the production limits, so the tests below describe what a real
    /// scan does rather than a configuration only they see.
    fn limits() -> SourceBatchLimits {
        SourceBatchLimits::production()
    }

    /// A tree of `count` text files; the file at `oversized` carries more chunk text than
    /// one batch may hold.
    struct FakeWalker {
        count: usize,
        oversized: Option<usize>,
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
            for index in 0..self.count {
                let path = root.join(&format!("note-{index:05}.md")).unwrap();
                if visitor.admit(&path) == Admission::Skip {
                    continue;
                }
                let text_bytes = if self.oversized == Some(index) {
                    limits().payload_bytes() + 1
                } else {
                    16
                };
                visitor.visit_entry(&FakeEntry { path, text_bytes })?;
            }
            Ok(())
        }
    }

    struct FakeEntry {
        path: NormalizedPath,
        text_bytes: usize,
    }

    impl SourceEntry for FakeEntry {
        fn path(&self) -> &NormalizedPath {
            &self.path
        }

        fn observed_identity(&self) -> Result<ObservedFileIdentity, IndexError> {
            Ok(ObservedFileIdentity::distinct_for_test(self.path.as_str()))
        }

        fn location(&self, file_id: &str) -> Result<FileLocation, IndexError> {
            FileLocation::observed(
                file_id,
                self.path.clone(),
                FileKind::RegularFile,
                self.text_bytes as u64,
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
            let version = FileVersion::new(
                format!("{}@hash", expected.file_id()),
                expected.file_id(),
                "hash",
                100,
            )
            .unwrap();
            let observation = FileObservation::new(
                format!("observation:{}", expected.file_id()),
                expected.file_id(),
                version.id(),
                100,
            )
            .unwrap();
            let chunk = ContentChunk::new(
                format!("chunk:{}", expected.file_id()),
                version.id(),
                0,
                0,
                self.text_bytes as u64,
                "text-hash",
                "x".repeat(self.text_bytes),
                ChunkScript::Latin,
                "plain-text",
                "plain-text",
                "1",
                "config:1",
                version.content_hash(),
                100,
            )
            .unwrap();
            Ok(EvidenceRead::Commit(
                EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
            ))
        }
    }

    /// Records every batch, and can be told to fail at a chosen one.
    #[derive(Default)]
    struct RecordingSink {
        batches: RefCell<Vec<Vec<String>>>,
        fail_at_batch: Option<usize>,
    }

    impl SourceSink for &RecordingSink {
        fn commit_batch(&mut self, commits: Vec<SourceCommit>) -> Result<(), RepositoryError> {
            if self.fail_at_batch == Some(self.batches.borrow().len()) {
                return Err(RepositoryError::Storage("disk full".to_string()));
            }
            self.batches.borrow_mut().push(
                commits
                    .iter()
                    .map(|commit| commit.location().path().file_name().to_string())
                    .collect(),
            );
            Ok(())
        }
    }

    /// A graph that has never seen any of these identities, so the batching tests stay
    /// about batching.
    struct UnseenIdentities;

    impl FileIdentityHistory for UnseenIdentities {
        fn known_identity(&self, _base_id: &str) -> Result<KnownIdentity, RepositoryError> {
            Ok(KnownIdentity::Unseen)
        }
    }

    fn scan(walker: &FakeWalker, sink: &RecordingSink) -> Result<ScanReport, IndexError> {
        scan_with_policy(
            walker,
            &NormalizedPath::from_absolute(ROOT).unwrap(),
            &IndexPolicy::new(vec![], 1024 * 1024 * 1024)
                .with_scan_root(&NormalizedPath::from_absolute(ROOT).unwrap()),
            &AnalysisRules::new(vec![PathPattern::parse("**/*").unwrap()], Vec::new()),
            &UnseenIdentities,
            sink,
        )
    }

    fn batch_sizes(sink: &RecordingSink) -> Vec<usize> {
        sink.batches.borrow().iter().map(Vec::len).collect()
    }

    #[test]
    fn sources_are_published_in_full_batches_and_a_final_partial_one() {
        let count = limits().sources() * 2 + 7;
        let sink = RecordingSink::default();

        let report = scan(
            &FakeWalker {
                count,
                oversized: None,
            },
            &sink,
        )
        .unwrap();

        assert_eq!(
            batch_sizes(&sink),
            [limits().sources(), limits().sources(), 7],
            "full batches, then whatever the final flush still holds"
        );
        assert_eq!(report.indexed_files(), count);
        assert_eq!(report.indexed_chunks(), count);
    }

    #[test]
    fn the_walker_order_survives_batching() {
        let count = limits().sources() + 3;
        let sink = RecordingSink::default();

        scan(
            &FakeWalker {
                count,
                oversized: None,
            },
            &sink,
        )
        .unwrap();

        let published: Vec<String> = sink.batches.borrow().iter().flatten().cloned().collect();
        let expected: Vec<String> = (0..count)
            .map(|index| format!("note-{index:05}.md"))
            .collect();
        assert_eq!(published, expected);
    }

    #[test]
    fn a_source_larger_than_the_payload_limit_travels_alone() {
        let sink = RecordingSink::default();

        scan(
            &FakeWalker {
                count: 5,
                oversized: Some(2),
            },
            &sink,
        )
        .unwrap();

        assert_eq!(
            batch_sizes(&sink),
            [2, 1, 2],
            "the oversized source is published by itself, between its neighbours"
        );
        assert_eq!(sink.batches.borrow()[1], ["note-00002.md"]);
    }

    #[test]
    fn a_failed_batch_stops_the_scan_and_credits_nothing() {
        let sink = RecordingSink {
            fail_at_batch: Some(1),
            ..RecordingSink::default()
        };

        let error = scan(
            &FakeWalker {
                count: limits().sources() + 5,
                oversized: None,
            },
            &sink,
        )
        .unwrap_err();

        assert_eq!(
            error,
            IndexError::Repository(RepositoryError::Storage("disk full".to_string()))
        );
        // The first batch was published; the second never was, and no report escapes to
        // claim its sources.
        assert_eq!(batch_sizes(&sink), [limits().sources()]);
    }
}
