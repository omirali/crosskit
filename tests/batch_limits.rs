// What the batch limits cost in memory, measured on the scale corpus.
//
// Ignored by default. The binary under test must be built with the limits under test, and
// this benchmark reads back what that binary actually compiled in rather than trusting the
// request, so a table of runs cannot be mislabelled:
//
//   CROSSKIT_BATCH_LIMIT_SOURCES=50 CROSSKIT_BATCH_LIMIT_PAYLOAD_MIB=2 \
//     cargo test --release --offline --features batch-limit-benchmark \
//     --test batch_limits -- --ignored --nocapture
//
// The limits are a compile-time value of that binary; nothing about batching is read from
// the environment of a running Crosskit, and without the feature the binary has one fixed
// pair of limits and says nothing about them.
//
// Peak resident set size is read from `/usr/bin/time -l` around the already built binary,
// never from the process measuring itself, which is why this is macOS-only and says so
// instead of reporting a number it did not measure. The corpus and the counter parsing are
// the scale benchmark's own, so the two benchmarks cannot drift apart.
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
#[ignore = "batch limit benchmark: cargo test --release --offline --features batch-limit-benchmark --test batch_limits -- --ignored --nocapture"]
fn the_cost_of_the_batch_limits_is_measured() {
    benchmark::run();
}

#[cfg(not(target_os = "macos"))]
#[test]
#[ignore = "batch limit benchmark: macOS only"]
fn the_cost_of_the_batch_limits_is_measured() {
    panic!(
        "this benchmark reads peak resident set size from `/usr/bin/time -l`, which only macOS provides here; no measurement is reported on this platform"
    );
}

#[cfg(target_os = "macos")]
mod benchmark {

    use super::cli::{Bench, ScanCounters, megabytes};
    use super::corpus::{Corpus, MAX_CONTENT_MB, Profile};
    use super::measure::Measurement;

    /// The limits the binary under test compiled in, as it reports them itself.
    struct Limits {
        sources: usize,
        payload_bytes: usize,
    }

    impl Limits {
        /// Reads the limits out of the binary and checks them against what the operator
        /// asked for. A binary built without the benchmark feature, or built before the
        /// request changed, is refused rather than measured under the wrong label.
        fn of(bench: &Bench) -> Self {
            let reported = bench.run_ok(&["doctor"]);
            let line = reported
                .lines()
                .find_map(|line| line.strip_prefix("Batch limits: "))
                .unwrap_or_else(|| {
                    panic!(
                        "the binary does not report its batch limits; build it with --features batch-limit-benchmark"
                    )
                });
            let (sources, payload) = line
                .split_once(" sources, ")
                .expect("the reported limits must name sources and payload");
            let limits = Self {
                sources: sources.parse().expect("the source limit must be a number"),
                payload_bytes: payload
                    .trim_end_matches(" payload bytes")
                    .parse()
                    .expect("the payload limit must be a number"),
            };
            limits.agrees_with_request();
            limits
        }

        fn agrees_with_request(&self) {
            if let Ok(requested) = std::env::var("CROSSKIT_BATCH_LIMIT_SOURCES") {
                assert_eq!(
                    requested,
                    self.sources.to_string(),
                    "the binary was built with a different source limit than this run asked for"
                );
            }
            if let Ok(requested) = std::env::var("CROSSKIT_BATCH_LIMIT_PAYLOAD_MIB") {
                assert_eq!(
                    requested,
                    (self.payload_bytes / (1024 * 1024)).to_string(),
                    "the binary was built with a different payload limit than this run asked for"
                );
            }
        }

        fn payload_mib(&self) -> usize {
            self.payload_bytes / (1024 * 1024)
        }
    }

    pub fn run() {
        let bench = Bench::create("batch-limits");
        println!("benchmark directory: {}", bench.directory.display());
        let limits = Limits::of(&bench);
        let corpus = Corpus::generate(&bench.directory, Profile::Scale);
        bench.write_config(&corpus.root, &corpus.metadata_directory, MAX_CONTENT_MB);
        let root = corpus.root.to_string_lossy().into_owned();
        println!(
            "limits the binary compiled in: {} sources, {} MiB payload; corpus {} files, {}",
            limits.sources,
            limits.payload_mib(),
            corpus.regular_files,
            megabytes(corpus.total_bytes)
        );

        let cold = phase(&bench, "cold index init", &["index", "init", &root]);
        let idle = phase(&bench, "idle reconcile", &["index", "reconcile", &root]);

        println!();
        println!(
            "sources | payload MiB | phase | wall s | peak RSS MiB | db MiB | jobs | counters"
        );
        for (measurement, counters, jobs, database_bytes) in [cold, idle] {
            println!(
                "{} | {} | {} | {:.1} | {:.0} | {:.1} | {} | {}",
                limits.sources,
                limits.payload_mib(),
                measurement.label,
                measurement.duration.as_secs_f64(),
                measurement.peak_rss_mib(),
                database_bytes as f64 / (1024.0 * 1024.0),
                jobs,
                counters.describe()
            );
            assert_eq!(measurement.exit_code, Some(0), "{}", measurement.label);
        }
        println!(
            "benchmark directory kept for inspection: {}",
            bench.directory.display()
        );
    }

    fn phase(
        bench: &Bench,
        label: &str,
        arguments: &[&str],
    ) -> (Measurement, ScanCounters, usize, u64) {
        let measurement = bench.measured(label, arguments);
        let counters = ScanCounters::parse(&measurement.stdout);
        (
            measurement,
            counters,
            bench.queued_jobs(),
            bench.database_bytes(),
        )
    }
}
