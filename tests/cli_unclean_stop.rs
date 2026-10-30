// CLI smoke: what a pass that was killed in the middle leaves behind, and the one command
// that brings the context back.
//
// A pass writes the graph in batches and makes its search index visible once, at the end.
// What is left when the process does not reach that end — a Ctrl-C on `index init`, an OOM
// kill, a laptop that lost power — is not an index one batch behind. The engine keeps a
// committed batch in a write-ahead log beside the database and folds that log in only when
// it checkpoints, so a pass that wrote new content and was then killed leaves a log the
// next process cannot replay: the whole context is refused, not just the work in flight.
//
// `crosskit index recover` is the answer to exactly that state. It moves the log aside and
// keeps it, says what is not coming back, and leaves the graph to be written again from the
// files on disk — which is what makes the loss time rather than data.
//
// A pass that only re-observed files that had not changed does not do this to the database;
// that difference is measured in ADR 0014.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use common::{Sandbox, WatchProcess};

/// Files enough for a pass to commit several batches before it can finish, and small
/// enough for the whole test to be a smoke test. A batch is at most 100 sources.
const FILES: usize = 600;

/// The pass is killed once its log holds this much: far past the first batch and far
/// short of the roughly 16 MiB at which the engine folds the log into the database on its
/// own. Both ends matter — before the first batch nothing would have been committed, and
/// after a checkpoint the killed pass would leave a database that opens.
const KILL_AFTER_LOG_BYTES: u64 = 1_000_000;

const PASS_WAIT: Duration = Duration::from_secs(120);

#[test]
fn a_pass_killed_in_the_middle_is_recovered_and_the_next_ordinary_pass_writes_it_again() {
    let sandbox = Sandbox::new("unclean-stop");
    write_corpus(&sandbox);
    let log = log_path(&sandbox);

    let killed = kill_a_pass_in_the_middle(&sandbox, &log);
    assert!(
        killed > KILL_AFTER_LOG_BYTES,
        "the pass must have committed before it was killed"
    );

    // No command opens this context now, and what the operator is told is this program's
    // own sentence with the command that answers it — not the engine's words about a log
    // record, and not the silence of a process that died. How the engine refuses is not one
    // thing: on Linux it died of a signal on three databases out of six left this way, and
    // the check that runs before the open is what turns that into a sentence.
    let refused = sandbox.crosskit(&["doctor"]);
    assert!(
        !refused.status.success(),
        "a database left by a killed pass must not be opened as if nothing happened: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(
        message.contains("index recover"),
        "a refusal without the command that repairs it is the dead end this stage removed: \
         {message:?}"
    );
    assert!(
        !message.contains("Corrupted wal") && !message.contains("Checksum"),
        "the engine's words are not an instruction: {message}"
    );
    assert!(log.is_file(), "the log the operator has to move aside");
    let log_bytes = log.metadata().unwrap().len();

    // The repair, in one command.
    let recovered = sandbox.crosskit(&["index", "recover"]);
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let stdout = String::from_utf8(recovered.stdout).unwrap();
    assert!(
        stdout.contains("Recovered: the graph opens again."),
        "{stdout}"
    );
    assert!(
        stdout.contains("does not come back"),
        "the command says what the repair costs: {stdout}"
    );

    // Moved, not deleted: the same bytes, under a name with the moment in it.
    assert!(
        !log.is_file(),
        "the log the engine refuses must be out of the way"
    );
    let kept = aside_logs(&sandbox);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(
        kept[0].metadata().unwrap().len(),
        log_bytes,
        "the log was moved, and moving is not editing"
    );

    // A first pass killed before any checkpoint takes the migrations down with the log, so
    // what `recover` leaves is a graph recording no schema at all. A command that writes
    // nothing names that and the repair for it rather than migrating it on the way past
    // (Stage 28), which is how an ordinary `doctor` used to change a context's schema.
    let refused = sandbox.crosskit(&["doctor"]);
    assert!(
        !refused.status.success(),
        "a reader must not read a graph through a schema it was not written with"
    );
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("schema version 0"), "{message}");
    assert!(message.contains("crosskit migrate"), "{message}");

    // The repair the refusal names, asked for once and by name (ADR 0031).
    assert!(
        sandbox.crosskit(&["migrate"]).status.success(),
        "the command the refusal names has to be the command that works"
    );
    let opened = doctor(&sandbox);
    assert!(
        opened.contains("Search index: 0 documents against 0 chunks"),
        "the killed pass left nothing behind, in either half: {opened}"
    );

    let repaired = sandbox.crosskit(&["index", "reconcile", &sandbox.documents_argument()]);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    assert!(
        String::from_utf8_lossy(&repaired.stdout)
            .contains(&format!("Indexed files (metadata and content): {FILES}")),
        "the ordinary pass reads every file again: {}",
        String::from_utf8_lossy(&repaired.stdout)
    );

    let healed = doctor(&sandbox);
    let (documents, chunks) = index_counts(&healed);
    assert!(
        documents == chunks && documents >= FILES,
        "one ordinary pass, no rebuild: {healed}"
    );
    assert!(healed.contains("in step with the graph"), "{healed}");
    let found = search(&sandbox, "sentinel0");
    assert!(found.contains("Results: 1"), "{found}");
    assert!(found.contains("note-0000.md"), "{found}");
}

/// Starts `index init` and kills it once it has committed, returning the size of the log
/// it left. `SIGKILL`, because that is what the cases this is about look like: the process
/// gets no chance to close anything.
fn kill_a_pass_in_the_middle(sandbox: &Sandbox, log: &Path) -> u64 {
    let mut pass = sandbox
        .command(&["index", "init", &sandbox.documents_argument()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + PASS_WAIT;
    let size = loop {
        let size = log.metadata().map(|meta| meta.len()).unwrap_or(0);
        if size > KILL_AFTER_LOG_BYTES {
            break size;
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
    };
    pass.kill().unwrap();
    pass.wait().unwrap();
    size
}

fn write_corpus(sandbox: &Sandbox) {
    const WORDS: [&str; 8] = [
        "обход",
        "индекса",
        "сверка",
        "хранилище",
        "файл",
        "версия",
        "чанк",
        "локация",
    ];
    for index in 0..FILES {
        let mut text = String::with_capacity(4096);
        for word in 0..600 {
            text.push_str(WORDS[(index + word) % WORDS.len()]);
            text.push(' ');
        }
        text.push_str(&format!("\nsentinel{index}\n"));
        fs::write(sandbox.documents.join(format!("note-{index:04}.md")), text).unwrap();
    }
    // The sandbox writes one note of its own; the corpus is what this test counts.
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
}

/// Every log a recovery moved aside, as full paths.
fn aside_logs(sandbox: &Sandbox) -> Vec<PathBuf> {
    fs::read_dir(&sandbox.data_directory)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".wal.aside-"))
        })
        .collect()
}

/// The write-ahead log of this context's graph, beside the database itself.
fn log_path(sandbox: &Sandbox) -> PathBuf {
    sandbox
        .data_directory
        .join(format!("graph-{}.lbug.wal", Sandbox::CONTEXT))
}

/// The two numbers of the search index line: documents in the index against chunks of the
/// current versions the graph holds. A file of this corpus is cut into more than one chunk,
/// so what the test states is that the two agree, not how many there are.
fn index_counts(doctor: &str) -> (usize, usize) {
    let line = doctor
        .lines()
        .find(|line| line.starts_with("Search index: "))
        .unwrap_or_else(|| panic!("no search index line in:\n{doctor}"));
    let numbers: Vec<usize> = line
        .split_whitespace()
        .filter_map(|word| word.parse().ok())
        .collect();
    match numbers.as_slice() {
        [documents, chunks, ..] => (*documents, *chunks),
        _ => panic!("no counts in {line:?}"),
    }
}

fn doctor(sandbox: &Sandbox) -> String {
    let output = sandbox.crosskit(&["doctor"]);
    assert!(
        output.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn search(sandbox: &Sandbox, query: &str) -> String {
    let output = sandbox.crosskit(&["search", query]);
    assert!(
        output.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A context that opens is not a context to repair. The command says so, moves nothing and
/// leaves the graph exactly as it was.
#[test]
fn recovering_a_context_that_opens_changes_nothing() {
    let sandbox = Sandbox::new("unclean-healthy");
    fs::write(sandbox.documents.join("notes.md"), "обычная заметка").unwrap();
    let indexed = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(indexed.status.success());
    let before = doctor(&sandbox);

    let recovered = sandbox.crosskit(&["index", "recover"]);

    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let stdout = String::from_utf8(recovered.stdout).unwrap();
    assert!(stdout.contains("Nothing to recover"), "{stdout}");
    assert!(aside_logs(&sandbox).is_empty(), "nothing may have moved");
    assert_eq!(doctor(&sandbox), before, "and nothing may have changed");
    assert!(search(&sandbox, "заметка").contains("Results: 1"));
}

/// A context nobody has indexed has no database, and asking after one must not create it:
/// a question that answers itself by changing the answer is not a diagnosis.
#[test]
fn recovering_a_context_with_no_database_creates_none() {
    let sandbox = Sandbox::new("unclean-absent");

    let recovered = sandbox.crosskit(&["index", "recover"]);

    assert!(recovered.status.success());
    assert!(
        String::from_utf8_lossy(&recovered.stdout).contains("Nothing to recover"),
        "{}",
        String::from_utf8_lossy(&recovered.stdout)
    );
    assert!(!sandbox.database_exists(), "no database may have been made");
}

/// The third way a pass ends early, and the one a real tree produced: the process died of a
/// panic rather than of a signal.
///
/// A panic unwinds, so the database is closed on the way out and its log is folded in. What
/// is left is therefore not the state above — nothing refuses to open, and `index recover`
/// has nothing to move aside. What is left is a graph holding the sources the pass had
/// reached and a search index holding nothing, because the index is published once, at the
/// end of a pass the panic never reached. That was measured on the panic this test's
/// sibling in `cli_wide_spaces` now prevents: 0 documents against 800 chunks, with the
/// database opening normally.
///
/// **What this test does not do is panic.** The panic that produced this state has been
/// fixed and no input reaches it any more, so the state is set up by publishing a pass and
/// then taking the search index away. Reaching it by a real panic would need a way to make
/// the binary panic on demand, and production code does not carry test hooks.
///
/// That is enough, because the state is what the repair acts on, and the state was measured
/// on the real panic before the fix: no write-ahead log, a database that opens, and a graph
/// holding what the pass had written while the index held nothing — `0 documents against
/// 800 chunks`. Both halves of what a panic leaves are therefore covered: that it leaves no
/// log is a property of unwinding, which the fixture cannot fake and does not need to,
/// and what an ordinary pass does about the rest is what is asserted below.
#[test]
fn a_graph_left_ahead_of_its_search_index_is_brought_back_by_an_ordinary_reconcile() {
    let sandbox = Sandbox::new("unclean-panic");
    write_corpus(&sandbox);
    let indexed = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    fs::remove_dir_all(search_index_path(&sandbox)).unwrap();

    let behind = doctor(&sandbox);
    let (documents, chunks) = index_counts(&behind);
    assert_eq!(documents, 0, "the pass published no index: {behind}");
    assert!(
        chunks >= FILES,
        "the graph kept what the pass wrote: {behind}"
    );
    assert!(behind.contains("behind the graph"), "{behind}");

    // Nothing to recover: a panic leaves no log, so the command that answers a killed pass
    // must not claim there is something here for it to repair.
    let recovered = sandbox.crosskit(&["index", "recover"]);
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(
        String::from_utf8_lossy(&recovered.stdout).contains("Nothing to recover"),
        "{}",
        String::from_utf8_lossy(&recovered.stdout)
    );
    assert!(aside_logs(&sandbox).is_empty(), "nothing may have moved");

    // The ordinary pass, not `index rebuild --search`.
    let repaired = sandbox.crosskit(&["index", "reconcile", &sandbox.documents_argument()]);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );

    let healed = doctor(&sandbox);
    let (documents, chunks) = index_counts(&healed);
    assert!(
        documents == chunks && documents >= FILES,
        "one ordinary pass, no rebuild: {healed}"
    );
    assert!(healed.contains("in step with the graph"), "{healed}");
    let found = search(&sandbox, "sentinel0");
    assert!(found.contains("Results: 1"), "{found}");
}

/// The search index of this context, beside the graph it belongs to.
fn search_index_path(sandbox: &Sandbox) -> PathBuf {
    sandbox
        .data_directory
        .join(format!("search-{}", Sandbox::CONTEXT))
}

/// The other half of the same question: a watcher killed between its passes.
///
/// A watcher may not close its database for days, and until it does, everything it has
/// written lives in the log — so before this stage, killing an idle watcher lost every pass
/// it had ever run and left the context unopenable. It now folds each pass into the
/// database before reporting it, so a kill costs at most the pass in flight, and between
/// passes there is no log to refuse at all.
#[test]
fn a_watcher_killed_between_passes_keeps_what_its_passes_wrote() {
    let sandbox = Sandbox::new("unclean-watch");
    write_corpus(&sandbox);
    let mut watch = WatchProcess::spawn(&sandbox);
    watch.wait_for_line("Watching ");
    watch.wait_for_line("Pass 1 (bootstrap) completed");
    // Between passes: the pass is reported only once it is folded in.
    assert!(
        !log_path(&sandbox).exists(),
        "a watcher waiting for the next change has nothing left in its log"
    );

    watch.kill();

    let opened = doctor(&sandbox);
    let (documents, chunks) = index_counts(&opened);
    assert!(
        documents == chunks && documents >= FILES,
        "the bootstrap pass survived the kill whole: {opened}"
    );
    assert!(opened.contains("in step with the graph"), "{opened}");
    let found = search(&sandbox, "sentinel0");
    assert!(found.contains("Results: 1"), "{found}");
    assert!(
        aside_logs(&sandbox).is_empty(),
        "nothing had to be recovered"
    );
}
