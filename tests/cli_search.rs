// `crosskit search` from the outside: the whole program, through its own command line,
// against a real database and a real search index.
//
// What is checked here is what an operator sees — the lines, the exit codes, the way a
// page continues — plus the two things only a whole run can show: that Crosskit does not
// index its own data directory, and that a configuration naming a language this build does
// not have fails before anything opens.
mod common;

use std::fs;

use common::Sandbox;

/// The index is written as part of indexing, so an ordinary `index init` is all it takes
/// to be able to search.
fn indexed(test_name: &str) -> Sandbox {
    let sandbox = Sandbox::new(test_name);
    let output = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        output.status.success(),
        "index init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    sandbox
}

fn search(sandbox: &Sandbox, arguments: &[&str]) -> String {
    let mut all = vec!["search"];
    all.extend_from_slice(arguments);
    let output = sandbox.crosskit(&all);
    assert!(
        output.status.success(),
        "{arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A result line says where, what matched and what the file is now.
#[test]
fn a_search_shows_the_path_the_fragment_what_matched_and_the_state() {
    let sandbox = indexed("search-basic");
    let stdout = search(&sandbox, &["keeps"]);

    assert!(stdout.contains("Results: 1"), "{stdout}");
    assert!(stdout.contains("[present]"), "{stdout}");
    assert!(stdout.contains("notes.md"), "{stdout}");
    assert!(
        stdout.contains("Crosskit keeps this note"),
        "the fragment must show the text that matched: {stdout}"
    );
    assert!(stdout.contains("matched: keeps"), "{stdout}");
}

/// Two languages in one sentence, each found by a form of its own.
#[test]
fn a_query_in_either_language_finds_the_sentence_that_holds_both() {
    let sandbox = Sandbox::new("search-bilingual");
    fs::write(
        sandbox.documents.join("mixed.md"),
        "The walker обходит trees и пишет индексы",
    )
    .unwrap();
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );

    for query in ["индекс", "индексов", "обход", "trees", "tree"] {
        let stdout = search(&sandbox, &[query]);
        assert!(
            stdout.contains("mixed.md"),
            "{query} did not find the sentence: {stdout}"
        );
    }
}

/// Undeclared, Kazakh is findable only by the form it was written in: no rules of its own
/// are applied, and the answer does not pretend otherwise.
#[test]
fn an_undeclared_language_is_found_by_the_form_it_was_written_in() {
    let sandbox = Sandbox::new("search-kazakh-undeclared");
    fs::write(
        sandbox.documents.join("kk.md"),
        "мүмкіндік қазақша кітаптар и русские индексы рядом",
    )
    .unwrap();
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );

    assert!(search(&sandbox, &["мүмкіндік"]).contains("kk.md"));
    assert!(search(&sandbox, &["қазақша"]).contains("kk.md"));
    assert!(search(&sandbox, &["индексами"]).contains("kk.md"));
    assert!(
        search(&sandbox, &["кітап"]).contains("Results: 0"),
        "an undeclared language has no rules here, and `кітаптар` is not `кітап` without them"
    );
}

/// Declared, Kazakh brings the forms of one word together — and the Russian word in the same
/// sentence is still reduced by Russian rules.
#[test]
fn a_declared_language_pack_brings_the_forms_of_one_word_together() {
    let sandbox = Sandbox::new("search-kazakh-declared");
    sandbox.write_config("[search]\nlanguages = [\"ru\", \"en\", \"kk\"]\n\n");
    fs::write(
        sandbox.documents.join("kk.md"),
        "мүмкіндік қазақша кітаптар и русские индексы рядом",
    )
    .unwrap();
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );

    for query in ["кітап", "кітаптар", "кітапта", "мүмкіндіктер", "қазақша"]
    {
        assert!(
            search(&sandbox, &[query]).contains("kk.md"),
            "{query} did not find the sentence"
        );
    }
    assert!(
        search(&sandbox, &["индексами"]).contains("kk.md"),
        "the Russian word beside it is still reduced by Russian rules"
    );
    assert!(
        doctor(&sandbox).contains("normalizations=ru/1,en/1,kk/1"),
        "the analyzer names every pack and its version: {}",
        doctor(&sandbox)
    );
}

/// Declaring a pack changes the analyzer, so an index written before it is reported as
/// written by another one — the same mechanism a changed pack version goes through, and the
/// reason a pack carries a version at all.
#[test]
fn declaring_a_pack_is_a_change_of_analyzer_that_doctor_reports() {
    let sandbox = Sandbox::new("search-kazakh-fingerprint");
    fs::write(sandbox.documents.join("kk.md"), "кітаптар және индексы").unwrap();
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );
    assert!(doctor(&sandbox).contains("normalizations=ru/1,en/1"));

    sandbox.write_config("[search]\nlanguages = [\"ru\", \"en\", \"kk\"]\n\n");

    let reported = doctor(&sandbox);
    assert!(
        reported.contains("the index was written by")
            && reported.contains("normalizations=ru/1,en/1"),
        "{reported}"
    );
    assert!(
        search(&sandbox, &["кітап"]).contains("Results: 0"),
        "the disagreement is real: the index holds the word by rules this build no longer applies"
    );

    assert!(
        sandbox
            .crosskit(&["index", "rebuild", "--search"])
            .status
            .success()
    );
    assert!(search(&sandbox, &["кітап"]).contains("kk.md"));
    assert!(!doctor(&sandbox).contains("the index was written by"));
}

/// A page smaller than the number of matches continues where it ended, and the same page
/// asked for twice is the same page.
#[test]
fn pages_continue_from_a_cursor_and_repeat_identically() {
    let sandbox = Sandbox::new("search-paging");
    for index in 0..5 {
        fs::write(
            sandbox.documents.join(format!("note-{index}.md")),
            "обход индекса",
        )
        .unwrap();
    }
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );

    let mut walked = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..6 {
        let stdout = match &cursor {
            Some(after) => search(&sandbox, &["обход", "--limit", "2", "--after", after]),
            None => search(&sandbox, &["обход", "--limit", "2"]),
        };
        walked.extend(result_paths(&stdout));
        match next_cursor(&stdout) {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    walked.sort();
    walked.dedup();
    assert_eq!(walked.len(), 5, "every match exactly once: {walked:?}");

    let first = search(&sandbox, &["обход", "--limit", "2"]);
    let again = search(&sandbox, &["обход", "--limit", "2"]);
    assert_eq!(first, again, "the same page twice must be the same page");

    let rejected = sandbox.crosskit(&["search", "обход", "--after", "not-a-cursor"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("--after"),
        "{:?}",
        String::from_utf8_lossy(&rejected.stderr)
    );
}

/// A file whose locations are gone keeps its text in the graph. It is out of the way by
/// default and marked when it is asked for.
#[test]
fn a_file_that_disappeared_is_left_out_by_default_and_marked_when_asked_for() {
    let sandbox = indexed("search-missing");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    sandbox.reconcile();

    let default = search(&sandbox, &["keeps"]);
    assert!(default.contains("Results: 0"), "{default}");
    assert!(
        default.contains("--include-missing"),
        "the answer must say what it left out and how to see it: {default}"
    );

    let asked = search(&sandbox, &["keeps", "--include-missing"]);
    assert!(asked.contains("Results: 1"), "{asked}");
    assert!(asked.contains("[missing]"), "{asked}");
    assert!(asked.contains("notes.md"), "{asked}");
}

/// A new current version replaces the documents of the one before it, in the whole
/// program: the old words answer nothing and the file is found once, not twice.
#[test]
fn a_new_version_replaces_what_the_old_one_answered() {
    let sandbox = indexed("search-new-version");
    fs::write(
        sandbox.documents.join("notes.md"),
        "Crosskit rewrote this note.",
    )
    .unwrap();
    sandbox.reconcile();

    assert!(search(&sandbox, &["keeps"]).contains("Results: 0"));
    let stdout = search(&sandbox, &["rewrote"]);
    assert!(stdout.contains("Results: 1"), "{stdout}");
    assert!(stdout.contains("notes.md"), "{stdout}");
    assert!(
        doctor(&sandbox).contains("Search index: 1 documents against 1 chunks"),
        "{}",
        doctor(&sandbox)
    );
}

/// An empty query and an empty index are answered, not refused.
///
/// The fixture for the empty index is a context that has a graph and no index, because a
/// context with no graph at all is a different thing and is now refused with what to run
/// (Stage 28). Until then it stood in for this case, which it could only do because
/// searching it quietly created the graph it was said to be searching.
#[test]
fn an_empty_query_and_an_empty_index_are_answered_with_code_zero() {
    let sandbox = indexed("search-empty");
    fs::remove_dir_all(
        sandbox
            .data_directory
            .join(format!("search-{}", Sandbox::CONTEXT)),
    )
    .unwrap();

    let empty_index = sandbox.crosskit(&["search", "обход"]);
    assert!(empty_index.status.success());
    let stdout = String::from_utf8(empty_index.stdout).unwrap();
    assert!(stdout.contains("Results: 0"), "{stdout}");
    assert!(
        stdout.contains("holds nothing yet"),
        "an empty index must say it is empty rather than look like an answer: {stdout}"
    );

    // A query with no words in it is answered before anything is opened at all, which is
    // why this half needs no graph.
    let empty_query = sandbox.crosskit(&["search", ""]);
    assert!(empty_query.status.success());
    let stdout = String::from_utf8(empty_query.stdout).unwrap();
    assert!(stdout.contains("Empty query"), "{stdout}");
}

/// The index is a derivative: its directory can be removed and written again from the
/// graph, and everything answers as before.
#[test]
fn the_index_directory_can_be_removed_and_rebuilt_from_the_graph() {
    let sandbox = indexed("search-rebuild");
    let before = search(&sandbox, &["keeps"]);
    let index_directory = sandbox
        .data_directory
        .join(format!("search-{}", Sandbox::CONTEXT));
    assert!(index_directory.is_dir(), "the index lives beside the graph");
    fs::remove_dir_all(&index_directory).unwrap();

    let gone = doctor(&sandbox);
    assert!(
        gone.contains("Search index: 0 documents against 1 chunks"),
        "an index that is not there is behind the graph: {gone}"
    );
    assert!(gone.contains("behind the graph"), "{gone}");

    let rebuilt = sandbox.crosskit(&["index", "rebuild", "--search"]);
    assert!(
        rebuilt.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    let stdout = String::from_utf8(rebuilt.stdout).unwrap();
    assert!(stdout.contains("Indexed documents: 1"), "{stdout}");
    assert!(stdout.contains("Chunks of current versions: 1"), "{stdout}");

    assert_eq!(
        search(&sandbox, &["keeps"]),
        before,
        "the answer must return"
    );
    let healed = doctor(&sandbox);
    assert!(healed.contains("in step with the graph"), "{healed}");
}

/// `doctor` names the rules the index was written by, and a rebuild is the only thing
/// `index rebuild` will do.
#[test]
fn doctor_names_the_rules_the_index_was_written_by_and_rebuild_only_rebuilds_it() {
    let sandbox = indexed("search-doctor");
    let stdout = doctor(&sandbox);
    assert!(
        stdout.contains(
            "Search index written by: fields=chunk_id,file_id,source_language,text,\
             conversation_name_prefix;suffixes=1;tokenizer=simple-lowercase/1+prefix-5/1;\
             normalizations=ru/1,en/1"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("in step with the graph"), "{stdout}");

    let without_flag = sandbox.crosskit(&["index", "rebuild"]);
    assert!(!without_flag.status.success());
    assert!(
        String::from_utf8_lossy(&without_flag.stderr).contains("--search"),
        "{:?}",
        String::from_utf8_lossy(&without_flag.stderr)
    );
}

/// An index written with other text rules than the configuration now declares is reported
/// by `doctor`, and the same rebuild repairs it. Nothing about the results themselves shows
/// it, which is why the fingerprint is stored at all.
#[test]
fn an_index_written_by_another_analyzer_is_reported_and_repaired() {
    let sandbox = Sandbox::new("search-analyzer");
    fs::write(
        sandbox.documents.join("mixed.md"),
        "русские индексы и english trees",
    )
    .unwrap();
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );
    assert!(search(&sandbox, &["индекс"]).contains("mixed.md"));

    // The same corpus and the same graph, a different declared set of languages.
    sandbox.write_config("[search]\nlanguages = [\"en\"]\n\n");

    let stdout = doctor(&sandbox);
    assert!(
        stdout.contains("Search index: 2 documents against 2 chunks"),
        "every document is there, which is why the counts cannot catch this: {stdout}"
    );
    assert!(
        stdout.contains("the index was written by") && stdout.contains("normalizations=ru/1,en/1"),
        "doctor must name both analyzers: {stdout}"
    );
    // And the disagreement is real, not bookkeeping: the Russian term is stored in a form
    // this configuration no longer asks for.
    assert!(search(&sandbox, &["индексы"]).contains("Results: 0"));

    assert!(
        sandbox
            .crosskit(&["index", "rebuild", "--search"])
            .status
            .success()
    );
    let healed = doctor(&sandbox);
    assert!(
        healed.contains("tokenizer=simple-lowercase/1+prefix-5/1;normalizations=en/1"),
        "{healed}"
    );
    assert!(!healed.contains("the index was written by"), "{healed}");
    assert!(search(&sandbox, &["индексы"]).contains("mixed.md"));
}

/// Declaring a language this build has no rules for fails the load, before anything is
/// opened or written.
#[test]
fn a_language_this_build_does_not_have_fails_every_command() {
    let sandbox = Sandbox::new("search-unknown-language");
    sandbox.write_config("[search]\nlanguages = [\"ru\", \"tr\"]\n\n");

    for command in [
        vec!["doctor"],
        vec!["search", "обход"],
        vec!["index", "rebuild", "--search"],
    ] {
        let output = sandbox.crosskit(&command);
        assert!(!output.status.success(), "{command:?} should have failed");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("search languages") && stderr.contains("tr"),
            "{command:?}: {stderr}"
        );
        assert!(
            stderr.contains("ru, en, kk"),
            "the refusal must say what this build does carry: {stderr}"
        );
    }
}

/// Crosskit does not index its own directory. The graph database and the search index
/// live there, and indexing them would feed the program its own writes — the watch-probe
/// loop, with the index as the thing that keeps changing.
#[test]
fn the_data_directory_is_never_indexed() {
    let sandbox = Sandbox::new("search-data-directory");
    let root = fs::canonicalize(sandbox.documents.parent().unwrap()).unwrap();
    // A context covering the whole sandbox, which holds both the documents and Crosskit's
    // own data directory.
    fs::write(
        sandbox.data_directory.join("config.toml"),
        format!(
            "[workspace.{}]\ninclude = [\"{}\"]\n",
            Sandbox::CONTEXT,
            root.display()
        ),
    )
    .unwrap();
    fs::write(
        sandbox.data_directory.join("stray.md"),
        "DATADIRSENTINEL never belongs in an index",
    )
    .unwrap();

    let output = sandbox.crosskit(&["index", "init", root.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stray = search(&sandbox, &["DATADIRSENTINEL"]);
    assert!(
        stray.contains("Results: 0"),
        "Crosskit indexed its own data directory: {stray}"
    );
    let note = search(&sandbox, &["keeps"]);
    assert!(
        note.contains("notes.md"),
        "the rest of the tree must still be indexed: {note}"
    );
}

fn doctor(sandbox: &Sandbox) -> String {
    let output = sandbox.crosskit(&["doctor"]);
    assert!(
        output.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The paths of a page, in the order the page printed them.
fn result_paths(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.split_once("] "))
        .filter(|(head, _)| head.contains("[present") || head.contains("[missing"))
        .map(|(_, path)| path.to_string())
        .collect()
}

fn next_cursor(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("Next page: --after "))
        .map(str::to_string)
}

/// Selecting by the language a file's name declares, typed the way a reader types it.
///
/// The field is answered by the query syntax that was already there, so there is nothing
/// here to learn and nothing in this program that parses `source_language:` itself.
#[test]
fn files_can_be_selected_by_the_language_their_name_declares() {
    let sandbox = Sandbox::new("search-language");
    for (name, text) in [
        ("routes.py", "обход индекса в этом файле"),
        ("Controller.php", "обход индекса в этом файле"),
        ("LICENSE", "обход индекса в этом файле"),
    ] {
        fs::write(sandbox.documents.join(name), text).unwrap();
    }
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );

    let everything = search(&sandbox, &["обход"]);
    assert!(everything.contains("Results: 3"), "{everything}");

    let python = search(&sandbox, &["source_language:python"]);
    assert!(
        python.contains("Results: 1") && python.contains("routes.py"),
        "{python}"
    );

    let php = search(&sandbox, &["обход", "AND", "source_language:php"]);
    assert!(
        php.contains("Results: 1") && php.contains("Controller.php"),
        "{php}"
    );

    // A name that declares nothing is in the index and in no language, so it answers a
    // query for one with nothing — including a query for a word meaning "none".
    let none = search(&sandbox, &["source_language:none"]);
    assert!(none.contains("Results: 0"), "{none}");
}
