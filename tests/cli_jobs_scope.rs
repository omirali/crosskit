// CLI smoke: a run takes the work one directory names, and a recheck asks no model.
//
// The queue is taken oldest revision first, and on a real tree the oldest is rarely what the
// owner wants read today. `--under` takes the revisions under a directory, `--touching` the
// conversations that worked on files there; every other job stays queued. The engine is the
// scripted `claude` of `cli_analysis_engine`.
mod common;

use std::fs;

use common::{Sandbox, assert_counters, fake_engine};

const SENDING: &str = "send_content_outside = true\n";
/// Transcripts get a job only when a rule covers them.
const TRANSCRIPTS_APPROVED: &str = "[analysis]\napproved = [\"**/*.jsonl\"]\n\n";
const ANSWER: &str = r#"cat >/dev/null; printf '%s' '{"type":"result","is_error":false,"total_cost_usd":0.01,"usage":{"input_tokens":10,"output_tokens":5},"structured_output":{"topic":"заметки","confidence":0.8,"entities":[],"relations":[]}}'"#;

/// `registry/` holds two documents, `other/` one, and a conversation edited a file in
/// `registry/` and only read one in `other/`.
fn prepared(name: &str) -> Sandbox {
    prepared_with(name, TRANSCRIPTS_APPROVED)
}

fn prepared_with(name: &str, index_section: &str) -> Sandbox {
    let sandbox = Sandbox::new(name);
    let registry = sandbox.documents.join("registry");
    let other = sandbox.documents.join("other");
    fs::create_dir_all(&registry).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::write(registry.join("a.md"), "Контрагент уникален по БИН.").unwrap();
    fs::write(registry.join("b.md"), "Пакет принадлежит контрагенту.").unwrap();
    fs::write(other.join("c.md"), "Совсем другое.").unwrap();
    let registry = fs::canonicalize(&registry).unwrap();
    let other = fs::canonicalize(&other).unwrap();
    let edited = format!(
        r#"{{"type":"assistant","sessionId":"worked","timestamp":"2026-09-03T07:09:30.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Edit","input":{{"file_path":"{}/a.md"}}}}]}}}}"#,
        registry.display()
    );
    let glanced = format!(
        r#"{{"type":"assistant","sessionId":"glanced","timestamp":"2026-09-03T07:09:30.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Read","input":{{"file_path":"{}/a.md"}}}}]}}}}"#,
        registry.display()
    );
    let elsewhere = format!(
        r#"{{"type":"assistant","sessionId":"elsewhere","timestamp":"2026-09-03T07:09:30.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Edit","input":{{"file_path":"{}/c.md"}}}}]}}}}"#,
        other.display()
    );
    // A transcript is a conversation once somebody says something in it.
    let said = |session: &str| {
        format!(
            r#"{{"type":"user","sessionId":"{session}","timestamp":"2026-09-03T07:09:25.000Z","message":{{"role":"user","content":[{{"type":"text","text":"поправь реестр"}}]}}}}"#
        )
    };
    for (name, line) in [
        ("worked", edited),
        ("glanced", glanced),
        ("elsewhere", elsewhere),
    ] {
        fs::write(
            sandbox.documents.join(format!("{name}.jsonl")),
            format!("{}\n{line}\n", said(name)),
        )
        .unwrap();
    }
    sandbox.write_config_sections(index_section, SENDING);
    sandbox.reconcile();
    sandbox
}

fn run(sandbox: &Sandbox, arguments: &[&str], tool: &str) -> std::process::Output {
    sandbox
        .command(arguments)
        .env("PATH", tool)
        .output()
        .unwrap()
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn directory(sandbox: &Sandbox, name: &str) -> String {
    fs::canonicalize(sandbox.documents.join(name))
        .unwrap()
        .display()
        .to_string()
}

#[test]
fn under_a_directory_takes_its_jobs_and_leaves_the_rest_queued() {
    let sandbox = prepared("scope-under");
    let (root, tool) = fake_engine(ANSWER);
    let registry = directory(&sandbox, "registry");

    let worked = run(
        &sandbox,
        &[
            "index", "jobs", "run", "--under", &registry, "--limit", "10",
        ],
        &tool,
    );

    let text = stdout(&worked);
    assert!(worked.status.success(), "{text}");
    assert_counters(
        &text,
        &[
            &format!("Selected: 2 revisions under {registry}; 2 of them queued"),
            "Completed jobs: 2",
            "Cost: 2 calls",
        ],
    );
    // notes.md, other/c.md and three transcripts are still waiting.
    assert_counters(
        &stdout(&sandbox.crosskit(&["index", "jobs"])),
        &["Insight jobs queued: 5", "Insight jobs done: 2"],
    );
    fs::remove_dir_all(root).unwrap();
}

/// The same pairs and rule as `touches`: the conversation that edited a file there is taken;
/// one that only read it, and one that worked elsewhere, are not.
#[test]
fn touching_a_directory_takes_the_conversations_that_worked_there() {
    let sandbox = prepared("scope-touching");
    let (root, tool) = fake_engine(ANSWER);
    let registry = directory(&sandbox, "registry");

    let worked = run(
        &sandbox,
        &[
            "index",
            "jobs",
            "run",
            "--touching",
            &registry,
            "--limit",
            "10",
        ],
        &tool,
    );

    let text = stdout(&worked);
    assert!(worked.status.success(), "{text}");
    assert_counters(
        &text,
        &[
            &format!(
                "Selected: 1 conversations that worked on files under {registry}, 1 transcript revisions; 1 of them queued"
            ),
            "Completed jobs: 1",
        ],
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_directory_outside_the_context_is_refused_before_anything_is_claimed() {
    let sandbox = prepared("scope-outside");
    let (root, tool) = fake_engine(ANSWER);
    // Beside the context's root, holding none of it.
    let outside = sandbox.data_directory.canonicalize().unwrap();

    let refused = run(
        &sandbox,
        &[
            "index",
            "jobs",
            "run",
            "--under",
            &outside.display().to_string(),
            "--limit",
            "10",
        ],
        &tool,
    );

    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("is not in context sandbox"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_counters(
        &stdout(&sandbox.crosskit(&["index", "jobs"])),
        &["Insight jobs queued: 7", "Insight jobs claimed: 0"],
    );
    fs::remove_dir_all(root).unwrap();
}

/// A recheck proves stored answers again from the cache: the tool it would call is one that
/// leaves a mark, and the mark is not there.
#[test]
fn a_recheck_rewrites_from_the_cache_and_asks_no_model() {
    let sandbox = prepared("scope-recheck");
    let (root, tool) = fake_engine(ANSWER);
    let registry = directory(&sandbox, "registry");
    run(
        &sandbox,
        &[
            "index", "jobs", "run", "--under", &registry, "--limit", "10",
        ],
        &tool,
    );
    let mark = sandbox.data_directory.join("asked");
    let (silent_root, silent) = fake_engine(&format!("touch '{}'; exit 1", mark.display()));

    let rechecked = run(&sandbox, &["index", "jobs", "recheck"], &silent);

    let text = stdout(&rechecked);
    assert!(rechecked.status.success(), "{text}");
    assert_counters(
        &text,
        &[
            "Rechecked: 2 revisions from their cached answers (0 had none, 0 could not be read); no model was asked",
            "Facts: 0 → 0",
        ],
    );
    assert!(!mark.exists(), "the recheck must not start the tool");
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(silent_root).unwrap();
}

/// Without a rule for transcripts the selection says what to do, and one command approves
/// exactly the conversation that worked there — priced before anything is written.
#[test]
fn the_conversations_of_a_directory_are_approved_by_one_command_priced_first() {
    let sandbox = prepared_with("scope-approve", "");
    let (root, tool) = fake_engine(ANSWER);
    let registry = directory(&sandbox, "registry");

    let unapproved = stdout(&run(
        &sandbox,
        &[
            "index",
            "jobs",
            "run",
            "--touching",
            &registry,
            "--limit",
            "10",
        ],
        &tool,
    ));
    assert_counters(
        &unapproved,
        &[
            "1 of them have no job: no analysis rule covers them",
            &format!("`crosskit index review approve --touching {registry}`"),
            "Claimed jobs: 0",
        ],
    );

    let approved = run(
        &sandbox,
        &[
            "index",
            "review",
            "approve",
            "--touching",
            &registry,
            "--yes",
        ],
        &tool,
    );
    let text = stdout(&approved);
    assert!(
        approved.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&approved.stderr)
    );
    assert_counters(
        &text,
        &[
            "To approve: the 1 conversation(s) that worked on files under",
            "1 transcript(s), 1 parts, ",
            "no call has been paid in this context yet, so no price",
            "An approved conversation is asked again every time it grows",
            "Approved: 1 transcript(s)",
            "Queued for analysis: 1 revision(s)",
        ],
    );
    let config = fs::read_to_string(sandbox.data_directory.join("config.toml")).unwrap();
    assert!(config.contains("worked.jsonl\""), "{config}");
    // A reply has no byte range; its size is its text, not zero.
    assert!(!text.contains(" 0 bytes"), "{text}");
    assert!(
        !config.contains("glanced.jsonl"),
        "only the conversation that worked there"
    );

    let worked = stdout(&run(
        &sandbox,
        &[
            "index",
            "jobs",
            "run",
            "--touching",
            &registry,
            "--limit",
            "10",
        ],
        &tool,
    ));
    assert_counters(&worked, &["1 of them queued", "Completed jobs: 1"]);
    fs::remove_dir_all(root).unwrap();
}

/// A revision larger than a run may send unasked stays queued, priced; asked by name, it goes.
#[test]
fn a_large_job_is_held_back_with_its_price_until_it_is_asked_for() {
    let sandbox = prepared("scope-large");
    let (root, tool) = fake_engine(ANSWER);
    let other = directory(&sandbox, "other");
    let paragraph = "Длинный абзац о реестре поставщиков и его сверке. ".repeat(200);
    fs::write(
        sandbox.documents.join("other/long.md"),
        format!("{paragraph}\n\n{paragraph}"),
    )
    .unwrap();
    sandbox.reconcile();

    let held = stdout(&run(
        &sandbox,
        &[
            "index",
            "jobs",
            "run",
            "--under",
            &other,
            "--max-parts",
            "1",
            "--limit",
            "10",
        ],
        &tool,
    ));
    assert_counters(
        &held,
        &[
            "Completed jobs: 1",
            "Held back: 1 job(s) over 1 parts",
            "They stay queued: `--allow-large` takes them",
        ],
    );

    let taken = stdout(&run(
        &sandbox,
        &[
            "index",
            "jobs",
            "run",
            "--under",
            &other,
            "--allow-large",
            "--limit",
            "10",
        ],
        &tool,
    ));
    assert_counters(&taken, &["Completed jobs: 1"]);
    assert!(!taken.contains("Held back"), "{taken}");
    fs::remove_dir_all(root).unwrap();
}

/// What a model answered under an earlier question goes back in the queue, priced, and only
/// that: an answer to the current question stays done.
#[test]
fn an_answer_to_an_earlier_question_is_requeued_and_priced() {
    let sandbox = prepared("scope-requeue");
    let (root, tool) = fake_engine(ANSWER);
    let registry = directory(&sandbox, "registry");
    run(
        &sandbox,
        &[
            "index", "jobs", "run", "--under", &registry, "--limit", "10",
        ],
        &tool,
    );
    let nothing = stdout(&run(&sandbox, &["index", "jobs", "requeue"], &tool));
    assert_counters(&nothing, &["Requeued: nothing"]);

    // One of the two results is made to say it was answered under the previous question.
    {
        let database = lbug::Database::new(
            sandbox
                .data_directory
                .join(format!("graph-{}.lbug", Sandbox::CONTEXT)),
            lbug::SystemConfig::default(),
        )
        .unwrap();
        let connection = lbug::Connection::new(&database).unwrap();
        connection
            .query("MATCH (job:InsightJob)-[:HAS_INSIGHT_RESULT]->(result:InsightResult) WITH result ORDER BY result.id LIMIT 1 SET result.prompt_version = 'revision-knowledge-tool-v1';")
            .unwrap();
    }

    let requeued = stdout(&run(
        &sandbox,
        &["index", "jobs", "requeue", "--under", &registry],
        &tool,
    ));
    assert_counters(
        &requeued,
        &[
            "Requeued: 1 revision(s) a model answered under an earlier question than revision-knowledge-tool-v2",
            "Asking them again: about $0.0100",
        ],
    );
    assert_counters(
        &stdout(&sandbox.crosskit(&["index", "jobs"])),
        &["Insight jobs done: 1"],
    );
    fs::remove_dir_all(root).unwrap();
}
