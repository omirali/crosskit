// The benchmark itself: four measured phases over a deterministic corpus, then two
// recovery scenarios. Unix-only, because it drives the descriptor-relative walker and
// needs POSIX permissions to make a directory unreadable mid-pass; the platform gate lives
// in the test file that calls `run`.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::thread;
use std::time::{Duration, Instant};

use super::cli::{
    Bench, GraphCounts, PhaseResult, ScanCounters, allocated_peak_label, megabytes, peak_rss_label,
};
use super::corpus::{Corpus, Expected, MAX_CONTENT_MB, Profile, TextFile};
use super::measure::Measurement;
use super::search::{assert_search_is_in_step, measure_search};

/// Runs the whole benchmark: four measured phases and two recovery scenarios.
pub fn run() {
    let profile = Profile::from_env();
    let bench = Bench::create(&profile.name());
    println!("benchmark directory: {}", bench.directory.display());

    let started = Instant::now();
    let corpus = Corpus::generate(&bench.directory, profile);
    bench.write_config(&corpus.root, &corpus.metadata_directory, MAX_CONTENT_MB);
    println!(
        "profile {}: {} regular files, {} in corpus, generated in {:.2?}, hard link {}",
        profile.name(),
        corpus.regular_files,
        megabytes(corpus.total_bytes),
        started.elapsed(),
        if corpus.hard_link { "yes" } else { "no" }
    );
    let root = corpus.root.to_string_lossy().into_owned();
    let mut results = Vec::new();

    // ---- A. cold init ----
    let initial = corpus.expected_initial();
    let phase_a = bench.measured("A cold index init", &["index", "init", &root]);
    assert_completed(&phase_a);
    let counters = ScanCounters::parse(&phase_a.stdout);
    assert_scan(&counters, &initial, None);
    let graph = bench.graph();
    assert_graph(&graph, &initial, 0);
    assert_eq!(bench.queued_jobs(), initial.file_versions);
    assert!(
        !GraphCounts::chunk_text_contains(&bench.database_path(), corpus.symlink_sentinel),
        "the symlink target must never be read"
    );
    results.push(phase(&bench, &phase_a, &counters));
    // The search index is written by the same command that wrote the graph, so it is in
    // step the moment the cold pass ends.
    assert_search_is_in_step(&bench, initial.chunks_stored);
    measure_search(&bench, &corpus);

    // ---- B. idle reconcile ----
    let phase_b = bench.measured("B idle reconcile", &["index", "reconcile", &root]);
    assert_completed(&phase_b);
    let counters = ScanCounters::parse(&phase_b.stdout);
    assert_scan(&counters, &initial, Some((0, 0)));
    let after_idle = bench.graph();
    assert_eq!(
        after_idle.evidence(),
        graph.evidence(),
        "an idle reconcile must change no evidence"
    );
    assert_eq!(after_idle.runs_completed, graph.runs_completed + 1);
    assert_eq!(bench.queued_jobs(), initial.file_versions);
    results.push(phase(&bench, &phase_b, &counters));
    let idle_duration = phase_b.duration;
    // What the cold pass migrated the database to. Every later check compares against this
    // rather than against a number written here.
    let migrated_version = bench.schema_version();

    // ---- C. controlled mutation ----
    let plan = corpus.plan_mutation();
    let mutation = corpus.apply_mutation(&plan);
    let files_after = mutation.files;
    let mutated = corpus.expected_after_mutation(&plan, &files_after);
    let expected_missing = plan.deleted.len() + plan.renamed.len() + plan.moved_under_exclude.len();
    // A renamed file keeps its identity at a new path, which is what relocation means, and
    // it is the only thing that does. How often the filesystem handed a freed inode number
    // to a newly created file is reported below as an observation about the filesystem: the
    // identity a file is published under no longer depends on it.
    let expected_relocated = plan.renamed.len();
    println!(
        "mutation: modified {}, deleted {}, renamed {}, moved under Exclude {}, created {}, deleted identities reused by creation {}",
        plan.modified.len(),
        plan.deleted.len(),
        plan.renamed.len(),
        plan.moved_under_exclude.len(),
        plan.created.len(),
        mutation.reused_identities
    );
    let phase_c = bench.measured("C reconcile after mutation", &["index", "reconcile", &root]);
    assert_completed(&phase_c);
    let counters = ScanCounters::parse(&phase_c.stdout);
    assert_scan(
        &counters,
        &mutated,
        Some((expected_missing, expected_relocated)),
    );
    let graph = bench.graph();
    assert_graph(&graph, &mutated, expected_missing);
    assert_eq!(bench.queued_jobs(), mutated.file_versions);
    results.push(phase(&bench, &phase_c, &counters));

    // ---- D. reopen, recovery and interruption ----
    // Against the version this run itself produced, not a literal. A literal rots on every
    // migration, and this one had: it still expected 9 at schema v12, and nothing noticed
    // because the benchmark is ignored by default. What matters here is that reopening
    // applies nothing further, and that is what this compares.
    let schema_version = bench.schema_version();
    assert_eq!(
        schema_version, migrated_version,
        "reopening must not migrate the database again"
    );
    let phase_d = bench.measured("D reconcile after reopen", &["index", "reconcile", &root]);
    assert_completed(&phase_d);
    let counters = ScanCounters::parse(&phase_d.stdout);
    assert_scan(&counters, &mutated, Some((0, 0)));
    let after_reopen = bench.graph();
    assert_eq!(
        after_reopen.evidence(),
        graph.evidence(),
        "a repeated reconcile must change no evidence"
    );
    assert_eq!(after_reopen.runs_completed, graph.runs_completed + 1);
    results.push(phase(&bench, &phase_d, &counters));
    let graph = after_reopen;

    // After a mutation and a reopen, the index still holds one document per chunk of a
    // current version — fewer than the graph stores, because a new revision replaces the
    // documents of the one it succeeded instead of adding to them. A retired location
    // changes nothing about what is indexed: its file keeps its last version.
    assert_search_is_in_step(&bench, corpus.expected_current_chunks(&plan));

    let interrupted = interrupted_pass_changes_nothing(&bench, &corpus, &root, graph, &files_after);
    results.extend(interrupted.measured);
    let graph = killed_pass_leaves_a_consistent_graph(
        &bench,
        &root,
        interrupted.graph,
        idle_duration,
        &files_after,
        migrated_version,
    );
    assert_eq!(bench.queued_jobs(), graph.file_versions);

    let report_path = write_report(&bench, &corpus, &results);
    println!(
        "benchmark directory kept for inspection: {}\nreport: {}",
        bench.directory.display(),
        report_path.display()
    );
}

/// A phase that had to finish. Under a hard memory limit the kernel ends a process with a
/// signal and leaves no exit code, so the reason is named here instead of appearing as an
/// absent number.
fn assert_completed(measurement: &Measurement) {
    assert_eq!(
        measurement.exit_code,
        Some(0),
        "{} did not complete: {}; {}",
        measurement.label,
        measurement.outcome(),
        measurement.program_stderr()
    );
}

fn assert_scan(counters: &ScanCounters, expected: &Expected, retirement: Option<(usize, usize)>) {
    assert_eq!(
        counters.indexed_files, expected.indexed_files,
        "{counters:?}"
    );
    assert_eq!(
        counters.indexed_chunks, expected.indexed_chunks,
        "{counters:?}"
    );
    assert_eq!(
        counters.metadata_only_files, expected.metadata_only_files,
        "{counters:?}"
    );
    assert_eq!(
        counters.content_skipped_files, expected.content_skipped_files,
        "{counters:?}"
    );
    assert_eq!(
        counters.excluded_paths, expected.excluded_paths,
        "{counters:?}"
    );
    assert_eq!(counters.unstable_sources, 0, "{counters:?}");
    assert_eq!(counters.unrepresentable_names, 0, "{counters:?}");
    assert_eq!(counters.failed_paths, 0, "{counters:?}");
    match retirement {
        Some((missing, relocated)) => {
            assert_eq!(counters.marked_missing, Some(missing), "{counters:?}");
            assert_eq!(counters.relocated, Some(relocated), "{counters:?}");
        }
        None => assert_eq!(counters.marked_missing, None, "{counters:?}"),
    }
}

fn assert_graph(graph: &GraphCounts, expected: &Expected, missing: usize) {
    graph.assert_consistent();
    assert_eq!(graph.file_versions, expected.file_versions, "{graph:?}");
    assert_eq!(graph.chunks, expected.chunks_stored, "{graph:?}");
    assert_eq!(graph.insight_jobs, expected.file_versions, "{graph:?}");
    assert_eq!(graph.queued_jobs, expected.file_versions, "{graph:?}");
    assert_eq!(graph.locations, expected.locations, "{graph:?}");
    assert_eq!(graph.missing, missing, "{graph:?}");
    assert_eq!(graph.runs_running, 0, "{graph:?}");
    assert_eq!(graph.runs_incomplete, 0, "{graph:?}");
}

/// What the interruption scenario leaves behind: the graph after recovery, and the measured
/// incomplete pass when the scenario ran. It is absent only when the scenario was skipped.
struct InterruptedPass {
    graph: GraphCounts,
    measured: Option<PhaseResult>,
}

/// A directory becomes unreadable in the middle of the walk while two files disappear: the
/// pass ends incomplete, exits with code 1 and leaves every status and every source as it
/// was. Once the directory is readable again a complete pass retires the two files.
fn interrupted_pass_changes_nothing(
    bench: &Bench,
    corpus: &Corpus,
    root: &str,
    before: GraphCounts,
    files: &[TextFile],
) -> InterruptedPass {
    if std::env::var_os("USER").is_some_and(|user| user == "root") {
        println!("interruption scenario skipped: root reads directories without permissions");
        return InterruptedPass {
            graph: before,
            measured: None,
        };
    }
    let locked = corpus
        .root
        .join(files[files.len() / 2].relative.parent().unwrap());
    // The hard-linked first note is not a victim: retiring one of its two paths while the
    // other stays present is a relocation, which this scenario does not test.
    let victims: Vec<_> = files
        .iter()
        .filter(|file| file.relative != corpus.text_files[0].relative)
        .filter(|file| {
            !file
                .relative
                .starts_with(locked.strip_prefix(&corpus.root).unwrap())
        })
        .take(2)
        .map(|file| corpus.root.join(&file.relative))
        .collect();
    for victim in &victims {
        fs::remove_file(victim).unwrap();
    }
    let readable = fs::metadata(&locked).unwrap().permissions();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let interrupted = bench.measured("interrupted pass", &["index", "reconcile", root]);
    fs::set_permissions(&locked, readable).unwrap();
    assert_eq!(
        interrupted.exit_code,
        Some(1),
        "an incomplete pass must fail the command: {}",
        interrupted.outcome()
    );
    let stdout = interrupted.stdout.clone();
    let duration = interrupted.duration;
    let counters = ScanCounters::parse(&stdout);
    assert_eq!(counters.failed_paths, 1, "{counters:?}");
    assert!(stdout.contains("Reconciliation incomplete"), "{stdout}");
    let after = bench.graph();
    assert_eq!(
        after.missing, before.missing,
        "an incomplete pass must not retire anything"
    );
    assert_eq!(after.file_versions, before.file_versions);
    assert_eq!(after.chunks, before.chunks);
    assert_eq!(after.insight_jobs, before.insight_jobs);
    assert_eq!(after.runs_incomplete, before.runs_incomplete + 1);
    after.assert_consistent();
    println!(
        "interrupted pass (unreadable directory): {:.2?}, {}{}, exit 1, {}, missing unchanged at {}",
        duration,
        peak_rss_label(&interrupted),
        allocated_peak_label(&interrupted),
        counters.describe(),
        after.missing
    );

    let measured = phase_result(bench, &interrupted, &counters);

    let complete = ScanCounters::parse(&bench.run_ok(&["index", "reconcile", root]));
    assert_eq!(complete.marked_missing, Some(victims.len()));
    assert_eq!(complete.relocated, Some(0));
    let recovered = bench.graph();
    assert_eq!(recovered.missing, before.missing + victims.len());
    recovered.assert_consistent();
    InterruptedPass {
        graph: recovered,
        measured: Some(measured),
    }
}

/// A pass is killed while it runs. Whatever it committed is whole (one transaction per
/// source), the run stays `running`, no status changed, and the next pass finishes the
/// work. The kill lands mid-pass only if the pass is slow enough; a pass that finished
/// first is reported and the scenario is skipped honestly.
#[allow(clippy::too_many_arguments)]
fn killed_pass_leaves_a_consistent_graph(
    bench: &Bench,
    root: &str,
    before: GraphCounts,
    idle_duration: Duration,
    files: &[TextFile],
    migrated_version: u64,
) -> GraphCounts {
    let victims: Vec<_> = files
        .iter()
        .rev()
        .filter(|file| file.relative.as_os_str() != "docs/d000/s00/note-000000.md")
        .take(3)
        .map(|file| file.relative.clone())
        .collect();
    let corpus_root = std::path::Path::new(root);
    for victim in &victims {
        if let Err(error) = fs::remove_file(corpus_root.join(victim)) {
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{victim:?}");
        }
    }
    let delay = idle_duration
        .mul_f32(0.3)
        .clamp(Duration::from_millis(100), Duration::from_secs(5));
    let mut child = bench
        .command(&["index", "reconcile", root])
        .spawn()
        .unwrap();
    thread::sleep(delay);
    if child.try_wait().unwrap().is_some() {
        println!("kill scenario skipped: the pass finished within {delay:.2?}");
        let after = bench.graph();
        after.assert_consistent();
        return after;
    }
    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(
        bench.schema_version(),
        migrated_version,
        "the database must reopen after the kill, at the version it already had"
    );
    let after = bench.graph();
    after.assert_consistent();
    assert_eq!(
        after.missing, before.missing,
        "a killed pass must not retire anything"
    );
    assert_eq!(after.runs_running, before.runs_running + 1);
    assert!(after.file_versions >= before.file_versions);
    println!(
        "killed pass after {delay:.2?}: reopened, {} run(s) left running, missing unchanged at {}, versions {} -> {}",
        after.runs_running, after.missing, before.file_versions, after.file_versions
    );

    let complete = ScanCounters::parse(&bench.run_ok(&["index", "reconcile", root]));
    assert_eq!(complete.marked_missing, Some(victims.len()));
    let recovered = bench.graph();
    recovered.assert_consistent();
    assert_eq!(recovered.missing, before.missing + victims.len());
    assert_eq!(recovered.runs_running, before.runs_running + 1);
    let prune = bench.run_ok(&["index", "runs", "prune", "--keep", "0"]);
    assert!(
        prune.contains(&format!(
            "Protected running runs: {}",
            recovered.runs_running
        )),
        "{prune}"
    );
    recovered
}

/// The row the report keeps for one measured command. Every command that is measured
/// builds it, including the recovery pass, so the report and stdout never disagree about
/// which commands were measured.
fn phase_result(bench: &Bench, measurement: &Measurement, counters: &ScanCounters) -> PhaseResult {
    PhaseResult {
        name: measurement.label.clone(),
        duration: measurement.duration,
        peak_rss_bytes: measurement.peak_rss_bytes,
        allocation_peak_bytes: measurement.allocation_peak_bytes,
        exit_code: measurement.exit_code,
        database_bytes: bench.database_bytes(),
        counters: counters.describe(),
        queued_jobs: bench.queued_jobs(),
    }
}

fn phase(bench: &Bench, measurement: &Measurement, counters: &ScanCounters) -> PhaseResult {
    let result = phase_result(bench, measurement, counters);
    println!(
        "{}: {:.2?}, {}{}, database {}, {}, queued insight jobs {}",
        result.name,
        result.duration,
        peak_rss_label(measurement),
        allocated_peak_label(measurement),
        megabytes(result.database_bytes),
        result.counters,
        result.queued_jobs
    );
    result
}

/// Writes the machine-readable summary next to the corpus and returns its path.
fn write_report(bench: &Bench, corpus: &Corpus, results: &[PhaseResult]) -> std::path::PathBuf {
    let mut report = format!(
        "profile: {}\nplatform: {} {}\nregular files: {}\ncorpus bytes: {}\nhard link: {}\nmeasured: every phase below, each in its own process; the killed pass is terminated on purpose and carries no timing\n\n",
        corpus.profile.name(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        corpus.regular_files,
        corpus.total_bytes,
        corpus.hard_link
    );
    for result in results {
        report.push_str(&format!(
            "{}\n  duration: {:.3?}\n  peak rss bytes: {}\n  allocation peak bytes: {}\n  exit code: {}\n  database bytes: {}\n  counters: {}\n  queued insight jobs: {}\n",
            result.name,
            result.duration,
            result.peak_rss_bytes,
            result
                .allocation_peak_bytes
                .map_or_else(|| "not counted in this build".to_string(), |bytes| bytes.to_string()),
            result
                .exit_code
                .map_or_else(|| "terminated by signal".to_string(), |code| code.to_string()),
            result.database_bytes,
            result.counters,
            result.queued_jobs
        ));
    }
    let path = bench.directory.join("report.txt");
    fs::write(&path, report).unwrap();
    path
}
