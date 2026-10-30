//! The points of the corpus size curve, and the layer attribution taken on one size.
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::cli::{Bench, megabytes};
use crate::corpus::{Corpus, MAX_CONTENT_MB, Profile};
use crate::measure::Measurement;

use super::report::{
    cell, finished, finished_counters, numbers_from_env, peak_rss, ratio, run_directory,
};

/// The corpus sizes the curve is taken at. Ten thousand is where every memory number the
/// product stands on was measured; a hundred thousand is what a real indexed tree can
/// plausibly reach once the policy has excluded what it excludes.
///
/// `CROSSKIT_CORPUS_SCALE_POINTS` replaces them, as a comma-separated list, for a run that
/// needs a point between two of these or one beyond them.
const POINTS: [usize; 4] = [10_000, 25_000, 50_000, 100_000];

/// The size the layer attribution is taken at. Large enough that the layers are further
/// apart than the run-to-run spread, small enough that three passes over it are minutes
/// rather than an hour. `CROSSKIT_CORPUS_SCALE_ATTRIBUTION` replaces it.
const ATTRIBUTION_FILES: usize = 25_000;

pub fn run() {
    let directory = run_directory("corpus-scale");
    println!("curve directory: {}", directory.display());
    let mut rows = Vec::new();
    for files in points() {
        let row = measure_point(&directory, files);
        println!("{}", row.one_line());
        let finished = row.finished();
        rows.push(row);
        if !finished {
            println!(
                "the curve stops here: a point that did not finish leaves a graph the next \
                 point would not be measuring a cold pass over"
            );
            break;
        }
    }
    report(&rows);
}

/// Where the memory of one pass goes, by layer, on one corpus. Three passes over the same
/// tree, each in its own process:
///
/// * everything — the pass an operator runs;
/// * locations only — the same tree with every file metadata-only, so the walker and the
///   location writes are measured without a single version, chunk, job or search document;
/// * the search index alone — written again from the graph the first pass left, so the
///   index writer is measured with no graph write under it at all.
///
/// The first minus the second is what content costs, the third is what of that belongs to
/// the search index, and the remainder is what the graph costs for versions, chunks,
/// observations and the outbox.
pub fn attribute() {
    let directory = run_directory("corpus-attribution");
    let files = attribution_files();
    println!("attribution directory: {}", directory.display());
    println!("corpus of {files} text files, three passes, one process each");

    let full = Bench::create_in(&directory, "everything");
    let corpus = generate(&full, files);
    let root = corpus.root.to_string_lossy().into_owned();
    full.write_config(&corpus.root, &corpus.metadata_directory, MAX_CONTENT_MB);
    let everything = full.measured("everything", &["index", "init", &root]);
    let expected = corpus.expected_initial();
    if let Some(counters) = finished_counters(&everything) {
        assert_eq!(
            counters.indexed_files, expected.indexed_files,
            "the layer that indexes content must index all of it"
        );
    }

    // The same tree, read by a policy that allows nothing but metadata. The corpus root is
    // the metadata-only pattern, so it covers every file under it and no content is opened.
    let metadata = Bench::create_in(&directory, "locations-only");
    metadata.write_config(&corpus.root, &corpus.root, MAX_CONTENT_MB);
    let locations_only = metadata.measured("locations only", &["index", "init", &root]);
    if let Some(counters) = finished_counters(&locations_only) {
        assert_eq!(
            counters.indexed_files, 0,
            "a metadata-only policy must index no content"
        );
        assert_eq!(
            counters.metadata_only_files,
            expected.indexed_files + expected.metadata_only_files + expected.content_skipped_files,
            "every file the content pass published must be published here too, as metadata"
        );
    }

    // Written again from the graph of the first pass, which is the only source it has.
    let search_only = full.measured("search index alone", &["index", "rebuild", "--search"]);

    let layers = [
        Layer {
            measurement: everything,
            database_bytes: full.database_bytes(),
            search_index_bytes: full.search_index_bytes(),
        },
        Layer {
            measurement: locations_only,
            database_bytes: metadata.database_bytes(),
            search_index_bytes: metadata.search_index_bytes(),
        },
        Layer {
            measurement: search_only,
            database_bytes: full.database_bytes(),
            search_index_bytes: full.search_index_bytes(),
        },
    ];
    report_layers(&layers);
}

/// One point: a corpus of its own, a graph of its own, and one process per measured pass.
///
/// Fresh rather than cumulative, for the same reason the worker curve is: a graph that
/// already held a smaller corpus would make every later point carry its predecessor, and
/// the curve would rise for a reason that has nothing to do with the size of the tree.
fn measure_point(parent: &Path, files: usize) -> Row {
    let bench = Bench::create_in(parent, &Profile::Files(files).name());
    let corpus = generate(&bench, files);
    bench.write_config(&corpus.root, &corpus.metadata_directory, MAX_CONTENT_MB);
    let root = corpus.root.to_string_lossy().into_owned();
    let expected = corpus.expected_initial();

    let cold = bench.measured("cold index init", &["index", "init", &root]);
    let cold_counters = finished_counters(&cold);
    if let Some(counters) = &cold_counters {
        assert_eq!(
            counters.indexed_files, expected.indexed_files,
            "a cold pass that finished must have indexed the whole corpus"
        );
        assert_eq!(
            counters.indexed_chunks, expected.indexed_chunks,
            "a cold pass that finished must have indexed every chunk"
        );
        assert_eq!(
            counters.failed_paths, 0,
            "the corpus is readable throughout"
        );
    }

    // Only where the cold pass finished. Over a graph a killed pass left behind, the next
    // command measures recovery, not reconciliation.
    let idle = cold_counters
        .is_some()
        .then(|| bench.measured("idle reconcile", &["index", "reconcile", &root]));
    if let Some(counters) = idle.as_ref().and_then(finished_counters) {
        assert_eq!(
            counters.marked_missing,
            Some(0),
            "an idle reconcile over an unchanged tree retires nothing"
        );
        assert_eq!(
            counters.indexed_files, expected.indexed_files,
            "an idle reconcile sees the same tree the cold pass saw"
        );
    }

    // Read from the graph only when both passes finished: the queue of a killed pass is
    // whatever it had written, which is not a number to compare anything against.
    let finished = idle.as_ref().is_some_and(finished);
    let queued_jobs = if finished {
        let queued = bench.queued_jobs();
        assert_eq!(
            queued, expected.file_versions,
            "one queued job per indexed revision"
        );
        queued
    } else {
        0
    };

    let row = Row {
        files,
        corpus_bytes: corpus.total_bytes,
        text_bytes: corpus.text_bytes,
        database_bytes: bench.database_bytes(),
        search_index_bytes: bench.search_index_bytes(),
        queued_jobs,
        directory: bench.directory.clone(),
        cold,
        idle,
    };
    row.release_disk();
    row
}

/// The corpus of one point, with how long writing it took: at a hundred thousand files
/// that is minutes, and a report that does not say so invites the number to be read as
/// part of the pass.
fn generate(bench: &Bench, files: usize) -> Corpus {
    let started = Instant::now();
    let corpus = Corpus::generate(&bench.directory, Profile::Files(files));
    println!(
        "corpus of {files} text files: {} regular files, {} on disk, {} of indexable text, written in {:.1}s",
        corpus.regular_files,
        megabytes(corpus.total_bytes),
        megabytes(corpus.text_bytes),
        started.elapsed().as_secs_f64()
    );
    corpus
}

struct Row {
    files: usize,
    corpus_bytes: u64,
    text_bytes: u64,
    database_bytes: u64,
    search_index_bytes: u64,
    queued_jobs: usize,
    directory: PathBuf,
    cold: Measurement,
    idle: Option<Measurement>,
}

impl Row {
    fn finished(&self) -> bool {
        self.idle.as_ref().is_some_and(finished)
    }

    fn one_line(&self) -> String {
        format!(
            "{} files: cold {} in {:.1}s, peak RSS {}; idle {}",
            self.files,
            self.cold.outcome(),
            self.cold.duration.as_secs_f64(),
            peak_rss(&self.cold),
            match &self.idle {
                Some(idle) => format!(
                    "{} in {:.1}s, peak RSS {}",
                    idle.outcome(),
                    idle.duration.as_secs_f64(),
                    peak_rss(idle)
                ),
                None => "not run: the cold pass did not finish".to_string(),
            }
        )
    }

    /// The corpus and the graph of a point that finished are removed once its numbers are
    /// taken. Four corpora of these sizes and the graphs written from them are tens of
    /// gigabytes, and a benchmark that fills the disk halfway through measures nothing. A
    /// point that did not finish is kept: its state is the one thing left to look at.
    fn release_disk(&self) {
        if !self.finished() {
            println!("kept for inspection: {}", self.directory.display());
            return;
        }
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            println!(
                "could not remove {}: {error}; later points may run out of disk",
                self.directory.display()
            );
        }
    }
}

/// One layer of the attribution and what it left on disk.
struct Layer {
    measurement: Measurement,
    database_bytes: u64,
    search_index_bytes: u64,
}

fn points() -> Vec<usize> {
    numbers_from_env("CROSSKIT_CORPUS_SCALE_POINTS").unwrap_or_else(|| POINTS.to_vec())
}

fn attribution_files() -> usize {
    numbers_from_env("CROSSKIT_CORPUS_SCALE_ATTRIBUTION")
        .and_then(|listed| listed.first().copied())
        .unwrap_or(ATTRIBUTION_FILES)
}

fn report(rows: &[Row]) {
    println!();
    println!("cold index init:");
    println!(
        "files | corpus | outcome | wall | peak RSS | our allocations | database | search index"
    );
    for row in rows {
        println!(
            "{:>7} | {} | {} | {} | {}",
            row.files,
            megabytes(row.corpus_bytes),
            cell(Some(&row.cold)),
            megabytes(row.database_bytes),
            megabytes(row.search_index_bytes),
        );
    }
    println!();
    println!("idle reconcile over the same tree:");
    println!("files | outcome | wall | peak RSS | our allocations | queued jobs");
    for row in rows {
        println!(
            "{:>7} | {} | {}",
            row.files,
            cell(row.idle.as_ref()),
            row.queued_jobs,
        );
    }
    println!();
    // Against the smallest point rather than per file: the database has a fixed floor of
    // its own, so a per-file number would fall for a reason that is not a trend. What the
    // acceptance criterion asks is whether the peak has a trend at all, and that is a ratio.
    println!("each point against the smallest, which is what a trend means here:");
    println!("files | text | files x | cold wall x | cold peak RSS x | database x");
    let Some(first) = rows.first() else {
        return;
    };
    for row in rows {
        println!(
            "{:>7} | {} | {} | {} | {} | {}",
            row.files,
            megabytes(row.text_bytes),
            ratio(row.files as f64, first.files as f64),
            ratio(
                row.cold.duration.as_secs_f64(),
                first.cold.duration.as_secs_f64()
            ),
            ratio(
                row.cold.peak_rss_bytes as f64,
                first.cold.peak_rss_bytes as f64
            ),
            ratio(row.database_bytes as f64, first.database_bytes as f64),
        );
    }
}

fn report_layers(layers: &[Layer]) {
    println!();
    println!("where the memory of a pass belongs, one process per layer:");
    println!("layer | outcome | wall | peak RSS | our allocations | database | search index");
    for layer in layers {
        println!(
            "{} | {} | {} | {}",
            layer.measurement.label,
            cell(Some(&layer.measurement)),
            megabytes(layer.database_bytes),
            megabytes(layer.search_index_bytes),
        );
    }
}
