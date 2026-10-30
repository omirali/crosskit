// CLI smoke: `crosskit index runs prune` against an isolated data directory.
mod common;

use common::{Sandbox, assert_counters};

/// Runs `index runs prune` with `arguments` and returns its stdout on success.
fn prune(sandbox: &Sandbox, arguments: &[&str]) -> String {
    let mut full = vec!["index", "runs", "prune"];
    full.extend_from_slice(arguments);
    let output = sandbox.crosskit(&full);
    assert!(
        output.status.success(),
        "prune {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn a_preview_prints_counters_only_and_changes_nothing() {
    let sandbox = Sandbox::new("preview");
    for _ in 0..3 {
        sandbox.reconcile();
    }

    let first = prune(&sandbox, &["--keep", "1"]);
    let second = prune(&sandbox, &["--keep", "1"]);

    assert_counters(
        &first,
        &[
            "nothing was changed",
            "Eligible closed runs: 3",
            "Protected running runs: 0",
            "Protected legacy OBSERVED runs: 0",
            "Retained runs: 1",
            "Would remove runs: 2",
        ],
    );
    assert_eq!(second, first, "a preview must not change the next preview");
    assert!(
        !first.contains("reconcile-run:"),
        "no run id may be printed"
    );
    assert!(
        !first.contains(&sandbox.documents_argument()),
        "no path may be printed"
    );
}

#[test]
fn apply_removes_the_older_runs_and_a_second_apply_removes_none() {
    let sandbox = Sandbox::new("apply");
    for _ in 0..3 {
        sandbox.reconcile();
    }

    let applied = prune(&sandbox, &["--keep", "1", "--apply"]);
    let repeated = prune(&sandbox, &["--apply", "--keep=1"]);
    let everything = prune(&sandbox, &["--keep", "0", "--apply"]);

    assert_counters(
        &applied,
        &[
            "retention applied",
            "Eligible closed runs: 3",
            "Retained runs: 1",
            "Removed runs: 2",
        ],
    );
    assert_counters(
        &repeated,
        &[
            "Eligible closed runs: 1",
            "Retained runs: 1",
            "Removed runs: 0",
        ],
    );
    assert_counters(&everything, &["Eligible closed runs: 1", "Removed runs: 1"]);
    // Reconciliation keeps working after the history is gone.
    sandbox.reconcile();
    assert_counters(
        &prune(&sandbox, &["--keep", "5"]),
        &["Eligible closed runs: 1", "Would remove runs: 0"],
    );
}

#[test]
fn invalid_arguments_fail_with_exit_code_1_before_touching_storage() {
    let sandbox = Sandbox::new("invalid");
    for arguments in [
        vec!["index", "runs", "prune"],
        vec!["index", "runs", "prune", "--apply"],
        vec!["index", "runs", "prune", "--keep"],
        vec!["index", "runs", "prune", "--keep", "-1"],
        vec!["index", "runs", "prune", "--keep", "many"],
        vec!["index", "runs", "prune", "--keep", "1", "--now"],
        vec!["index", "runs", "list"],
        vec!["index", "runs"],
    ] {
        let output = sandbox.crosskit(&arguments);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{arguments:?} must exit with code 1"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("crosskit failed"),
            "{arguments:?} must explain the failure, got: {stderr}"
        );
    }
    assert!(
        !sandbox.database_exists(),
        "a usage error must not open the database"
    );
}
