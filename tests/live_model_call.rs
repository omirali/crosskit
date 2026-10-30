// One real call to a real model, so the request this program builds is known to be a
// request the service actually accepts.
//
// It is ignored, so no gate runs it: it needs a key, a network and money, and none of the
// three says anything about whether the code around it is right — every other property of
// the model path is checked against a fake provider, where a refusal, a timeout and a
// malformed answer can all be produced on demand.
//
// Without a key it does not fail and does not pretend to have run: it says why it was
// skipped and stops. A test that quietly passes without doing anything is worse than no
// test, so the reason is printed rather than swallowed.
mod common;

use std::fs;

use common::Sandbox;

const KEY: &str = "ANTHROPIC_API_KEY";

#[test]
#[ignore = "live model call: cargo test --offline --test live_model_call -- --ignored --nocapture"]
fn a_real_model_answers_the_request_this_program_builds() {
    let Ok(key) = std::env::var(KEY) else {
        println!("skipped: {KEY} is not set, so there is nothing to call");
        return;
    };
    if key.is_empty() {
        println!("skipped: {KEY} is empty, so there is nothing to call");
        return;
    }

    let sandbox = Sandbox::new("live-model");
    // A context that says its content may leave, which is the only kind that reaches a
    // model at all.
    sandbox.write_config_sections("", "send_content_outside = true\n");
    fs::write(
        sandbox.documents.join("notes.md"),
        "Q3 budget review. Marketing spend is 12% over plan; engineering is 4% under. \
         Decision: freeze new contractor hiring until the next quarter.",
    )
    .unwrap();
    sandbox.reconcile();

    let output = sandbox
        .command(&["index", "jobs", "run", "--limit", "1"])
        .env(KEY, key)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    println!("{stdout}");
    assert!(
        output.status.success(),
        "the command failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Analysis: model "),
        "the model path must be the one that ran:\n{stdout}"
    );
    // A completed job is the whole assertion: the request was accepted, the answer came
    // back in the agreed shape and passed the domain's own check. What the model said the
    // document is about is not asserted — that is the model's judgement, not this
    // program's contract.
    assert!(
        stdout.contains("Completed jobs: 1"),
        "the job must have been completed by the model:\n{stdout}"
    );
    assert!(
        stdout.contains("Failed jobs: 0"),
        "no job may have failed:\n{stdout}"
    );
}
