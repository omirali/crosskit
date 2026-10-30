// CLI smoke: the three commands that answer from a context write nothing to it.
//
// `search`, `ask` and `doctor` are declared read-only, and until Stage 28 each of them
// opened the graph for writing whenever nothing else held it. A write opening is three
// writes wearing one name: it creates the database of a context nobody has indexed, applies
// every migration this build has that the graph has not, and folds the write-ahead log into
// the database as it closes. So an ordinary search rewrote the graph file it was reading,
// and against a graph from an older build it migrated it — silently, and with no way back,
// because there are no down migrations.
//
// Both halves are asked of the disk rather than of what the command printed. `status`
// printed perfectly true reports the whole time `search` was migrating the context beside
// it, so a test on the words would have watched this happen and said nothing.
mod common;

use std::fs;

use common::{Sandbox, fake_engine, snapshot};

/// A context that may send its content out, and an engine that answers, so `ask` reaches
/// the graph at all: the permission and the engine are both checked before it is opened.
const SENDING: &str = "send_content_outside = true\n";

/// An answer in the shape the tool promises, so the run gets past the call.
const ANSWERS: &str = "cat >/dev/null; printf '%s' '{\"type\":\"result\",\"is_error\":false,\
     \"structured_output\":{\"answer\":\"нечего сказать\",\"sources\":[]},\
     \"usage\":{\"input_tokens\":10,\"output_tokens\":5}}'";

/// A context indexed once and closed, which is a graph at this build's schema with nothing
/// left in a log. Nothing about it needs repairing, migrating or checkpointing — so every
/// byte that moves from here moves because a command opened it for writing.
fn indexed(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    sandbox.write_config_sections("", SENDING);
    sandbox.reconcile();
    sandbox
}

#[test]
fn a_search_does_not_touch_one_byte_of_the_context_it_answers_from() {
    let sandbox = indexed("reads-search");
    let before = snapshot(&sandbox.data_directory);

    let output = sandbox.crosskit(&["search", "Crosskit"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(before, snapshot(&sandbox.data_directory));
}

/// The fourth command that answers from a context, and the newest: it reads the touches of a
/// file and may not pay for them with a write opening.
#[test]
fn asking_where_a_file_was_worked_on_does_not_touch_one_byte() {
    let sandbox = indexed("reads-touches");
    let before = snapshot(&sandbox.data_directory);

    let output = sandbox.crosskit(&["touches", "notes.md"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(before, snapshot(&sandbox.data_directory));
}

#[test]
fn doctor_does_not_touch_one_byte_of_the_context_it_reports_on() {
    let sandbox = indexed("reads-doctor");
    let before = snapshot(&sandbox.data_directory);

    let output = sandbox.crosskit(&["doctor"]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(before, snapshot(&sandbox.data_directory));
}

/// A read-only opening is how `doctor` always opens a context (ADR 0020, 0034), so it may
/// not be read as evidence that somebody else is writing. While it was, `doctor` opened
/// every one of its reports with "another process is writing to this context" whether or
/// not one was — a sentence that was usually false, on the command whose whole job is to say
/// whether things are in order. The true case is checked next door, in
/// `tests/cli_concurrent_read.rs`, with a watcher actually running.
#[test]
fn doctor_says_nothing_about_a_writer_when_there_is_none() {
    let sandbox = indexed("reads-doctor-quiet");

    let output = sandbox.crosskit(&["doctor"]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("another process is writing"),
        "nothing is writing to this context: {stdout}"
    );
    assert!(stdout.contains("Schema version: "), "{stdout}");
}

/// `ask` writes its own answer cache, which is its own directory and not the context's
/// graph. The graph is what this asks about, so the graph is what is compared.
#[test]
fn asking_a_question_does_not_touch_one_byte_of_the_graph() {
    let sandbox = indexed("reads-ask");
    let (root, engine) = fake_engine(ANSWERS);
    let graph = sandbox
        .data_directory
        .join(format!("graph-{}.lbug", Sandbox::CONTEXT));
    let before = snapshot(&graph);

    let output = sandbox
        .command(&["ask", "что известно про Crosskit?"])
        .env("PATH", &engine)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(before, snapshot(&graph));
    fs::remove_dir_all(root).unwrap();
}

/// The other half of the same rule, and the one that cannot be mistaken for a coincidence:
/// a context nobody has indexed does not gain a graph because somebody asked a question
/// about it. Checked against the data directory, like the same rule in `cli_migrate`.
#[test]
fn a_context_nobody_indexed_gains_no_graph_from_a_command_that_only_reads() {
    let sandbox = Sandbox::new("reads-never-indexed");
    sandbox.write_config_sections("", SENDING);
    let (root, engine) = fake_engine(ANSWERS);

    for arguments in [
        vec!["search", "что-нибудь"],
        vec!["doctor"],
        vec!["ask", "что-нибудь?"],
    ] {
        let output = sandbox
            .command(&arguments)
            .env("PATH", &engine)
            .output()
            .unwrap();

        assert!(
            !output.status.success(),
            "{arguments:?} answered about a context that has no graph"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("has not been indexed yet"),
            "{arguments:?} must name the state rather than the engine's words: {stderr}"
        );
        assert!(
            !sandbox.database_exists(),
            "{arguments:?} created the graph it was only supposed to read"
        );
    }
    fs::remove_dir_all(root).unwrap();
}
