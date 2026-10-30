// The proposal against real trees, through the walker the index itself uses. What is
// checked here is what the walk sees and what it refuses to see: the object store of a
// repository, a directory it may not enter, a symlink pointing back up the tree.
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::application::ContextProposalService;
use crate::domain::{ContextProposal, NormalizedPath, NotAttributed};

use super::filesystem::DescriptorRelativeWalker;

static NEXT_TREE: AtomicU64 = AtomicU64::new(0);

/// A temporary tree that removes itself, with the permissions restored first so a directory
/// the test locked cannot outlive it.
struct Tree {
    root: PathBuf,
    locked: Vec<PathBuf>,
}

impl Tree {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-propose-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            locked: Vec::new(),
        }
    }

    fn file(&self, path: &str, contents: &str) -> &Self {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    fn directory(&self, path: &str) -> PathBuf {
        let path = self.root.join(path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// A repository root: a `.git` directory with the configuration a clone leaves behind.
    fn repository(&self, path: &str, config: &str) -> &Self {
        self.file(&format!("{path}/.git/config"), config)
    }

    fn lock(&mut self, path: &str) {
        let path = self.directory(path);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        self.locked.push(path);
    }

    /// The scan resolves its root, and on macOS a temp path resolves through `/private`; a
    /// path written from the unresolved spelling would name nothing the walk reports.
    fn resolved(&self, path: &str) -> String {
        let resolved = fs::canonicalize(&self.root).unwrap();
        if path.is_empty() {
            return resolved.to_string_lossy().into_owned();
        }
        resolved.join(path).to_string_lossy().into_owned()
    }

    fn propose(&self) -> ContextProposal {
        self.propose_as_home(None)
    }

    fn propose_as_home(&self, home: Option<&Path>) -> ContextProposal {
        let home = home.map(|home| {
            let resolved = fs::canonicalize(home).unwrap();
            NormalizedPath::from_absolute(&resolved.to_string_lossy()).unwrap()
        });
        ContextProposalService::new(&DescriptorRelativeWalker)
            .execute(&self.root, home.as_ref())
            .unwrap()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        for path in &self.locked {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("failed to remove {}: {error}", self.root.display());
        }
    }
}

fn origin(url: &str) -> String {
    format!(
        "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
    )
}

/// Every context and the paths it covers, in the order the proposal prints them.
fn contexts(proposal: &ContextProposal) -> Vec<(String, Vec<String>)> {
    proposal
        .contexts()
        .iter()
        .map(|context| {
            (
                context.name().as_str().to_string(),
                context
                    .includes()
                    .iter()
                    .map(|include| include.path().as_str().to_string())
                    .collect(),
            )
        })
        .collect()
}

fn unattributed(proposal: &ContextProposal) -> Vec<(String, String)> {
    proposal
        .unattributed()
        .iter()
        .map(|subtree| {
            (
                subtree.path().as_str().to_string(),
                format!("{:?}", subtree.reason()),
            )
        })
        .collect()
}

/// A tree of projects under two hosts plus a directory that is neither. The draft groups by
/// the host, and the directory with no signal is named as undecided rather than guessed at.
#[test]
fn projects_under_two_hosts_become_two_contexts_and_the_rest_is_left_open() {
    let tree = Tree::new("two-hosts");
    tree.repository("work/alpha", &origin("git@git.example.org:team/alpha.git"));
    tree.file("work/alpha/src/main.rs", "fn main() {}");
    tree.file("work/alpha/Cargo.toml", "[package]\nname = \"alpha\"\n");
    tree.repository(
        "work/beta",
        &origin("https://git.example.org/team/beta.git"),
    );
    tree.file("work/beta/README.md", "beta");
    tree.repository("mine/gamma", &origin("https://github.com/me/gamma.git"));
    tree.file("mine/gamma/notes.md", "gamma");
    tree.file("archive/receipt.txt", "nothing names this a project");

    let proposal = tree.propose();

    assert_eq!(
        contexts(&proposal),
        [
            (
                "git-example-org".to_string(),
                vec![tree.resolved("work/alpha"), tree.resolved("work/beta")]
            ),
            ("github-com".to_string(), vec![tree.resolved("mine/gamma")]),
        ]
    );
    assert_eq!(
        unattributed(&proposal),
        [(tree.resolved("archive"), "NoSignal".to_string())]
    );
}

/// The same tree proposed twice gives the same draft. Nothing in the assembly depends on
/// the order the filesystem hands entries back in.
#[test]
fn the_same_tree_proposes_the_same_draft_twice() {
    let tree = Tree::new("repeatable");
    tree.repository("one", &origin("https://github.com/me/one.git"));
    tree.file("one/a.md", "a");
    tree.file("one/deep/b.md", "b");
    tree.repository("two", &origin("git@git.example.org:team/two.git"));
    tree.file("two/c.md", "c");
    tree.file("loose.txt", "loose");

    let first = tree.propose();
    let second = tree.propose();

    assert_eq!(first, second);
    assert_eq!(first.whole().files(), 4, "the loose file counts once");
}

/// Nothing of a repository's own storage is walked or counted: the walk enters `.git` for
/// one file and refuses every other entry in it before stat-ing it.
#[test]
fn only_the_configuration_of_a_repository_is_read_out_of_its_git_directory() {
    let tree = Tree::new("git-directory");
    tree.repository("repo", &origin("https://github.com/me/repo.git"));
    tree.file("repo/.git/HEAD", "ref: refs/heads/main\n");
    tree.file("repo/.git/objects/ab/cdef", &"x".repeat(4096));
    tree.file("repo/.git/hooks/pre-commit", "#!/bin/sh\n");
    tree.file("repo/kept.md", "the only file of this repository");

    let proposal = tree.propose();

    let include = &proposal.contexts()[0].includes()[0];
    assert_eq!(include.path().as_str(), tree.resolved("repo"));
    assert_eq!(
        include.volume().files(),
        1,
        "only the repository's own file is counted"
    );
    assert_eq!(
        include.volume().bytes(),
        "the only file of this repository".len() as u64
    );
}

/// A directory that cannot be read, a symlink that points back up the tree and a `.git` that
/// is a file rather than a directory: each is counted and none of them stops the walk.
#[test]
fn an_unreadable_directory_a_symlink_upwards_and_a_git_file_are_all_survived() {
    let mut tree = Tree::new("hostile");
    tree.file("worktree/main.py", "print('hello')");
    tree.file("worktree/.git", "gitdir: /elsewhere/.git/worktrees/one\n");
    tree.file("plain/note.md", "note");
    symlink(&tree.root, tree.root.join("plain/upwards")).unwrap();
    tree.lock("forbidden");

    let proposal = tree.propose();

    assert_eq!(
        proposal.walk().symlinks,
        1,
        "the symlink was seen, not followed"
    );
    assert_eq!(
        proposal.walk().failures.len(),
        1,
        "the unreadable directory is counted: {:?}",
        proposal.walk().failures
    );
    assert!(
        proposal.walk().failures[0].contains("forbidden"),
        "{:?}",
        proposal.walk().failures
    );
    let include = &proposal.contexts()[0].includes()[0];
    assert_eq!(include.path().as_str(), tree.resolved("worktree"));
    assert!(
        format!("{:?}", include.evidence()).contains("NotADirectory"),
        "a `.git` that is not a directory is said to be one: {:?}",
        include.evidence()
    );
}

/// A configuration without a remote and one with several hosts land on opposite sides: the
/// first is a repository with nothing to stand under, the second is nobody's to decide here.
#[test]
fn a_repository_without_a_remote_is_grouped_and_one_with_several_hosts_is_not() {
    let tree = Tree::new("remotes");
    tree.repository("local", "[core]\n\trepositoryformatversion = 0\n");
    tree.file("local/a.md", "a");
    tree.repository(
        "split",
        "[remote \"github\"]\n\turl = https://github.com/me/split.git\n\
         [remote \"internal\"]\n\turl = git@git.example.org:team/split.git\n",
    );
    tree.file("split/b.md", "b");
    tree.repository(
        "agreed",
        "[remote \"fork\"]\n\turl = https://github.com/me/agreed.git\n\
         [remote \"upstream\"]\n\turl = https://github.com/them/agreed.git\n",
    );
    tree.file("agreed/c.md", "c");

    let proposal = tree.propose();

    assert_eq!(
        contexts(&proposal),
        [
            ("github-com".to_string(), vec![tree.resolved("agreed")]),
            ("without-remote".to_string(), vec![tree.resolved("local")]),
        ]
    );
    let left = proposal.unattributed();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].path().as_str(), tree.resolved("split"));
    let NotAttributed::SeveralRemoteHosts(hosts) = left[0].reason() else {
        panic!("two hosts and no origin must stay undecided: {:?}", left[0]);
    };
    assert_eq!(hosts.len(), 2);
}

/// The standard home layout is evidence only for the children of the home directory itself,
/// and only while nothing inside them would have to be carved out of the offer.
#[test]
fn the_home_layout_is_read_only_when_the_walked_root_is_the_home_directory() {
    let tree = Tree::new("home");
    tree.file("Documents/letter.md", "a letter");
    tree.file("Downloads/installer.bin", "downloaded");
    tree.file("Desktop/scratch.md", "scratch");
    tree.repository("Desktop/experiment", &origin("https://github.com/me/x.git"));
    tree.file("Desktop/experiment/x.md", "x");

    let elsewhere = tree.propose();
    assert!(
        elsewhere
            .contexts()
            .iter()
            .all(|context| context.name().as_str() != "personal"),
        "a directory that is not a home directory has no layout: {:?}",
        contexts(&elsewhere)
    );

    let at_home = tree.propose_as_home(Some(&tree.root));

    assert_eq!(
        contexts(&at_home),
        [
            (
                "github-com".to_string(),
                vec![tree.resolved("Desktop/experiment")]
            ),
            ("personal".to_string(), vec![tree.resolved("Documents")]),
        ],
        "the Desktop holds a repository, so it is not offered whole"
    );
    assert_eq!(
        unattributed(&at_home),
        [
            (tree.resolved("Desktop"), "OverlapsProjects".to_string()),
            (tree.resolved("Downloads"), "MixedInbox".to_string()),
        ],
        "with equal remainders the order falls back to the path, so the list is stable"
    );
}

/// An empty directory is answered rather than proposed for.
#[test]
fn an_empty_root_is_answered_with_nothing_to_propose() {
    let tree = Tree::new("empty");
    tree.directory("still/empty/inside");

    let proposal = tree.propose();

    assert!(proposal.is_empty());
    assert!(proposal.contexts().is_empty());
    assert!(proposal.unattributed().is_empty());
    assert!(proposal.walk().failures.is_empty());
}
