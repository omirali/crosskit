// What the engine's own memory and thread policy costs, measured on the scale corpus.
//
// Ignored by default. The binary under test must be built with the configuration under
// test, and this benchmark reads back what that binary actually compiled in rather than
// trusting the request, so a table of runs cannot be mislabelled:
//
//   CROSSKIT_BUFFER_POOL_MIB=512 CROSSKIT_MAX_THREADS=2 \
//     cargo test --release --offline --features engine-tuning-benchmark \
//     --test engine_tuning -- --ignored --nocapture
//
// Either variable may be left out, which leaves that half to the engine and is what a
// released Crosskit does. The values are a compile-time property of the binary; nothing
// about the engine is read from the environment of a running Crosskit.
//
// Three phases are measured, because Stage 2L found the peak moving between the first
// indexing pass and the pass after a mutation: cold init, idle reconcile, and reconcile
// after the same deterministic mutation the scale benchmark applies. A refusal by the
// engine to execute a statement is a result of this benchmark, not a failure of it: the
// phase is reported with the message the engine gave, and the remaining phases are
// skipped because they would measure a graph that was never written.
//
// Peak resident set size is read from `/usr/bin/time -l` around the already built binary,
// never from the process measuring itself, which is why this is macOS-only and says so
// instead of reporting a number it did not measure. The corpus, the counters and the
// measurement are the scale benchmark's own, so the benchmarks cannot drift apart.
#[cfg(target_os = "macos")]
#[path = "scale_indexing/cli.rs"]
mod cli;
#[cfg(target_os = "macos")]
#[path = "scale_indexing/corpus.rs"]
mod corpus;
#[cfg(target_os = "macos")]
#[path = "memory_diagnostic/measure.rs"]
mod measure;

#[cfg(target_os = "macos")]
#[test]
#[ignore = "engine tuning benchmark: cargo test --release --offline --features engine-tuning-benchmark --test engine_tuning -- --ignored --nocapture"]
fn the_cost_of_the_engine_configuration_is_measured() {
    benchmark::run();
}

/// The worst input the index policy allows: one file at the compiled content ceiling,
/// which the batch writer publishes in a single transaction. Stage 2P found that a fixed
/// buffer pool stops being a memory setting and starts being an undeclared limit on file
/// size, which is why production leaves the pool to the engine; this holds that a binary
/// which does so indexes its worst allowed input.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "engine tuning boundary: cargo test --release --offline --features engine-tuning-benchmark --test engine_tuning -- --ignored --nocapture"]
fn the_largest_allowed_file_is_indexed_unless_the_buffer_pool_is_fixed() {
    benchmark::largest_allowed_file();
}

#[cfg(not(target_os = "macos"))]
#[test]
#[ignore = "engine tuning benchmark: macOS only"]
fn the_cost_of_the_engine_configuration_is_measured() {
    panic!(
        "this benchmark reads peak resident set size from `/usr/bin/time -l`, which only macOS provides here; no measurement is reported on this platform"
    );
}

#[cfg(target_os = "macos")]
mod benchmark {
    use std::path::Path;

    use super::cli::{Bench, ScanCounters, megabytes};
    use super::corpus::{CONTENT_CEILING_MIB, Corpus, MAX_CONTENT_MB, Profile};
    use super::measure::Measurement;

    /// The engine configuration the binary under test compiled in, as it reports it.
    struct Tuning {
        buffer_pool: String,
        max_threads: String,
    }

    impl Tuning {
        /// Reads the configuration out of the binary and checks it against what the
        /// operator asked for. A binary built without the benchmark feature, or built
        /// before the request changed, is refused rather than measured under the wrong
        /// label.
        fn of(bench: &Bench) -> Self {
            let reported = bench.run_ok(&["doctor"]);
            let line = reported
                .lines()
                .find_map(|line| line.strip_prefix("Engine tuning: "))
                .unwrap_or_else(|| {
                    panic!(
                        "the binary does not report its engine configuration; build it with --features engine-tuning-benchmark"
                    )
                });
            let (pool, threads) = line
                .split_once(", max threads ")
                .expect("the reported configuration must name the pool and the thread count");
            let tuning = Self {
                buffer_pool: pool
                    .strip_prefix("buffer pool ")
                    .expect("the reported configuration must name the buffer pool")
                    .to_string(),
                max_threads: threads.to_string(),
            };
            tuning.agrees_with_request();
            tuning
        }

        /// Absence of a variable means the binary does what a released one does, and it
        /// then reports `auto` — on its own where the machine is the limit, or with the
        /// ceiling it derived from a stricter one. Any other disagreement means the binary
        /// predates the request.
        fn agrees_with_request(&self) {
            match std::env::var("CROSSKIT_BUFFER_POOL_MIB") {
                Ok(mib) => assert_eq!(
                    self.buffer_pool,
                    format!("{mib} MiB"),
                    "the binary was built with a different buffer pool than this run asked for"
                ),
                Err(_) => assert!(
                    self.buffer_pool.starts_with("auto"),
                    "no pool was asked for, so the binary must size it the way a released one does, not compile one in: {}",
                    self.buffer_pool
                ),
            }
            let requested_threads =
                std::env::var("CROSSKIT_MAX_THREADS").unwrap_or_else(|_| "auto".to_string());
            assert_eq!(
                self.max_threads, requested_threads,
                "the binary was built with a different thread count than this run asked for"
            );
        }

        fn label(&self) -> String {
            format!("pool {} | threads {}", self.buffer_pool, self.max_threads)
        }
    }

    /// One measured phase, with everything the report needs to stand on its own. The
    /// counters and the job count are absent when the phase did not finish: they are read
    /// from what the run printed and from the graph it wrote, and a refused run has
    /// neither. Asking for them anyway would turn a recorded refusal into a crashed
    /// benchmark, which is exactly what this measurement must not do.
    struct Phase {
        measurement: Measurement,
        counters: Option<ScanCounters>,
        queued_jobs: Option<usize>,
        database_bytes: u64,
    }

    impl Phase {
        fn completed(&self) -> bool {
            self.measurement.exit_code == Some(0)
        }

        /// `ok`, or what the engine said when it refused. The message carries the
        /// statement context the store names, which is what a refusal has to be recorded
        /// with to be useful.
        fn outcome(&self) -> String {
            if self.completed() {
                return "ok".to_string();
            }
            format!(
                "REFUSED {}: {}",
                self.measurement.outcome(),
                self.measurement.program_stderr()
            )
        }
    }

    pub fn run() {
        let bench = Bench::create("engine-tuning");
        println!("benchmark directory: {}", bench.directory.display());
        let tuning = Tuning::of(&bench);
        let corpus = Corpus::generate(&bench.directory, Profile::Scale);
        bench.write_config(&corpus.root, &corpus.metadata_directory, MAX_CONTENT_MB);
        let root = corpus.root.to_string_lossy().into_owned();
        println!(
            "configuration the binary compiled in: {}; corpus {} files, {}",
            tuning.label(),
            corpus.regular_files,
            megabytes(corpus.total_bytes)
        );

        let mut phases = Vec::new();
        phases.push(phase(
            &bench,
            "A cold index init",
            &["index", "init", &root],
        ));
        if phases[0].completed() {
            phases.push(phase(
                &bench,
                "B idle reconcile",
                &["index", "reconcile", &root],
            ));
        }
        if phases.last().is_some_and(Phase::completed) {
            let plan = corpus.plan_mutation();
            corpus.apply_mutation(&plan);
            println!(
                "mutation: modified {}, deleted {}, renamed {}, moved under Exclude {}, created {}",
                plan.modified.len(),
                plan.deleted.len(),
                plan.renamed.len(),
                plan.moved_under_exclude.len(),
                plan.created.len()
            );
            phases.push(phase(
                &bench,
                "C reconcile after mutation",
                &["index", "reconcile", &root],
            ));
        }

        report(&tuning, &phases);
        println!(
            "benchmark directory kept for inspection: {}",
            bench.directory.display()
        );
    }

    fn report(tuning: &Tuning, phases: &[Phase]) {
        println!();
        println!(
            "configuration | phase | wall s | peak RSS MiB | db MiB | jobs | outcome | counters"
        );
        for phase in phases {
            println!(
                "{} | {} | {:.1} | {:.0} | {:.1} | {} | {} | {}",
                tuning.label(),
                phase.measurement.label,
                phase.measurement.duration.as_secs_f64(),
                phase.measurement.peak_rss_mib(),
                phase.database_bytes as f64 / (1024.0 * 1024.0),
                phase
                    .queued_jobs
                    .map_or_else(|| "-".to_string(), |jobs| jobs.to_string()),
                phase.outcome(),
                phase
                    .counters
                    .as_ref()
                    .map_or_else(|| "-".to_string(), ScanCounters::describe)
            );
        }
        let completed = phases.iter().filter(|phase| phase.completed()).count();
        println!(
            "phases completed: {completed} of 3{}",
            if completed == 3 {
                String::new()
            } else {
                format!(
                    "; this configuration did not finish the run: {}",
                    phases
                        .iter()
                        .find(|phase| !phase.completed())
                        .map(Phase::outcome)
                        .unwrap_or_else(|| "a later phase was never reached".to_string())
                )
            }
        );
    }

    /// One file at the policy's content ceiling, indexed on its own. Production leaves the
    /// buffer pool to the engine, and what this holds is that such a binary indexes its
    /// worst allowed input. Stage 2P recorded that a fixed pool refused a 100 MiB source,
    /// but Stage 2T lowered the ceiling to 16 MiB, so that boundary is no longer a
    /// reachable input: a fixed pool is measured here and reported, not required to fail.
    /// A refusal, whichever configuration produced it, must still arrive as a category with
    /// nothing written.
    pub fn largest_allowed_file() {
        let bench = Bench::create("engine-tuning-limit");
        let corpus = bench.directory.join("corpus");
        std::fs::create_dir_all(&corpus).unwrap();
        write_file_of(&corpus.join("largest.txt"), CONTENT_CEILING_MIB);
        bench.write_config(
            &corpus,
            &bench.directory.join("metadata"),
            CONTENT_CEILING_MIB,
        );
        // After the policy file exists, not before it: every command names a context, and
        // `doctor` is a command. Read first, this probe could not run at all.
        let tuning = Tuning::of(&bench);

        let measured = phase(
            &bench,
            "largest allowed source",
            &["index", "init", &corpus.to_string_lossy()],
        );
        let stderr = measured.measurement.program_stderr();
        println!(
            "{} | one {CONTENT_CEILING_MIB} MiB file | {:.1} s | {:.0} MiB | {}",
            tuning.label(),
            measured.measurement.duration.as_secs_f64(),
            measured.measurement.peak_rss_mib(),
            measured.outcome()
        );

        if !measured.completed() {
            assert_ne!(
                tuning.buffer_pool, "auto",
                "the engine sizes its own pool, so the largest allowed source must be indexed: {stderr}"
            );
            assert!(
                stderr.contains("knowledge graph storage ran out of memory"),
                "the refusal must arrive as a category: {stderr}"
            );
            for engine_word in ["Buffer manager", "buffer pool", "rollback failed"] {
                assert!(
                    !stderr.contains(engine_word),
                    "the engine's own words must not reach a reader ({engine_word}): {stderr}"
                );
            }
            assert_eq!(
                bench.graph().file_versions,
                0,
                "a refused index must leave nothing written"
            );
            return;
        }

        let counters = measured
            .counters
            .as_ref()
            .expect("a completed phase reports its counters");
        assert_eq!(counters.indexed_files, 1, "{counters:?}");
        assert_eq!(counters.content_skipped_files, 0, "{counters:?}");
        assert_eq!(
            bench.graph().file_versions,
            1,
            "the source must be published whole"
        );
    }

    /// A file of `megabytes` of plain ASCII text, written without holding it in memory.
    fn write_file_of(path: &Path, megabytes: u64) {
        use std::io::Write;
        let line = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod\n";
        let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let mut written = 0;
        while written < megabytes * 1024 * 1024 - 4096 {
            file.write_all(line.as_bytes()).unwrap();
            written += line.len() as u64;
        }
        file.flush().unwrap();
    }

    fn phase(bench: &Bench, label: &str, arguments: &[&str]) -> Phase {
        let measurement = bench.measured(label, arguments);
        let completed = measurement.exit_code == Some(0);
        Phase {
            counters: completed.then(|| ScanCounters::parse(&measurement.stdout)),
            queued_jobs: completed.then(|| bench.queued_jobs()),
            database_bytes: bench.database_bytes(),
            measurement,
        }
    }
}
