// `crosskit contexts propose` end to end: what it prints has to be configuration the
// program itself accepts, and running it has to leave the machine exactly as it was.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TREE: AtomicU64 = AtomicU64::new(0);

/// A tree to propose for, beside a data directory that starts empty and must stay empty.
struct Sandbox {
    root: PathBuf,
    data_directory: PathBuf,
    tree: PathBuf,
}

impl Sandbox {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-propose-cli-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        let sandbox = Self {
            data_directory: root.join("data"),
            tree: root.join("tree"),
            root,
        };
        fs::create_dir_all(&sandbox.data_directory).unwrap();
        fs::create_dir_all(&sandbox.tree).unwrap();
        sandbox
    }

    fn file(&self, path: &str, contents: &str) -> &Self {
        let path = self.tree.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    fn repository(&self, path: &str, config: &str) -> &Self {
        self.file(&format!("{path}/.git/config"), config)
    }

    /// The spelling the walk reports, which on macOS resolves through `/private`.
    fn resolved(&self, path: &str) -> String {
        let root = fs::canonicalize(&self.tree).unwrap();
        if path.is_empty() {
            return root.to_string_lossy().into_owned();
        }
        root.join(path).to_string_lossy().into_owned()
    }

    fn config_path(&self) -> PathBuf {
        self.data_directory.join("config.toml")
    }

    /// The binary with its storage pointed at an empty directory and its policy file at a
    /// path that does not exist: a first run, before anything is configured.
    fn crosskit(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_crosskit"))
            .args(arguments)
            .env("CROSSKIT_DATA_DIR", &self.data_directory)
            .env("CROSSKIT_CONFIG", self.config_path())
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }

    fn propose(&self) -> Output {
        self.crosskit(&["contexts", "propose", &self.tree.to_string_lossy()])
    }

    fn propose_successfully(&self) -> String {
        let output = self.propose();
        assert!(
            output.status.success(),
            "proposing failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn data_directory_entries(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.data_directory) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("failed to remove {}: {error}", self.root.display());
        }
    }
}

fn origin(url: &str) -> String {
    format!("[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n")
}

/// Every entry of a tree with the two things a write would change: its size and the moment
/// it was last modified.
fn snapshot(root: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(read) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in read.filter_map(Result::ok) {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            entries.push(format!(
                "{} {} {:?}",
                path.display(),
                metadata.len(),
                metadata.modified().ok()
            ));
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    entries.sort();
    entries
}

/// A tree of two projects under two hosts.
fn two_hosts(name: &str) -> Sandbox {
    let sandbox = Sandbox::new(name);
    sandbox.repository("work/alpha", &origin("git@git.example.org:team/alpha.git"));
    sandbox.file("work/alpha/src/main.rs", "fn main() {}");
    sandbox.repository("mine/gamma", &origin("https://github.com/me/gamma.git"));
    sandbox.file("mine/gamma/notes.md", "gamma");
    sandbox.file("archive/receipt.txt", "nothing names this a project");
    sandbox
}

/// The point of the command: what it prints is configuration, and pasting it gives the
/// paths the contexts the draft proposed for them.
#[test]
fn the_draft_is_configuration_the_program_accepts_as_written() {
    let sandbox = two_hosts("pasteable");

    let draft = sandbox.propose_successfully();
    assert!(
        draft.contains("[workspace.git-example-org]") && draft.contains("[workspace.github-com]"),
        "{draft}"
    );
    fs::write(sandbox.config_path(), &draft).unwrap();

    let explained = sandbox.crosskit(&[
        "policy",
        "explain",
        &sandbox.resolved("work/alpha/src/main.rs"),
    ]);
    assert!(
        explained.status.success(),
        "the draft did not load: {}",
        String::from_utf8_lossy(&explained.stderr)
    );
    assert!(
        String::from_utf8_lossy(&explained.stdout).contains("Context: git-example-org"),
        "{}",
        String::from_utf8_lossy(&explained.stdout)
    );

    let other = sandbox.crosskit(&[
        "policy",
        "explain",
        &sandbox.resolved("mine/gamma/notes.md"),
    ]);
    assert!(
        String::from_utf8_lossy(&other.stdout).contains("Context: github-com"),
        "{}",
        String::from_utf8_lossy(&other.stdout)
    );
}

/// The command reads. It creates no database, no index and no configuration, and it leaves
/// the tree it walked byte for byte as it found it.
#[test]
fn proposing_writes_nothing_anywhere() {
    let sandbox = two_hosts("read-only");
    let before = snapshot(&sandbox.tree);
    assert!(sandbox.data_directory_entries().is_empty());

    sandbox.propose_successfully();

    assert_eq!(
        sandbox.data_directory_entries(),
        Vec::<String>::new(),
        "no database, index or configuration may be created by a command that proposes"
    );
    assert!(
        !sandbox.config_path().exists(),
        "the command works before a policy file exists and does not write one"
    );
    assert_eq!(
        snapshot(&sandbox.tree),
        before,
        "the walked tree must be left exactly as it was"
    );
}

/// Neither the database nor the policy file is touched, and this is shown by putting both
/// in a state that every command which does touch them refuses: a graph from before contexts
/// existed, and a configuration whose two contexts cover one path. A command that opened
/// either would fail here; this one answers.
#[test]
fn a_graph_and_a_configuration_that_stop_every_other_command_do_not_stop_this_one() {
    let sandbox = two_hosts("untouched");
    fs::write(
        sandbox.data_directory.join("graph.lbug"),
        "a graph from before contexts existed",
    )
    .unwrap();
    fs::write(
        sandbox.config_path(),
        format!(
            "[workspace.work]\ninclude = [\"{}\"]\n\n[workspace.personal]\ninclude = [\"{}\"]\n",
            sandbox.resolved(""),
            sandbox.resolved("work/alpha")
        ),
    )
    .unwrap();
    let refused = sandbox.crosskit(&["index", "jobs"]);
    assert_eq!(
        refused.status.code(),
        Some(1),
        "the sandbox must be in a state that stops a command which reads them"
    );

    let draft = sandbox.propose_successfully();

    assert!(draft.contains("[workspace.git-example-org]"), "{draft}");
}

/// Two runs over one tree print the same draft, character for character.
#[test]
fn the_same_tree_gives_the_same_draft_twice() {
    let sandbox = two_hosts("repeatable");

    assert_eq!(
        sandbox.propose_successfully(),
        sandbox.propose_successfully()
    );
}

/// A repository whose configuration names no remote is grouped by that; one whose remotes
/// name two hosts is handed back with both named.
#[test]
fn a_configuration_without_a_remote_and_one_with_several_hosts_are_both_reported() {
    let sandbox = Sandbox::new("remotes");
    sandbox.repository("local", "[core]\n\trepositoryformatversion = 0\n");
    sandbox.file("local/a.md", "a");
    sandbox.repository(
        "split",
        "[remote \"github\"]\n\turl = https://github.com/me/split.git\n\
         [remote \"internal\"]\n\turl = git@git.example.org:team/split.git\n",
    );
    sandbox.file("split/b.md", "b");

    let draft = sandbox.propose_successfully();

    assert!(
        draft.contains("[workspace.without-remote]") && draft.contains(".git declaring no remote"),
        "{draft}"
    );
    assert!(
        draft.contains("git.example.org and github.com"),
        "both hosts must be named so the operator can settle it: {draft}"
    );
    assert!(
        !draft.contains("[workspace.github-com]"),
        "a repository nothing settles must not be placed anyway: {draft}"
    );
    assert!(
        !draft.contains("split.git") && !draft.contains("me/split"),
        "only the host of a remote is printed, never the URL: {draft}"
    );
}

/// An empty directory is answered plainly and counts as a successful run.
#[test]
fn an_empty_directory_is_answered_rather_than_drafted_for() {
    let sandbox = Sandbox::new("empty");

    let output = sandbox.propose();

    assert_eq!(output.status.code(), Some(0));
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(printed.contains("Nothing to propose"), "{printed}");
    assert!(!printed.contains("[workspace."), "{printed}");
}

/// The command proposes what the contexts could be, so it cannot be told to run in one.
/// Ignoring the option would answer a different question without saying so.
#[test]
fn a_named_context_is_refused_rather_than_ignored() {
    let sandbox = two_hosts("named-context");

    let refused = sandbox.crosskit(&[
        "--context",
        "work",
        "contexts",
        "propose",
        &sandbox.tree.to_string_lossy(),
    ]);

    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--context work"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(String::from_utf8_lossy(&refused.stdout).is_empty());
}

/// A directory that does not exist is a refusal with the path in it, not a draft.
#[test]
fn a_root_that_is_not_a_directory_is_refused() {
    let sandbox = Sandbox::new("missing");

    let refused = sandbox.crosskit(&[
        "contexts",
        "propose",
        &sandbox.tree.join("nowhere").to_string_lossy(),
    ]);

    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("nowhere"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}
