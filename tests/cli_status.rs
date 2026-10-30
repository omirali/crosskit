// `crosskit status` end to end: every context in one answer, and nothing written.
//
// Two things are checked here that nothing else can check. One is that the four states a
// context can be in are told apart and reported side by side, rather than the first broken
// one ending the command — a report that stops at the context needing attention hides the
// three that do not. The other is that this command never writes: not a database for a
// context nobody has indexed, not a migration, not a search index, not a checkpoint. That
// is asked of the disk, before and after, byte for byte.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use common::{Sandbox, WatchProcess, snapshot};

/// Files enough for a pass to commit before it can finish, as in the unclean-stop tests.
const FILES: usize = 600;

/// The pass is killed once its log holds this much: past the first batch and far short of
/// the roughly 16 MiB at which the engine folds the log in on its own.
const KILL_AFTER_LOG_BYTES: u64 = 1_000_000;

const PASS_WAIT: Duration = Duration::from_secs(120);

fn status(sandbox: &Sandbox) -> String {
    let output = sandbox.crosskit(&["status"]);
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The block `status` prints for one context.
fn block_for<'a>(status: &'a str, context: &str) -> &'a str {
    status
        .split("\n\n")
        .find(|block| block.starts_with(&format!("{context}\n")))
        .unwrap_or_else(|| panic!("no block for {context} in:\n{status}"))
}

/// Every entry of a directory with what a write would change: its size and the moment it
/// was last modified.
/// A sandbox whose policy file declares four contexts over four directories: the shape the
/// four-state test needs, and one this sandbox does not write on its own.
struct FourContexts {
    sandbox: Sandbox,
    directories: PathBuf,
}

impl FourContexts {
    const NAMES: [&'static str; 4] = ["ordinary", "unindexed", "stopped", "busy"];

    fn new(test_name: &str) -> Self {
        let sandbox = Sandbox::new(test_name);
        let directories = sandbox.documents.clone();
        let mut sections = String::new();
        for name in Self::NAMES {
            let directory = directories.join(name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("note.md"),
                format!("заметка контекста {name}"),
            )
            .unwrap();
            sections.push_str(&format!(
                "[workspace.{name}]\ninclude = [\"{}\"]\n\n",
                fs::canonicalize(&directory).unwrap().display()
            ));
        }
        // The sandbox's own note lies directly in the documents directory, which no context
        // covers here; removing it keeps every file in exactly one context.
        fs::remove_file(directories.join("notes.md")).unwrap();
        fs::write(sandbox.data_directory.join("config.toml"), sections).unwrap();
        Self {
            sandbox,
            directories,
        }
    }

    fn directory(&self, name: &str) -> String {
        self.directories.join(name).to_string_lossy().into_owned()
    }

    fn index(&self, name: &str) {
        let output =
            self.sandbox
                .crosskit(&["--context", name, "index", "init", &self.directory(name)]);
        assert!(
            output.status.success(),
            "indexing {name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn log_of(&self, name: &str) -> PathBuf {
        self.sandbox
            .data_directory
            .join(format!("graph-{name}.lbug.wal"))
    }
}

/// A pass killed in the middle, which is what leaves a log the next process cannot replay.
/// The fixture of the unclean-stop tests, used here for the state rather than the repair.
fn kill_a_pass_in_the_middle(sandbox: &Sandbox, context: &str, directory: &str, log: &Path) {
    let mut pass = sandbox
        .command(&["--context", context, "index", "init", directory])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + PASS_WAIT;
    loop {
        if log.metadata().map(|meta| meta.len()).unwrap_or(0) > KILL_AFTER_LOG_BYTES {
            break;
        }
        assert!(
            pass.try_wait().unwrap().is_none(),
            "the pass finished before it could be killed; {FILES} files were not enough"
        );
        assert!(
            Instant::now() < deadline,
            "the pass never committed a batch"
        );
        thread::sleep(Duration::from_millis(10));
    }
    pass.kill().unwrap();
    pass.wait().unwrap();
}

fn write_corpus(directory: &Path) {
    const WORDS: [&str; 4] = ["обход", "индекса", "сверка", "хранилище"];
    for index in 0..FILES {
        let mut text = String::with_capacity(4096);
        for word in 0..600 {
            text.push_str(WORDS[(index + word) % WORDS.len()]);
            text.push(' ');
        }
        fs::write(directory.join(format!("note-{index:04}.md")), text).unwrap();
    }
}

/// The report, over four contexts in four different states at once.
///
/// A watcher holds one, a killed pass left a log beside another, a third was never indexed
/// and the fourth is ordinary. Every one of them gets its line, the command succeeds, and
/// the two that need attention say what to run.
#[test]
fn four_contexts_in_four_states_are_reported_side_by_side() {
    let four = FourContexts::new("four-states");
    four.index("ordinary");
    four.index("busy");

    // A pass killed mid-write: enough files that it commits before it can finish.
    write_corpus(&four.directories.join("stopped"));
    kill_a_pass_in_the_middle(
        &four.sandbox,
        "stopped",
        &four.directory("stopped"),
        &four.log_of("stopped"),
    );
    assert!(
        four.log_of("stopped").is_file(),
        "the killed pass must have left a log"
    );

    // And a writer holding one of them.
    let mut watch = WatchProcess::spawn_for(&four.sandbox, "busy", &four.directory("busy"));
    watch.wait_for_line("Watching ");
    watch.wait_for_line("Pass 1 (bootstrap) completed");

    let reported = status(&four.sandbox);

    assert!(reported.contains("Contexts: 4"), "{reported}");

    let ordinary = block_for(&reported, "ordinary");
    assert!(ordinary.contains("State: indexed"), "{ordinary}");
    assert!(
        ordinary.contains("Files: 1 (1 revisions kept)"),
        "{ordinary}"
    );
    assert!(ordinary.contains("Schema version: "), "{ordinary}");
    assert!(
        ordinary.contains("Insight jobs: queued 1, claimed 0, done 0, failed 0, dead 0"),
        "{ordinary}"
    );
    assert!(
        ordinary.contains("Search index: 1 documents against 1 chunks"),
        "{ordinary}"
    );
    assert!(
        ordinary.contains("Search index written by: the rules this build reads with"),
        "whether the analyzer matches is an answer in both directions: {ordinary}"
    );

    let unindexed = block_for(&reported, "unindexed");
    assert!(unindexed.contains("State: not indexed yet"), "{unindexed}");
    assert!(unindexed.contains("index init"), "{unindexed}");

    let stopped = block_for(&reported, "stopped");
    assert!(
        stopped.contains("State: a pass was stopped without closing this context"),
        "{stopped}"
    );
    assert!(
        stopped.contains("index recover"),
        "a state with no repair beside it is a dead end: {stopped}"
    );

    let busy = block_for(&reported, "busy");
    assert!(
        busy.contains("held between passes by a writer"),
        "a context a writer holds is a line, not a refusal: {busy}"
    );
    assert!(busy.contains("Files: 1"), "{busy}");

    watch.interrupt();
    let (exit, _, stderr) = watch.wait_for_exit();
    assert_eq!(exit.code(), Some(0), "stderr: {stderr}");
}

/// The command reports. It creates no database for a context nobody has indexed, writes no
/// search index, migrates nothing and checkpoints nothing — asked of the disk rather than
/// of the wording, over the same four states.
#[test]
fn reporting_writes_nothing_in_any_state() {
    let four = FourContexts::new("read-only");
    four.index("ordinary");
    write_corpus(&four.directories.join("stopped"));
    kill_a_pass_in_the_middle(
        &four.sandbox,
        "stopped",
        &four.directory("stopped"),
        &four.log_of("stopped"),
    );

    // The search index of an indexed context, removed: a command that opened one would
    // create it again, and an empty index is both a write and a wrong answer.
    let search = four.sandbox.data_directory.join("search-ordinary");
    fs::remove_dir_all(&search).unwrap();

    let before = snapshot(&four.sandbox.data_directory);
    let documents = snapshot(&four.directories);

    let reported = status(&four.sandbox);
    // Twice, because a command that creates on the first run and reports on the second
    // would pass a single-run comparison.
    let again = status(&four.sandbox);

    assert_eq!(
        snapshot(&four.sandbox.data_directory),
        before,
        "status wrote to the data directory"
    );
    assert_eq!(
        snapshot(&four.directories),
        documents,
        "status wrote to the tree it reports on"
    );
    assert_eq!(reported, again, "and it answers the same way twice");
    assert!(
        !search.exists(),
        "a missing search index must not be created to be reported on"
    );
    assert!(
        block_for(&reported, "ordinary").contains("Search index: none built yet"),
        "{reported}"
    );
    assert!(
        four.log_of("stopped").is_file(),
        "the log of a stopped pass is not this command's to move"
    );
}

/// A machine with nothing configured is where a person starts, so the report says so and
/// says what to run — rather than failing the way every command that needs a context does.
#[test]
fn a_machine_with_no_contexts_is_told_where_to_start() {
    let sandbox = Sandbox::new("no-contexts");
    fs::write(sandbox.data_directory.join("config.toml"), "").unwrap();

    let reported = status(&sandbox);

    assert!(
        reported.contains("Contexts: none configured."),
        "{reported}"
    );
    assert!(reported.contains("crosskit setup"), "{reported}");
}

/// One context is asked for by name, and then it is the only one answered.
#[test]
fn a_named_context_is_reported_on_its_own() {
    let four = FourContexts::new("named");
    four.index("ordinary");

    let output = four.sandbox.crosskit(&["--context", "ordinary", "status"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reported = String::from_utf8(output.stdout).unwrap();
    assert!(reported.contains("Contexts: 1"), "{reported}");
    assert!(reported.contains("ordinary\n"), "{reported}");
    for other in ["unindexed", "stopped", "busy"] {
        assert!(
            !reported.contains(&format!("\n{other}\n")),
            "{other} was not asked about: {reported}"
        );
    }
}

/// A graph written before contexts existed is one condition of the data directory, and the
/// report says it once.
///
/// It reached `status` through every context, because every context derives its database
/// path from that directory and the refusal lives there. Printed where it was found, it was
/// the same paragraph once per context — four of them on the owner's machine — and not one
/// word about the four contexts, which were in order. The count is the assertion: a report
/// that says one thing four times is the defect, and "contains" would pass on all four.
#[test]
fn a_graph_from_before_contexts_is_reported_once_and_not_once_per_context() {
    let four = FourContexts::new("legacy-graph");
    four.index("ordinary");
    fs::write(
        four.sandbox.data_directory.join("graph.lbug"),
        b"not a graph",
    )
    .unwrap();

    let reported = status(&four.sandbox);

    assert_eq!(
        reported
            .matches("was written before contexts existed")
            .count(),
        1,
        "the condition belongs to the data directory, so it is said once: {reported}"
    );
    assert!(reported.contains("Contexts: 4"), "{reported}");
    for name in FourContexts::NAMES {
        assert!(
            reported.contains(&format!("\n{name}\n")),
            "{name} is configured and the report never named it: {reported}"
        );
    }
}
