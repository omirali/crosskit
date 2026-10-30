// `crosskit graph` from the outside: the view the panel draws, read through the whole
// program from a graph a real pass wrote.
//
// One synthetic context: two conversations that both worked on one file, a file one of them
// only glanced at, and a path that lies in no location of the context. Nothing here comes
// from anybody's machine.
mod common;

use std::fs;
use std::path::Path;

use common::Sandbox;
use serde_json::Value;

/// A transcript of one session: a named conversation, a reply of the owner, and a tool call
/// at a later moment for each `(tool, path)`.
fn transcript(session: &str, title: &str, said: &str, tools: &[(&str, String)]) -> String {
    let mut lines = vec![
        format!(r#"{{"type":"custom-title","sessionId":"{session}","customTitle":"{title}"}}"#),
        format!(
            r#"{{"type":"user","sessionId":"{session}","timestamp":"2026-09-03T07:00:00.000Z","message":{{"role":"user","content":[{{"type":"text","text":"{said}"}}]}}}}"#
        ),
        format!(
            r#"{{"type":"assistant","sessionId":"{session}","timestamp":"2026-09-03T07:00:05.000Z","message":{{"role":"assistant","content":[{{"type":"text","text":"Смотрю."}}]}}}}"#
        ),
    ];
    for (minute, (tool, path)) in tools.iter().enumerate() {
        lines.push(format!(
            r#"{{"type":"assistant","sessionId":"{session}","timestamp":"2026-09-03T07:{:02}:10.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"{tool}","input":{{"file_path":"{path}"}}}}]}}}}"#,
            minute + 1
        ));
    }
    lines.join("\n") + "\n"
}

/// The documents of the context, resolved: a touch names the path the tool saw, and on macOS
/// a temporary directory resolves through `/private`.
fn documents(sandbox: &Sandbox) -> String {
    fs::canonicalize(&sandbox.documents)
        .unwrap()
        .display()
        .to_string()
}

fn with_two_conversations(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    let root = documents(&sandbox);
    let ledger = Path::new(&root).join("ledger");
    fs::create_dir_all(&ledger).unwrap();
    fs::write(ledger.join("contract.rs"), "pub fn contract() {}\n").unwrap();
    fs::write(ledger.join("glance.rs"), "pub fn glance() {}\n").unwrap();
    let contract = format!("{root}/ledger/contract.rs");
    let glance = format!("{root}/ledger/glance.rs");

    fs::write(
        sandbox.documents.join("first.jsonl"),
        transcript(
            "s-first",
            "Договоры в payments-api",
            "поправь договоры",
            &[
                ("Edit", contract.clone()),
                ("Read", glance),
                ("Edit", "/elsewhere/registry.rs".to_string()),
            ],
        ),
    )
    .unwrap();
    fs::write(
        sandbox.documents.join("second.jsonl"),
        transcript(
            "s-second",
            "Сверка договоров",
            "проверь сверку",
            &[("Write", contract)],
        ),
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

fn graph(sandbox: &Sandbox, arguments: &[&str]) -> (bool, String) {
    let mut all = vec!["graph"];
    all.extend_from_slice(arguments);
    let output = sandbox.crosskit(&all);
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
    )
}

fn json(sandbox: &Sandbox, arguments: &[&str]) -> Value {
    let mut all = arguments.to_vec();
    all.push("--json");
    let (_, stdout) = graph(sandbox, &all);
    assert_eq!(
        stdout.lines().count(),
        1,
        "одна строка, один объект: {stdout}"
    );
    serde_json::from_str(&stdout).unwrap()
}

/// The first file of the view the context holds: a path outside it is a file node too.
fn inside(view: &Value) -> Value {
    nodes_of(view, "file")
        .into_iter()
        .find(|file| file["file"]["state"] == "present")
        .cloned()
        .expect("файл в контексте")
}

fn node<'v>(view: &'v Value, id: &str) -> Option<&'v Value> {
    view["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
}

fn nodes_of<'v>(view: &'v Value, kind: &str) -> Vec<&'v Value> {
    view["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == kind)
        .collect()
}

/// The acceptance of the stage, in the small: from one conversation, its replies, the file it
/// worked on, the project, and on the second step the other conversation of that file.
#[test]
fn two_steps_from_a_conversation_reach_the_other_conversation_of_its_file() {
    let sandbox = with_two_conversations("graph-two-steps");
    let view = json(&sandbox, &["--focus", "conversation:s-first"]);
    assert_eq!(view["schema"], 2);
    assert_eq!(view["focus"], "conversation:s-first");
    assert_eq!(view["depth"], 2);

    let files: Vec<&Value> = nodes_of(&view, "file")
        .into_iter()
        .filter(|file| file["file"]["state"] != "outside")
        .collect();
    assert_eq!(files.len(), 1, "{view}");
    assert!(
        files[0]["file"]["path"]
            .as_str()
            .unwrap()
            .ends_with("/ledger/contract.rs")
    );
    assert_eq!(files[0]["file"]["state"], "present");
    assert!(
        !nodes_of(&view, "reply").is_empty(),
        "реплики, трогавшие файл"
    );
    assert_eq!(nodes_of(&view, "project").len(), 1);
    let second = node(&view, "conversation:s-second").expect("вторая беседа того же файла");
    assert_eq!(second["label"], "Сверка договоров");

    let one_step = json(
        &sandbox,
        &["--focus", "conversation:s-first", "--depth", "1"],
    );
    assert!(node(&one_step, "conversation:s-second").is_none());
}

/// A path no location of the context holds is shown by its path and is not a place to stand;
/// a file only read once is counted, as `touches` counts it.
#[test]
fn a_path_outside_the_context_is_a_node_and_a_glance_is_a_count() {
    let sandbox = with_two_conversations("graph-outside");
    let view = json(
        &sandbox,
        &["--focus", "conversation:s-first", "--depth", "1"],
    );
    let outside = node(&view, "outside:/elsewhere/registry.rs").expect("узел вне контекста");
    assert_eq!(outside["focusable"], false);
    assert_eq!(outside["kind"], "file", "вне контекста — состояние файла");
    assert_eq!(outside["file"]["state"], "outside");
    assert_eq!(view["not_shown"]["in_passing"], 1);
    assert!(nodes_of(&view, "file").iter().all(|file| {
        !file["file"]["path"]
            .as_str()
            .unwrap()
            .ends_with("glance.rs")
    }));
    assert!(
        nodes_of(&view, "topic").is_empty(),
        "разметка не запускалась"
    );
}

/// Clicking a file in the panel is this: its id as the next focus.
#[test]
fn a_file_node_is_a_focus_and_its_step_is_its_conversations() {
    let sandbox = with_two_conversations("graph-refocus");
    let first = json(
        &sandbox,
        &["--focus", "conversation:s-first", "--depth", "1"],
    );
    let file = inside(&first)["id"].as_str().unwrap().to_string();

    let view = json(&sandbox, &["--focus", &file, "--depth", "1"]);
    assert_eq!(view["focus"], file);
    let mut sessions: Vec<&str> = nodes_of(&view, "conversation")
        .iter()
        .map(|node| node["conversation"]["session_id"].as_str().unwrap())
        .collect();
    sessions.sort_unstable();
    assert_eq!(sessions, vec!["s-first", "s-second"]);

    let path = inside(&first)["file"]["path"].as_str().unwrap().to_string();
    let by_path = json(
        &sandbox,
        &["--focus", &format!("path:{path}"), "--depth", "1"],
    );
    assert_eq!(by_path["focus"], file, "путь строки выдачи — тот же файл");
}

/// Every field the panel reads, with its type: the contract, spelled out.
#[test]
fn the_view_carries_its_bound_its_counts_and_where_each_node_came_from() {
    let sandbox = with_two_conversations("graph-schema");
    let view = json(
        &sandbox,
        &["--focus", "conversation:s-first", "--limit", "3"],
    );
    assert_eq!(view["limit"], 3);
    assert_eq!(view["truncated"], true);
    assert!(view["nodes"].as_array().unwrap().len() <= 3);
    for kind in ["conversation", "reply", "file", "project", "topic"] {
        assert!(view["counts"][kind]["shown"].is_u64(), "{kind}: {view}");
        assert!(view["counts"][kind]["found"].is_u64(), "{kind}: {view}");
    }
    let focus = node(&view, "conversation:s-first").unwrap();
    assert_eq!(focus["provenance"]["record"], "Conversation");
    assert!(focus["conversation"]["replies_not_shown"].is_u64());
    assert!(view["provenance_summary"][0]["nodes"].is_u64());
    for edge in view["edges"].as_array().unwrap() {
        assert!(node(&view, edge["from"].as_str().unwrap()).is_some());
        assert!(node(&view, edge["to"].as_str().unwrap()).is_some());
        assert_eq!(
            edge["touch"].is_object(),
            edge["kind"] == "touched",
            "{edge}"
        );
    }
}

#[test]
fn a_focus_nothing_answers_to_is_refused_with_an_object() {
    let sandbox = with_two_conversations("graph-not-found");
    let (succeeded, stdout) = graph(&sandbox, &["--focus", "conversation:nobody", "--json"]);
    assert!(!succeeded);
    let refused: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(refused["schema"], 2);
    assert_eq!(refused["error"]["category"], "not-found");
    assert_eq!(refused["error"]["context"], Sandbox::CONTEXT);

    let (succeeded, stdout) = graph(&sandbox, &["--focus", "project:/x", "--json"]);
    assert!(!succeeded);
    let refused: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(refused["error"]["category"], "usage");
}

/// The text form is a tree a person can check without the panel.
#[test]
fn the_text_form_is_a_tree_from_the_focus() {
    let sandbox = with_two_conversations("graph-text");
    let (succeeded, stdout) = graph(&sandbox, &["--focus", "conversation:s-first"]);
    assert!(succeeded, "{stdout}");
    let first = stdout
        .lines()
        .find(|line| line.starts_with("conversation"))
        .unwrap();
    assert!(first.contains("«Договоры в payments-api»"), "{stdout}");
    assert!(
        stdout.contains("[edited, reply placed by time] file [present]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("file [outside this context] /elsewhere/registry.rs"),
        "{stdout}"
    );
    assert!(
        stdout.contains("conversation «Сверка договоров»"),
        "{stdout}"
    );
    assert!(stdout.contains("Nodes: "), "{stdout}");
    assert!(
        stdout.contains("Left out, read and never named: 1 (add --include-in-passing)"),
        "{stdout}"
    );
}
