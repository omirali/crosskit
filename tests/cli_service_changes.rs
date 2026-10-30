// The service walks what changed, not the context (ADR 0050) — and ends where a whole pass
// would have ended.
//
// A real service over a sandbox context with real FSEvents. Each change below is made, and
// the test waits until `search` — answered through the service — shows its effect; the
// journal then has to say that those passes walked changed directories and not roots. The
// last word is a whole `index reconcile` once the service is gone: if the scoped passes left
// anything behind, it would retire or publish something, and the graph would differ.
mod common;

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use common::Sandbox;
use common::service::{BOUND, ServiceSandbox, stderr, stdout, terminate};

/// What `search <word> --include-missing --json` says about the file at `relative`: present,
/// missing (gone from disk), or absent — not a row at all, which is what the old place of a
/// moved file is, since the file is one row at its new place.
fn state_of(service: &ServiceSandbox, word: &str, relative: &str) -> &'static str {
    let output = service.crosskit(&["search", word, "--include-missing", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let wanted = fs::canonicalize(&service.sandbox.documents)
        .unwrap()
        .join(relative);
    for row in answer["results"].as_array().unwrap() {
        if row["path"].as_str() == wanted.to_str() {
            return match row["state"].as_str() {
                Some("present") => "present",
                _ => "missing",
            };
        }
    }
    "absent"
}

/// Waits until every `(word, path, state)` holds.
fn until(service: &ServiceSandbox, expected: &[(&str, &str, &str)]) {
    let deadline = Instant::now() + BOUND;
    loop {
        let seen: Vec<&str> = expected
            .iter()
            .map(|(word, path, _)| state_of(service, word, path))
            .collect();
        if seen
            .iter()
            .zip(expected)
            .all(|(seen, (_, _, wanted))| seen == wanted)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected:?}, saw {seen:?}\njournal:\n{}",
            service.journal()
        );
        thread::sleep(Duration::from_millis(300));
    }
}

/// Every location and its status, and how many revisions the graph keeps.
fn graph_state(sandbox: &Sandbox) -> (Vec<String>, Vec<String>) {
    (
        sandbox.graph_rows(
            "MATCH (l:FileLocation) RETURN l.normalized_path, l.status ORDER BY l.normalized_path;",
        ),
        sandbox.graph_rows("MATCH (v:FileVersion) RETURN COUNT(*);"),
    )
}

#[test]
fn moves_renames_removals_and_excluded_paths_are_walked_where_they_happened() {
    let service = ServiceSandbox::new("service-changes");
    let documents = service.sandbox.documents.clone();
    let write = |relative: &str, text: &str| {
        let path = documents.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("a/moving.md", "a zebrafish note that moves");
    fs::create_dir_all(documents.join("b")).unwrap();
    write(
        "old-dir/inner.md",
        "an okapi inside a directory that is renamed",
    );
    write(
        "gone-dir/deep/file.md",
        "a narwhal in a directory that is removed",
    );
    let running = service.start();
    service.status_saying("Passes: 1 completed");
    until(
        &service,
        &[
            ("zebrafish", "a/moving.md", "present"),
            ("okapi", "old-dir/inner.md", "present"),
            ("narwhal", "gone-dir/deep/file.md", "present"),
        ],
    );

    // A file moved from one directory to another: both sides. It is one file, so the search
    // shows it once, at the place it is now — the old place is not a missing file of its own.
    fs::rename(documents.join("a/moving.md"), documents.join("b/moving.md")).unwrap();
    until(
        &service,
        &[
            ("zebrafish", "a/moving.md", "absent"),
            ("zebrafish", "b/moving.md", "present"),
        ],
    );

    // A directory renamed: everything under the old name, and the new one.
    fs::rename(documents.join("old-dir"), documents.join("new-dir")).unwrap();
    until(
        &service,
        &[
            ("okapi", "old-dir/inner.md", "absent"),
            ("okapi", "new-dir/inner.md", "present"),
        ],
    );

    // A directory removed with everything in it.
    fs::remove_dir_all(documents.join("gone-dir")).unwrap();
    until(&service, &[("narwhal", "gone-dir/deep/file.md", "missing")]);

    // A path the policy excludes, written before one it does not: once the second is in the
    // index, the pass that would have taken the first has run too.
    write(
        "web/node_modules/pkg/readme.md",
        "an axolotl nobody indexes",
    );
    write("web/index.md", "a quokka beside it");
    until(&service, &[("quokka", "web/index.md", "present")]);
    assert_eq!(
        state_of(&service, "axolotl", "web/node_modules/pkg/readme.md"),
        "absent"
    );

    let journal = service.journal();
    assert!(
        journal.contains("changed directory(ies)"),
        "no pass was scoped:\n{journal}"
    );
    assert!(terminate(running).success());

    // What a whole pass would still change, after everything the scoped ones did: nothing.
    let before = graph_state(&service.sandbox);
    let whole = service.crosskit(&["index", "reconcile"]);
    assert!(
        whole.status.success(),
        "{}{}",
        stdout(&whole),
        stderr(&whole)
    );
    assert_eq!(graph_state(&service.sandbox), before);
}
