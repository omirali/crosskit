// CLI smoke: the knowledge layer from the queue to the map, through the real binary.
//
// The engine is a script named `claude` on a PATH of its own, as in `cli_analysis_engine`: the
// real adapter spawns it, writes the numbered parts to its standard input and reads its answer.
// The answer is the same for every revision, so one run shows both halves of the rule at once —
// the quote that is in the registry is kept there, and the same quote proposed about a note that
// does not hold it is refused.
mod common;

use std::fs;

use serde_json::Value;

use common::{Sandbox, assert_counters, fake_engine};

const SENDING: &str = "send_content_outside = true\n";
const REGISTRY: &str = "# Реестр\n\nКонтрагент уникален по БИН: второй с тем же БИН — ошибка.\n\
                        Каждый пакет документов принадлежит одному контрагенту.\n";

/// Three entities and four facts: two whose quotes the registry holds, one paraphrase, and one
/// quote the registry holds that names only one end.
const ANSWER: &str = r#"{"type":"result","is_error":false,"total_cost_usd":0.0125,
"usage":{"input_tokens":400,"output_tokens":90},
"structured_output":{"topic":"реестр контрагентов","confidence":0.9,
"entities":[
 {"name":"Контрагент","type":"concept","description":"Сторона договора."},
 {"name":"БИН","type":"attribute","description":"Номер юрлица."},
 {"name":"Пакеты документов","type":"document","description":"Документы одного контрагента."}],
"relations":[
 {"subject":"Контрагент","predicate":"identified-by","object":"БИН","description":"Уникален по БИН.","weight":9,"part":1,"quote":"Контрагент уникален по БИН"},
 {"subject":"Пакет документов","predicate":"belongs-to","object":"Контрагент","description":"","weight":7,"part":1,"quote":"пакет документов принадлежит одному контрагенту"},
 {"subject":"Пакет документов","predicate":"belongs-to","object":"Контрагент","description":"","weight":3,"part":1,"quote":"пакет всегда у одного контрагента"},
 {"subject":"Контрагент","predicate":"identified-by","object":"БИН","description":"","weight":5,"part":1,"quote":"второй с тем же БИН"}]}}"#;

fn prepared(name: &str, workspace_lines: &str) -> Sandbox {
    let sandbox = Sandbox::new(name);
    fs::write(sandbox.documents.join("registry.md"), REGISTRY).unwrap();
    sandbox.write_config_sections("", workspace_lines);
    sandbox.reconcile();
    sandbox
}

fn engine_answering(sandbox: &Sandbox) -> (std::path::PathBuf, String) {
    let answer = sandbox.data_directory.join("answer.json");
    fs::write(&answer, ANSWER).unwrap();
    fake_engine(&format!("cat >/dev/null; cat '{}'", answer.display()))
}

fn stdout(sandbox: &Sandbox, arguments: &[&str], path: &str) -> String {
    let output = sandbox
        .command(arguments)
        .env("PATH", path)
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{arguments:?}: {}\n{text}",
        String::from_utf8_lossy(&output.stderr)
    );
    text
}

#[test]
fn a_run_keeps_what_its_quote_proves_says_what_it_refused_and_prints_the_cost() {
    let sandbox = prepared("knowledge-run", SENDING);
    let (root, tool) = engine_answering(&sandbox);

    let worked = stdout(&sandbox, &["index", "jobs", "run", "--limit", "5"], &tool);

    // Two revisions, four facts proposed about each. The registry proves two and holds a third
    // quote that names only «БИН»; the note proves none, and a fact whose entities the note
    // never proves leaves no entity behind.
    assert_counters(
        &worked,
        &[
            "Completed jobs: 2",
            "Knowledge: 2 facts kept of 8 proposed — 5 rejected because the quote is not in its part, 1 because it does not name both ends, 0 otherwise; 3 entities, 0 spellings glued",
            "Cost: 2 calls (0 answered from the cache)",
            "800 input and 180 output tokens, $0.0250 as the engine reported it",
        ],
    );

    let summary = stdout(&sandbox, &["knowledge"], &tool);
    assert_counters(
        &summary,
        &[
            "Entities: 3",
            "Facts: 2",
            "Revisions asked: 2",
            "Facts proposed: 8 — kept 2, rejected because the quote is not in its part 5 (62.5 %), because it does not name both ends 1 (12.5 %)",
            "$0.0250 as the engine reported it — $0.0125 per part",
            "Still queued: 0 jobs",
        ],
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_entity_is_shown_with_every_fact_its_quote_and_where_it_was_read() {
    let sandbox = prepared("knowledge-show", SENDING);
    let (root, tool) = engine_answering(&sandbox);
    stdout(&sandbox, &["index", "jobs", "run", "--limit", "5"], &tool);

    // Asked in another form of the word: the key glues them.
    let shown = stdout(&sandbox, &["knowledge", "show", "контрагенты"], &tool);

    assert_counters(
        &shown,
        &[
            "Контрагент (concept)",
            "2 facts, from 1 sources",
            "Контрагент —identified-by→ БИН  (weight 9)",
            "«Контрагент уникален по БИН»",
            "registry.md, part 1",
            "Пакеты документов —belongs-to→ Контрагент",
        ],
    );
    let listed = stdout(
        &sandbox,
        &["knowledge", "entities", "--type", "attribute"],
        &tool,
    );
    assert_counters(
        &listed,
        &[
            "БИН (attribute) — 1 facts, 1 sources",
            "Entities: 1 of 1 of type attribute",
        ],
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_map_answers_a_program_with_entities_by_id_and_facts_with_their_source() {
    let sandbox = prepared("knowledge-map", SENDING);
    let (root, tool) = engine_answering(&sandbox);
    stdout(&sandbox, &["index", "jobs", "run", "--limit", "5"], &tool);

    let line = stdout(&sandbox, &["knowledge", "map", "--json"], &tool);
    let map: Value = serde_json::from_str(line.trim()).unwrap();

    assert_eq!(map["schema"], 2);
    assert_eq!(map["context"], Sandbox::CONTEXT);
    assert_eq!(map["counts"]["entities"]["shown"], 3);
    assert_eq!(map["counts"]["facts"]["found"], 2);
    assert_eq!(map["truncated"], false);
    assert_eq!(map["entity_types"][0], "concept");
    let facts = map["facts"].as_array().unwrap();
    let unique = facts
        .iter()
        .find(|fact| fact["predicate"] == "identified-by")
        .unwrap();
    assert_eq!(unique["from"], "entity:контрагент");
    assert_eq!(unique["to"], "entity:бин");
    assert_eq!(unique["quote"], "Контрагент уникален по БИН");
    assert_eq!(unique["source"]["kind"], "file");
    assert!(
        unique["source"]["path"]
            .as_str()
            .unwrap()
            .ends_with("/registry.md")
    );
    assert_eq!(unique["source"]["part"], 1);
    assert_eq!(map["roots"].as_array().unwrap().len(), 1);
    let counterparty = map["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| entity["id"] == "entity:контрагент")
        .unwrap();
    // Stems of the name and the description, so a reader finds it by any form of a word.
    let terms: Vec<&str> = counterparty["terms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|term| term.as_str().unwrap())
        .collect();
    assert!(
        terms.contains(&"контрагент") && terms.contains(&"договор"),
        "{terms:?}"
    );
    let ends: Vec<&str> = map["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| entity["id"].as_str().unwrap())
        .collect();
    for fact in facts {
        assert!(ends.contains(&fact["from"].as_str().unwrap()));
        assert!(ends.contains(&fact["to"].as_str().unwrap()));
    }
    fs::remove_dir_all(root).unwrap();
}

/// A context that keeps its content to itself asks no model, so it has nothing to show — and
/// says so instead of an empty table that reads as "the files state nothing".
#[test]
fn a_context_that_sends_nothing_has_no_facts_and_says_why() {
    let sandbox = prepared("knowledge-denied", "");
    let (root, tool) = engine_answering(&sandbox);

    let worked = stdout(&sandbox, &["index", "jobs", "run", "--limit", "5"], &tool);
    let summary = stdout(&sandbox, &["knowledge"], &tool);

    assert!(!worked.contains("Knowledge:"), "{worked}");
    assert_counters(
        &summary,
        &["Facts: 0", "No revision has been asked for facts yet"],
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_map_asked_of_a_context_that_does_not_exist_is_a_refusal_object() {
    let sandbox = prepared("knowledge-refusal", SENDING);

    let output = sandbox.crosskit(&["--context", "elsewhere", "knowledge", "map", "--json"]);
    let refusal: Value =
        serde_json::from_str(String::from_utf8(output.stdout).unwrap().trim()).unwrap();

    assert!(!output.status.success());
    assert_eq!(refusal["schema"], 2);
    assert_eq!(refusal["error"]["category"], "unknown-context");
}
