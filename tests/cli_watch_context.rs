// `index watch` watches a context, and one root it cannot register does not silence the
// others — nor, and this is the point, does it stop that root being reconciled.
//
// Stage 35 made a pass cover the context; Stage 36 made the watcher do the same. The two
// together turn a root the backend refuses from a coverage problem into a latency one:
// every pass walks every root whichever root a hint came from, so the refused root is
// indexed on the periodic pass like all the others.
mod common;

use std::fs;
use std::path::PathBuf;

use common::{Sandbox, WatchProcess};

const ROOTS: [&str; 3] = ["first", "second", "third"];

/// A context of three directories, the middle one unreadable, so the backend refuses to
/// register it while the other two register normally.
struct Tree {
    sandbox: Sandbox,
    roots: Vec<PathBuf>,
}

impl Tree {
    fn new(name: &str) -> Self {
        let sandbox = Sandbox::new(name);
        let mut roots = Vec::new();
        for root in ROOTS {
            let directory = sandbox.documents.join(root);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("note.md"), format!("a note in {root}\n")).unwrap();
            roots.push(directory);
        }
        let includes = roots
            .iter()
            .map(|root| format!("\"{}\"", fs::canonicalize(root).unwrap().to_string_lossy()))
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            sandbox.data_directory.join("config.toml"),
            format!("[workspace.{}]\ninclude = [{includes}]\n", Sandbox::CONTEXT),
        )
        .unwrap();
        Self { sandbox, roots }
    }

    /// Removes a configured root from disk after the configuration named it.
    ///
    /// Not `chmod 000`, which was tried first and does not work: FSEvents takes any path
    /// that exists, so on macOS an unreadable root registers without a word — measured.
    /// Registration therefore proves less here than on Linux, where inotify walks the tree
    /// and denies the whole root. A root that is gone is refused by both.
    fn remove(&self, index: usize) {
        fs::remove_dir_all(&self.roots[index]).unwrap();
    }
}

/// The property of this stage, in the form Stage 35 established for a pass.
#[test]
fn a_root_that_cannot_be_watched_leaves_the_others_watched_and_is_still_reconciled() {
    let tree = Tree::new("watch-one-root-refused");
    tree.remove(1);

    let mut watch = WatchProcess::spawn_context(&tree.sandbox);
    let opening = watch.wait_for_line("Watching context ");
    assert!(
        opening.contains("3 root(s)"),
        "the watcher names the whole context: {opening}"
    );

    let counted = watch.wait_for_line("Watching: ");
    assert!(
        counted.starts_with("Watching: 2 of 3 root(s)."),
        "one root of three could not be registered: {counted}"
    );
    let refused = watch.wait_for_line("  /");
    assert!(
        refused.contains("no change notifications"),
        "the root that was refused is named with its category: {refused}"
    );
    let consequence = watch.wait_for_line("  Every pass still walks");
    assert!(
        consequence.contains("latency of an event"),
        "and what that costs is said: {consequence}"
    );

    // The pass covers the context whatever the backend took, so the two roots that are
    // there are walked by the bootstrap pass — and the pass says it walked two of three
    // rather than answering with the one that failed.
    let bootstrap = watch.wait_for_line("Pass 1 (bootstrap)");
    assert!(
        bootstrap.contains("over 2 of 3 root(s)") && bootstrap.contains("1 root(s) not walked"),
        "the pass covers what it can and names what it could not: {bootstrap}"
    );

    watch.interrupt();
    let (status, stdout, stderr) = watch.wait_for_exit();
    // One root of the context could not be walked for the whole session, so the run did not
    // do what was asked — the same rule a partial `index reconcile` follows. The stop is
    // still clean, and the summary is still printed.
    assert_eq!(status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stdout.iter().any(|line| line == "Watch stopped."),
        "{stdout:?}"
    );
}

/// Ctrl-C leaves nothing behind, including when a root never came up.
#[test]
fn a_stop_leaves_no_process_and_no_probe_file_even_when_a_root_never_came_up() {
    let tree = Tree::new("watch-clean-stop");
    tree.remove(1);

    let mut watch = WatchProcess::spawn_context(&tree.sandbox);
    watch.wait_for_line("Pass 1 (bootstrap)");
    watch.interrupt();
    let (status, stdout, stderr) = watch.wait_for_exit();

    // The process ends on its own and says so; the exit code is about the root it could not
    // walk, not about the stop.
    assert_eq!(status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stdout.iter().any(|line| line == "Watch stopped."),
        "{stdout:?}"
    );
    let probe = tree.sandbox.data_directory.join("watch-probe");
    let left_behind: Vec<String> = fs::read_dir(&probe)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        left_behind.is_empty(),
        "the probe file must be gone whatever became of the roots: {left_behind:?}"
    );
}
