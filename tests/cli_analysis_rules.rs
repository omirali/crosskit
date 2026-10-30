// CLI smoke: what is analysed without asking, what waits for the owner, and what one
// decision covers.
//
// Stage 22 measured the cost: a queue of every revision on a real tree is seventy-seven
// hours of a subscription. Stage 23 answers with a rule — documents analyse themselves, the
// rest waits and is visible — so these tests are about which revisions become work at all.
//
// The engine is never reached here. Every assertion is about the queue and the review list,
// which are decided before a job is claimed.
mod common;

use std::fs;

use common::{Sandbox, assert_counters};

fn queue(sandbox: &Sandbox) -> String {
    String::from_utf8(sandbox.crosskit(&["index", "jobs"]).stdout).unwrap()
}

fn review(sandbox: &Sandbox) -> String {
    let output = sandbox.crosskit(&["index", "review"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A tree with one document and three sources, which is the shape of every real tree: the
/// thing a person wrote prose in, and the code around it.
fn mixed(name: &str) -> Sandbox {
    let sandbox = Sandbox::new(name);
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    fs::write(sandbox.documents.join("readme.md"), "# Заметка о проекте\n").unwrap();
    for index in 0..3 {
        fs::write(
            sandbox.documents.join(format!("module{index}.py")),
            format!("def f{index}(): pass\n"),
        )
        .unwrap();
    }
    sandbox.reconcile();
    sandbox
}

#[test]
fn a_document_gets_a_job_and_a_source_file_does_not() {
    let sandbox = mixed("rules-documents");

    assert_counters(&queue(&sandbox), &["Insight jobs queued: 1"]);
    let waiting = review(&sandbox);
    assert_counters(
        &waiting,
        &[
            "Analysed by rule: 1 revision(s)",
            "Waiting for a decision: 3",
        ],
    );
    assert!(waiting.contains("**/*.py"), "{waiting}");
}

/// The estimate is what makes the list a decision rather than a number: a group says what
/// approving it would cost, at the price one job was measured to take.
#[test]
fn every_group_says_what_approving_it_would_cost() {
    let sandbox = mixed("rules-estimate");

    let waiting = review(&sandbox);

    assert!(
        waiting.contains("(~81 s)"),
        "three jobs at the measured price: {waiting}"
    );
}

/// Approving a mask covers what is already there. The revisions exist, so the approval
/// reaches back over them rather than waiting for them to change.
#[test]
fn approving_a_mask_fills_the_queue_for_what_is_already_indexed() {
    let sandbox = mixed("rules-approve");

    let approved = sandbox.crosskit(&["index", "review", "approve", "**/*.py", "--yes"]);

    let stdout = String::from_utf8(approved.stdout).unwrap();
    assert!(approved.status.success(), "{stdout}");
    assert!(stdout.contains("Queued for analysis: 3"), "{stdout}");
    assert_counters(&queue(&sandbox), &["Insight jobs queued: 4"]);
    // And the decision is in the file, so the next revision of that kind is covered too.
    let written = fs::read_to_string(sandbox.data_directory.join("config.toml")).unwrap();
    assert!(written.contains("approved = [\"**/*.py\"]"), "{written}");
}

/// Approving the same mask twice writes nothing and queues nothing. An approval is an
/// answer to a question, and the question was already answered.
#[test]
fn approving_the_same_mask_twice_changes_nothing() {
    let sandbox = mixed("rules-approve-twice");
    assert!(
        sandbox
            .crosskit(&["index", "review", "approve", "**/*.py", "--yes"])
            .status
            .success()
    );

    let again = sandbox.crosskit(&["index", "review", "approve", "**/*.py", "--yes"]);

    let stdout = String::from_utf8(again.stdout).unwrap();
    assert!(stdout.contains("Already approved"), "{stdout}");
    assert!(stdout.contains("Nothing was written."), "{stdout}");
    assert_counters(&queue(&sandbox), &["Insight jobs queued: 4"]);
}

/// The previous configuration is kept, as `setup` keeps it: a command that rewrites a file
/// a person wrote leaves the old one reachable.
#[test]
fn approving_keeps_the_configuration_it_replaced() {
    let sandbox = mixed("rules-approve-backup");

    let approved = sandbox.crosskit(&["index", "review", "approve", "**/*.py", "--yes"]);

    assert!(
        String::from_utf8_lossy(&approved.stdout).contains("Previous configuration kept at"),
        "{}",
        String::from_utf8_lossy(&approved.stdout)
    );
    let kept: Vec<_> = fs::read_dir(&sandbox.data_directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("config.toml.")
        })
        .collect();
    assert_eq!(kept.len(), 1, "exactly one kept configuration");
}

/// A revision that stops being covered stops being asked about — through the one outcome
/// stage 21 gave, not a second state of its own.
#[test]
fn a_revision_that_stops_being_covered_is_skipped_rather_than_worked() {
    let sandbox = mixed("rules-uncovered");
    assert_counters(&queue(&sandbox), &["Insight jobs queued: 1"]);

    // The document stops being covered: the automatic list is replaced by one that does not
    // mention it.
    sandbox.write_config("[analysis]\nautomatic = [\"**/*.never\"]\n\n");
    let worked = sandbox.crosskit(&["index", "jobs", "run", "--limit", "5"]);

    let stdout = String::from_utf8(worked.stdout).unwrap();
    assert!(
        stdout.contains("Skipped jobs (the policy no longer asks): 1"),
        "{stdout}"
    );
    assert_counters(
        &queue(&sandbox),
        &["Insight jobs skipped: 1", "Insight jobs queued: 0"],
    );
}

/// And a revision covered again gets its job back on the next ordinary pass, by the same
/// mechanism stage 21 built for a path returning under the policy.
#[test]
fn a_revision_covered_again_gets_its_job_back_on_the_next_pass() {
    let sandbox = mixed("rules-returns");
    sandbox.write_config("[analysis]\nautomatic = [\"**/*.never\"]\n\n");
    assert!(
        sandbox
            .crosskit(&["index", "jobs", "run", "--limit", "5"])
            .status
            .success()
    );
    assert_counters(&queue(&sandbox), &["Insight jobs skipped: 1"]);

    sandbox.write_config("");
    sandbox.reconcile();

    assert_counters(
        &queue(&sandbox),
        &["Insight jobs queued: 1", "Insight jobs skipped: 0"],
    );
}

/// The point of the whole stage: a topic is something a person sees.
///
/// And its absence is an absence. A file nobody has analysed prints no line about it — not
/// an empty label, not a dash, not "unknown". Most files on a real tree are that file, and
/// a placeholder on every one of them would be noise claiming to be information.
#[test]
fn a_topic_is_visible_in_search_and_its_absence_looks_like_absence() {
    let sandbox = Sandbox::new("rules-topic-visible");
    fs::remove_file(sandbox.documents.join("notes.md")).unwrap();
    fs::write(
        sandbox.documents.join("analysed.md"),
        "Договор аренды склада уникальноеслово.\n",
    )
    .unwrap();
    fs::write(
        sandbox.documents.join("untouched.py"),
        "# уникальноеслово в коде\n",
    )
    .unwrap();
    sandbox.write_config_sections("", "send_content_outside = true\n");
    sandbox.reconcile();

    // A tool that answers, so the document gets a topic and the source file does not: the
    // source file has no job at all, which is the rule this stage added.
    let (root, directory) = fake_tool("аренда склада");
    let worked = sandbox
        .command(&["index", "jobs", "run", "--limit", "5"])
        .env("PATH", &directory)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&worked.stdout).contains("Completed jobs: 1"),
        "{}",
        String::from_utf8_lossy(&worked.stdout)
    );

    let found = String::from_utf8(sandbox.crosskit(&["search", "уникальноеслово"]).stdout).unwrap();

    assert!(found.contains("Results: 2"), "{found}");
    assert!(
        found.contains("about: аренда склада"),
        "the analysed file says what it is about: {found}"
    );
    assert_eq!(
        found.matches("about:").count(),
        1,
        "and the file nobody analysed says nothing at all: {found}"
    );
    fs::remove_dir_all(root).unwrap();
}

/// A script called `claude` that answers with one topic, standing in for the installed tool.
fn fake_tool(topic: &str) -> (std::path::PathBuf, String) {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!(
        "crosskit-rules-tool-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("claude");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nPATH=/usr/bin:/bin\ncat >/dev/null\nprintf '%s' \
             '{{\"type\":\"result\",\"is_error\":false,\"structured_output\":\
             {{\"topic\":\"{topic}\",\"confidence\":0.9,\"entities\":[],\"relations\":[]}},\
             \"usage\":{{\"input_tokens\":10,\"output_tokens\":5}}}}'\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let directory = root.to_string_lossy().into_owned();
    (root, directory)
}
