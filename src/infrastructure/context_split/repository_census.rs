// Every git repository under the project roots, found by the product's walker and read from its
// own files.
//
// The walk is `DescriptorRelativeWalker` with a visitor of this measurement's own — the shape
// Stage 13 set for `contexts propose`. A `.git` directory is entered for exactly two files,
// `config` and `logs/HEAD`, and nothing else in it is listed, stat-ed or opened. Both are read
// through `SourceEntry::read_evidence`, the one door to content a visitor has, so neither can be
// swapped for a link between the walk and the read. `git` itself is never run: the reflog is a
// text file, and it holds the committer of every local action.
use std::collections::BTreeMap;

use crate::application::{
    Admission, EvidenceRead, IndexError, SourceEntry, SourceVisitor, SourceWalker,
};
use crate::domain::context_split::{Identity, Origin, ReflogIdentities, identity_in_git_config};
use crate::domain::{
    ContentChunk, FileKind, GitMarker, NormalizedPath, generated_directory_marker,
};

const GIT_ENTRY: &str = ".git";
const CONFIG: &str = "config";
const LOGS: &str = "logs";
const REFLOG: &str = "logs/HEAD";
/// The ceiling `contexts propose` reads a `.git/config` under, for the same reason: a file a
/// tool wrote is a few hundred bytes, and anything larger is not the file this is about.
const CONFIG_LIMIT_BYTES: u64 = 256 * 1024;
/// A reflog grows a line per local action. Years of them are a few megabytes; a file above this
/// is counted as unread rather than held.
const REFLOG_LIMIT_BYTES: u64 = 16 * 1024 * 1024;
/// The identity entries are described under. Nothing is published, so it never leaves the
/// process.
const CENSUS_ENTRY: &str = "file:context-split";

/// One repository and what its files said.
#[derive(Debug, Clone)]
pub(super) struct FoundRepository {
    pub(super) root: NormalizedPath,
    pub(super) marker: GitMarker,
    /// `None` when no `config` was read: `.git` is a pointer file, or the file was not there.
    pub(super) origin: Option<Origin>,
    pub(super) own_identity: Option<Identity>,
    /// `None` when there is no reflog: nothing was ever committed or checked out here.
    pub(super) reflog: Option<ReflogIdentities>,
}

impl FoundRepository {
    fn new(root: NormalizedPath) -> Self {
        Self {
            root,
            marker: GitMarker::Directory,
            origin: None,
            own_identity: None,
            reflog: None,
        }
    }
}

/// Every repository the walks found, sorted by path, and what they could not do.
#[derive(Debug, Default)]
pub(super) struct RepositoryCensus {
    pub(super) repositories: Vec<FoundRepository>,
    pub(super) generated_directories: u64,
    pub(super) walk_failures: usize,
    pub(super) unread_files: usize,
}

/// Walks each root and gathers every repository under it. A root that cannot be walked is
/// counted as a failure of the walk, not a reason to stop the others.
pub(super) fn census_of_repositories(
    walker: &impl SourceWalker,
    roots: &[NormalizedPath],
) -> RepositoryCensus {
    let mut scan = RepositoryScan::default();
    for root in roots {
        let resolved = match walker.resolve_root(std::path::Path::new(root.as_str())) {
            Ok(resolved) => resolved,
            Err(_) => {
                scan.walk_failures += 1;
                continue;
            }
        };
        if walker.walk(&resolved, &mut scan).is_err() {
            scan.walk_failures += 1;
        }
    }
    RepositoryCensus {
        repositories: scan.found.into_values().collect(),
        generated_directories: scan.generated_directories,
        walk_failures: scan.walk_failures,
        unread_files: scan.unread_files,
    }
}

#[derive(Default)]
struct RepositoryScan {
    found: BTreeMap<String, FoundRepository>,
    generated_directories: u64,
    walk_failures: usize,
    unread_files: usize,
}

impl RepositoryScan {
    fn at(&mut self, root: &NormalizedPath) -> &mut FoundRepository {
        self.found
            .entry(root.as_str().to_string())
            .or_insert_with(|| FoundRepository::new(root.clone()))
    }

    fn read_git_file(&mut self, entry: &dyn SourceEntry, root: &NormalizedPath, inside: &str) {
        let limit = match inside {
            CONFIG => CONFIG_LIMIT_BYTES,
            REFLOG => REFLOG_LIMIT_BYTES,
            _ => return,
        };
        let Some(text) = text_of(entry, limit) else {
            self.unread_files += 1;
            return;
        };
        let repository = self.at(root);
        match inside {
            CONFIG => {
                repository.origin = Some(Origin::of_git_config(&text));
                repository.own_identity = identity_in_git_config(&text);
            }
            _ => repository.reflog = Some(ReflogIdentities::of_reflog(&text)),
        }
    }
}

impl SourceVisitor for RepositoryScan {
    fn admit(&mut self, path: &NormalizedPath) -> Admission {
        if let Some((_, inside)) = inside_a_git_entry(path) {
            return match inside {
                CONFIG | LOGS | REFLOG => Admission::Inspect,
                _ => Admission::Skip,
            };
        }
        if path.file_name() == GIT_ENTRY
            && let Some(root) = path.parent()
        {
            // A `.git` that turns out not to be a directory says so when it arrives as an
            // entry; until then it is the directory the walker is about to enter.
            self.at(&root).marker = GitMarker::Directory;
        }
        Admission::Inspect
    }

    /// The rule `contexts propose` applies, for its reason: a directory a tool made is not
    /// where anybody keeps a project.
    fn admit_directory(
        &mut self,
        path: &NormalizedPath,
        holds: &dyn Fn(&str) -> bool,
    ) -> Admission {
        if inside_a_git_entry(path).is_none() && generated_directory_marker(holds).is_some() {
            self.generated_directories += 1;
            return Admission::Skip;
        }
        Admission::Inspect
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        let path = entry.path().clone();
        if let Some((root, inside)) = inside_a_git_entry(&path) {
            self.read_git_file(entry, &root, inside);
            return Ok(());
        }
        if path.file_name() == GIT_ENTRY
            && let Some(root) = path.parent()
        {
            self.at(&root).marker = GitMarker::NotADirectory;
        }
        Ok(())
    }

    fn skip_unrepresentable_name(&mut self) {}

    fn record_failure(&mut self, _path: &NormalizedPath, _error: IndexError) {
        self.walk_failures += 1;
    }
}

/// The repository a path inside `.git` belongs to, and the path within `.git`.
fn inside_a_git_entry(path: &NormalizedPath) -> Option<(NormalizedPath, &str)> {
    let text = path.as_str();
    let cut = text.rfind("/.git/")?;
    let root = NormalizedPath::from_absolute(&text[..cut]).ok()?;
    Some((root, &text[cut + "/.git/".len()..]))
}

/// The text of one small file, read the way `contexts propose` reads a `.git/config`: through
/// the walker's own entry, pieces joined in order. Anything that is not a readable text file
/// under the limit is nothing.
fn text_of(entry: &dyn SourceEntry, limit: u64) -> Option<String> {
    let location = entry.location(CENSUS_ENTRY).ok()?;
    if location.kind() != FileKind::RegularFile || location.size_bytes() > limit {
        return None;
    }
    match entry.read_evidence(&location, limit).ok()? {
        EvidenceRead::Commit(commit) => {
            let mut chunks: Vec<&ContentChunk> = commit.chunks().iter().collect();
            chunks.sort_by_key(|chunk| chunk.ordinal());
            Some(chunks.into_iter().map(ContentChunk::text).collect())
        }
        EvidenceRead::Skipped(_) => None,
    }
}
