// Tasks from the outside: the whole program, a real graph indexed from transcripts, and the
// owner's decisions kept in the task book beside it (ADR 0052, ADR 0053).
//
// Checked here because it is only observable from out here: a decision is taken by naming a
// conversation the way the listing prints it, it is kept between runs, it never opens the graph
// for writing, and what was dropped stays out.
mod common;

use std::fs;

use common::Sandbox;

/// Two conversations handing a note on and one about something else, a week later.
fn with_three_conversations(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    common::write_task_conversations(&sandbox.documents);
    let output = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        output.status.success(),
        "index init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    sandbox
}

fn tasks(sandbox: &Sandbox, arguments: &[&str]) -> String {
    let mut all = vec!["tasks"];
    all.extend_from_slice(arguments);
    let output = sandbox.crosskit(&all);
    assert!(
        output.status.success(),
        "tasks {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn a_confirmed_task_is_kept_and_a_dropped_conversation_stays_out() {
    let sandbox = with_three_conversations("tasks-decisions");

    let listing = tasks(&sandbox, &[]);
    assert!(listing.contains("Candidates (1)"), "{listing}");
    assert!(listing.contains("aaaa1111"), "{listing}");
    assert!(!listing.contains("Отчёт по продажам"), "{listing}");

    let confirmed = tasks(&sandbox, &["confirm", "bbbb", "--name", "Скидки"]);
    assert!(
        confirmed.contains("Скидки — 2 conversation(s)"),
        "{confirmed}"
    );
    assert!(!confirmed.contains("Candidates"), "{confirmed}");
    assert!(sandbox.data_directory.join("tasks-sandbox.json").exists());

    let again = tasks(&sandbox, &[]);
    assert!(again.contains("Скидки — 2 conversation(s)"), "{again}");

    let dropped = tasks(&sandbox, &["drop", "Скидки", "bbbb2222"]);
    assert!(dropped.contains("Скидки — 1 conversation(s)"), "{dropped}");
    assert!(
        !dropped.contains("Proposed"),
        "a dropped conversation is not proposed again:\n{dropped}"
    );

    let machine = tasks(&sandbox, &["--json"]);
    let answer: serde_json::Value = serde_json::from_str(machine.trim()).unwrap();
    assert_eq!(answer["confirmed"][0]["name"], "Скидки");
    assert_eq!(
        answer["confirmed"][0]["conversations"][0]["session"],
        "aaaa1111-discount"
    );
    assert_eq!(answer["tasks"].as_array().map(Vec::len), Some(0));
}

#[test]
fn a_conversation_nobody_has_is_refused_by_name() {
    let sandbox = with_three_conversations("tasks-unknown");
    let output = sandbox.crosskit(&["tasks", "reject", "zzzz9999"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No conversation of this context is zzzz9999"),
        "{stderr}"
    );
    assert!(!sandbox.data_directory.join("tasks-sandbox.json").exists());
}

/// A map is built by the tool, two questions to a window, keeps only what a quote proves, and a
/// second build asks nothing: every reply is already covered. Showing never asks.
#[test]
fn a_task_map_keeps_what_a_quote_proves_and_is_grown_not_rebuilt() {
    let sandbox = with_three_conversations("tasks-map");
    sandbox.write_config_sections("", "send_content_outside = true\n");
    tasks(&sandbox, &["confirm", "bbbb", "--name", "Скидки"]);

    let calls = sandbox.data_directory.join("tool-calls");
    let (root, directory) = common::fake_map_engine(&calls);
    let map = |arguments: &[&str]| {
        let mut all = vec!["tasks", "map", "Скидки"];
        all.extend_from_slice(arguments);
        let output = sandbox
            .command(&all)
            .env("PATH", &directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tasks map {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let asked = || fs::read_to_string(&calls).unwrap_or_default();
    let call_count = || asked().lines().count();

    let shown = map(&[]);
    assert!(shown.contains("not mapped"), "{shown}");
    assert_eq!(call_count(), 0, "showing a map asks nothing");

    let built = map(&["--build", "--detail"]);
    assert!(built.contains("◆ [1] Где считать скидку?"), "{built}");
    assert!(
        built.contains("the task's decision map, joined from 2 of 2"),
        "{built}"
    );
    assert!(
        built.contains("причина не найдена"),
        "the risks pass's risk is in the same map:\n{built}"
    );
    assert!(
        !built.contains("выдумано"),
        "a node no quote proves is not shown:\n{built}"
    );
    assert_eq!(
        asked(),
        "forks\nrisks\nforks\nrisks\njoin\n",
        "two passes over one window of each of the two conversations, and one join of their maps"
    );

    let again = map(&["--build"]);
    assert_eq!(
        call_count(),
        5,
        "maps covering every reply and a join made from them ask nothing again:\n{again}"
    );

    let machine = map(&["--json"]);
    let answer: serde_json::Value = serde_json::from_str(machine.trim()).unwrap();
    assert_eq!(answer["question"], "decision-map-v4");
    assert_eq!(answer["joined"]["question"], "task-synthesis-v1");
    assert_eq!(answer["joined"]["stale"], false);
    assert_eq!(
        answer["conversations"][0]["forks"][0]["branches"][0]["state"],
        "chosen"
    );
    fs::remove_dir_all(root).unwrap();
}
