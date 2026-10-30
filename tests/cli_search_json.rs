// `crosskit search --json` and `crosskit contexts --json` from the outside: the machine
// answer the panel reads, and the human answer it must leave alone.
//
// Two promises, checked through the whole program:
//
//   * without the flag nothing changes — the text a person reads is compared byte for byte
//     with the text this command printed before the flag existed;
//   * with it, stdout is one object of one schema, a failure included, and nothing else.
mod common;

use std::fs;
use std::process::Output;

use common::Sandbox;
use serde_json::Value;

/// One small transcript, written here and not taken from anybody's machine: a name its owner
/// gave, a session id, and two replies on a fixed day.
const TRANSCRIPT: &[&str] = &[
    r#"{"type":"custom-title","sessionId":"5e55-10n","customTitle":"Взаимозачёты в payments-api"}"#,
    r#"{"type":"user","sessionId":"5e55-10n","timestamp":"2026-09-03T07:09:25.457Z","message":{"role":"user","content":[{"type":"text","text":"посмотри как считаются взаимозачёты"}]}}"#,
    r#"{"type":"assistant","sessionId":"5e55-10n","timestamp":"2026-09-03T07:09:30.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Взаимозачёты считаются в модуле расчётов, keeps them apart."}]}}"#,
];

/// A sandbox with a note, a transcript, a file that is gone and a file the policy hides —
/// every line the text page can print about what it did not show.
fn indexed(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    fs::write(
        sandbox.documents.join("session.jsonl"),
        format!("{}\n", TRANSCRIPT.join("\n")),
    )
    .unwrap();
    fs::write(
        sandbox.documents.join("gone.md"),
        "Crosskit keeps what is gone.",
    )
    .unwrap();
    fs::write(
        sandbox.documents.join("ledger.confidential"),
        "Crosskit keeps a ledger.",
    )
    .unwrap();
    fs::write(
        sandbox.documents.join("plan.md"),
        "The plan keeps two notes apart.",
    )
    .unwrap();
    let output = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        output.status.success(),
        "index init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_file(sandbox.documents.join("gone.md")).unwrap();
    sandbox.reconcile();
    sandbox.write_config("[index]\nexclude = [\"**/*.confidential\"]\n");
    sandbox
}

/// The binary with the zone fixed, so a period and its events print the same everywhere.
fn run(sandbox: &Sandbox, arguments: &[&str]) -> Output {
    sandbox
        .command(arguments)
        .env("TZ", "UTC")
        .output()
        .unwrap()
}

/// What a run printed, with the two things that differ between machines and runs named
/// instead of spelled: where the sandbox is, and the cursor, which carries a file id.
fn normalized(sandbox: &Sandbox, output: &Output) -> String {
    let documents = fs::canonicalize(&sandbox.documents).unwrap();
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    let text = text.replace(&documents.display().to_string(), "<documents>");
    text.split_inclusive('\n')
        .map(|line| match line.starts_with("Next page: --after ") {
            true => "Next page: --after <cursor>\n".to_string(),
            false => line.to_string(),
        })
        .collect()
}

/// The text of `search` as it was before `--json` existed, captured from that build on this
/// fixture. A change here is a change a person reads, and has to be one somebody meant.
const TEXT_BEFORE_THE_FLAG: &str = r#"$ search keeps
Context: sandbox
Query: keeps
Results: 3 (at most 10 a page)
1. [present] <documents>/notes.md
   Crosskit keeps this note
   matched: keeps
2. [present] <documents>/plan.md
   The plan keeps two notes apart
   matched: keeps
3. [present] <documents>/session.jsonl
   Взаимозачёты считаются в модуле расчётов, keeps them apart
   matched: keeps
Files with no location left, not shown: 1 (add --include-missing)
Files at paths this context's policy no longer admits, not shown: 1
$ search keeps --limit 1
Context: sandbox
Query: keeps
Results: 1 (at most 1 a page)
1. [present] <documents>/notes.md
   Crosskit keeps this note
   matched: keeps
Next page: --after <cursor>
$ search взаимозачёты
Context: sandbox
Query: взаимозачёты
Results: 1 (at most 10 a page)
1. [present] <documents>/session.jsonl
   Взаимозачёты в payments-api
   matched: Взаимозачёты
   and in 2 more places in this file
$ search взаимозачёты --since 2026-09-01 --until 2026-09-07
Context: sandbox
Query: взаимозачёты
Period: 1–7 Sep 2026 (--since/--until, local time)
Results: 1 (at most 10 a page)
1. [present] <documents>/session.jsonl
   event: reply 2026-09-03 07:09
   Взаимозачёты в payments-api
   matched: Взаимозачёты
   and in 2 more places in this file
$ search keeps --include-missing
Context: sandbox
Query: keeps
Results: 4 (at most 10 a page)
1. [present] <documents>/notes.md
   Crosskit keeps this note
   matched: keeps
2. [missing] <documents>/gone.md
   Crosskit keeps what is gone
   matched: keeps
3. [present] <documents>/plan.md
   The plan keeps two notes apart
   matched: keeps
4. [present] <documents>/session.jsonl
   Взаимозачёты считаются в модуле расчётов, keeps them apart
   matched: keeps
Files at paths this context's policy no longer admits, not shown: 1
$ search nothing-here-at-all
Context: sandbox
Query: nothing-here-at-all
Results: 0 (at most 10 a page)
$ search 
Context: sandbox
Empty query: nothing to look for.
"#;

#[test]
fn without_the_flag_the_text_is_byte_for_byte_what_it_was() {
    let sandbox = indexed("json-text-unchanged");
    let mut printed = Vec::new();
    for arguments in [
        &["search", "keeps"][..],
        &["search", "keeps", "--limit", "1"],
        &["search", "взаимозачёты"],
        &[
            "search",
            "взаимозачёты",
            "--since",
            "2026-09-01",
            "--until",
            "2026-09-07",
        ],
        &["search", "keeps", "--include-missing"],
        &["search", "nothing-here-at-all"],
        &["search", ""],
    ] {
        let output = run(&sandbox, arguments);
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        printed.push(format!(
            "$ {}\n{}",
            arguments.join(" "),
            normalized(&sandbox, &output)
        ));
    }
    let printed = printed.concat();
    assert_eq!(printed, TEXT_BEFORE_THE_FLAG, "\n{printed}");
}

/// The schema number every answer of this build carries.
const SCHEMA: u64 = 2;

/// The one object a machine answer is: exactly one line on stdout, whatever happened.
fn answer(sandbox: &Sandbox, arguments: &[&str]) -> (Value, Output) {
    let output = run(sandbox, arguments);
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(
        stdout.lines().count(),
        1,
        "{arguments:?} must print exactly one line on stdout:\n{stdout}"
    );
    let object: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(object["schema"], SCHEMA, "{stdout}");
    (object, output)
}

fn paths(page: &Value) -> Vec<String> {
    page["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["path"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_page_says_what_each_row_is_and_what_was_not_shown() {
    let sandbox = indexed("json-page");
    let (page, output) = answer(&sandbox, &["search", "keeps", "--json"]);
    assert!(output.status.success());
    assert_eq!(page["context"], Sandbox::CONTEXT);
    assert_eq!(page["query"], "keeps");
    assert_eq!(page["period"], Value::Null);
    assert_eq!(
        page["next"],
        Value::Null,
        "a page that did not fill has no next"
    );
    assert_eq!(page["not_shown"]["by_policy"], 1);
    assert_eq!(page["not_shown"]["missing"], 1);
    assert_eq!(page["not_shown"]["lost_by_graph"], 0);
    assert_eq!(page["index"]["written_by_this_build"], true);

    let rows = page["results"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "{page}");
    for row in rows {
        let is_transcript = row["path"].as_str().unwrap().ends_with("session.jsonl");
        assert_eq!(
            row["kind"],
            if is_transcript {
                "conversation"
            } else {
                "file"
            },
            "{row}"
        );
        assert_eq!(row["conversation"].is_null(), !is_transcript, "{row}");
        assert_eq!(row["state"], "present");
        assert_eq!(row["matched"], serde_json::json!(["keeps"]));
    }
    let conversation = rows
        .iter()
        .find(|row| row["kind"] == "conversation")
        .unwrap();
    assert_eq!(conversation["conversation"]["session_id"], "5e55-10n");
    assert_eq!(
        conversation["conversation"]["name"],
        "Взаимозачёты в payments-api"
    );
    assert_eq!(conversation["conversation"]["message_count"], 2);
    // 07:09:30 UTC on 3 September 2026, the moment of the last reply.
    assert_eq!(
        conversation["conversation"]["last_message_at_unix_ms"],
        1_788_419_370_000_i64
    );
}

/// Following `next` page by page yields the one-page answer in the same order, and a cursor
/// from the machine answer is the same cursor the text page takes.
#[test]
fn the_cursor_goes_there_and_back() {
    let sandbox = indexed("json-cursor");
    let (whole, _) = answer(
        &sandbox,
        &["search", "keeps", "--json", "--include-missing"],
    );

    let mut walked = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut arguments = vec![
            "search",
            "keeps",
            "--json",
            "--include-missing",
            "--limit",
            "1",
        ];
        if let Some(cursor) = after.as_deref() {
            arguments.extend(["--after", cursor]);
        }
        let (page, _) = answer(&sandbox, &arguments);
        walked.extend(paths(&page));
        match page["next"].as_str() {
            Some(next) => after = Some(next.to_string()),
            None => break,
        }
        assert!(walked.len() <= 4, "the walk must end: {walked:?}");
    }
    assert_eq!(walked, paths(&whole));

    let (present, _) = answer(&sandbox, &["search", "keeps", "--json"]);
    let (first, _) = answer(&sandbox, &["search", "keeps", "--json", "--limit", "1"]);
    let cursor = first["next"].as_str().unwrap();
    let text = run(
        &sandbox,
        &["search", "keeps", "--limit", "1", "--after", cursor],
    );
    let second = &paths(&present)[1];
    assert!(
        String::from_utf8(text.stdout)
            .unwrap()
            .contains(second.as_str()),
        "the text page must continue where the machine page stopped"
    );
}

/// A refusal is an object of the schema with a category from a closed list. The sentence a
/// person reads stays on stderr, and nothing on stdout names a path.
#[test]
fn a_failure_is_an_object_with_a_category() {
    let sandbox = indexed("json-failure");
    let root = fs::canonicalize(&sandbox.documents).unwrap();
    for (arguments, category) in [
        (
            &["--context", "elsewhere", "search", "keeps", "--json"][..],
            "unknown-context",
        ),
        (&["search", "\"unclosed", "--json"], "unreadable-query"),
        (
            &["search", "keeps", "--after", "not-a-cursor", "--json"],
            "usage",
        ),
        (&["search", "--json", "--limit"], "usage"),
    ] {
        let (refused, output) = answer(&sandbox, arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert_eq!(
            refused["error"]["category"], category,
            "{arguments:?}: {refused}"
        );
        assert!(refused.get("results").is_none(), "{refused}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.contains(&root.display().to_string()), "{stdout}");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .starts_with("crosskit failed: "),
            "{arguments:?}: the person's sentence is still on stderr"
        );
    }

    let never_indexed = Sandbox::new("json-never-indexed");
    let (refused, _) = answer(&never_indexed, &["search", "keeps", "--json"]);
    assert_eq!(refused["error"]["category"], "not-indexed");
    assert_eq!(refused["error"]["context"], Sandbox::CONTEXT);
}

#[test]
fn a_query_with_nothing_to_look_for_is_an_empty_page_without_an_index() {
    let sandbox = Sandbox::new("json-empty-query");
    let (page, output) = answer(&sandbox, &["search", "", "--json"]);
    assert!(output.status.success());
    assert_eq!(page["results"], serde_json::json!([]));
    assert_eq!(page["index"], Value::Null);
    assert!(!sandbox.database_exists(), "an empty query opens nothing");
}

#[test]
fn the_contexts_are_listed_from_the_configuration_alone() {
    let sandbox = Sandbox::new("json-contexts");
    let (listed, output) = answer(&sandbox, &["contexts", "--json"]);
    assert!(output.status.success());
    assert_eq!(listed["contexts"], serde_json::json!([Sandbox::CONTEXT]));
    assert!(
        !sandbox.database_exists(),
        "naming the contexts opens none of them"
    );
}
