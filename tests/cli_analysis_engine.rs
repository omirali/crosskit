// CLI smoke: the engine that answers is an installed tool, and every way that can go wrong.
//
// The tool is stood in for by a script named `claude` on a PATH of its own. That is not a
// fake of the provider — the real adapter runs, spawns a real process, writes to its
// standard input, reads its standard output and maps what comes back. Only the thing at the
// other end is scripted, which is the only part a test must not reach over the network.
//
// The live call is `a_live_call_through_the_installed_tool`, ignored by default.
mod common;

use std::fs;

use common::{Sandbox, assert_counters, fake_engine as tool_that};

/// A context that may send its content out, so the engine is reached at all.
const SENDING: &str = "send_content_outside = true\n";

/// One answer in the shape the tool gives: the envelope, with the schema-checked object
/// under `structured_output`.
fn answering(topic: &str, confidence: &str) -> String {
    format!(
        "cat >/dev/null; printf '%s' '{{\"type\":\"result\",\"is_error\":false,\
         \"structured_output\":{{\"topic\":\"{topic}\",\"confidence\":{confidence},\
         \"entities\":[],\"relations\":[]}},\
         \"usage\":{{\"input_tokens\":10,\"output_tokens\":5}}}}'"
    )
}

fn run_jobs(sandbox: &Sandbox, tool_directory: &str) -> String {
    let output = sandbox
        .command(&["index", "jobs", "run", "--limit", "5"])
        .env("PATH", tool_directory)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{}\n{stdout}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn queue(sandbox: &Sandbox) -> String {
    String::from_utf8(sandbox.crosskit(&["index", "jobs"]).stdout).unwrap()
}

/// What the next pass decides about what the last one failed. A failed job sits in `failed`
/// until a pass recovers it, and recovery is the first thing every pass does — so this is
/// that pass, taking no jobs of its own.
///
/// The tool still has to be reachable: a pass that cannot build a handler says so and stops
/// before it recovers anything, which is the same rule that keeps it from claiming.
fn recover(sandbox: &Sandbox, tool_directory: &str) {
    let output = sandbox
        .command(&["index", "jobs", "run", "--limit", "0"])
        .env("PATH", tool_directory)
        .output()
        .unwrap();
    assert!(output.status.success());
}

fn prepared(name: &str) -> Sandbox {
    let sandbox = Sandbox::new(name);
    sandbox.write_config_sections("", SENDING);
    sandbox.reconcile();
    sandbox
}

#[test]
fn an_answer_in_the_shape_the_tool_promises_is_taken_and_the_job_is_done() {
    let sandbox = prepared("engine-ok");
    let (root, directory) = tool_that(&answering("заметки о проекте", "0.8"));

    let worked = run_jobs(&sandbox, &directory);

    assert_counters(
        &worked,
        &["Claimed jobs: 1", "Completed jobs: 1", "Failed jobs: 0"],
    );
    assert_counters(&queue(&sandbox), &["Insight jobs done: 1"]);
    fs::remove_dir_all(root).unwrap();
}

/// A tool that says it failed is a temporary failure: the window of a subscription moves,
/// the machine unloads, the runtime stops being busy. The job goes back in the queue with
/// the attempt counted, which is what an operator running the next batch needs.
#[test]
fn a_tool_that_reports_trouble_leaves_the_job_to_be_tried_again() {
    let sandbox = prepared("engine-temporary");
    let (root, directory) = tool_that(
        "cat >/dev/null; printf '%s' '{\"type\":\"result\",\"is_error\":true,\
         \"subtype\":\"error_during_execution\"}'",
    );

    let worked = run_jobs(&sandbox, &directory);

    assert_counters(
        &worked,
        &["Claimed jobs: 1", "Completed jobs: 0", "Failed jobs: 1"],
    );
    assert_counters(&queue(&sandbox), &["Insight jobs failed: 1"]);

    recover(&sandbox, &directory);

    assert_counters(
        &queue(&sandbox),
        &["Insight jobs queued: 1", "Insight jobs dead: 0"],
    );
    fs::remove_dir_all(root).unwrap();
}

/// A refusal is not. Asking the same model the same thing again is the same refusal, so the
/// job is retired rather than kept coming back for the rest of the queue's life.
#[test]
fn a_refusal_retires_the_job_instead_of_asking_again() {
    let sandbox = prepared("engine-refusal");
    let (root, directory) = tool_that(
        "cat >/dev/null; printf '%s' '{\"type\":\"result\",\"is_error\":true,\
         \"subtype\":\"refusal\"}'",
    );

    let worked = run_jobs(&sandbox, &directory);

    assert_counters(&worked, &["Failed jobs: 1"]);

    recover(&sandbox, &directory);

    assert_counters(
        &queue(&sandbox),
        &["Insight jobs queued: 0", "Insight jobs dead: 1"],
    );
    fs::remove_dir_all(root).unwrap();
}

/// Output that is not the shape this program accepts is refused whole. The schema the tool
/// enforces is a promise; the check on this side is the boundary, and a promise that was
/// not kept costs the job rather than reaching the graph.
#[test]
fn output_that_is_not_the_agreed_shape_is_refused_and_nothing_of_it_is_printed() {
    let sandbox = prepared("engine-schema");
    let (root, directory) = tool_that(
        "cat >/dev/null; printf '%s' 'I am a helpful assistant and here is your topic: notes'",
    );

    let worked = run_jobs(&sandbox, &directory);

    assert_counters(&worked, &["Failed jobs: 1"]);
    assert!(
        !worked.contains("helpful assistant"),
        "nothing the tool wrote may be printed:\n{worked}"
    );

    recover(&sandbox, &directory);

    assert_counters(&queue(&sandbox), &["Insight jobs dead: 1"]);
    fs::remove_dir_all(root).unwrap();
}

/// The tool is not on this machine. The reason is said once and no job is claimed: claiming
/// would count an attempt against a call that was never going to happen.
#[test]
fn a_tool_that_is_not_installed_claims_nothing_and_says_why_once() {
    let sandbox = prepared("engine-absent");

    let output = sandbox
        .command(&["index", "jobs", "run", "--limit", "5"])
        .env("PATH", "")
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("claude-code"), "{stdout}");
    assert!(stdout.contains("No job was claimed."), "{stdout}");
    assert_eq!(stdout.lines().count(), 2, "one context line, one reason");
    assert_counters(&queue(&sandbox), &["Insight jobs queued: 1"]);
}

/// A context that keeps its content to itself never builds a call, whatever engine is
/// configured and whatever is installed. The deterministic handler works the queue instead,
/// exactly as before this stage: the permission rule did not move one letter.
#[test]
fn a_context_that_keeps_its_content_never_reaches_the_tool() {
    let sandbox = Sandbox::new("engine-denied");
    sandbox.reconcile();
    // A tool that would fail loudly if it were ever started.
    let (root, directory) = tool_that("exit 97");

    let worked = run_jobs(&sandbox, &directory);

    assert!(
        worked.contains("local (this context does not send content outside)"),
        "{worked}"
    );
    assert_counters(&worked, &["Completed jobs: 1", "Failed jobs: 0"]);
    fs::remove_dir_all(root).unwrap();
}

/// The topic reaches a question as a field of its own, never spliced into the excerpt.
///
/// A label glued into the text would be indistinguishable from a sentence the file
/// contains, and a model told to answer only from the excerpts would then be answering from
/// us. So this asserts what actually goes down the pipe: the tool writes its input to a
/// file, and the test reads it.
#[test]
fn a_topic_reaches_the_question_as_its_own_field_and_not_inside_the_text() {
    let sandbox = Sandbox::new("engine-ask-topic");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    fs::write(
        sandbox.documents.join("readme.md"),
        "Склад на улице Промышленной сдан в аренду на три года.\n",
    )
    .unwrap();
    sandbox.write_config_sections("", SENDING);
    sandbox.reconcile();

    // One pass of the queue, so the document has a topic at all.
    let (labeller, labelling) = tool_that(&answering("аренда склада", "0.9"));
    run_jobs(&sandbox, &labelling);

    // Then a question, with a tool that keeps what it was asked.
    let asked = std::env::temp_dir().join(format!("crosskit-asked-{}", std::process::id()));
    let (root, directory) = tool_that(&format!(
        "cat > {}; printf '%s' '{{\"type\":\"result\",\"is_error\":false,\
         \"structured_output\":{{\"answer\":\"на три года\",\"sources\":[1]}}}}'",
        asked.display()
    ));
    let answered = sandbox
        .command(&["ask", "на сколько сдан склад?"])
        .env("PATH", &directory)
        .output()
        .unwrap();
    assert!(
        answered.status.success(),
        "{}",
        String::from_utf8_lossy(&answered.stderr)
    );

    let sent = fs::read_to_string(&asked).unwrap();
    assert!(
        sent.contains("about: аренда склада"),
        "the topic must travel as its own labelled line:\n{sent}"
    );
    assert!(
        sent.contains("text:"),
        "and the excerpt's text must be labelled apart from it:\n{sent}"
    );
    // The label is above the text, not inside it.
    let about = sent.find("about: аренда склада").unwrap();
    let text = sent.find("text:").unwrap();
    assert!(about < text, "the label belongs above the text:\n{sent}");

    fs::remove_file(&asked).unwrap();
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(labeller).unwrap();
}

/// The live call, against the tool actually installed. Ignored by default: it spends the
/// subscription of whoever runs it. Without the tool it says so and passes, because a test
/// that cannot reach its subject has not found a defect.
#[test]
#[ignore = "live call: cargo test --offline --test cli_analysis_engine -- --ignored --nocapture"]
fn a_live_call_through_the_installed_tool() {
    let Some(path) = std::env::var_os("PATH") else {
        return println!("skipped: no PATH");
    };
    let installed = std::env::split_paths(&path).any(|directory| directory.join("claude").exists());
    if !installed {
        return println!("skipped: `claude` is not installed on this machine");
    }
    let sandbox = prepared("engine-live");
    fs::write(
        sandbox.documents.join("notes.md"),
        "Договор аренды склада на три года с правом пролонгации.\n",
    )
    .unwrap();
    sandbox.reconcile();

    let worked = String::from_utf8(
        sandbox
            .crosskit(&["index", "jobs", "run", "--limit", "1"])
            .stdout,
    )
    .unwrap();

    println!("{worked}");
    assert!(worked.contains("engine claude-code"), "{worked}");
    assert!(worked.contains("Completed jobs: 1"), "{worked}");
}
