// The heaviest source the index policy allows, indexed on its own. The scale corpus is
// made of small files and never holds one source large enough to show what a single
// source costs, so Stage 2R, 2S and 2T measured this case separately every time; this
// makes it a scenario the machine can run instead of a recipe a person follows.
//
// Ignored by default; run explicitly with
//
//   cargo test --release --offline --test largest_source -- --ignored --nocapture
//
// Add `--features allocation-profile` to the same command to also get what Crosskit's own
// code allocated in each phase.
//
// The corpus is one synthetic file, generated from a seed, in its own benchmark directory
// with its own CROSSKIT_DATA_DIR; nothing outside that directory is touched and it is kept
// afterwards for inspection. Timings and memory are printed, never asserted: they belong to
// the machine. What is asserted is that the source is indexed as content, whole, once.
//
// Unix-only, like the scale benchmark: it drives the descriptor-relative walker. On another
// platform it compiles out and an explicit run says why instead of pretending to measure.
#[cfg(unix)]
#[path = "scale_indexing/cli.rs"]
mod cli;
#[cfg(unix)]
#[path = "scale_indexing/corpus.rs"]
mod corpus;
#[cfg(unix)]
#[path = "memory_diagnostic/measure.rs"]
mod measure;

#[cfg(unix)]
#[test]
#[ignore = "largest allowed source: cargo test --release --offline --test largest_source -- --ignored --nocapture"]
fn the_largest_allowed_source_is_indexed_whole() {
    benchmark::run();
}

#[cfg(not(unix))]
#[test]
#[ignore = "largest allowed source: Unix only"]
fn the_largest_allowed_source_is_indexed_whole() {
    panic!(
        "this benchmark requires the Unix descriptor-relative walker; it is not available on this platform"
    );
}

#[cfg(unix)]
mod benchmark {
    use std::fs;

    use super::cli::{Bench, ScanCounters, allocated_peak_label, megabytes, peak_rss_label};
    use super::corpus::{CHUNK_BYTES, CONTENT_CEILING_MIB, large_text};
    use super::measure::Measurement;

    pub fn run() {
        let bench = Bench::create("largest-source");
        println!("benchmark directory: {}", bench.directory.display());
        let root = bench.directory.join("corpus");
        fs::create_dir(&root).unwrap();
        let chunks = CONTENT_CEILING_MIB as usize * 1024 * 1024 / CHUNK_BYTES;
        fs::write(root.join("largest.txt"), large_text(0, chunks)).unwrap();
        // The metadata-only directory of the policy is named but never created: this corpus
        // is the one source under measurement and nothing else.
        bench.write_config(
            &root,
            &bench.directory.join("metadata"),
            CONTENT_CEILING_MIB,
        );
        let root = root.to_string_lossy().into_owned();
        println!("corpus: one file of {CONTENT_CEILING_MIB} MiB, {chunks} chunks");

        let cold = bench.measured("cold index init", &["index", "init", &root]);
        assert_indexed_whole(&cold, chunks);
        let graph = bench.graph();
        graph.assert_consistent();
        assert_eq!(graph.file_versions, 1, "{graph:?}");
        assert_eq!(graph.chunks, chunks, "{graph:?}");
        assert_eq!(graph.locations, 1, "{graph:?}");
        report(&bench, &cold);

        // The same content again: the source is read and chunked a second time, and none of
        // it is written, which is the cheaper half of the cost and worth its own number.
        let idle = bench.measured("idle reconcile", &["index", "reconcile", &root]);
        assert_indexed_whole(&idle, chunks);
        let after = bench.graph();
        assert_eq!(
            after.evidence(),
            graph.evidence(),
            "an idle reconcile must change no evidence"
        );
        report(&bench, &idle);

        println!(
            "benchmark directory kept for inspection: {}",
            bench.directory.display()
        );
    }

    /// The source was read as content and cut into every one of its chunks. A ceiling below
    /// the size of this file would leave it metadata-only, and that shows up here.
    fn assert_indexed_whole(measurement: &Measurement, chunks: usize) {
        assert_eq!(
            measurement.exit_code,
            Some(0),
            "{} did not complete: {}; {}",
            measurement.label,
            measurement.outcome(),
            measurement.program_stderr()
        );
        let counters = ScanCounters::parse(&measurement.stdout);
        assert_eq!(counters.indexed_files, 1, "{counters:?}");
        assert_eq!(counters.indexed_chunks, chunks, "{counters:?}");
        assert_eq!(counters.metadata_only_files, 0, "{counters:?}");
        assert_eq!(counters.content_skipped_files, 0, "{counters:?}");
        assert_eq!(counters.unstable_sources, 0, "{counters:?}");
        assert_eq!(counters.failed_paths, 0, "{counters:?}");
    }

    fn report(bench: &Bench, measurement: &Measurement) {
        println!(
            "{}: {:.2?}, {}{}, database {}",
            measurement.label,
            measurement.duration,
            peak_rss_label(measurement),
            allocated_peak_label(measurement),
            megabytes(bench.database_bytes())
        );
    }
}
