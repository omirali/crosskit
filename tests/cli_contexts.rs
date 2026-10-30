// Context isolation, end to end through the CLI: two contexts over one tree, each with a
// graph of its own. What is checked here is not that a query filters the other context out
// but that the other context's files are not in the database at all.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use lbug::{Connection, Database, SystemConfig, Value};

static NEXT_TREE: AtomicU64 = AtomicU64::new(0);

/// One tree whose directories alternate between two contexts, plus a directory no context
/// covers. Every file carries a sentinel of its own, so a stray chunk is recognisable
/// wherever it turns up.
struct Tree {
    root: PathBuf,
    data_directory: PathBuf,
    tree: PathBuf,
}

const WORK_DIRECTORIES: &[&str] = &["alpha", "gamma"];
const PERSONAL_DIRECTORIES: &[&str] = &["beta", "delta"];
const UNCOVERED_DIRECTORY: &str = "orphan";

impl Tree {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-contexts-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        let data_directory = root.join("data");
        let tree = root.join("tree");
        fs::create_dir_all(&data_directory).unwrap();
        for directory in WORK_DIRECTORIES
            .iter()
            .chain(PERSONAL_DIRECTORIES)
            .chain([&UNCOVERED_DIRECTORY])
        {
            let path = tree.join(directory);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("note.md"), sentinel(directory)).unwrap();
        }
        let sandbox = Self {
            root,
            data_directory,
            tree,
        };
        sandbox.write_config(&sandbox.separate_contexts());
        sandbox
    }

    /// The scan resolves its root, and on macOS a temp path resolves through `/private`; a
    /// pattern written from the unresolved path would cover nothing.
    fn covered(&self, directory: &str) -> String {
        fs::canonicalize(self.tree.join(directory))
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn patterns(&self, directories: &[&str]) -> String {
        directories
            .iter()
            .map(|directory| format!("\"{}\"", self.covered(directory)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn separate_contexts(&self) -> String {
        format!(
            "[workspace.work]\ninclude = [{}]\n\n[workspace.personal]\ninclude = [{}]\n",
            self.patterns(WORK_DIRECTORIES),
            self.patterns(PERSONAL_DIRECTORIES)
        )
    }

    fn write_config(&self, contexts: &str) {
        fs::write(self.data_directory.join("config.toml"), contexts).unwrap();
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

    fn index(&self, context: &str, directory: &str) -> Output {
        self.crosskit(&[
            "--context",
            context,
            "index",
            "init",
            &self.tree.join(directory).to_string_lossy(),
        ])
    }

    fn index_ok(&self, context: &str, directory: &str) {
        let output = self.index(context, directory);
        assert!(
            output.status.success(),
            "indexing {directory} as {context} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn reconcile(&self, context: &str, directory: &str) -> Output {
        self.crosskit(&[
            "--context",
            context,
            "index",
            "reconcile",
            &self.tree.join(directory).to_string_lossy(),
        ])
    }

    fn database_path(&self, context: &str) -> PathBuf {
        self.data_directory.join(format!("graph-{context}.lbug"))
    }

    fn databases(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.data_directory) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("graph-"))
            .collect()
    }

    fn graph(&self, context: &str) -> Graph {
        Graph::read(&self.database_path(context))
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("failed to remove {}: {error}", self.root.display());
        }
    }
}

/// What one file says, so a chunk of it is recognisable in any database it reaches.
fn sentinel(directory: &str) -> String {
    format!("sentinel-{directory}-must-stay-in-its-context")
}

/// Everything one context's graph holds that could name another context's files.
struct Graph {
    location_paths: Vec<String>,
    chunk_texts: Vec<String>,
    queued_jobs: i64,
    missing_locations: i64,
    completed_runs: i64,
    /// The sighting epoch is one durable counter per graph, so it is contextual only if
    /// each context has a graph of its own. Read here rather than assumed.
    epoch: i64,
    schema_version: i64,
}

impl Graph {
    fn read(database_path: &Path) -> Self {
        let database = Database::new(database_path, SystemConfig::default()).unwrap();
        let connection = Connection::new(&database).unwrap();
        Self {
            location_paths: strings(
                &connection,
                "MATCH (location:FileLocation) RETURN location.normalized_path;",
            ),
            chunk_texts: strings(&connection, "MATCH (chunk:ContentChunk) RETURN chunk.text;"),
            queued_jobs: count(
                &connection,
                "MATCH (:InsightJob {state: 'queued'}) RETURN COUNT(*);",
            ),
            missing_locations: count(
                &connection,
                "MATCH (:FileLocation {status: 'missing'}) RETURN COUNT(*);",
            ),
            completed_runs: count(
                &connection,
                "MATCH (:ReconcileRun {status: 'completed'}) RETURN COUNT(*);",
            ),
            epoch: count(
                &connection,
                "MATCH (epoch:ReconcileEpoch {id: 'current'}) RETURN epoch.sequence;",
            ),
            schema_version: count(
                &connection,
                "MATCH (migration:SchemaMigration) RETURN MAX(migration.version);",
            ),
        }
    }

    /// No node of this graph may name or quote anything from `directories`.
    fn holds_nothing_of(&self, directories: &[&str]) {
        for directory in directories {
            for path in &self.location_paths {
                assert!(
                    !path.contains(&format!("/{directory}/")),
                    "a location of {directory} reached another context: {path}"
                );
            }
            let sentinel = sentinel(directory);
            for text in &self.chunk_texts {
                assert!(
                    !text.contains(&sentinel),
                    "content of {directory} reached another context"
                );
            }
        }
    }

    fn holds_one_file_of_each(&self, directories: &[&str]) {
        assert_eq!(
            self.location_paths.len(),
            directories.len(),
            "{:?}",
            self.location_paths
        );
        for directory in directories {
            assert!(
                self.location_paths
                    .iter()
                    .any(|path| path.contains(&format!("/{directory}/"))),
                "{directory} is missing from its own context: {:?}",
                self.location_paths
            );
        }
    }
}

fn strings(connection: &Connection<'_>, query: &str) -> Vec<String> {
    let mut statement = connection.prepare(query).unwrap();
    let result = connection.execute(&mut statement, vec![]).unwrap();
    result
        .map(|row| match row.as_slice() {
            [Value::String(value)] => value.clone(),
            values => panic!("expected one string, received {values:?}"),
        })
        .collect()
}

fn count(connection: &Connection<'_>, query: &str) -> i64 {
    let mut statement = connection.prepare(query).unwrap();
    let mut result = connection.execute(&mut statement, vec![]).unwrap();
    match result.next().unwrap().as_slice() {
        [Value::Int64(value)] => *value,
        values => panic!("expected one count, received {values:?}"),
    }
}

/// The guarantee itself: each context's graph holds its own files and nothing of the other,
/// neither a path nor a line of content.
#[test]
fn two_contexts_over_one_tree_hold_only_their_own_files() {
    let tree = Tree::new("isolated");
    for directory in WORK_DIRECTORIES {
        tree.index_ok("work", directory);
    }
    for directory in PERSONAL_DIRECTORIES {
        tree.index_ok("personal", directory);
    }

    let work = tree.graph("work");
    work.holds_one_file_of_each(WORK_DIRECTORIES);
    work.holds_nothing_of(PERSONAL_DIRECTORIES);
    work.holds_nothing_of(&[UNCOVERED_DIRECTORY]);

    let personal = tree.graph("personal");
    personal.holds_one_file_of_each(PERSONAL_DIRECTORIES);
    personal.holds_nothing_of(WORK_DIRECTORIES);
    personal.holds_nothing_of(&[UNCOVERED_DIRECTORY]);
}

/// A path no context covers is refused before the walk, so nothing of it is read: no
/// location, no chunk, no queued job — and no failure either, because nothing was attempted.
#[test]
fn a_path_no_context_covers_is_never_indexed() {
    let tree = Tree::new("uncovered");
    tree.index_ok("work", WORK_DIRECTORIES[0]);

    let refused = tree.index("work", UNCOVERED_DIRECTORY);
    assert_eq!(refused.status.code(), Some(1));
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(
        message.contains("belongs to no context"),
        "the refusal must say why: {message}"
    );
    assert!(
        String::from_utf8_lossy(&refused.stdout).is_empty(),
        "a refused run reports no scan"
    );

    let work = tree.graph("work");
    work.holds_one_file_of_each(&[WORK_DIRECTORIES[0]]);
    work.holds_nothing_of(&[UNCOVERED_DIRECTORY]);
    assert_eq!(work.queued_jobs, 1, "only the covered file queued a job");
}

/// Two contexts that could both cover one path are refused when the file is read, naming
/// both contexts and both rules as they are written.
#[test]
fn an_overlapping_configuration_is_refused_when_it_is_read() {
    let tree = Tree::new("overlap");
    tree.write_config(&format!(
        "[workspace.work]\ninclude = [\"{}\"]\n\n[workspace.personal]\ninclude = [\"{}\"]\n",
        fs::canonicalize(&tree.tree).unwrap().display(),
        tree.covered(PERSONAL_DIRECTORIES[0])
    ));

    let refused = tree.crosskit(&["--context", "work", "index", "jobs"]);
    assert_eq!(refused.status.code(), Some(1));
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(
        message.contains("work") && message.contains("personal"),
        "{message}"
    );
    assert!(
        message.contains("tree"),
        "the rules must be named: {message}"
    );
    assert!(
        tree.databases().is_empty(),
        "a configuration that is refused must not open a database"
    );
}

/// With several contexts configured and none named, the command stops before it opens
/// anything: choosing one for the operator is the one thing a boundary must never do.
#[test]
fn a_command_without_a_context_refuses_before_opening_a_database() {
    let tree = Tree::new("ambiguous");

    let refused = tree.crosskit(&[
        "index",
        "init",
        &tree.tree.join(WORK_DIRECTORIES[0]).to_string_lossy(),
    ]);

    assert_eq!(refused.status.code(), Some(1));
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("--context"), "{message}");
    assert!(
        tree.databases().is_empty(),
        "nothing may be created before the context is known"
    );
}

/// Reconciling one context cannot retire, queue or record anything in another: the run, its
/// epoch and its queue live in the graph it opened.
#[test]
fn reconcile_in_one_context_leaves_the_other_untouched() {
    let tree = Tree::new("reconcile");
    for directory in WORK_DIRECTORIES {
        tree.index_ok("work", directory);
    }
    for directory in PERSONAL_DIRECTORIES {
        tree.index_ok("personal", directory);
    }
    let before = tree.graph("personal");

    fs::remove_file(tree.tree.join(WORK_DIRECTORIES[0]).join("note.md")).unwrap();
    let reconciled = tree.reconcile("work", WORK_DIRECTORIES[0]);
    assert!(
        reconciled.status.success(),
        "{}",
        String::from_utf8_lossy(&reconciled.stderr)
    );

    let work = tree.graph("work");
    assert_eq!(
        work.missing_locations, 1,
        "the deleted file is retired here"
    );
    assert_eq!(work.completed_runs, 1);
    assert!(work.epoch > before.epoch, "the run advanced its own epoch");

    let after = tree.graph("personal");
    assert_eq!(after.missing_locations, before.missing_locations);
    assert_eq!(after.location_paths.len(), before.location_paths.len());
    assert_eq!(after.queued_jobs, before.queued_jobs);
    assert_eq!(
        after.completed_runs, before.completed_runs,
        "a run of one context is not a run of another"
    );
    assert_eq!(
        after.epoch, before.epoch,
        "the sighting epoch of one context is not advanced by another"
    );
    assert_eq!(
        after.schema_version, work.schema_version,
        "each graph carries its own schema, migrated on its own"
    );
}

/// The name becomes part of a database file name, so a separator or a way upwards is
/// refused where the file is read, not cleaned up and used anyway.
#[test]
fn a_context_name_that_could_leave_the_data_directory_is_refused() {
    for name in ["work/personal", "..", "../escape", ".hidden"] {
        let tree = Tree::new("bad-name");
        tree.write_config(&format!(
            "[workspace.\"{name}\"]\ninclude = [\"{}\"]\n",
            tree.covered(WORK_DIRECTORIES[0])
        ));

        let refused = tree.crosskit(&["index", "jobs"]);
        assert_eq!(refused.status.code(), Some(1), "{name:?} must be refused");
        let message = String::from_utf8_lossy(&refused.stderr);
        assert!(
            message.contains("context name"),
            "{name:?}: the refusal must name the problem: {message}"
        );
        assert!(tree.databases().is_empty(), "{name:?} must open nothing");
    }
}

/// A graph written before contexts existed records nothing about which of its files belong
/// where. Giving it to a context would put unassigned files inside one, so it is refused and
/// the operator is told what to do instead.
#[test]
fn a_graph_written_before_contexts_is_refused_rather_than_adopted() {
    let tree = Tree::new("legacy");
    tree.write_config(&format!(
        "[workspace.work]\ninclude = [\"{}\"]\n",
        tree.covered(WORK_DIRECTORIES[0])
    ));
    fs::write(
        tree.data_directory.join("graph.lbug"),
        "a graph from before",
    )
    .unwrap();

    let refused = tree.crosskit(&["index", "jobs"]);

    assert_eq!(refused.status.code(), Some(1));
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(
        message.contains("before contexts existed") && message.contains("index again"),
        "the refusal must say what to do: {message}"
    );
    assert!(tree.databases().is_empty(), "nothing new may be opened");
}
