// A transcript indexed as a conversation, from the outside: the whole program, its own
// command line, a real database and a real search index.
//
// Three things are checked here and nothing else, because they are the three the stage
// promised and they are only observable from out here:
//
//   * a conversation is found by its own name, which is the field Stage 31 measured at 0.80
//     strict against 0.00 for the body alone;
//   * a conversation is found by a path it touched, which is what "in which chat did we touch
//     this file" has to mean before it can be asked;
//   * the tool output is gone. That is the whole point of the profile — 2.46% of a transcript
//     is speech and the rest was being indexed as if it were.
mod common;

use std::fs;

use common::Sandbox;

/// One transcript, small and written here rather than taken from anybody's machine.
///
/// It holds every shape the stage cares about: a name the model gave, a name the owner gave
/// afterwards, both parties speaking, a tool call with a file path, a tool this build has no
/// verb for, a file-history snapshot, and — the line that must disappear — tool output
/// carrying a word that appears nowhere else.
const TRANSCRIPT: &[&str] = &[
    r#"{"type":"ai-title","sessionId":"s1","aiTitle":"Implement peer-to-peer debt tracking"}"#,
    r#"{"type":"custom-title","sessionId":"s1","customTitle":"Беседа Е"}"#,
    r#"{"type":"user","sessionId":"s1","timestamp":"2026-09-03T07:09:25.457Z","message":{"role":"user","content":[{"type":"text","text":"посмотри как считаются взаимозачёты"}]}}"#,
    r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-09-03T07:09:30.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"надо открыть модуль"},{"type":"tool_use","name":"Read","input":{"file_path":"/work/ledger/settlement.rs"}},{"type":"text","text":"Смотрю модуль расчётов."}]}}"#,
    r#"{"type":"user","sessionId":"s1","toolUseResult":{"stdout":"КРАКОЗЯБРА"},"message":{"role":"user","content":[{"type":"tool_result","content":"КРАКОЗЯБРА КРАКОЗЯБРА КРАКОЗЯБРА"}]}}"#,
    r#"{"type":"assistant","sessionId":"s1","message":{"role":"assistant","content":[{"type":"tool_use","name":"Write","input":{"file_path":"/work/ledger/settlement.rs"}}]}}"#,
    r#"{"type":"assistant","sessionId":"s1","message":{"role":"assistant","content":[{"type":"tool_use","name":"str_replace_editor","input":{"file_path":"/work/ledger/unknown.rs"}}]}}"#,
    r#"{"type":"file-history-snapshot","messageId":"m","snapshot":{"messageId":"m","trackedFileBackups":{"/work/ledger/report.rs":{"x":1}}}}"#,
];

/// A sandbox holding one transcript beside the note every sandbox starts with.
fn with_a_transcript(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    fs::write(
        sandbox.documents.join("session.jsonl"),
        format!("{}\n", TRANSCRIPT.join("\n")),
    )
    .unwrap();
    let output = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        output.status.success(),
        "index init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    sandbox
}

fn search(sandbox: &Sandbox, query: &str) -> String {
    let output = sandbox.crosskit(&["search", query]);
    assert!(
        output.status.success(),
        "search {query:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The name the owner gave wins over the name the model gave, and the conversation is found
/// by it. Stage 31 measured this on one question of the fifteen that could not be answered
/// without it.
#[test]
fn a_conversation_is_found_by_the_name_its_owner_gave_it() {
    let sandbox = with_a_transcript("conversations-name");
    let stdout = search(&sandbox, "Беседа Е");
    assert!(
        stdout.contains("session.jsonl"),
        "беседа должна находиться по собственному имени:\n{stdout}"
    );
}

/// The model's name is in the transcript too and still reaches the conversation: the owner's
/// name wins in the graph, and the model's name is an ordinary reply-less line that the name
/// field does not carry — so this passes through the body, which is the point. A question
/// asked in either wording finds the same file.
#[test]
fn the_name_field_does_not_take_the_place_of_the_body() {
    let sandbox = with_a_transcript("conversations-body");
    let stdout = search(&sandbox, "взаимозачёты");
    assert!(
        stdout.contains("session.jsonl"),
        "слова владельца должны оставаться в индексе:\n{stdout}"
    );
}

/// The whole reason the profile exists. The tool output held a word that appears nowhere
/// else in the transcript; after Stage 32 the index does not hold it.
#[test]
fn the_output_of_a_tool_is_no_longer_in_the_index() {
    let sandbox = with_a_transcript("conversations-tool-output");
    let stdout = search(&sandbox, "кракозябра");
    assert!(
        stdout.contains("Results: 0"),
        "вывод инструмента обязан исчезнуть из индекса:\n{stdout}"
    );
}

/// "In which chat did we touch this file?" — asked of the graph, because a path is a string
/// there and not an edge, and answered by the conversation rather than by the transcript's
/// own text. The path appears in no reply, so a match can only come from a `FileTouch`.
#[test]
fn the_files_a_conversation_touched_are_in_the_graph_with_their_verbs() {
    let sandbox = with_a_transcript("conversations-touches");
    let touches = sandbox.graph_rows(
        "MATCH (:Conversation)-[:TOUCHED_FILE]->(touch:FileTouch) RETURN touch.path, touch.verb, touch.source ORDER BY touch.path, touch.verb;",
    );
    assert_eq!(
        touches,
        vec![
            "/work/ledger/report.rs|edited|file_history".to_string(),
            "/work/ledger/settlement.rs|read|tool_input".to_string(),
            "/work/ledger/settlement.rs|wrote|tool_input".to_string(),
        ],
        "ожидались три ссылки: две от аргументов инструментов и одна из снимка истории"
    );
}

/// A tool this build has no verb for writes nothing. The transcript names
/// `/work/ledger/unknown.rs` through `str_replace_editor`, and the graph must not claim to
/// know what happened to it.
#[test]
fn a_tool_without_a_known_verb_puts_nothing_in_the_graph() {
    let sandbox = with_a_transcript("conversations-unknown-tool");
    let touches = sandbox.graph_rows(
        "MATCH (touch:FileTouch) WHERE touch.path = '/work/ledger/unknown.rs' RETURN touch.path;",
    );
    assert!(
        touches.is_empty(),
        "инструмент без известного глагола не должен ничего записывать: {touches:?}"
    );
}

/// The conversation itself: named, by whom, how many messages and when they ran.
#[test]
fn the_conversation_carries_its_name_its_session_and_its_span() {
    let sandbox = with_a_transcript("conversations-shape");
    let rows = sandbox.graph_rows(
        "MATCH (conversation:Conversation) RETURN conversation.session_id, conversation.name, conversation.named_by, conversation.message_count;",
    );
    assert_eq!(rows, vec!["s1|Беседа Е|owner|2".to_string()]);

    let roles = sandbox.graph_rows(
        "MATCH (:Conversation)-[:HAS_MESSAGE]->(message:Message) RETURN message.ordinal, message.role ORDER BY message.ordinal;",
    );
    assert_eq!(roles, vec!["0|owner".to_string(), "1|model".to_string()]);
}

/// Verdict И of the stage: no row of one context names a node of another. The check is that
/// every edge a conversation has lands inside the database it was written in — which is the
/// guarantee ADR 0008 makes physical, restated as an assertion so a later change cannot take
/// it away quietly.
#[test]
fn nothing_a_conversation_writes_reaches_outside_its_own_database() {
    let sandbox = with_a_transcript("conversations-isolation");

    // Every conversation's revision is in this database.
    let dangling = sandbox.graph_rows(
        "MATCH (conversation:Conversation) WHERE NOT EXISTS { MATCH (conversation)-[:EVIDENCED_BY]->(:FileVersion) } RETURN conversation.id;",
    );
    assert!(
        dangling.is_empty(),
        "беседа без ревизии-источника: {dangling:?}"
    );

    // A touch names a path and never a node. The path of the only file this transcript
    // touched lies outside the indexed tree entirely, and that is exactly the case an edge
    // could not have represented.
    let outside = sandbox.graph_rows(
        "MATCH (touch:FileTouch) WHERE NOT EXISTS { MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation) WHERE location.normalized_path = touch.path } RETURN touch.path ORDER BY touch.path;",
    );
    assert_eq!(
        outside.len(),
        3,
        "все три пути ведут за пределы этого контекста, и строка — единственное, чем их можно \
         записать: {outside:?}"
    );
}

/// A second transcript, for the command that answers "where did we work on this file".
///
/// It holds the three cases the default answer is made of: a file the owner named and the
/// agent only read, a file the agent read twice and nobody ever named, and a file edited
/// without being named. The name `settlement.rs` is deliberately the same as in the first
/// transcript's directory, so a tail can match two different files.
const WORKED_ON: &[&str] = &[
    r#"{"type":"ai-title","sessionId":"s2","aiTitle":"Разбор расчётов"}"#,
    r#"{"type":"user","sessionId":"s2","timestamp":"2026-09-04T07:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"посмотри settlement.rs, там расчёт неверный"}]}}"#,
    r#"{"type":"assistant","sessionId":"s2","message":{"role":"assistant","content":[{"type":"tool_use","name":"Read","input":{"file_path":"/other/ledger/settlement.rs"}}]}}"#,
    r#"{"type":"assistant","sessionId":"s2","message":{"role":"assistant","content":[{"type":"tool_use","name":"Read","input":{"file_path":"/other/ledger/registry.rs"}}]}}"#,
    r#"{"type":"assistant","sessionId":"s2","message":{"role":"assistant","content":[{"type":"tool_use","name":"Read","input":{"file_path":"/other/ledger/registry.rs"}}]}}"#,
    r#"{"type":"assistant","sessionId":"s2","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/other/ledger/totals.rs"}}]}}"#,
];

/// Both transcripts in one context: two conversations, and one file name that belongs to two
/// different files.
fn with_two_transcripts(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    fs::write(
        sandbox.documents.join("session.jsonl"),
        format!("{}\n", TRANSCRIPT.join("\n")),
    )
    .unwrap();
    fs::write(
        sandbox.documents.join("worked-on.jsonl"),
        format!("{}\n", WORKED_ON.join("\n")),
    )
    .unwrap();
    let output = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        output.status.success(),
        "index init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    sandbox
}

fn touches(sandbox: &Sandbox, arguments: &[&str]) -> String {
    let mut all = vec!["touches"];
    all.extend_from_slice(arguments);
    let output = sandbox.crosskit(&all);
    assert!(
        output.status.success(),
        "touches {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The question the direction was for, from the outside: the owner names a file and is told
/// which conversation worked on it.
#[test]
fn a_file_is_answered_with_the_conversation_that_worked_on_it() {
    let sandbox = with_two_transcripts("touches-file");
    let stdout = touches(&sandbox, &["totals.rs"]);
    assert!(stdout.contains("/other/ledger/totals.rs"), "{stdout}");
    assert!(stdout.contains("Разбор расчётов"), "{stdout}");
    assert!(stdout.contains("[edited]"), "{stdout}");
}

/// A file the owner named and nobody wrote to is in the default answer, and the sentence
/// that says why is the mark itself.
#[test]
fn a_file_the_owner_named_is_in_the_answer_although_it_was_only_read() {
    let sandbox = with_two_transcripts("touches-named");
    let stdout = touches(&sandbox, &["/other/ledger/settlement.rs"]);
    assert!(stdout.contains("[read]"), "{stdout}");
    assert!(stdout.contains("you named this file"), "{stdout}");
}

/// The requirement of the stage, checked where a person would meet it: a file that was only
/// read and never named is not in the default answer, it is counted, and asking for it shows
/// it with the number of times it happened.
#[test]
fn a_file_nobody_named_is_left_out_of_the_default_answer_and_counted() {
    let sandbox = with_two_transcripts("touches-in-passing");

    let strict = touches(&sandbox, &["registry.rs"]);
    assert!(
        strict.contains("Left out, read and never named: 1"),
        "{strict}"
    );
    assert!(
        !strict.contains("Разбор расчётов"),
        "беседа, которая только читала файл, не должна стоять в умолчании:\n{strict}"
    );

    let widened = touches(&sandbox, &["registry.rs", "--include-in-passing"]);
    assert!(widened.contains("Разбор расчётов"), "{widened}");
    assert!(widened.contains("2 times"), "{widened}");
    assert!(!widened.contains("Left out"), "{widened}");
}

/// A tail that fits two different files is answered about both, and the answer says so
/// before it says anything else.
#[test]
fn a_name_that_fits_two_files_is_not_resolved_quietly() {
    let sandbox = with_two_transcripts("touches-ambiguous");
    let stdout = touches(&sandbox, &["settlement.rs"]);
    assert!(stdout.contains("That name fits 2 of them"), "{stdout}");
    assert!(stdout.contains("/work/ledger/settlement.rs"), "{stdout}");
    assert!(stdout.contains("/other/ledger/settlement.rs"), "{stdout}");
}

/// The other direction: a conversation named by part of its name, answered with what it
/// worked on. What it only read is left out here too.
#[test]
fn a_conversation_is_answered_with_the_files_it_worked_on() {
    let sandbox = with_two_transcripts("touches-conversation");
    let stdout = touches(&sandbox, &["--in", "Разбор расчётов"]);
    assert!(stdout.contains("/other/ledger/totals.rs"), "{stdout}");
    assert!(stdout.contains("/other/ledger/settlement.rs"), "{stdout}");
    assert!(
        !stdout.contains("/other/ledger/registry.rs"),
        "прочитанное мимоходом не входит в умолчание:\n{stdout}"
    );
    assert!(
        stdout.contains("Left out, read and never named: 1"),
        "{stdout}"
    );
}

/// What this command cannot know, said where somebody who got nothing will read it: a path
/// that only ever appeared in a shell command is a path the graph has never heard of.
#[test]
fn a_file_nobody_touched_is_answered_with_what_is_not_captured() {
    let sandbox = with_two_transcripts("touches-nothing");
    let stdout = touches(&sandbox, &["never_opened.rs"]);
    assert!(
        stdout.contains("Nothing in this context was touched"),
        "{stdout}"
    );
    assert!(stdout.contains("shell command"), "{stdout}");
}

/// What the command promises about a row nobody marked, checked rather than asserted: a pass
/// over a transcript that has not changed publishes nothing, so a conversation that has ended
/// keeps the marks it was written with until the context is indexed afresh.
///
/// This is here because the first wording of that message was wrong — it offered `index
/// reconcile` as the cure — and a sentence a person acts on has to be a sentence something
/// holds.
#[test]
fn a_pass_over_an_unchanged_transcript_rewrites_no_conversation() {
    let sandbox = with_two_transcripts("touches-unchanged");
    let before = sandbox.graph_rows(
        "MATCH (:Conversation)-[:TOUCHED_FILE]->(touch:FileTouch) RETURN touch.id, touch.naming, touch.times ORDER BY touch.id;",
    );
    assert!(!before.is_empty());

    let output = sandbox.crosskit(&["index", "reconcile", &sandbox.documents_argument()]);
    assert!(
        output.status.success(),
        "reconcile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = sandbox.graph_rows(
        "MATCH (:Conversation)-[:TOUCHED_FILE]->(touch:FileTouch) RETURN touch.id, touch.naming, touch.times ORDER BY touch.id;",
    );
    assert_eq!(
        before, after,
        "ревизия, уже лежащая в графе, вторым проходом не переписывается"
    );
}
