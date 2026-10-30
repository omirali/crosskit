// A pass works over a context, and one root of it failing does not take the others with it.
//
// This is the test that would have made Stage 34 impossible. Before Stage 35 a context of
// ten roots was ten separate commands, and the one place that ran them for you — `setup` —
// used `?`, so the third root failing left the seven behind it unwalked with nothing saying
// so. Eight of the owner's ten roots in one context sat empty for fifteen stages while every
// report said `indexed` (`docs/coverage-audit.md`).
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use lbug::{Connection, Database, SystemConfig, Value};

static NEXT_TREE: AtomicU64 = AtomicU64::new(0);

const CONTEXT: &str = "work";
const ROOTS: [&str; 3] = ["first", "second", "third"];

/// A context of three directories, each with one file in it.
struct Tree {
    root: PathBuf,
    data_directory: PathBuf,
    tree: PathBuf,
}

impl Tree {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-index-context-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        let data_directory = root.join("data");
        let tree = root.join("tree");
        fs::create_dir_all(&data_directory).unwrap();
        for name in ROOTS {
            let directory = tree.join(name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("note.md"), format!("a note in {name}\n")).unwrap();
        }
        let sandbox = Self {
            root,
            data_directory,
            tree,
        };
        sandbox.write_includes(
            &ROOTS
                .iter()
                .map(|name| format!("\"{}\"", sandbox.canonical(name)))
                .collect::<Vec<_>>()
                .join(", "),
        );
        sandbox
    }

    /// The scan resolves its root, and on macOS a temp path resolves through `/private`; a
    /// pattern written from the unresolved path would cover nothing.
    fn canonical(&self, name: &str) -> String {
        fs::canonicalize(self.tree.join(name))
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn write_includes(&self, includes: &str) {
        fs::write(
            self.data_directory.join("config.toml"),
            format!("[workspace.{CONTEXT}]\ninclude = [{includes}]\n"),
        )
        .unwrap();
    }

    fn crosskit(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_crosskit"))
            .args(arguments)
            .env("CROSSKIT_DATA_DIR", &self.data_directory)
            .env("CROSSKIT_CONFIG", self.data_directory.join("config.toml"))
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }

    /// `index init` with no directory: the whole context.
    fn index_context(&self) -> Output {
        self.crosskit(&["--context", CONTEXT, "index", "init"])
    }

    fn status(&self) -> String {
        String::from_utf8_lossy(&self.crosskit(&["status"]).stdout).into_owned()
    }

    fn rows(&self, query: &str) -> Vec<String> {
        let path = self.data_directory.join(format!("graph-{CONTEXT}.lbug"));
        let database = Database::new(&path, SystemConfig::default())
            .unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
        let connection = Connection::new(&database).unwrap();
        connection
            .query(query)
            .unwrap_or_else(|error| panic!("query {query:?}: {error}"))
            .map(|values| {
                values
                    .iter()
                    .map(|value| match value {
                        Value::String(text) => text.clone(),
                        other => format!("{other:?}"),
                    })
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .collect()
    }

    /// The files the graph holds, by the last segment of their path.
    fn indexed_roots(&self) -> Vec<String> {
        self.rows("MATCH (:File)-[:LOCATED_AT]->(l:FileLocation) RETURN l.normalized_path;")
            .into_iter()
            .filter_map(|path| {
                Path::new(&path)
                    .parent()?
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        // A directory this test made unreadable has to be readable again, or the tree cannot
        // be removed.
        let _ = fs::set_permissions(self.tree.join(ROOTS[1]), fs::Permissions::from_mode(0o755));
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("failed to remove {}: {error}", self.root.display());
        }
    }
}

/// Makes one root unreadable, so the walk fails on it after the context's database has
/// already opened and after the root before it has been indexed. This is the shape of the
/// failure Stage 34 found: the database was working, the root was not.
fn make_unreadable(directory: &Path) {
    fs::set_permissions(directory, fs::Permissions::from_mode(0o000)).unwrap();
}

/// The property this stage exists for.
#[test]
fn a_root_that_fails_leaves_the_roots_after_it_indexed() {
    let tree = Tree::new("one-root-fails");
    make_unreadable(&tree.tree.join(ROOTS[1]));

    let output = tree.index_context();
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut indexed = tree.indexed_roots();
    indexed.sort();
    assert_eq!(
        indexed,
        vec!["first".to_string(), "third".to_string()],
        "the root before the failure and the root after it are both indexed:\n{stdout}"
    );
    assert!(
        stdout.contains("Roots walked: 2 of 3"),
        "the report counts the roots rather than answering with the first failure:\n{stdout}"
    );
    assert!(
        stdout.contains("Roots: 3"),
        "every root of the context is named:\n{stdout}"
    );
}

/// A partial success is not a success, and the sentence says which root and what else got
/// done — the two things `Context X was not indexed` left out while claiming a third that
/// was false.
#[test]
fn a_partial_pass_fails_the_run_and_names_the_root_that_did_not_walk() {
    let tree = Tree::new("partial-exit");
    make_unreadable(&tree.tree.join(ROOTS[1]));

    let output = tree.index_context();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a run that walked two roots of three did not do what was asked"
    );
    assert!(
        stderr.contains(&tree.canonical(ROOTS[1])),
        "the root that did not walk is named: {stderr}"
    );
    assert!(
        stderr.contains("(source)"),
        "and so is the category of what stopped it: {stderr}"
    );
    assert!(
        stderr.contains("The other 2 were walked"),
        "and the run says what it did do, so nobody looks for a broken database: {stderr}"
    );
}

/// The trace. Until Stage 35 a failure before the walk left nothing anywhere, so "why is
/// this root empty" had no answer once the terminal was closed.
#[test]
fn a_pass_that_did_not_happen_is_recorded_in_the_graph_with_its_category() {
    let tree = Tree::new("failed-pass-trace");
    make_unreadable(&tree.tree.join(ROOTS[1]));
    tree.index_context();

    let failed = tree
        .rows("MATCH (run:ReconcileRun) WHERE run.status = 'failed' RETURN run.root, run.failure;");
    assert_eq!(
        failed,
        vec![format!("{}|source", tree.canonical(ROOTS[1]))],
        "the attempt is in the graph, under the root it was made on and the category that \
         stopped it"
    );
}

/// The line that would have made the whole story impossible: a root nobody has walked is
/// visible as one, every day, in the command people already run.
#[test]
fn status_says_which_root_was_never_walked() {
    let tree = Tree::new("status-names-roots");
    make_unreadable(&tree.tree.join(ROOTS[1]));
    tree.index_context();

    let status = tree.status();
    assert!(
        status.contains("Roots: 3"),
        "the context says how many roots it has: {status}"
    );
    assert!(
        status.contains(&format!("{}: NEVER WALKED", tree.canonical(ROOTS[1]))),
        "the root that holds nothing says so: {status}"
    );
    assert!(
        status.contains("The last attempt failed (source)"),
        "and says what stopped it, read back out of the graph: {status}"
    );
    for walked in [ROOTS[0], ROOTS[2]] {
        assert!(
            status.contains(&format!("{}: 1 file(s)", tree.canonical(walked))),
            "a root that was walked says how much it holds: {status}"
        );
    }
}

/// An `include` that is a pattern covers files without naming a directory, so a pass has
/// nowhere to start. It is named rather than skipped: an include a pass ignores silently is
/// how a root goes unindexed with nobody told.
#[test]
fn an_include_that_is_a_mask_is_named_and_not_walked() {
    let tree = Tree::new("mask-include");
    tree.write_includes(&format!(
        "\"{}\", \"{}/*\"",
        tree.canonical(ROOTS[0]),
        tree.canonical(ROOTS[1])
    ));

    let output = tree.index_context();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("a pattern and not a directory"),
        "the mask is named where it stands: {stdout}"
    );
    assert!(
        stderr.contains("(not a directory)"),
        "and again in the line the run ends with: {stderr}"
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        tree.indexed_roots(),
        vec!["first".to_string()],
        "the include that does name a directory is still walked"
    );
}

/// The narrowing still works, and it is still checked against the context's coverage.
#[test]
fn naming_one_directory_walks_only_that_one() {
    let tree = Tree::new("one-directory");

    let output = tree.crosskit(&[
        "--context",
        CONTEXT,
        "index",
        "init",
        &tree.canonical(ROOTS[2]),
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(tree.indexed_roots(), vec!["third".to_string()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Roots: 1"), "{stdout}");
    assert!(stdout.contains("Roots walked: 1 of 1"), "{stdout}");
}
