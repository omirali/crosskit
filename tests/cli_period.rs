// `search` and `ask` about a period, from the outside: the line that says how the period was
// read, the flags beside a phrase, and a period with nothing in it.
//
// The flags make every period here a fixed set of days, whatever day the test runs on, and
// `TZ` makes the local zone a fixed one. What a phrase means against today's date is held by
// the unit tests, where today is a parameter.
mod common;

use std::fs;

use common::Sandbox;

/// A conversation with replies in two weeks of September 2026, and a file written in the
/// second, indexed into the sandbox context.
fn indexed(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    let notes = fs::canonicalize(&sandbox.documents)
        .unwrap()
        .join("ledger.md");
    fs::write(&notes, "сверка остатков").unwrap();
    let lines = [
        r#"{"type":"user","sessionId":"s","timestamp":"2026-09-15T07:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"сверка пятнадцатого"}]}}"#.to_string(),
        r#"{"type":"user","sessionId":"s","timestamp":"2026-09-23T07:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"сверка двадцать третьего"}]}}"#.to_string(),
        format!(
            r#"{{"type":"assistant","sessionId":"s","timestamp":"2026-09-23T07:05:00.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Edit","input":{{"file_path":"{}"}}}}]}}}}"#,
            notes.display()
        ),
    ];
    fs::write(
        sandbox.documents.join("chat.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
    let output = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    sandbox
}

fn run(sandbox: &Sandbox, arguments: &[&str]) -> (bool, String, String) {
    let output = sandbox
        .command(arguments)
        .env("TZ", "Asia/Almaty")
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// The period line comes first and says the days; every result names the event that put it
/// there, in local time.
#[test]
fn a_search_within_flags_prints_the_period_and_the_event_of_each_result() {
    let sandbox = indexed("period-flags");
    let (ok, stdout, stderr) = run(
        &sandbox,
        &[
            "search",
            "сверка",
            "--since",
            "2026-09-21",
            "--until",
            "2026-09-27",
        ],
    );
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("Period: 21–27 Sep 2026 (--since/--until, local time)"),
        "{stdout}"
    );
    assert!(stdout.contains("Results: 2"), "{stdout}");
    assert!(stdout.contains("chat.jsonl"), "{stdout}");
    assert!(stdout.contains("ledger.md"), "{stdout}");
    assert!(
        stdout.contains("event: edited 2026-09-23 12:05"),
        "{stdout}"
    );
    assert!(stdout.contains("event: reply 2026-09-23 12:00"), "{stdout}");
    assert!(
        stdout.contains("двадцать третьего") && !stdout.contains("пятнадцатого"),
        "the reply of the other week must not be the one shown: {stdout}"
    );
}

/// Flags and a phrase together: the flags are the period, the phrase is named as set aside,
/// and its words are not looked for.
#[test]
fn flags_win_over_a_phrase_and_the_phrase_is_not_looked_for() {
    let sandbox = indexed("period-flags-and-phrase");
    let (ok, stdout, stderr) = run(
        &sandbox,
        &[
            "search",
            "сверка",
            "на",
            "прошлой",
            "неделе",
            "--since",
            "2026-09-14",
            "--until",
            "2026-09-20",
        ],
    );
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains(
            "Period: 14–20 Sep 2026 (--since/--until, local time); the flags win over \"на прошлой неделе\", which was not used"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("Looked for: сверка"), "{stdout}");
    assert!(stdout.contains("пятнадцатого"), "{stdout}");
    assert!(!stdout.contains("ledger.md"), "{stdout}");
}

/// A period nothing happened in says so, and shows nothing from another week instead.
#[test]
fn a_period_with_nothing_in_it_says_so() {
    let sandbox = indexed("period-empty");
    let (ok, stdout, stderr) = run(
        &sandbox,
        &[
            "search",
            "сверка",
            "--since",
            "2026-08-03",
            "--until",
            "2026-08-09",
        ],
    );
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Results: 0"), "{stdout}");
    assert!(
        stdout.contains("Nothing in this context has an event in 3–9 Aug 2026"),
        "{stdout}"
    );
}

/// A query without a period prints exactly what it printed before this stage: no period line,
/// no "looked for", the same results.
#[test]
fn a_query_without_a_period_is_answered_as_before() {
    let sandbox = indexed("period-none");
    let (ok, stdout, stderr) = run(&sandbox, &["search", "сверка"]);
    assert!(ok, "{stderr}");
    assert!(!stdout.contains("Period:"), "{stdout}");
    assert!(!stdout.contains("Looked for:"), "{stdout}");
    assert!(!stdout.contains("event:"), "{stdout}");
    assert!(stdout.contains("Results: 2"), "{stdout}");
}

#[test]
fn a_flag_that_is_not_a_period_is_refused_before_anything_opens() {
    let sandbox = Sandbox::new("period-refused");
    for arguments in [
        &["search", "сверка", "--since", "21.09.2026"][..],
        &["search", "сверка", "--until", "2026-09-27"][..],
        &[
            "search",
            "сверка",
            "--since",
            "2026-09-27",
            "--until",
            "2026-09-21",
        ][..],
        &["ask", "что было", "--until", "2026-09-27"][..],
    ] {
        let (ok, _, stderr) = run(&sandbox, arguments);
        assert!(!ok, "{arguments:?} was accepted");
        assert!(!stderr.is_empty(), "{arguments:?}");
    }
    assert!(!sandbox.database_exists());
}

/// A conversation that grew past the content limit after it was indexed: what it said in the
/// period is in no index, and the command says so in one line instead of a silent nothing.
#[test]
fn a_conversation_that_outgrew_the_limit_is_named_as_not_shown() {
    let sandbox = Sandbox::new("period-outgrew");
    sandbox.write_config("[index]\nmax_content_size_mb = 1\n");
    let chat = sandbox.documents.join("chat.jsonl");
    let first = r#"{"type":"user","sessionId":"s","timestamp":"2026-09-02T07:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"сверка начата"}]}}"#;
    fs::write(&chat, format!("{first}\n")).unwrap();
    let output = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let long = "слово ".repeat(200_000);
    let second = format!(
        r#"{{"type":"user","sessionId":"s","timestamp":"2026-09-23T07:00:00.000Z","message":{{"role":"user","content":[{{"type":"text","text":"сверка {long}"}}]}}}}"#
    );
    fs::write(&chat, format!("{first}\n{second}\n")).unwrap();
    sandbox.reconcile();

    let (ok, stdout, stderr) = run(
        &sandbox,
        &[
            "search",
            "сверка",
            "--since",
            "2026-09-21",
            "--until",
            "2026-09-27",
        ],
    );
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Results: 0"), "{stdout}");
    assert!(
        stdout.contains("Not shown: 1 conversation(s) written to during or after this period"),
        "{stdout}"
    );
}
