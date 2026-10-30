// CLI smoke: what a second command may do while `index watch` holds a context.
//
// The storage engine locks the database file for a writer and takes no lock at all for a
// reader, so a command that writes nothing can open the same database read-only and answer
// from it. That is what `search` and `doctor` do here, against a watcher that is running
// in another process.
//
// What a read-only opening cannot do is read what the writer has committed but not yet
// checkpointed: the engine refuses a write-ahead log it is not the author of while that log
// is being written. So a command that lands in the middle of a pass fails instead of
// answering — with the failure of the write open in front of it, because that is the one an
// operator acts on. These tests read between passes, which is where a watcher spends its
// time.
mod common;

use std::fs;

use common::{Sandbox, WatchProcess};

/// A sandbox whose database has been written and closed once, which is what leaves it
/// checkpointed: nothing of it is left in a log only its writer can replay.
fn indexed(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    sandbox.reconcile();
    sandbox
}

fn watching(sandbox: &Sandbox) -> WatchProcess {
    let mut watch = WatchProcess::spawn(sandbox);
    watch.wait_for_line("Watching ");
    watch.wait_for_line("Pass 1 (bootstrap) completed");
    watch
}

/// The plain case, and the one the limitation was about: a watcher is running, and a search
/// in another process answers from the same context.
#[test]
fn a_search_answers_while_a_watcher_holds_the_context() {
    let sandbox = indexed("concurrent-search");
    let watch = watching(&sandbox);

    let found = sandbox.crosskit(&["search", "keeps"]);
    assert!(
        found.status.success(),
        "search failed while the watcher was running: {}",
        String::from_utf8_lossy(&found.stderr)
    );
    let stdout = String::from_utf8(found.stdout).unwrap();
    assert!(stdout.contains("Results: 1"), "{stdout}");
    assert!(stdout.contains("notes.md"), "{stdout}");

    watch.interrupt();
    let (status, _, stderr) = watch.wait_for_exit();
    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
}

/// `doctor` answers too, and says that what it reports was read from a database somebody
/// else is writing to.
#[test]
fn doctor_answers_while_a_watcher_holds_the_context_and_says_so() {
    let sandbox = indexed("concurrent-doctor");
    let watch = watching(&sandbox);

    let reported = sandbox.crosskit(&["doctor"]);
    assert!(
        reported.status.success(),
        "doctor failed while the watcher was running: {}",
        String::from_utf8_lossy(&reported.stderr)
    );
    let stdout = String::from_utf8(reported.stdout).unwrap();
    assert!(
        stdout.contains("another process is writing to this context"),
        "doctor must say that what it read is a snapshot of another process's work: {stdout}"
    );
    assert!(stdout.contains("Schema version: "), "{stdout}");
    assert!(
        stdout.contains("Search index: 1 documents against 1 chunks"),
        "{stdout}"
    );

    watch.interrupt();
    let (status, _, stderr) = watch.wait_for_exit();
    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
}

/// A reader opens the database again for every command, so what the watcher wrote after the
/// previous search is in the next one. Nothing here is a copy that ages.
#[test]
fn a_later_search_sees_what_the_watcher_indexed_in_between() {
    let sandbox = indexed("concurrent-fresh");
    let mut watch = watching(&sandbox);
    let before = sandbox.crosskit(&["search", "переиндексация"]);
    assert!(before.status.success());
    assert!(String::from_utf8_lossy(&before.stdout).contains("Results: 0"));

    fs::write(
        sandbox.documents.join("added.md"),
        "переиндексация под наблюдением",
    )
    .unwrap();
    // The watcher may or may not receive change hints in this environment; either way it
    // reconciles, and the line it prints is what says the pass is over.
    watch.wait_for_line(") completed");

    let after = sandbox.crosskit(&["search", "переиндексация"]);
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
    let stdout = String::from_utf8(after.stdout).unwrap();
    assert!(stdout.contains("Results: 1"), "{stdout}");
    assert!(stdout.contains("added.md"), "{stdout}");

    watch.interrupt();
    let (status, _, stderr) = watch.wait_for_exit();
    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
}

/// A command that writes is still one writer at a time. What it gets is the busy category
/// in this program's words — the context is being indexed right now — and not the storage
/// engine's sentence about a lock, which says nothing about what to do.
#[test]
fn a_writing_command_is_still_refused_while_a_watcher_holds_the_context() {
    let sandbox = indexed("concurrent-writer");
    let watch = watching(&sandbox);

    let refused = sandbox.crosskit(&["index", "reconcile", &sandbox.documents_argument()]);
    assert!(
        !refused.status.success(),
        "a second writer must not reach a database a watcher holds"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("is being indexed by another process right now"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("Could not set lock"),
        "the engine's words are not an instruction: {stderr}"
    );

    watch.interrupt();
    let (status, _, stderr) = watch.wait_for_exit();
    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
}

/// A reading command meets the same writer and has two honest answers: the page, or that
/// the context is being indexed right now. Which one depends on whether the pass in flight
/// has written anything the reader cannot replay yet, so the test states the pair rather
/// than pretending to know — what it does state is that a reader never sees the storage
/// engine's own sentence, which is what this stage was about.
#[test]
fn a_reading_command_answers_or_says_the_context_is_busy_but_never_quotes_the_engine() {
    let sandbox = indexed("concurrent-reader-busy");
    let mut watch = watching(&sandbox);
    fs::write(sandbox.documents.join("added.md"), "ещё один файл").unwrap();
    // Asked while the pass this change triggers may still be running.
    let answer = sandbox.crosskit(&["search", "keeps"]);
    watch.wait_for_line(") completed");

    let stdout = String::from_utf8_lossy(&answer.stdout);
    let stderr = String::from_utf8_lossy(&answer.stderr);
    if answer.status.success() {
        assert!(stdout.contains("Results: "), "{stdout}");
    } else {
        assert!(
            stderr.contains("is being indexed by another process right now"),
            "{stderr}"
        );
    }
    assert!(
        !stderr.contains("Corrupted wal") && !stderr.contains("Could not set lock"),
        "the engine's words must not reach the operator: {stderr}"
    );

    watch.interrupt();
    let (status, _, stderr) = watch.wait_for_exit();
    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
}

/// Recovery is for a context nobody is writing to. While a watcher holds this one, it is
/// refused as busy and moves nothing: the log it would move aside is the one the pass in
/// flight is writing.
#[test]
fn a_recovery_is_refused_while_a_watcher_holds_the_context() {
    let sandbox = indexed("concurrent-recover");
    let watch = watching(&sandbox);

    let refused = sandbox.crosskit(&["index", "recover"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("is being indexed by another process right now"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        aside_logs(&sandbox).is_empty(),
        "nothing may have been moved"
    );

    watch.interrupt();
    let (status, _, stderr) = watch.wait_for_exit();
    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
}

/// Every log this context's data directory holds that a recovery moved aside.
fn aside_logs(sandbox: &Sandbox) -> Vec<String> {
    fs::read_dir(&sandbox.data_directory)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".wal.aside-"))
        .collect()
}
