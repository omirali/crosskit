// CLI smoke: `crosskit index jobs` and `index jobs run` against an isolated data
// directory. Both print counters and nothing else, so the checks are on the numbers and on
// the absence of any path, id or hash.
mod common;

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

fn jobs_summary(sandbox: &Sandbox) -> String {
    succeeds(sandbox, &["index", "jobs"])
}

/// No path the sandbox indexed, no job or result id and no content hash may appear.
fn assert_says_nothing_about_the_file(output: &str) {
    assert!(
        !output.contains("notes.md")
            && !output.contains("/")
            && !output.contains("insight-")
            && !output.contains("hash"),
        "no path, id or hash may be printed:\n{output}"
    );
}

#[test]
fn the_summary_counts_every_state_and_only_evidence_commits_fill_it() {
    let sandbox = Sandbox::new("jobs");

    assert_counters(
        &jobs_summary(&sandbox),
        &[
            "Insight jobs queued: 0",
            "Insight jobs claimed: 0",
            "Insight jobs done: 0",
            "Insight jobs failed: 0",
            "Insight jobs dead: 0",
        ],
    );

    // One indexed note is one revision is one job; re-reading it adds none.
    sandbox.reconcile();
    sandbox.reconcile();
    let summary = jobs_summary(&sandbox);

    assert_counters(&summary, &["Insight jobs queued: 1"]);
    // The context it counted and one line per state: nothing else is printed.
    assert_eq!(summary.lines().count(), 7, "aggregates only:\n{summary}");
    assert_says_nothing_about_the_file(&summary);
}

#[test]
fn a_worker_pass_finishes_the_queued_job_and_the_next_pass_finds_nothing_to_do() {
    let sandbox = Sandbox::new("jobs-run");
    sandbox.reconcile();

    let first = succeeds(&sandbox, &["index", "jobs", "run", "--limit", "10"]);

    assert_counters(
        &first,
        &[
            "Returned to the queue: 0",
            "Retired as dead: 0",
            "Claimed jobs: 1",
            "Completed jobs: 1",
            "Failed jobs: 0",
            "Skipped jobs (the policy no longer asks): 0",
        ],
    );
    assert_eq!(
        first.lines().count(),
        8,
        "the context, which analysis ran, and counters:\n{first}"
    );
    assert_says_nothing_about_the_file(&first);
    assert_counters(
        &jobs_summary(&sandbox),
        &["Insight jobs queued: 0", "Insight jobs done: 1"],
    );

    // The finished job is not worked again, and nothing is recovered: it is done, not lost.
    let second = succeeds(&sandbox, &["index", "jobs", "run", "--limit", "10"]);

    assert_counters(
        &second,
        &[
            "Returned to the queue: 0",
            "Retired as dead: 0",
            "Claimed jobs: 0",
            "Completed jobs: 0",
        ],
    );
    assert_counters(&jobs_summary(&sandbox), &["Insight jobs done: 1"]);
}

#[test]
fn a_limit_of_zero_leaves_the_queue_where_it_was() {
    let sandbox = Sandbox::new("jobs-run-zero");
    sandbox.reconcile();

    let report = succeeds(&sandbox, &["index", "jobs", "run", "--limit=0"]);

    assert_counters(&report, &["Claimed jobs: 0", "Completed jobs: 0"]);
    assert_counters(&jobs_summary(&sandbox), &["Insight jobs queued: 1"]);
}

/// The boundary, checked by behaviour rather than by reading the code. A key is set and
/// the endpoint is compiled in, so if this context built a request at all it would reach
/// the network with a key the service rejects — and the job would come back failed, not
/// done. A completed job with a local analysis is the proof that nothing was built.
#[test]
fn a_context_that_keeps_its_content_never_builds_a_request_even_with_a_key_present() {
    let sandbox = Sandbox::new("jobs-denied-egress");
    sandbox.reconcile();

    let output = sandbox
        .command(&["index", "jobs", "run", "--limit", "10"])
        .env("ANTHROPIC_API_KEY", "not-a-real-key")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert!(output.status.success(), "{stdout}");
    assert_counters(
        &stdout,
        &[
            "Analysis: local (this context does not send content outside)",
            "Claimed jobs: 1",
            "Completed jobs: 1",
            "Failed jobs: 0",
        ],
    );
    assert_says_nothing_about_the_file(&stdout);
    assert_counters(&jobs_summary(&sandbox), &["Insight jobs done: 1"]);
}

/// A context that may send content, with no key: the reason is said once and nothing is
/// claimed. Claiming would spend attempts on a call that was never going to happen, and a
/// job that quietly fell back to counting could never be analysed later.
#[test]
fn a_context_that_may_send_content_says_why_it_did_not_run_and_leaves_the_queue_alone() {
    let sandbox = Sandbox::new("jobs-no-key");
    // The engine is named, because the default one is a tool that may be installed on the
    // machine running the tests, and then this would be a live call on a subscription.
    sandbox.write_config_sections(
        "[analysis]\nengine = \"anthropic-api\"\n\n",
        "send_content_outside = true\n",
    );
    sandbox.reconcile();

    let output = sandbox
        .command(&["index", "jobs", "run", "--limit", "10"])
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("ANTHROPIC_API_KEY") && stdout.contains("No job was claimed"),
        "the reason must be said once:\n{stdout}"
    );
    assert_eq!(stdout.lines().count(), 2, "one context line, one reason");
    // The queue is exactly as it was: nothing claimed, nothing attempted, nothing done.
    assert_counters(
        &jobs_summary(&sandbox),
        &[
            "Insight jobs queued: 1",
            "Insight jobs claimed: 0",
            "Insight jobs done: 0",
            "Insight jobs failed: 0",
            "Insight jobs dead: 0",
        ],
    );
}

/// The reason `withdraw` exists, end to end.
///
/// `--limit` bounds what is claimed and only what is worked costs anything, so a queue
/// filled before the exclusions learned the machine hands out pages that are skipped for
/// nothing and counted against a limit that was set for money. On the owner's tree a run of
/// twenty claimed twenty and completed three, and reaching 1 703 documents that way meant
/// running the command eighty-six times.
///
/// One sweep brings the whole queue to the rules instead, and the run after it spends its
/// limit on work.
#[test]
fn one_sweep_brings_the_whole_queue_to_the_rules_and_the_next_run_reaches_the_documents() {
    let sandbox = Sandbox::new("jobs-withdraw");
    let vendored = sandbox.documents.join("vendored");
    std::fs::create_dir_all(&vendored).unwrap();
    for ordinal in 0..5 {
        std::fs::write(
            vendored.join(format!("package{ordinal}.md")),
            "чужой пакет\n",
        )
        .unwrap();
    }
    sandbox.reconcile();
    assert_counters(&jobs_summary(&sandbox), &["Insight jobs queued: 6"]);

    // The policy changes after the jobs were queued, which is the whole point.
    sandbox.write_config(&format!(
        "[index]\nexclude = [\"{}/**\"]\n\n",
        std::fs::canonicalize(&vendored).unwrap().display()
    ));

    let swept = succeeds(&sandbox, &["index", "jobs", "withdraw"]);

    assert_counters(
        &swept,
        &[
            "Queued jobs examined: 6",
            "Withdrawn (no rule asks for them any more): 5",
            "Still queued: 1",
        ],
    );
    assert_eq!(
        swept.lines().count(),
        4,
        "the context and counters:\n{swept}"
    );
    assert_says_nothing_about_the_file(&swept);
    // Retired, not failed and not dead: nothing went wrong and nothing was attempted.
    assert_counters(
        &jobs_summary(&sandbox),
        &[
            "Insight jobs queued: 1",
            "Insight jobs claimed: 0",
            "Insight jobs failed: 0",
            "Insight jobs dead: 0",
            "Insight jobs skipped: 5",
        ],
    );

    // The limit now buys work: one job claimed, one done, nothing skipped out of it.
    let worked = succeeds(&sandbox, &["index", "jobs", "run", "--limit", "1"]);

    assert_counters(
        &worked,
        &[
            "Claimed jobs: 1",
            "Completed jobs: 1",
            "Skipped jobs (the policy no longer asks): 0",
        ],
    );
}

/// A sweep over a queue that already agrees with the rules withdraws nothing and takes
/// nothing out of a worker's hands, so running it twice is the same as running it once.
#[test]
fn a_second_sweep_finds_nothing_left_to_withdraw() {
    let sandbox = Sandbox::new("jobs-withdraw-twice");
    sandbox.reconcile();

    let first = succeeds(&sandbox, &["index", "jobs", "withdraw"]);
    let second = succeeds(&sandbox, &["index", "jobs", "withdraw"]);

    for swept in [&first, &second] {
        assert_counters(
            swept,
            &[
                "Queued jobs examined: 1",
                "Withdrawn (no rule asks for them any more): 0",
                "Still queued: 1",
            ],
        );
    }
    assert_counters(&jobs_summary(&sandbox), &["Insight jobs queued: 1"]);
}

/// A sweep withdraws in the same state a claim does, so the path that comes back under
/// indexing gets its analysis back by the same route — stage 21's one reversible state, not
/// a second kind of retirement this command would have had to teach the rest of the queue.
#[test]
fn a_path_that_comes_back_after_a_sweep_gets_its_analysis_back() {
    let sandbox = Sandbox::new("jobs-withdraw-returns");
    let excluded = format!(
        "[index]\nexclude = [\"{}/notes.md\"]\n\n",
        std::fs::canonicalize(&sandbox.documents).unwrap().display()
    );
    sandbox.reconcile();
    sandbox.write_config(&excluded);
    succeeds(&sandbox, &["index", "jobs", "withdraw"]);
    assert_counters(&jobs_summary(&sandbox), &["Insight jobs skipped: 1"]);

    sandbox.write_config("");
    sandbox.reconcile();

    assert_counters(
        &jobs_summary(&sandbox),
        &["Insight jobs queued: 1", "Insight jobs skipped: 0"],
    );
}

/// A sweep sends nothing and reads no file, so it answers in a context that may send
/// content and has no key — where `index jobs run` refuses to claim anything at all.
#[test]
fn a_sweep_needs_no_engine_and_no_key() {
    let sandbox = Sandbox::new("jobs-withdraw-no-key");
    sandbox.write_config_sections(
        "[analysis]\nengine = \"anthropic-api\"\n\n",
        "send_content_outside = true\n",
    );
    sandbox.reconcile();

    let output = sandbox
        .command(&["index", "jobs", "withdraw"])
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert_counters(&stdout, &["Queued jobs examined: 1", "Still queued: 1"]);
    assert!(
        !stdout.contains("ANTHROPIC_API_KEY") && !stdout.contains("Analysis:"),
        "no engine is decided on:\n{stdout}"
    );
}

#[test]
fn a_permission_for_a_context_that_covers_nothing_is_refused_before_anything_opens() {
    let sandbox = Sandbox::new("jobs-egress-typo");
    sandbox.write_config_sections("", "\n[workspace.typo]\nsend_content_outside = true\n");

    let output = sandbox.crosskit(&["index", "jobs"]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("covers no path"),
        "the message must name the mistake"
    );
    assert!(!sandbox.database_exists());
}

#[test]
fn invalid_arguments_fail_with_exit_code_1_before_the_database_is_opened() {
    let sandbox = Sandbox::new("jobs-invalid");
    for arguments in [
        vec!["index", "jobs", "--all"],
        vec!["index", "jobs", "list"],
        vec!["index", "jobs", "run"],
        vec!["index", "jobs", "run", "--limit"],
        vec!["index", "jobs", "run", "--limit", "all"],
        vec!["index", "jobs", "run", "--limit", "1", "--force"],
        vec!["index", "jobs", "withdraw", "--limit", "5"],
        vec!["index", "jobs", "withdraw", "--yes"],
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

/// A queue outlives the policy that filled it, so the policy is asked when a job is claimed
/// and not when it was queued.
///
/// This is the pass that costs money. On the owner's tree a queue built before the
/// exclusions learned the machine held 177 800 jobs, of which 151 848 — 85.4 % — were
/// revisions of other people's packages that the policy had since stopped asking about.
/// Every one of them would have been read, sent to a model and paid for.
#[test]
fn a_job_the_policy_stopped_asking_about_is_retired_instead_of_worked() {
    let sandbox = Sandbox::new("jobs-policy");
    std::fs::write(sandbox.documents.join("vendored.md"), "чужой пакет\n").unwrap();
    sandbox.reconcile();
    assert_counters(&jobs_summary(&sandbox), &["Insight jobs queued: 2"]);

    // The policy changes after the jobs were queued, which is the whole point.
    sandbox.write_config(&format!(
        "[index]\nexclude = [\"{}/vendored.md\"]\n\n",
        std::fs::canonicalize(&sandbox.documents).unwrap().display()
    ));

    let worked = succeeds(&sandbox, &["index", "jobs", "run", "--limit", "10"]);

    assert_counters(
        &worked,
        &[
            "Claimed jobs: 2",
            "Completed jobs: 1",
            "Failed jobs: 0",
            "Skipped jobs (the policy no longer asks): 1",
        ],
    );
    // Retired, not failed and not dead: nothing went wrong, the question was withdrawn.
    assert_counters(
        &jobs_summary(&sandbox),
        &[
            "Insight jobs queued: 0",
            "Insight jobs failed: 0",
            "Insight jobs dead: 0",
            "Insight jobs done: 1",
            "Insight jobs skipped: 1",
        ],
    );
    assert_says_nothing_about_the_file(&worked);
}

/// And the other direction, which is the part that could not be left to chance: a path that
/// comes back under indexing gets its analysis back.
///
/// The job returns on the next observation rather than staying terminal for ever. It is the
/// one state a re-published revision may reset, and it is safe to reset precisely because
/// it stands for no work: nothing was attempted, nothing was produced, nothing was spent.
/// The pass that resets it runs only for a file the policy admits, so the condition that
/// withdrew the job is the same condition observed to be reversed.
#[test]
fn a_path_that_comes_back_under_indexing_gets_its_analysis_back() {
    let sandbox = Sandbox::new("jobs-returns");
    std::fs::write(sandbox.documents.join("returns.md"), "вернётся\n").unwrap();
    let excluded = format!(
        "[index]\nexclude = [\"{}/returns.md\"]\n\n",
        std::fs::canonicalize(&sandbox.documents).unwrap().display()
    );
    sandbox.reconcile();
    sandbox.write_config(&excluded);
    succeeds(&sandbox, &["index", "jobs", "run", "--limit", "10"]);
    assert_counters(&jobs_summary(&sandbox), &["Insight jobs skipped: 1"]);

    // The exclusion is withdrawn and an ordinary pass observes the file again.
    sandbox.write_config("");
    sandbox.reconcile();

    assert_counters(
        &jobs_summary(&sandbox),
        &["Insight jobs queued: 1", "Insight jobs skipped: 0"],
    );
    let worked = succeeds(&sandbox, &["index", "jobs", "run", "--limit", "10"]);
    assert_counters(
        &worked,
        &[
            "Completed jobs: 1",
            "Skipped jobs (the policy no longer asks): 0",
        ],
    );
}
