//! Does a pass cost what the tree it walks holds, or what the graph it writes into already
//! holds?
//!
//! Stage 17 asked this of the worker and answered it by splitting one queue of ten thousand
//! jobs over ten processes: every one of the ten cost the same, including the last, which
//! worked over a graph holding nine thousand results. Cost belonged to the process, not to
//! the data.
//!
//! A pass needs the same split the other way round. Equal passes, one after another, into
//! one graph that grows under them: every pass walks the same number of files and writes the
//! same number of sources, so the only thing that differs between the first and the last is
//! how much the graph already held when it started. A flat row of passes says the cost of a
//! pass is the tree it walks, and a bounded pass would bound it. A rising row says the cost
//! is what the graph already holds, and no pass length bounds anything.
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{Bench, megabytes};
use crate::corpus::text_content;
use crate::measure::Measurement;

use super::report::{cell, finished, finished_counters, numbers_from_env, ratio, run_directory};

/// Equal parts, each indexed by a pass of its own into the same graph.
const PARTS: usize = 5;
const FILES_PER_PART: usize = 10_000;

pub fn run() {
    let directory = run_directory("corpus-parts");
    let (parts, files_per_part) = shape();
    println!("parts directory: {}", directory.display());
    println!("{parts} passes of {files_per_part} files each, one after another into one graph");

    // The same row of passes twice, over trees of the same shape. The first reads content,
    // which is the pass an operator runs. The second is allowed nothing but metadata, so it
    // walks and writes locations and not one version, chunk, job or search document. What
    // the second still carries belongs to the walk and to the location writes; what only
    // the first carries belongs to content. Both rows are taken in one run on one machine,
    // which is the only way two peaks may be compared at all.
    let content = run_layer(&directory, "content", parts, files_per_part, false);
    report("every pass reads content", &content);
    let metadata = run_layer(&directory, "locations-only", parts, files_per_part, true);
    report("every pass is allowed metadata only", &metadata);

    cost_does_not_follow_the_size_of_the_graph("content", &content, parts);
    cost_does_not_follow_the_size_of_the_graph("locations only", &metadata, parts);
}

/// The one thing this benchmark asserts, and the reason it exists.
///
/// The last pass does exactly the work the first did, over a graph `parts` times bigger. If
/// what a pass costs followed what the graph holds — which is what a scan makes it do — the
/// last would cost about `parts` times the first. It is allowed to cost more than the first:
/// reaching a row by key in a larger table is dearer, and that is the storage engine rather
/// than this program. It is not allowed to cost in proportion.
///
/// The bar sits at half the proportional figure. Measured on the machine this was written
/// on, five passes of ten thousand files cost 2.0x with every scan removed and 9.4x with
/// them, so the bar separates the two with room on both sides rather than pinning a number
/// that belongs to one machine.
///
/// Wall clock belongs to the machine, so only the ratio inside one row of one run is read,
/// never the seconds. Peak memory is not asserted at all: it is diagnostic, by the same
/// rule Stage 2U set.
fn cost_does_not_follow_the_size_of_the_graph(layer: &str, passes: &[Pass], parts: usize) {
    let (Some(first), Some(last)) = (passes.first(), passes.last()) else {
        return;
    };
    if passes.len() < parts {
        // A row that stopped early has nothing to compare; the report already said why.
        return;
    }
    let proportional = parts as f64;
    let bar = proportional / 2.0;
    let grew = last.measurement.duration.as_secs_f64() / first.measurement.duration.as_secs_f64();
    assert!(
        grew < bar,
        "{layer}: the last pass did the work of the first over a graph {proportional:.0}x \
         bigger and cost {grew:.2}x of it, which is not below {bar:.2}x. A pass that costs \
         what the graph holds means something on the write path is reaching rows by \
         scanning again."
    );

    // Only when the binary counts it. Crosskit's own side of a pass is the same work every
    // time, so it may not grow at all; a tenth of a percent is the room left for the
    // allocator, not for the program.
    if let (Some(first_bytes), Some(last_bytes)) = (
        first.measurement.allocation_peak_bytes,
        last.measurement.allocation_peak_bytes,
    ) {
        let allocated = last_bytes as f64 / first_bytes as f64;
        assert!(
            allocated < 1.01,
            "{layer}: what Crosskit itself allocated grew {allocated:.4}x, from \
             {first_bytes} to {last_bytes} bytes, between two passes that did the same work"
        );
    }
}

/// One row of equal passes into one graph of its own.
fn run_layer(
    directory: &Path,
    name: &str,
    parts: usize,
    files_per_part: usize,
    metadata_only: bool,
) -> Vec<Pass> {
    println!();
    println!("--- {name} ---");
    let bench = Bench::create_in(directory, name);
    let root = bench.directory.join("corpus");
    write_parts(&root, parts, files_per_part);
    // The metadata-only pattern covers the whole corpus when this layer wants it to, and
    // names a path the corpus does not contain when it must match nothing: the
    // configuration always carries one.
    let metadata_pattern = if metadata_only {
        root.clone()
    } else {
        root.join("no-metadata-only-directory")
    };
    bench.write_config(&root, &metadata_pattern, 1);

    let mut passes = Vec::new();
    for part in 0..parts {
        let path = part_directory(&root, part);
        let measurement = bench.measured(
            &format!("pass {} of {parts}", part + 1),
            &["index", "init", path.to_str().unwrap()],
        );
        if let Some(counters) = finished_counters(&measurement) {
            let published = if metadata_only {
                counters.metadata_only_files
            } else {
                counters.indexed_files
            };
            assert_eq!(
                published, files_per_part,
                "every pass publishes the same number of files"
            );
        }
        println!(
            "{}: {} in {:.1}s, peak RSS {}",
            measurement.label,
            measurement.outcome(),
            measurement.duration.as_secs_f64(),
            super::report::peak_rss(&measurement),
        );
        let stop = !finished(&measurement);
        passes.push(Pass {
            measurement,
            database_bytes: bench.database_bytes(),
        });
        if stop {
            println!(
                "the row stops here: a pass that did not finish leaves a graph the next would not be adding to cleanly"
            );
            break;
        }
    }
    passes
}

struct Pass {
    measurement: Measurement,
    database_bytes: u64,
}

fn shape() -> (usize, usize) {
    let listed = numbers_from_env("CROSSKIT_CORPUS_PARTS").unwrap_or_default();
    match listed.as_slice() {
        [parts, files] => (*parts, *files),
        _ => (PARTS, FILES_PER_PART),
    }
}

/// `parts` sibling directories of `files` notes each, written from the same generator the
/// size curve uses, so a note here is a note there. The index every note is generated from
/// is its position in the whole corpus, so no two parts hold the same text.
fn write_parts(root: &Path, parts: usize, files: usize) {
    fs::create_dir_all(root).unwrap();
    for part in 0..parts {
        let directory = part_directory(root, part);
        for file in 0..files {
            let index = part * files + file;
            let nested = directory.join(format!("s{:02}", (file / 10) % 25));
            fs::create_dir_all(&nested).unwrap();
            fs::write(
                nested.join(format!("note-{index:06}.md")),
                text_content(index).0,
            )
            .unwrap();
        }
    }
}

fn part_directory(root: &Path, part: usize) -> PathBuf {
    root.join(format!("part-{part:02}"))
}

fn report(title: &str, passes: &[Pass]) {
    println!();
    println!("equal passes into one graph, {title}:");
    println!("pass | outcome | wall | peak RSS | our allocations | database after");
    for pass in passes {
        println!(
            "{} | {} | {}",
            pass.measurement.label,
            cell(Some(&pass.measurement)),
            megabytes(pass.database_bytes),
        );
    }
    let Some(first) = passes.first() else {
        return;
    };
    println!();
    println!("each pass against the first, which is what the answer is read from:");
    println!("pass | wall x | peak RSS x | database x");
    for pass in passes {
        println!(
            "{} | {} | {} | {}",
            pass.measurement.label,
            ratio(
                pass.measurement.duration.as_secs_f64(),
                first.measurement.duration.as_secs_f64()
            ),
            ratio(
                pass.measurement.peak_rss_bytes as f64,
                first.measurement.peak_rss_bytes as f64
            ),
            ratio(pass.database_bytes as f64, first.database_bytes as f64),
        );
    }
}
