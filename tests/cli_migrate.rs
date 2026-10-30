// CLI smoke: `crosskit migrate`, the one command that brings every context's graph to the
// schema this build reads.
//
// The two things worth checking are what it does and what it refuses to do. It has to reach
// every configured context in one run — that is the whole reason it exists, after a schema
// change left the owner repairing four contexts one command at a time. And it must not
// create a graph for a context nobody has indexed, because a migration that indexes is a
// first index by the wrong command.
mod common;

use std::fs;

use common::{Sandbox, assert_counters};

fn succeeds(sandbox: &Sandbox, arguments: &[&str]) -> String {
    let output = sandbox.crosskit(arguments);
    assert!(
        output.status.success(),
        "{arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A second context beside the sandbox's own, covering a directory that is never indexed.
fn with_a_second_context(sandbox: &Sandbox) {
    let spare = sandbox.documents.with_file_name("spare");
    fs::create_dir_all(&spare).unwrap();
    fs::write(
        sandbox.data_directory.join("config.toml"),
        format!(
            "[workspace.{}]\ninclude = [\"{}\"]\n\n[workspace.spare]\ninclude = [\"{}\"]\n",
            Sandbox::CONTEXT,
            fs::canonicalize(&sandbox.documents).unwrap().display(),
            fs::canonicalize(&spare).unwrap().display()
        ),
    )
    .unwrap();
}

/// One run, every context, and the schema each one now stands at. A graph already at this
/// build's schema is reported like any other: a command that only spoke up when it had
/// changed something would leave a reader unable to tell a migrated context from one it
/// forgot to name.
#[test]
fn one_run_names_every_context_and_the_schema_it_now_stands_at() {
    let sandbox = Sandbox::new("migrate-all");
    // Indexed first, so the second context is one that has never been indexed — which is
    // the mixture a real machine is in when a build outruns its schema.
    sandbox.reconcile();
    with_a_second_context(&sandbox);

    let migrated = succeeds(&sandbox, &["migrate"]);

    assert_counters(&migrated, &["Contexts: 2", "sandbox", "spare"]);
    assert!(
        migrated.contains("Schema version:"),
        "the graph that exists is named with what it now reads as:\n{migrated}"
    );
    assert!(
        migrated.contains("Nothing to migrate"),
        "the context with no graph is reported rather than left out:\n{migrated}"
    );
}

/// The line that matters most: a context nobody has indexed does not gain a database
/// because somebody asked for a migration. Checked against the data directory, not against
/// the words printed.
#[test]
fn a_context_that_was_never_indexed_is_reported_and_no_graph_is_created_for_it() {
    let sandbox = Sandbox::new("migrate-never-indexed");

    let migrated = succeeds(&sandbox, &["migrate"]);

    assert!(migrated.contains("Nothing to migrate"), "{migrated}");
    assert!(
        !sandbox.database_exists(),
        "a migration must not be a first index"
    );
}

/// `--context <name>` migrates that one, like every other command that works in a context.
#[test]
fn naming_a_context_migrates_that_one_alone() {
    let sandbox = Sandbox::new("migrate-one");
    // Indexed first, so the second context is one that has never been indexed — which is
    // the mixture a real machine is in when a build outruns its schema.
    sandbox.reconcile();
    with_a_second_context(&sandbox);

    let migrated = succeeds(&sandbox, &["--context", "sandbox", "migrate"]);

    assert_counters(&migrated, &["Contexts: 1", "sandbox"]);
    assert!(!migrated.contains("spare"), "{migrated}");
}

/// Nothing configured is an answer, not a failure: there is no graph to migrate until a
/// context exists, and the run says where contexts come from.
#[test]
fn a_machine_with_no_context_is_told_where_contexts_come_from() {
    let sandbox = Sandbox::new("migrate-no-context");
    fs::write(sandbox.data_directory.join("config.toml"), "").unwrap();

    let migrated = succeeds(&sandbox, &["migrate"]);

    assert_counters(&migrated, &["Contexts: none configured.", "crosskit setup"]);
}

#[test]
fn invalid_arguments_fail_with_exit_code_1_before_the_database_is_opened() {
    let sandbox = Sandbox::new("migrate-invalid");

    for arguments in [
        vec!["migrate", "--all"],
        vec!["migrate", "now"],
        vec!["migrate", "--context"],
    ] {
        let output = sandbox.crosskit(&arguments);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{arguments:?} must exit with code 1"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("crosskit failed"),
            "{arguments:?} must explain the failure"
        );
    }
    assert!(
        !sandbox.database_exists(),
        "a usage error must not open the database"
    );
}
