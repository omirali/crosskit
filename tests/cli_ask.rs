// CLI smoke: `crosskit ask` from the outside, as far as it goes without an engine that
// can run.
//
// What such a run can show is everything except the answer: that a context which keeps its
// content to itself is refused with the line that would change that, that a context which
// allows it is stopped by the engine rather than by anything else, and that the question
// itself is read before any of it happens. The answer needs a working engine and a person
// reading it — that procedure is `docs/ask-quality-run.md`.
//
// Every test here makes sure the engine cannot run, by name: the default engine is a tool
// that may be installed on the machine running the tests, and a test that assumed it was
// absent would be a live call on somebody's subscription.
mod common;

use std::fs;

use common::Sandbox;

/// The key is never set here. Every test in this file asserts what happens before one would
/// be used, and a key in the environment would silently turn them into live calls.
fn without_a_key(sandbox: &Sandbox, arguments: &[&str]) -> std::process::Output {
    sandbox
        .command(arguments)
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap()
}

/// A context that says nothing about sending content has not said yes, and the refusal says
/// what would change that. Nothing is opened, nothing is read, nothing is sent.
#[test]
fn a_context_that_keeps_its_content_refuses_to_ask_and_says_what_would_change_it() {
    let sandbox = Sandbox::new("ask-denied");
    let refused = without_a_key(&sandbox, &["ask", "что это за проект?"]);

    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("keeps its content to itself"), "{stderr}");
    assert!(
        stderr.contains("send_content_outside = true"),
        "the refusal must name the line that changes it: {stderr}"
    );
    assert!(
        stderr.contains("[workspace.sandbox]"),
        "and the context it belongs to: {stderr}"
    );
}

/// A context that allows content to leave gets as far as the engine, and no further. The
/// message names what is missing and nothing about its value.
///
/// The engine is named explicitly here, because the default one is a tool that may well be
/// installed on the machine running the tests — and then this would be a live call.
#[test]
fn a_context_that_allows_content_stops_at_the_engine_that_cannot_run() {
    let sandbox = Sandbox::new("ask-no-key");
    sandbox.write_config_sections(
        "[analysis]\nengine = \"anthropic-api\"\n\n",
        "send_content_outside = true\n",
    );

    let refused = without_a_key(&sandbox, &["ask", "что это за проект?"]);

    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("ANTHROPIC_API_KEY"), "{stderr}");
    assert!(
        !stderr.contains("keeps its content"),
        "this context did allow it: {stderr}"
    );
}

/// The other engine's own absence, said the same way: the tool is not on PATH, so nothing
/// was sent and nothing was asked. `PATH` is emptied rather than the tool uninstalled, which
/// is the only way a test can be sure of the answer on any machine.
#[test]
fn a_context_whose_engine_is_not_installed_says_so_and_sends_nothing() {
    let sandbox = Sandbox::new("ask-no-tool");
    sandbox.write_config_sections("", "send_content_outside = true\n");

    let refused = sandbox
        .command(&["ask", "что это за проект?"])
        .env("PATH", "")
        .output()
        .unwrap();

    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("claude-code"), "{stderr}");
    assert!(stderr.contains("PATH"), "{stderr}");
    assert!(
        !stderr.contains("ANTHROPIC_API_KEY"),
        "this installation did not choose that engine: {stderr}"
    );
}

/// The permission is checked before the key: a context that refuses is refused for its own
/// reason, whether or not a key is configured.
#[test]
fn the_permission_is_refused_before_the_key_is_looked_for() {
    let sandbox = Sandbox::new("ask-denied-with-key");

    let refused = sandbox
        .command(&["ask", "что это за проект?"])
        .env("ANTHROPIC_API_KEY", "not-a-real-key")
        .output()
        .unwrap();

    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("keeps its content to itself"), "{stderr}");
    assert!(!stderr.contains("ANTHROPIC_API_KEY"), "{stderr}");
}

/// A question that is not one is refused before anything opens, and the refusal is about
/// the question rather than about the model.
#[test]
fn a_question_that_is_not_one_is_refused_first() {
    let sandbox = Sandbox::new("ask-empty");
    sandbox.write_config_sections("", "send_content_outside = true\n");

    let no_arguments = without_a_key(&sandbox, &["ask"]);
    assert!(!no_arguments.status.success());
    assert!(
        String::from_utf8_lossy(&no_arguments.stderr).contains("Usage:"),
        "{:?}",
        String::from_utf8_lossy(&no_arguments.stderr)
    );

    let blank = without_a_key(&sandbox, &["ask", "   "]);
    assert!(!blank.status.success());
    assert!(
        String::from_utf8_lossy(&blank.stderr).contains("no words in it"),
        "{:?}",
        String::from_utf8_lossy(&blank.stderr)
    );

    let long = "а".repeat(1_001);
    let too_long = without_a_key(&sandbox, &["ask", &long]);
    assert!(!too_long.status.success());
    assert!(
        String::from_utf8_lossy(&too_long.stderr).contains("asked rather than sent"),
        "{:?}",
        String::from_utf8_lossy(&too_long.stderr)
    );
}

/// The words outside the options make up the question, so it needs no quoting beyond what
/// the shell already did.
#[test]
fn the_words_of_the_question_are_taken_as_they_were_typed() {
    let sandbox = Sandbox::new("ask-words");
    sandbox.write_config_sections(
        "[analysis]\nengine = \"anthropic-api\"\n\n",
        "send_content_outside = true\n",
    );
    fs::write(
        sandbox.documents.join("notes.md"),
        "Crosskit keeps this note.",
    )
    .unwrap();

    let refused = without_a_key(&sandbox, &["ask", "что", "делает", "сверка"]);

    // It got as far as the engine, which means the question parsed.
    assert!(String::from_utf8_lossy(&refused.stderr).contains("ANTHROPIC_API_KEY"));
}

/// Asking in a context that does not exist is refused like every other command's context.
#[test]
fn a_context_that_is_not_configured_is_refused() {
    let sandbox = Sandbox::new("ask-unknown-context");

    let refused = without_a_key(&sandbox, &["--context", "nowhere", "ask", "вопрос"]);

    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("nowhere"),
        "{:?}",
        String::from_utf8_lossy(&refused.stderr)
    );
}
