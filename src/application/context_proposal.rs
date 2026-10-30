// Use case: walk a tree and draft the contexts it suggests, without writing anything.
//
// It reuses the walk the index uses, unchanged: the same descriptor-relative traversal that
// never follows a symlink, counts a directory it cannot read and never stats an entry the
// visitor refused. What differs is the visitor. This one publishes nothing, opens no
// database and opens exactly one kind of file — a repository's `.git/config`, and only to
// take the host of its remotes and the group of its `origin` out of it (ADR 0019, 0042).
use std::collections::BTreeMap;
use std::path::Path;

use crate::domain::{
    ContentChunk, ContextProposal, FileKind, GitMarker, HomePlace, NormalizedPath, Origin,
    ProjectMarker, RepositoryRemote, RepositorySignal, SubtreeEvidence, SubtreeVolume,
    TreeEvidence, WalkOutcome, generated_directory_marker,
};

use super::initial_scan::IndexError;
use super::source_walker::{Admission, EvidenceRead, SourceEntry, SourceVisitor, SourceWalker};
use super::subtree_volumes::{DirectoryTally, parent_of, roll_up};

/// The entry whose presence names a directory as a repository root.
const GIT_ENTRY: &str = ".git";
/// The only file this command ever opens.
const GIT_CONFIG: &str = "config";
/// How much of a `.git/config` is read before it is left alone. A configuration a tool
/// wrote is a few hundred bytes; anything of this size is not the file this rule is about,
/// and it is counted as unread rather than parsed.
const GIT_CONFIG_LIMIT_BYTES: u64 = 256 * 1024;
/// The walker describes an entry under an identity it is given. Nothing here is published,
/// so this one names the command and never leaves the process.
const PROPOSAL_ENTRY: &str = "context-proposal";

pub struct ContextProposalService<'a, Walker> {
    walker: &'a Walker,
}

impl<'a, Walker: SourceWalker> ContextProposalService<'a, Walker> {
    pub fn new(walker: &'a Walker) -> Self {
        Self { walker }
    }

    /// The draft for `root`. `home` is the person's home directory when it is known, and
    /// the standard layout of a home directory is read as evidence only for the children of
    /// that exact directory.
    pub fn execute(
        &self,
        root: &Path,
        home: Option<&NormalizedPath>,
    ) -> Result<ContextProposal, IndexError> {
        Ok(ContextProposal::assemble(self.evidence(root, home)?))
    }

    /// What the walk saw, before any rule drafts contexts from it. `setup` groups this its
    /// own way — by the group of `origin`, with the conversations beside it — while `contexts
    /// propose` drafts by host; both stand on one walk.
    pub fn evidence(
        &self,
        root: &Path,
        home: Option<&NormalizedPath>,
    ) -> Result<TreeEvidence, IndexError> {
        let root = self.walker.resolve_root(root)?;
        let mut scan = ProposalScan::new(root.clone());
        self.walker.walk(&root, &mut scan)?;
        Ok(scan.into_evidence(home))
    }
}

/// What made a directory a candidate for a context of its own.
#[derive(Debug, Default)]
struct CandidateSignals {
    git: Option<GitMarker>,
    remote: Option<RepositoryRemote>,
    origin: Option<Origin>,
    markers: Vec<ProjectMarker>,
}

impl CandidateSignals {
    fn repository(&self) -> Option<RepositorySignal> {
        let marker = self.git?;
        let remote = self.remote.clone().unwrap_or(RepositoryRemote::NoConfig);
        Some(RepositorySignal::new(marker, remote, self.origin.clone()))
    }
}

/// Collects the evidence one walk produces. Tallies are kept per directory and rolled up
/// afterwards, so what is counted never depends on the order the walker offers entries in.
struct ProposalScan {
    root: NormalizedPath,
    directories: BTreeMap<String, DirectoryTally>,
    candidates: BTreeMap<String, CandidateSignals>,
    walk: WalkOutcome,
}

impl ProposalScan {
    fn new(root: NormalizedPath) -> Self {
        Self {
            root,
            directories: BTreeMap::new(),
            candidates: BTreeMap::new(),
            walk: WalkOutcome::default(),
        }
    }

    fn signals_at(&mut self, directory: &str) -> &mut CandidateSignals {
        self.candidates.entry(directory.to_string()).or_default()
    }

    fn count_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        // The walker's own description of the entry, taken from the `lstat` it already
        // made. The identity it is described under is a placeholder: nothing here is
        // published, so nothing needs a real one.
        let location = match entry.location(PROPOSAL_ENTRY) {
            Ok(location) => location,
            Err(IndexError::Source(message)) => {
                self.walk
                    .failures
                    .push(format!("{}: {message}", entry.path()));
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        match location.kind() {
            FileKind::RegularFile => {}
            FileKind::Symlink => self.walk.symlinks += 1,
            FileKind::Special => self.walk.special_entries += 1,
        }
        let bytes = match location.kind() {
            FileKind::RegularFile => location.size_bytes(),
            // What a symlink or a device measures is not content anybody would index.
            _ => 0,
        };
        let directory = parent_of(entry.path().as_str());
        match self.directories.get_mut(directory) {
            Some(tally) => {
                tally.files += 1;
                tally.bytes += bytes;
            }
            None => {
                self.directories
                    .insert(directory.to_string(), DirectoryTally { files: 1, bytes });
            }
        }
        Ok(())
    }

    /// Takes the remote hosts and the owner of `origin` out of one `.git/config`, and
    /// nothing else out of it.
    fn read_remote(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        let Some(repository) = repository_of_git_config(entry.path()) else {
            return Ok(());
        };
        match git_config_text(entry)? {
            Some(text) => {
                let signals = self.signals_at(repository.as_str());
                signals.remote = Some(RepositoryRemote::of_git_config(&text));
                signals.origin = Some(Origin::of_git_config(&text));
            }
            None => self.walk.unreadable_git_configs += 1,
        }
        Ok(())
    }

    /// The rolled-up evidence: one entry per directory that is either a candidate or a
    /// child of the root, each carrying the volume of its whole subtree.
    fn into_evidence(self, home: Option<&NormalizedPath>) -> TreeEvidence {
        let Self {
            root,
            directories,
            candidates,
            walk,
        } = self;
        let root_text = root.as_str().to_string();
        let is_home = home == Some(&root);

        let volumes = roll_up(
            &root_text,
            &directories,
            candidates.keys().map(String::as_str),
        );

        let whole = volumes.get(root_text.as_str()).copied().unwrap_or_default();
        let loose_at_the_root = directories
            .get(root_text.as_str())
            .map(|tally| SubtreeVolume::new(tally.files, tally.bytes, 1))
            .unwrap_or_default();
        // The root is described only when it carries evidence of its own; otherwise it is
        // the frame of the walk rather than something the draft has anything to say about.
        let subtrees = volumes
            .into_iter()
            .filter(|(path, _)| *path != root_text || candidates.contains_key(*path))
            .map(|(path, volume)| {
                let signals = candidates.get(path);
                let path = NormalizedPath::from_absolute(path)
                    .expect("the walker produced this path, so it begins with `/`");
                let home_place = home_place_of(&path, &root, is_home);
                SubtreeEvidence::new(
                    path,
                    volume,
                    signals.and_then(CandidateSignals::repository),
                    signals
                        .map(|signals| signals.markers.clone())
                        .unwrap_or_default(),
                    home_place,
                )
            })
            .collect();

        TreeEvidence {
            root,
            whole,
            loose_at_the_root,
            subtrees,
            walk,
        }
    }
}

impl SourceVisitor for ProposalScan {
    fn admit(&mut self, path: &NormalizedPath) -> Admission {
        if *path == self.root {
            return Admission::Inspect;
        }
        if inside_a_git_entry(path) {
            // The object store, the hooks, the index and the logs are never walked: one
            // file of a repository's own metadata is the whole exception.
            return if path.file_name() == GIT_CONFIG {
                Admission::Inspect
            } else {
                Admission::Skip
            };
        }
        if path.file_name() == GIT_ENTRY
            && let Some(repository) = path.parent()
        {
            // A `.git` that turns out not to be a directory says so when it arrives as an
            // entry; until then it is the directory the walker is about to descend into.
            self.signals_at(repository.as_str()).git = Some(GitMarker::Directory);
        }
        Admission::Inspect
    }

    /// The same rule the policy scan applies, for the same reason: a directory a tool made
    /// is not evidence of what a person keeps here, and proposing a context around one
    /// would name a directory the index then refuses to read.
    fn admit_directory(
        &mut self,
        _path: &NormalizedPath,
        holds: &dyn Fn(&str) -> bool,
    ) -> Admission {
        if generated_directory_marker(holds).is_some() {
            self.walk.generated_directories += 1;
            return Admission::Skip;
        }
        Admission::Inspect
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        let path = entry.path().clone();
        if inside_a_git_entry(&path) {
            return self.read_remote(entry);
        }
        if path.file_name() == GIT_ENTRY {
            if let Some(repository) = path.parent() {
                self.signals_at(repository.as_str()).git = Some(GitMarker::NotADirectory);
            }
            return Ok(());
        }
        if let Some(marker) = ProjectMarker::of_file_name(path.file_name())
            && let Some(project) = path.parent()
        {
            let markers = &mut self.signals_at(project.as_str()).markers;
            if !markers.contains(&marker) {
                markers.push(marker);
            }
        }
        self.count_entry(entry)
    }

    fn skip_unrepresentable_name(&mut self) {
        self.walk.unrepresentable_names += 1;
    }

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        match error {
            IndexError::NotPermitted(_) => self.walk.closed_by_the_system += 1,
            error => self.walk.failures.push(format!("{path}: {error}")),
        }
    }
}

/// The text of one `.git/config`, or nothing when the file is not one this command reads: not
/// a regular file, larger than the limit, or not text. It goes no further than the two rules
/// that take a host and an owner out of it.
fn git_config_text(entry: &dyn SourceEntry) -> Result<Option<String>, IndexError> {
    let location = entry.location(PROPOSAL_ENTRY)?;
    if location.kind() != FileKind::RegularFile || location.size_bytes() > GIT_CONFIG_LIMIT_BYTES {
        return Ok(None);
    }
    match entry.read_evidence(&location, GIT_CONFIG_LIMIT_BYTES)? {
        EvidenceRead::Commit(commit) => {
            let mut chunks: Vec<&ContentChunk> = commit.chunks().iter().collect();
            chunks.sort_by_key(|chunk| chunk.ordinal());
            Ok(Some(chunks.into_iter().map(ContentChunk::text).collect()))
        }
        EvidenceRead::Skipped(_) => Ok(None),
    }
}

/// Whether this path lies directly inside an entry named `.git`.
fn inside_a_git_entry(path: &NormalizedPath) -> bool {
    let text = path.as_str();
    match text.rfind('/') {
        Some(cut) => text[..cut].ends_with("/.git"),
        None => false,
    }
}

/// The repository a `.git/config` belongs to.
fn repository_of_git_config(path: &NormalizedPath) -> Option<NormalizedPath> {
    if path.file_name() != GIT_CONFIG {
        return None;
    }
    path.parent()?.parent()
}

/// The place a directory holds in the standard home layout — only for a direct child of the
/// home directory itself, and only when that is the directory being walked.
fn home_place_of(path: &NormalizedPath, root: &NormalizedPath, is_home: bool) -> Option<HomePlace> {
    if !is_home || path.parent().as_ref() != Some(root) {
        return None;
    }
    HomePlace::of_home_child(path.file_name())
}

#[cfg(test)]
mod tests {
    use super::ProposalScan;
    use crate::application::IndexError;
    use crate::application::source_walker::SourceVisitor;
    use crate::domain::NormalizedPath;

    fn path(value: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(value).unwrap()
    }

    /// A directory the system keeps closed is counted by the kind of its failure; any other
    /// failure of the very same directory is still named.
    #[test]
    fn a_directory_the_system_keeps_closed_is_counted_and_any_other_failure_is_named() {
        let mut scan = ProposalScan::new(path("/Users/alice"));
        let trash = path("/Users/alice/.Trash");

        scan.record_failure(
            &trash,
            IndexError::NotPermitted("Operation not permitted (os error 1)".to_string()),
        );
        scan.record_failure(
            &trash,
            IndexError::Source("Permission denied (os error 13)".to_string()),
        );

        assert_eq!(scan.walk.closed_by_the_system, 1);
        assert_eq!(
            scan.walk.failures,
            ["/Users/alice/.Trash: source scan failed: Permission denied (os error 13)"]
        );
    }
}
