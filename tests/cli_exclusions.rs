// CLI smoke: what the built-in exclusions keep out of a tree that looks like a real one,
// and what they must not keep out.
//
// The first live run on a real machine returned seven results out of ten from inside a
// Python environment, and filled the analysis queue with other people's packages. The
// answer is in two halves that are not the same kind of rule, and this test holds both to
// what they claim:
//
//   - a marker the tool itself leaves, which does not age and names no ecosystem;
//   - a short list of names, each one measured on a real tree rather than imagined.
//
// The third assertion is the one that keeps the other two honest: the source beside them is
// still indexed. A rule that removes noise by removing everything is not a rule.
mod common;

use std::fs;
use std::path::Path;

use common::Sandbox;

/// The tagging standard's own first line. Nothing reads past the name, but a fixture that
/// wrote something else would be a fixture of a file that is not the marker.
const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

#[test]
fn a_directory_that_carries_a_marker_is_skipped_whole_and_the_source_beside_it_is_not() {
    let sandbox = Sandbox::new("exclusions-markers");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    let documents = sandbox.documents.clone();

    // A Python environment, marked by the standard library that made it. Nothing in the
    // rules names Python, a version, or `site-packages`.
    write(
        &documents.join("project/.venv/pyvenv.cfg"),
        "home = /usr/bin\n",
    );
    write(
        &documents.join("project/.venv/lib/python3.12/site-packages/dep/mod.py"),
        "def product(): return 'пакет чужой'\n",
    );
    // A cache directory of a tool this program has never been told about.
    write(
        &documents.join("project/.sometool/CACHEDIR.TAG"),
        CACHEDIR_TAG,
    );
    write(&documents.join("project/.sometool/blob"), "кэш чужой\n");
    // And what the person actually wrote.
    write(
        &documents.join("project/src/main.py"),
        "def product(): return 'наш собственный product'\n",
    );

    let indexed = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);

    let stdout = String::from_utf8_lossy(&indexed.stdout);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    assert!(
        stdout.contains("Indexed files (metadata and content): 1"),
        "only the source the person wrote is content: {stdout}"
    );
    assert!(
        stdout.contains("Generated directories skipped whole (marker): 2"),
        "each marked directory is skipped once, on its own marker: {stdout}"
    );
    assert!(stdout.contains("Failed paths: 0"), "{stdout}");

    // The query from the live run, which used to answer mostly out of the environment.
    let found = search(&sandbox, "product");
    assert!(found.contains("Results: 1"), "{found}");
    assert!(found.contains("main.py"), "{found}");
    assert!(
        !found.contains("site-packages"),
        "a packaged dependency is not an answer about this machine: {found}"
    );
}

/// The names. Each is here because it was counted on a real tree, and each is a directory no
/// marker claims: `.pnpm-store` stands beside `node_modules` rather than inside it, `vendor`
/// is committed on purpose so `.gitignore` does not cover it, and `__pycache__` sits beside
/// the source it was compiled from rather than under an environment root.
#[test]
fn the_named_directories_no_marker_covers_are_excluded_too() {
    let sandbox = Sandbox::new("exclusions-names");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    let documents = sandbox.documents.clone();

    write(&documents.join(".pnpm-store/v10/files/00/blob"), "стор\n");
    write(
        &documents.join("app/vendor/lib/dep.php"),
        "<?php // чужое\n",
    );
    write(
        &documents.join("app/__pycache__/main.cpython-312.pyc"),
        "байткод\n",
    );
    write(&documents.join("app/main.py"), "печатает отчёт\n");

    let indexed = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);

    let stdout = String::from_utf8_lossy(&indexed.stdout);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    assert!(
        stdout.contains("Indexed files (metadata and content): 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Excluded paths: 3"),
        "one exclusion per directory the walk was kept out of: {stdout}"
    );
    assert!(search(&sandbox, "отчёт").contains("Results: 1"));
}

/// A file with nothing in it to index is not a failure, and — the half that matters — its
/// version is published anyway. That is what makes a file whose text was deleted stop
/// answering: the graph stops being current at the version that held the text, and the
/// documents of that version leave the index on this ordinary pass, with no rebuild.
#[test]
fn a_file_whose_text_was_deleted_stops_answering_after_one_ordinary_pass() {
    let sandbox = Sandbox::new("exclusions-no-text");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    write(&sandbox.documents.join("kept.md"), "остаётся навсегда\n");
    write(
        &sandbox.documents.join("emptied.md"),
        "уникальныйтермин здесь\n",
    );
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );
    assert!(search(&sandbox, "уникальныйтермин").contains("Results: 1"));

    // Its text is replaced by whitespace, which is the shape that used to be a failed path.
    write(&sandbox.documents.join("emptied.md"), "   \n\t\n  ");
    let repaired = sandbox.crosskit(&["index", "reconcile", &sandbox.documents_argument()]);

    let stdout = String::from_utf8_lossy(&repaired.stdout);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    assert!(
        stdout.contains("Files with no text in them (published, nothing to index): 1"),
        "counted, not listed as something to go and read: {stdout}"
    );
    assert!(stdout.contains("Failed paths: 0"), "{stdout}");

    let found = search(&sandbox, "уникальныйтермин");
    assert!(
        found.contains("Results: 0"),
        "text that is no longer in the file must not answer for it: {found}"
    );
    // And the two halves still agree, so the next pass does not decide the index is behind
    // and write the whole corpus again.
    let state = sandbox.crosskit(&["doctor"]);
    assert!(
        String::from_utf8_lossy(&state.stdout).contains("in step with the graph"),
        "{}",
        String::from_utf8_lossy(&state.stdout)
    );
    assert!(search(&sandbox, "остаётся").contains("Results: 1"));
}

/// Skipping a directory and excluding one are the same statement — "we do not look here" —
/// and a run that treats them as two declares files it can plainly see to be gone.
///
/// This is what it cost on the owner's machine: the markers of stage 20 started working, the
/// next ordinary reconcile skipped the directories they marked, and the finalization, which
/// only knew how to spare paths the *configuration* excluded, read the walk's silence about
/// them as proof of absence. `Locations marked missing: 727`, nothing deleted. The record is
/// not self-correcting either: `present` comes back only on an observation, and the whole
/// point of the marker is that there will not be another one.
#[test]
fn a_marker_that_appears_does_not_bury_the_files_already_indexed_under_it() {
    let sandbox = Sandbox::new("exclusions-burial");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    write(&sandbox.documents.join("project/src/main.py"), "наш код\n");
    for index in 0..3 {
        write(
            &sandbox
                .documents
                .join(format!("project/cache/f{index}.txt")),
            "содержимое кэша\n",
        );
    }
    let indexed = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(
        String::from_utf8_lossy(&indexed.stdout)
            .contains("Indexed files (metadata and content): 4"),
        "the cache is indexed first, before the tool marks it: {}",
        String::from_utf8_lossy(&indexed.stdout)
    );

    // The tool marks its own directory, which is all stage 20 ever asked of it.
    write(
        &sandbox.documents.join("project/cache/CACHEDIR.TAG"),
        CACHEDIR_TAG,
    );
    let reconciled = sandbox.crosskit(&["index", "reconcile", &sandbox.documents_argument()]);

    let stdout = String::from_utf8_lossy(&reconciled.stdout);
    assert!(
        reconciled.status.success(),
        "{}",
        String::from_utf8_lossy(&reconciled.stderr)
    );
    assert!(
        stdout.contains("Generated directories skipped whole (marker): 1"),
        "the directory is still skipped — that part was right: {stdout}"
    );
    assert!(
        stdout.contains("Locations marked missing: 0"),
        "nothing was deleted, so nothing may be recorded as gone: {stdout}"
    );
}

/// The mirror, and the reason the fix cannot simply be "retire nothing": a file that really
/// is gone must still be recorded as gone, in the same pass and the same tree.
#[test]
fn a_file_that_was_really_deleted_is_still_marked_missing() {
    let sandbox = Sandbox::new("exclusions-still-buries");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    write(&sandbox.documents.join("project/src/main.py"), "наш код\n");
    write(&sandbox.documents.join("project/cache/f0.txt"), "кэш\n");
    write(&sandbox.documents.join("project/src/gone.py"), "исчезнет\n");
    assert!(
        sandbox
            .crosskit(&["index", "init", &sandbox.documents_argument()])
            .status
            .success()
    );

    // One directory becomes marked and one file is genuinely deleted, in the same pass.
    write(
        &sandbox.documents.join("project/cache/CACHEDIR.TAG"),
        CACHEDIR_TAG,
    );
    fs::remove_file(sandbox.documents.join("project/src/gone.py")).unwrap();
    let reconciled = sandbox.crosskit(&["index", "reconcile", &sandbox.documents_argument()]);

    let stdout = String::from_utf8_lossy(&reconciled.stdout);
    assert!(
        stdout.contains("Locations marked missing: 1"),
        "the deleted file, and only it: {stdout}"
    );
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn search(sandbox: &Sandbox, query: &str) -> String {
    let output = sandbox.crosskit(&["search", query]);
    assert!(
        output.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
