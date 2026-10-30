// CLI smoke: `crosskit index watch` as a child process with an isolated data directory,
// followed through its stdout and stopped with SIGINT exactly like Ctrl-C.
mod common;

use std::fs;

use common::{Sandbox, WatchProcess, assert_counters};

fn counter(lines: &[String], prefix: &str) -> usize {
    let line = lines
        .iter()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starting with {prefix:?} in:\n{}", lines.join("\n")));
    line[prefix.len()..].trim().parse().unwrap()
}

#[test]
fn invalid_arguments_fail_with_exit_code_1_before_the_database_is_opened() {
    let sandbox = Sandbox::new("watch-invalid");
    let missing = sandbox
        .documents
        .join("missing")
        .to_string_lossy()
        .into_owned();
    // `index watch` with no directory is not a bad argument any more: it watches the whole
    // context, the way `index init` and `index reconcile` do.
    for arguments in [
        vec!["index", "watch", "a", "b"],
        vec!["index", "watch", missing.as_str()],
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
        "a bad argument must not open the database"
    );
}

#[test]
fn a_watch_reconciles_changes_and_stops_cleanly_on_ctrl_c() {
    let sandbox = Sandbox::new("watch-lifecycle");
    sandbox.write_config("[index]\nexclude = [\"**/*.confidential\"]\n\n");
    // A history exists before the watch starts, as it would for a real index.
    sandbox.reconcile();

    let mut watch = WatchProcess::spawn(&sandbox);
    // The watch itself says whether change hints arrive here; without them the
    // event-driven part is skipped and the degraded lifecycle is checked instead.
    let started = watch.wait_for_line("Watching ");
    let hints_arrive = started.contains("event-driven mode");
    if !hints_arrive {
        eprintln!("skipped: no filesystem change hints arrive in this environment");
        assert!(started.contains("degraded"), "{started}");
        watch.wait_for_line("Change notifications could not be confirmed");
    }
    watch.wait_for_line("Pass 1 (bootstrap) completed");

    if hints_arrive {
        fs::write(sandbox.documents.join("notes.md"), "edited under watch").unwrap();
        fs::write(
            sandbox.documents.join("ledger.confidential"),
            "never indexed",
        )
        .unwrap();
        let events_pass = watch.wait_for_line("(events) completed");
        assert!(events_pass.contains("1 excluded"), "{events_pass}");

        // The text this pass wrote is searchable from another process straight away: the
        // watcher holds the database for writing, and a command that writes nothing opens
        // the same database read-only beside it. What that opening can and cannot do is
        // `tests/cli_concurrent_read.rs`; what matters here is that a running watcher no
        // longer shuts the context.
        let found = sandbox.crosskit(&["search", "edited"]);
        assert!(
            found.status.success(),
            "a reader must reach a database a watcher holds: {}",
            String::from_utf8_lossy(&found.stderr)
        );
        assert!(
            String::from_utf8_lossy(&found.stdout).contains("notes.md"),
            "{}",
            String::from_utf8_lossy(&found.stdout)
        );
    }

    watch.interrupt();
    let (status, lines, stderr) = watch.wait_for_exit();

    assert_eq!(status.code(), Some(0), "stderr: {stderr}");
    let stdout = lines.join("\n");
    assert_counters(
        &stdout,
        &[
            "Stopping:",
            "Watch stopped.",
            if hints_arrive {
                "Mode: event-driven mode"
            } else {
                "Mode: degraded periodic mode"
            },
            "Passes failed: 0",
            "Retries: 0",
        ],
    );
    let probe_directory = sandbox.data_directory.join("watch-probe");
    assert!(
        fs::read_dir(&probe_directory).unwrap().next().is_none(),
        "the delivery probe must remove its file"
    );
    assert!(
        !stdout.contains("notes.md")
            && !stdout.contains("ledger")
            && !stdout.contains("confidential"),
        "no changed or excluded path may be printed:\n{stdout}"
    );
    let completed = counter(&lines, "Passes completed:");
    assert!(completed >= if hints_arrive { 2 } else { 1 }, "{stdout}");
    assert_eq!(counter(&lines, "Passes:"), completed);

    // Once the watcher is gone, what it wrote is searchable: every pass kept the search
    // index too, and the index is in step with the graph the watcher left behind.
    let doctor = sandbox.crosskit(&["doctor"]);
    assert!(doctor.status.success());
    assert!(
        String::from_utf8_lossy(&doctor.stdout).contains("in step with the graph"),
        "the watcher must leave the search index in step: {}",
        String::from_utf8_lossy(&doctor.stdout)
    );
    if hints_arrive {
        let found = sandbox.crosskit(&["search", "edited"]);
        assert!(
            found.status.success(),
            "{}",
            String::from_utf8_lossy(&found.stderr)
        );
        let found = String::from_utf8(found.stdout).unwrap();
        assert!(
            found.contains("notes.md"),
            "what the watcher indexed must be searchable afterwards: {found}"
        );
        assert!(
            !found.contains("confidential"),
            "an excluded file must not be searchable: {found}"
        );
    }

    // Every pass was a real reconcile run: the history now holds the earlier reconcile
    // plus one closed run per pass.
    let preview = sandbox.crosskit(&["index", "runs", "prune", "--keep", "0"]);
    assert!(preview.status.success());
    let preview = String::from_utf8(preview.stdout).unwrap();
    assert_counters(
        &preview,
        &[
            &format!("Eligible closed runs: {}", completed + 1),
            "Protected running runs: 0",
        ],
    );
}

#[test]
fn a_final_pass_that_cannot_complete_exits_with_code_1() {
    if std::env::var_os("USER").is_some_and(|user| user == "root") {
        eprintln!("skipped: root can read a directory without permissions");
        return;
    }
    let sandbox = Sandbox::new("watch-final-failure");
    let locked = sandbox.documents.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("unreachable.md"), "behind a closed door").unwrap();
    let readable = fs::metadata(&locked).unwrap().permissions();
    let mut closed = readable.clone();
    std::os::unix::fs::PermissionsExt::set_mode(&mut closed, 0o000);
    fs::set_permissions(&locked, closed).unwrap();

    let mut watch = WatchProcess::spawn(&sandbox);
    let bootstrap = watch.wait_for_line("Pass 1 (bootstrap) incomplete");
    assert!(bootstrap.contains("failed path(s)"), "{bootstrap}");
    watch.wait_for_line("Retry in 1s.");

    watch.interrupt();
    let (status, lines, stderr) = watch.wait_for_exit();
    fs::set_permissions(&locked, readable).unwrap();

    assert_eq!(status.code(), Some(1), "stdout:\n{}", lines.join("\n"));
    assert!(
        stderr.contains("final reconcile pass did not complete"),
        "stderr: {stderr}"
    );
    let stdout = lines.join("\n");
    assert_counters(
        &stdout,
        &[
            "Stopping: running one final reconcile pass",
            "(final) incomplete",
            "Watch stopped.",
            "Passes completed: 0",
        ],
    );
    assert!(
        !stdout.contains("unreachable") && !stdout.contains("locked"),
        "no failed path may be printed:\n{stdout}"
    );
}

/// Stage 16: the watcher also works the insight queue, but only when it was told to, only
/// within the bound it was given, and never instead of watching.
///
/// Every scenario here ends on the bootstrap pass, which runs in any environment: none of
/// them depends on filesystem change hints arriving.
const ANALYSING: &str = "[watch]\nanalyze = true\nanalyze_limit = 2\n\n";

fn job_count(sandbox: &Sandbox, state: &str) -> usize {
    let output = sandbox.crosskit(&["index", "jobs"]);
    assert!(
        output.status.success(),
        "index jobs failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let prefix = format!("Insight jobs {state}: ");
    let line = stdout
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} in:\n{stdout}"));
    line[prefix.len()..].trim().parse().unwrap()
}

/// Writes `count` notes beside the one the sandbox already holds, each with content of its
/// own so each is a revision of its own.
fn write_notes(sandbox: &Sandbox, count: usize) {
    for note in 0..count {
        fs::write(
            sandbox.documents.join(format!("note-{note}.md")),
            format!("a note about subject number {note}"),
        )
        .unwrap();
    }
}

fn stop_and_read(watch: WatchProcess) -> String {
    watch.interrupt();
    let (status, lines, stderr) = watch.wait_for_exit();
    assert_eq!(
        status.code(),
        Some(0),
        "stderr: {stderr}\nstdout:\n{}",
        lines.join("\n")
    );
    lines.join("\n")
}

/// The default: a watcher nobody asked to analyse does exactly what it did before analysis
/// existed. Not one job is taken, and the queue is where the next `index jobs run` expects
/// to find it.
#[test]
fn a_watcher_that_was_not_asked_to_analyse_leaves_the_queue_untouched() {
    let sandbox = Sandbox::new("watch-queue-off");
    write_notes(&sandbox, 2);
    sandbox.reconcile();
    assert_eq!(job_count(&sandbox, "queued"), 3);

    let mut watch = WatchProcess::spawn(&sandbox);
    watch.wait_for_line("Pass 1 (bootstrap) completed");
    let stdout = stop_and_read(watch);

    assert!(
        !stdout.contains("Analysis") && !stdout.contains("analysis:"),
        "a watcher that was not asked to analyse says nothing about it:\n{stdout}"
    );
    assert_eq!(job_count(&sandbox, "queued"), 3);
    assert_eq!(job_count(&sandbox, "done"), 0);
    assert_eq!(job_count(&sandbox, "claimed"), 0);
}

/// Turned on: one sweep per pass, never more jobs than the limit, and the numbers the
/// watcher printed are the numbers `index jobs` prints afterwards.
#[test]
fn an_analysing_watcher_takes_no_more_than_the_limit_and_its_counters_match_the_queue() {
    let sandbox = Sandbox::new("watch-analysis-on");
    sandbox.write_config(ANALYSING);
    write_notes(&sandbox, 4);
    sandbox.reconcile();
    assert_eq!(job_count(&sandbox, "queued"), 5);

    let mut watch = WatchProcess::spawn(&sandbox);
    let announced = watch.wait_for_line("Analysis: ");
    assert!(
        announced.contains("at most 2 job(s) per pass"),
        "the watcher says what it will spend: {announced}"
    );
    watch.wait_for_line("Pass 1 (bootstrap) completed");
    let first = watch.wait_for_line("Pass 1 analysis:");
    assert_eq!(
        first, "Pass 1 analysis: 2 claimed, 2 done, 0 failed, 0 skipped, 3 left in the queue.",
        "the first sweep takes the limit and no more"
    );
    let stdout = stop_and_read(watch);

    // The session's own counters are the queue's, read from the other side by another
    // process: a report that drifted from the queue would be worse than no report.
    let done = job_count(&sandbox, "done");
    let queued = job_count(&sandbox, "queued");
    assert_eq!(job_count(&sandbox, "claimed"), 0);
    assert_eq!(done + queued, 5);
    assert_counters(
        &stdout,
        &[
            &format!(
                "Jobs claimed: {done}; done: {done}; failed: 0; skipped: 0; left in the queue: {queued}"
            ),
            "Analysis sweeps: 1 (0 could not run)",
        ],
    );
    assert!(
        !stdout.contains("note-") && !stdout.contains("notes.md"),
        "a sweep names no path:\n{stdout}"
    );
}

/// A watcher spends its limit on jobs nobody asks for any more, and says so.
///
/// This is the watcher's half of the finding `index jobs withdraw` answers. The limit
/// bounds what is claimed; only what is worked costs anything. A sweep of twenty that skips
/// seventeen bought three jobs, and before this the line said `20 claimed, 3 done, 0
/// failed` and left the other seventeen unaccounted for.
///
/// The offer of the repair is said once for the whole session, not once per pass: the
/// backlog does not change between passes, and a watcher runs for hours.
#[test]
fn an_analysing_watcher_names_what_its_limit_bought_nothing_on_and_offers_the_repair_once() {
    let sandbox = Sandbox::new("watch-analysis-skipped");
    write_notes(&sandbox, 4);
    sandbox.reconcile();
    assert_eq!(job_count(&sandbox, "queued"), 5);
    // The policy changes after the jobs were queued, which is the case the queue outlives.
    // Everything is withdrawn, so which two jobs a sweep happens to take does not decide
    // what this test asserts.
    sandbox.write_config(&format!("{ANALYSING}[index]\nexclude = [\"**/*.md\"]\n\n"));

    let mut watch = WatchProcess::spawn(&sandbox);
    let started = watch.wait_for_line("Watching ");
    let hints_arrive = started.contains("event-driven mode");
    watch.wait_for_line("Pass 1 (bootstrap) completed");

    let first = watch.wait_for_line("Pass 1 analysis:");
    assert_eq!(
        first, "Pass 1 analysis: 2 claimed, 0 done, 0 failed, 2 skipped, 3 left in the queue.",
        "the sweep spent its whole limit and bought nothing with it"
    );
    let offer = watch.wait_for_line("index jobs withdraw");
    assert!(
        offer.contains("no rule asks for any more") && offer.contains("this watcher does not"),
        "the offer names the repair and says who does not do it: {offer}"
    );

    // A second pass, where hints arrive to start one: the numbers come again, the offer
    // does not. Where they do not arrive there is no second pass to check, and the run says
    // so rather than asserting something it did not see.
    if hints_arrive {
        fs::write(sandbox.documents.join("trigger.md"), "another note").unwrap();
        let second = watch.wait_for_line("Pass 2 analysis:");
        assert!(
            second.contains("skipped"),
            "every sweep names its skips: {second}"
        );
    } else {
        eprintln!("skipped the second pass: no filesystem change hints arrive here");
    }

    let stdout = stop_and_read(watch);

    assert_eq!(
        stdout.matches("index jobs withdraw").count(),
        1,
        "the offer belongs to the session, not to the pass:\n{stdout}"
    );
    assert_counters(&stdout, &["skipped: 4"]);
    assert_eq!(job_count(&sandbox, "skipped"), 4);
    assert_eq!(job_count(&sandbox, "done"), 0);
}

/// A context that may send content, on a machine with no key. The reason is said once, in
/// the opening notice, and the queue is not touched: claiming would spend an attempt on a
/// call that was never going to happen. Watching itself carries on as always.
#[test]
fn an_analysing_watcher_with_no_key_says_why_once_and_keeps_watching() {
    let sandbox = Sandbox::new("watch-analysis-no-key");
    // The engine is named: the default one is a tool that may be installed on the machine
    // running the tests, and then this would be a live call on a subscription.
    sandbox.write_config_sections(
        &format!("{ANALYSING}[analysis]\nengine = \"anthropic-api\"\n\n"),
        "send_content_outside = true\n",
    );
    sandbox.reconcile();

    let mut command = sandbox.command(&["index", "watch", &sandbox.documents_argument()]);
    command.env_remove("ANTHROPIC_API_KEY");
    let mut watch = WatchProcess::spawn_command(command);
    watch.wait_for_line("Pass 1 (bootstrap) completed");
    let stdout = stop_and_read(watch);

    assert_counters(
        &stdout,
        &[
            "ANTHROPIC_API_KEY",
            "No job is claimed while this watcher runs.",
            "Passes failed: 0",
        ],
    );
    assert_eq!(
        stdout.matches("ANTHROPIC_API_KEY").count(),
        1,
        "said once, not once per pass:\n{stdout}"
    );
    assert_eq!(job_count(&sandbox, "queued"), 1);
    assert_eq!(job_count(&sandbox, "done"), 0);
}

/// The boundary, checked by behaviour rather than by reading the code — the same way stage
/// 6 checks it for `index jobs run`. A key is set and the endpoint is compiled in, so a
/// context that built a request at all would reach the network with a key the service
/// rejects, and the job would come back failed. A job done by the local handler is the
/// proof that nothing was built.
#[test]
fn an_analysing_watcher_in_a_context_that_keeps_its_content_never_builds_a_request() {
    let sandbox = Sandbox::new("watch-analysis-denied-egress");
    sandbox.write_config(ANALYSING);
    sandbox.reconcile();

    let mut command = sandbox.command(&["index", "watch", &sandbox.documents_argument()]);
    command.env("ANTHROPIC_API_KEY", "not-a-real-key");
    let mut watch = WatchProcess::spawn_command(command);
    let announced = watch.wait_for_line("Analysis: ");
    assert!(
        announced.contains("local (this context does not send content outside)"),
        "{announced}"
    );
    watch.wait_for_line(
        "Pass 1 analysis: 1 claimed, 1 done, 0 failed, 0 skipped, 0 left in the queue.",
    );
    stop_and_read(watch);

    assert_eq!(job_count(&sandbox, "done"), 1);
    assert_eq!(job_count(&sandbox, "failed"), 0);
}

/// Taken for granted until now: a pass of the watcher keeps the search index too, so a file
/// that was never indexed by hand is findable while the watcher is still running, with no
/// rebuild in between. The bootstrap pass is enough, so this holds where change hints never
/// arrive at all.
#[test]
fn a_watcher_pass_makes_a_file_searchable_without_a_rebuild() {
    let sandbox = Sandbox::new("watch-search-index");
    fs::write(
        sandbox.documents.join("harvest.md"),
        "the pumpkin harvest was weighed on Tuesday",
    )
    .unwrap();

    let mut watch = WatchProcess::spawn(&sandbox);
    watch.wait_for_line("Pass 1 (bootstrap) completed");

    let found = sandbox.crosskit(&["search", "pumpkin"]);
    assert!(
        found.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&found.stderr)
    );
    let found = String::from_utf8(found.stdout).unwrap();
    assert!(
        found.contains("harvest.md"),
        "the watcher's own pass must have written the search index:\n{found}"
    );

    stop_and_read(watch);
}

/// Ctrl-C while the queue is being worked. The sweep in flight is bounded by the limit, so
/// the watcher stops rather than hanging, and nothing is left held: every job it claimed is
/// either finished or back in the queue by the time the process is gone. A claim a killed
/// worker leaves behind is the lease's business, and that is checked against the database in
/// `infrastructure::insight_run_tests`.
#[test]
fn ctrl_c_during_a_sweep_stops_the_watcher_and_leaves_no_job_held() {
    let sandbox = Sandbox::new("watch-analysis-interrupt");
    sandbox.write_config("[watch]\nanalyze = true\nanalyze_limit = 40\n\n");
    write_notes(&sandbox, 39);
    sandbox.reconcile();
    assert_eq!(job_count(&sandbox, "queued"), 40);

    let mut watch = WatchProcess::spawn(&sandbox);
    // The sweep starts the moment this line is printed, so the signal lands inside it.
    watch.wait_for_line("Pass 1 (bootstrap) completed");
    let stdout = stop_and_read(watch);

    assert_counters(&stdout, &["Watch stopped.", "Passes failed: 0"]);
    assert_eq!(
        job_count(&sandbox, "claimed"),
        0,
        "no job may be left held by a worker that is gone:\n{stdout}"
    );
    assert_eq!(
        job_count(&sandbox, "done") + job_count(&sandbox, "queued"),
        40
    );
}
