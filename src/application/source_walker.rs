// Ports through which the index use case sees a source tree. The adapter walks entries and
// exposes each one lazily, so the use case can apply the policy before any metadata is
// read and before any content is opened.
use std::path::Path;

use crate::domain::{EvidenceCommit, FileLocation, NormalizedPath, ObservedFileIdentity};

use super::initial_scan::IndexError;

pub trait SourceWalker {
    /// Resolves the user's root to its canonical normalized path and checks that it is a
    /// directory. Policy is evaluated against this path, so a symlinked root cannot reach
    /// an excluded directory under another name.
    fn resolve_root(&self, root: &Path) -> Result<NormalizedPath, IndexError>;

    /// Walks `root` in a deterministic order without following symlinks. Every entry,
    /// the root included, is offered to `admit` before the walker inspects it.
    fn walk(
        &self,
        root: &NormalizedPath,
        visitor: &mut dyn SourceVisitor,
    ) -> Result<(), IndexError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Inspect,
    /// The walker never stats, opens or descends into the entry.
    Skip,
}

pub trait SourceVisitor {
    fn admit(&mut self, path: &NormalizedPath) -> Admission;

    /// Called for a directory `admit` accepted, before it is listed. `holds` answers
    /// whether the directory holds a regular file of that name — one lookup per question,
    /// never a listing — so a visitor can decide from a marker the directory carries
    /// instead of from its name.
    ///
    /// The default keeps listing. It is the answer for a visitor that is not deciding what
    /// to index at all, which is what the test doubles and the diagnostics are; the two
    /// visitors that do decide — the policy scan and the context proposal — both override
    /// it, because a rule that one of them applied and the other did not would propose
    /// contexts out of directories it would then refuse to index.
    fn admit_directory(
        &mut self,
        path: &NormalizedPath,
        holds: &dyn Fn(&str) -> bool,
    ) -> Admission {
        let _ = (path, holds);
        Admission::Inspect
    }

    /// Called for every admitted entry that is not a directory.
    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError>;

    /// Called for an entry whose name is not valid UTF-8. The policy works on strings, so
    /// such an entry is never stat-ed, opened or named; only its count is reported.
    fn skip_unrepresentable_name(&mut self);

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError);
}

pub trait SourceEntry {
    fn path(&self) -> &NormalizedPath;

    /// What the filesystem reports about the entry's identity: the device and inode number
    /// the kernel names it by, and the birth time where the filesystem records one. The
    /// adapter reports; which identity the observation is published under is decided by
    /// the domain rule against what the graph already holds.
    fn observed_identity(&self) -> Result<ObservedFileIdentity, IndexError>;

    /// Metadata of the entry itself, never of a symlink target, described under the
    /// already resolved `file_id`. Does not open the entry.
    fn location(&self, file_id: &str) -> Result<FileLocation, IndexError>;

    /// Opens the entry without following symlinks and extracts plain-text evidence.
    ///
    /// `expected` must be exactly what `location()` returned for this entry under the same
    /// resolved identity: it becomes the location of the resulting `SourceCommit`, so a
    /// location that describes anything else is rejected with an error instead of being
    /// paired with content.
    /// Stability is judged against the adapter's own exact snapshot of the entry
    /// (identity, size and full-precision modification time) before and during the read;
    /// any drift yields `ChangedWhileReading`.
    fn read_evidence(
        &self,
        expected: &FileLocation,
        max_content_bytes: u64,
    ) -> Result<EvidenceRead, IndexError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceRead {
    Commit(EvidenceCommit),
    Skipped(ContentSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSkipReason {
    TooLarge,
    Binary,
    NotUtf8,
    /// The file no longer matches its described location; nothing about it is published.
    ChangedWhileReading,
}
