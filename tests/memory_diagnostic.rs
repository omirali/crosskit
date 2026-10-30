// Why peak resident memory grew when the batch became set-based, measured rather than
// guessed. Ignored by default; run explicitly with
//
//   cargo test --release --offline --test memory_diagnostic -- --ignored --nocapture
//
// The corpus is synthetic and deterministic, the database lives in its own
// CROSSKIT_DATA_DIR, and everything measured runs in its own process so the benchmark's
// own memory is never mistaken for the indexer's. Peak resident set size comes from
// `/usr/bin/time -l`, which macOS provides; the benchmark is therefore macOS-only and
// says so rather than reporting a number it did not measure.
//
// The benchmark asserts correctness invariants and prints measurements. It asserts no
// timing or memory threshold: those belong to the report, which a person reads.
#[cfg(target_os = "macos")]
mod memory_diagnostic {
    pub mod corpus;
    pub mod current_state;
    pub mod graph;
    pub mod measure;
    pub mod worker;

    // The store's own static statements, measured as production writes them. Including
    // the file keeps a measurement from drifting from the text it claims to measure. The
    // benchmark measures the batch path only, so the statements of the sequential fallback
    // are unused here; the binary crate still holds every one of them to its use.
    #[allow(dead_code)]
    #[path = "../../src/infrastructure/ladybug/current_state_statements.rs"]
    pub mod statements;
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "memory diagnosis: cargo test --release --offline --test memory_diagnostic -- --ignored --nocapture"]
fn peak_memory_growth_is_attributed() {
    diagnostic::run();
}

/// The worker half of the benchmark: the parent runs this same binary again, under the
/// operating system's measurement, so each read workload gets a process of its own.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "internal worker of the memory diagnosis; the benchmark runs it"]
fn memory_diagnostic_worker() {
    let Ok(database) = std::env::var("CROSSKIT_DIAGNOSTIC_DB") else {
        // `--ignored` also selects this internal test at the top level. Only a child
        // explicitly configured by the diagnostic is a worker process.
        return;
    };
    let pool = std::env::var("CROSSKIT_DIAGNOSTIC_POOL")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let rounds = std::env::var("CROSSKIT_DIAGNOSTIC_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);
    let plans = std::env::var("CROSSKIT_DIAGNOSTIC_PLANS").is_ok();
    let database = std::path::Path::new(&database);
    match std::env::var("CROSSKIT_DIAGNOSTIC_ROLE").as_deref() {
        Ok("write") => memory_diagnostic::worker::write_probe(
            database,
            pool,
            rounds,
            memory_diagnostic::worker::Links::from_env(
                std::env::var("CROSSKIT_DIAGNOSTIC_LINKS").ok().as_deref(),
            ),
        ),
        Ok("link-plans") => memory_diagnostic::worker::link_plans(database),
        Ok("current-state") => memory_diagnostic::current_state::run_worker(
            database,
            std::env::var("CROSSKIT_DIAGNOSTIC_CURRENT_STATE_WORKLOAD")
                .as_deref()
                .unwrap_or_else(|_| {
                    panic!("the current-state diagnostic worker requires its workload")
                }),
        ),
        _ => memory_diagnostic::worker::run(database, pool, rounds, plans),
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
#[ignore = "memory diagnosis: macOS only"]
fn peak_memory_growth_is_attributed() {
    panic!(
        "the memory diagnosis reads peak resident set size from `/usr/bin/time -l`, which only macOS provides here; no measurement is reported on this platform"
    );
}

#[cfg(target_os = "macos")]
mod diagnostic {
    /// The context this diagnostic works in; every command names it, like any other run.
    const DIAGNOSTIC_CONTEXT: &str = "diagnostic";

    /// What the CLI calls the graph of that context.
    const CLI_DATABASE_FILE: &str = "graph-diagnostic.lbug";

    fn cli_database_path(data: &Path) -> PathBuf {
        data.join(CLI_DATABASE_FILE)
    }

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::memory_diagnostic::corpus::{self, Corpus};
    use super::memory_diagnostic::graph::{self, History};
    use super::memory_diagnostic::measure::{Measurement, counter, measure};

    /// One row of the report.
    struct Row {
        step: String,
        history: History,
        database_bytes: u64,
        measurement: Measurement,
        schema_version: Option<usize>,
        indexed_files: Option<usize>,
    }

    /// Three independently measured executions of one isolated current-state workload.
    struct CurrentStateSeries {
        label: &'static str,
        samples: Vec<Measurement>,
    }

    pub fn run() {
        let bench = create_directory();
        println!("benchmark directory created");
        let corpus = Corpus::generate(&bench);
        let data = bench.join("data");
        fs::create_dir(&data).unwrap();
        let config = data.join("config.toml");
        // The diagnostic drives the real CLI, so it declares a context like any operator
        // would; the corpus root is already canonical.
        fs::write(
            &config,
            format!(
                "[index]\nmax_content_size_mb = 1\n\n[workspace.{DIAGNOSTIC_CONTEXT}]\ninclude = [\"{}\"]\n",
                corpus.root.display()
            ),
        )
        .unwrap();
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_crosskit"));
        let root = corpus.root.to_string_lossy().into_owned();
        println!(
            "corpus: {} files, {} bytes; history passes {}, measured every {}",
            corpus.files,
            corpus.bytes,
            corpus::history_passes(),
            corpus::checkpoint_every()
        );

        let mut rows = Vec::new();
        let run_cli = |label: &str, arguments: &[&str]| {
            let mut named = vec!["--context", DIAGNOSTIC_CONTEXT];
            named.extend_from_slice(arguments);
            measure(
                label,
                &binary,
                &named,
                &[
                    ("CROSSKIT_DATA_DIR", data.as_path()),
                    ("CROSSKIT_CONFIG", config.as_path()),
                ],
            )
        };

        // ---- a database that does not exist yet ----
        rows.push(row(
            "fresh: open (doctor)",
            run_cli("doctor", &["doctor"]),
            &data,
        ));
        rows.push(row(
            "fresh: index init",
            run_cli("init", &["index", "init", &root]),
            &data,
        ));
        rows.push(row(
            "fresh: idle reconcile",
            run_cli("reconcile", &["index", "reconcile", &root]),
            &data,
        ));

        // ---- the same database, with the history growing between measurements ----
        let passes = corpus::history_passes();
        let every = corpus::checkpoint_every().max(1);
        let mut done = 0;
        while done < passes {
            let until = (done + every).min(passes);
            for _ in done..until {
                let output = run_cli("history", &["index", "reconcile", &root]);
                assert_eq!(output.exit_code, Some(0), "{}", output.label);
            }
            done = until;
            rows.push(row(
                &format!("history {done} passes: idle reconcile"),
                run_cli("reconcile", &["index", "reconcile", &root]),
                &data,
            ));
        }

        let database_path = cli_database_path(&data);
        rows.push(row(
            "aged: open (doctor)",
            run_cli("doctor", &["doctor"]),
            &data,
        ));
        rows.push(row(
            "aged: idle reconcile again",
            run_cli("reconcile", &["index", "reconcile", &root]),
            &data,
        ));

        // ---- the same read workload, once with the default buffer pool and once capped ----
        let worker = std::env::current_exe().unwrap();
        let mut read_only = Vec::new();
        for (label, pool) in [
            ("aged: read workload, default pool", 0_u64),
            ("aged: read workload, 128 MiB pool", 128 * 1024 * 1024),
            ("aged: read workload, 512 MiB pool", 512 * 1024 * 1024),
        ] {
            let pool_text = pool.to_string();
            let measurement = measure_worker(&worker, &database_path, &pool_text, false);
            println!(
                "{label}: {:.2?}, peak RSS {:.0} MiB, exit {:?}",
                measurement.duration,
                measurement.peak_rss_mib(),
                measurement.exit_code
            );
            read_only.push((label, measurement));
        }

        // ---- the writes a pass makes, on copies of the database: with two buffer pool
        // sizes, and with the relationship links left out, which is what separates the
        // two explanations for the growth ----
        for (label, pool, links) in [
            (
                "aged copy: write workload without links, default pool",
                0_u64,
                "none",
            ),
            (
                "aged copy: links as written today, default pool",
                0_u64,
                "create",
            ),
            (
                "aged copy: links as written today, 128 MiB pool",
                128 * 1024 * 1024,
                "create",
            ),
            (
                "aged copy: links as they were written before, default pool",
                0_u64,
                "merge",
            ),
        ] {
            let copy = bench.join(format!("write-copy-{pool}-{links}"));
            fs::create_dir(&copy).unwrap();
            fs::copy(&database_path, copy.join("graph.lbug")).unwrap();
            let measurement =
                measure_write(&worker, &copy.join("graph.lbug"), &pool.to_string(), links);
            println!(
                "{label}: {:.2?}, peak RSS {:.0} MiB, {}",
                measurement.duration,
                measurement.peak_rss_mib(),
                outcome(&measurement)
            );
            read_only.push((label, measurement));
        }

        // ---- the plans the engine chose for those queries ----
        let plans = measure_worker(&worker, &database_path, "0", true);
        fs::write(bench.join("plans.txt"), &plans.stdout).unwrap();
        let link_plans = measure_link_plans(&worker, &database_path);
        fs::write(bench.join("link-plans.txt"), &link_plans.stdout).unwrap();
        println!("query plans recorded with the benchmark artifacts");

        let current_state = measure_current_state_workloads(&bench, &worker);

        graph::assert_invariants(&database_path);
        write_report(
            &bench,
            &corpus,
            &rows,
            &read_only,
            &plans.stdout,
            &current_state,
        );
        print_table(&rows, &read_only, &current_state);
        println!("benchmark artifacts retained for inspection");
    }

    /// What a measured process did, without repeating anything the engine said: a raw
    /// storage error is never shown to a reader of this report.
    fn outcome(measurement: &Measurement) -> String {
        match measurement.exit_code {
            Some(0) => "completed".to_string(),
            _ => "did not complete: the engine could not run the statement within this buffer pool"
                .to_string(),
        }
    }

    fn measure_write(worker: &Path, database_path: &Path, pool: &str, links: &str) -> Measurement {
        measure_test_worker(
            format!("write probe pool {pool}"),
            worker,
            database_path,
            |command| {
                command
                    .env("CROSSKIT_DIAGNOSTIC_POOL", pool)
                    .env("CROSSKIT_DIAGNOSTIC_ROLE", "write")
                    .env("CROSSKIT_DIAGNOSTIC_ROUNDS", "20")
                    .env("CROSSKIT_DIAGNOSTIC_LINKS", links);
            },
        )
    }

    fn measure_link_plans(worker: &Path, database_path: &Path) -> Measurement {
        let output = std::process::Command::new(worker)
            .arg("--ignored")
            .arg("--exact")
            .arg("--nocapture")
            .arg("memory_diagnostic_worker")
            .env("CROSSKIT_DIAGNOSTIC_DB", database_path)
            .env("CROSSKIT_DIAGNOSTIC_ROLE", "link-plans")
            .output()
            .unwrap();
        Measurement {
            label: "link plans".to_string(),
            duration: std::time::Duration::ZERO,
            peak_rss_bytes: 0,
            // The worker is this test binary, which never carries the counting allocator:
            // only the crosskit binary declares it.
            allocation_peak_bytes: None,
            exit_code: output.status.code(),
            signal: None,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn measure_worker(worker: &Path, database_path: &Path, pool: &str, plans: bool) -> Measurement {
        measure_test_worker(
            format!("worker pool {pool}"),
            worker,
            database_path,
            |command| {
                command.env("CROSSKIT_DIAGNOSTIC_POOL", pool);
                if plans {
                    command.env("CROSSKIT_DIAGNOSTIC_PLANS", "1");
                }
            },
        )
    }

    /// The common process boundary for every diagnostic worker. A workload configures only
    /// its role and inputs; timing and RSS always come from the same measurement.
    fn measure_test_worker(
        label: String,
        worker: &Path,
        database_path: &Path,
        configure: impl FnOnce(&mut Command),
    ) -> Measurement {
        let mut command = Command::new(worker);
        command
            .arg("--ignored")
            .arg("--exact")
            .arg("--nocapture")
            .arg("memory_diagnostic_worker")
            .env("CROSSKIT_DIAGNOSTIC_DB", database_path);
        configure(&mut command);
        super::memory_diagnostic::measure::measure_command(&label, command)
    }

    /// Creates two production-sized current-state fixture bases, then measures every
    /// workload three times on a private copy.
    fn measure_current_state_workloads(bench: &Path, worker: &Path) -> Vec<CurrentStateSeries> {
        const REPEATS: usize = 3;
        let fresh = prepare_current_state_copy(
            bench,
            "fresh",
            super::memory_diagnostic::current_state::Fixture::Fresh,
        );
        let reconcile = prepare_current_state_copy(
            bench,
            "reconcile",
            super::memory_diagnostic::current_state::Fixture::Reconcile,
        );
        let mut results = Vec::new();
        for workload in super::memory_diagnostic::current_state::WORKLOADS {
            let fixture = match workload.fixture() {
                super::memory_diagnostic::current_state::Fixture::Fresh => &fresh,
                super::memory_diagnostic::current_state::Fixture::Reconcile => &reconcile,
            };
            let mut samples = Vec::with_capacity(REPEATS);
            for sample in 1..=REPEATS {
                let copy = bench.join(format!("current-state-{}-{sample}", workload.environment()));
                fs::create_dir(&copy).unwrap();
                let database = copy.join("graph.lbug");
                fs::copy(fixture, &database).unwrap();
                let measurement = measure_test_worker(
                    workload.label().to_string(),
                    worker,
                    &database,
                    |command| {
                        command
                            .env("CROSSKIT_DIAGNOSTIC_ROLE", "current-state")
                            .env(
                                "CROSSKIT_DIAGNOSTIC_CURRENT_STATE_WORKLOAD",
                                workload.environment(),
                            );
                    },
                );
                assert_eq!(
                    measurement.exit_code,
                    Some(0),
                    "current-state worker {} sample {sample} must complete",
                    workload.label()
                );
                samples.push(measurement);
            }
            results.push(CurrentStateSeries {
                label: workload.label(),
                samples,
            });
        }
        results
    }

    fn prepare_current_state_copy(
        bench: &Path,
        name: &str,
        fixture: super::memory_diagnostic::current_state::Fixture,
    ) -> PathBuf {
        let directory = bench.join(format!("current-state-{name}-base"));
        fs::create_dir(&directory).unwrap();
        let database = directory.join("graph.lbug");
        super::memory_diagnostic::current_state::prepare_fixture(&database, fixture);
        database
    }

    fn row(step: &str, measurement: Measurement, data: &Path) -> Row {
        let database_path = cli_database_path(data);
        let history = if database_path.exists() {
            graph::history(&database_path)
        } else {
            History {
                file_versions: 0,
                observations: 0,
                observation_orders: 0,
                locations: 0,
                insight_jobs: 0,
            }
        };
        Row {
            step: step.to_string(),
            history,
            database_bytes: database_bytes(data),
            schema_version: counter(&measurement.stdout, "Schema version:"),
            indexed_files: counter(&measurement.stdout, "Indexed files (metadata and content):"),
            measurement,
        }
    }

    fn database_bytes(data: &Path) -> u64 {
        fs::read_dir(data)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(CLI_DATABASE_FILE)
                    })
                    .filter_map(|entry| entry.metadata().ok())
                    .map(|metadata| metadata.len())
                    .sum()
            })
            .unwrap_or(0)
    }

    fn create_directory() -> PathBuf {
        let directory = match std::env::var_os("CROSSKIT_DIAGNOSTIC_DIR") {
            Some(path) => PathBuf::from(path),
            None => std::env::temp_dir()
                .join(format!("crosskit-memory-diagnostic-{}", std::process::id())),
        };
        fs::create_dir(&directory).unwrap_or_else(|error| {
            panic!(
                "benchmark directory {} must be new: {error}",
                directory.display()
            )
        });
        directory
    }

    fn print_table(
        rows: &[Row],
        read_only: &[(&str, Measurement)],
        current_state: &[CurrentStateSeries],
    ) {
        println!();
        println!("step | versions | observations | orders | db MiB | wall s | peak RSS MiB | exit");
        for row in rows {
            println!(
                "{} | {} | {} | {} | {:.1} | {:.1} | {:.0} | {:?}",
                row.step,
                row.history.file_versions,
                row.history.observations,
                row.history.observation_orders,
                row.database_bytes as f64 / (1024.0 * 1024.0),
                row.measurement.duration.as_secs_f64(),
                row.measurement.peak_rss_mib(),
                row.measurement.exit_code
            );
        }
        for (label, measurement) in read_only {
            println!(
                "{label} | - | - | - | - | {:.1} | {:.0} | {:?}",
                measurement.duration.as_secs_f64(),
                measurement.peak_rss_mib(),
                measurement.exit_code
            );
        }
        println!();
        println!(
            "current-state isolated workload | wall s min/median/max | peak RSS MiB min/median/max"
        );
        for series in current_state {
            let summary = current_state_summary(&series.samples);
            println!(
                "{} | {:.1}/{:.1}/{:.1} | {:.0}/{:.0}/{:.0}",
                series.label,
                summary.wall_min.as_secs_f64(),
                summary.wall_median.as_secs_f64(),
                summary.wall_max.as_secs_f64(),
                summary.rss_min as f64 / (1024.0 * 1024.0),
                summary.rss_median as f64 / (1024.0 * 1024.0),
                summary.rss_max as f64 / (1024.0 * 1024.0),
            );
        }
    }

    struct CurrentStateSummary {
        wall_min: std::time::Duration,
        wall_median: std::time::Duration,
        wall_max: std::time::Duration,
        rss_min: u64,
        rss_median: u64,
        rss_max: u64,
    }

    fn current_state_summary(samples: &[Measurement]) -> CurrentStateSummary {
        assert_eq!(
            samples.len(),
            3,
            "each current-state workload has three samples"
        );
        let mut wall: Vec<_> = samples.iter().map(|sample| sample.duration).collect();
        wall.sort_unstable();
        let mut rss: Vec<_> = samples.iter().map(|sample| sample.peak_rss_bytes).collect();
        rss.sort_unstable();
        CurrentStateSummary {
            wall_min: wall[0],
            wall_median: wall[wall.len() / 2],
            wall_max: wall[wall.len() - 1],
            rss_min: rss[0],
            rss_median: rss[rss.len() / 2],
            rss_max: rss[rss.len() - 1],
        }
    }

    fn write_report(
        bench: &Path,
        corpus: &Corpus,
        rows: &[Row],
        read_only: &[(&str, Measurement)],
        worker_output: &str,
        current_state: &[CurrentStateSeries],
    ) -> PathBuf {
        let mut report = format!(
            "platform: {} {}\ncorpus files: {}\ncorpus bytes: {}\nhistory passes: {}\nmeasured every: {}\n\n",
            std::env::consts::OS,
            std::env::consts::ARCH,
            corpus.files,
            corpus.bytes,
            corpus::history_passes(),
            corpus::checkpoint_every()
        );
        for row in rows {
            report.push_str(&format!(
                "{}\n  file versions: {}\n  observations: {}\n  observation orders: {}\n  locations: {}\n  insight jobs: {}\n  database bytes: {}\n  wall clock: {:.3?}\n  peak rss bytes: {}\n  exit code: {:?}\n  schema version: {:?}\n  indexed files: {:?}\n",
                row.step,
                row.history.file_versions,
                row.history.observations,
                row.history.observation_orders,
                row.history.locations,
                row.history.insight_jobs,
                row.database_bytes,
                row.measurement.duration,
                row.measurement.peak_rss_bytes,
                row.measurement.exit_code,
                row.schema_version,
                row.indexed_files,
            ));
        }
        for (label, measurement) in read_only {
            report.push_str(&format!(
                "{label}\n  wall clock: {:.3?}\n  peak rss bytes: {}\n  outcome: {}\n",
                measurement.duration,
                measurement.peak_rss_bytes,
                outcome(measurement)
            ));
        }
        report.push_str("\nquery timings reported by the engine, default pool:\n");
        for line in worker_output
            .lines()
            .filter(|line| line.starts_with("query|"))
        {
            report.push_str(&format!("  {line}\n"));
        }
        report.push_str("\ncurrent-state isolated workloads; 10 407 source identities, each on a copied fixture, three samples:\n");
        report.push_str("workload | wall seconds min/median/max | peak RSS bytes min/median/max\n");
        for series in current_state {
            let summary = current_state_summary(&series.samples);
            report.push_str(&format!(
                "{} | {:.3}/{:.3}/{:.3} | {}/{}/{}\n",
                series.label,
                summary.wall_min.as_secs_f64(),
                summary.wall_median.as_secs_f64(),
                summary.wall_max.as_secs_f64(),
                summary.rss_min,
                summary.rss_median,
                summary.rss_max,
            ));
        }
        let path = bench.join("report.txt");
        fs::write(&path, report).unwrap();
        path
    }
}
