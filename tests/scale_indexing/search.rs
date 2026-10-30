//! Shared by more than one benchmark, each of which uses a part of it; the parts a
//! given benchmark does not use are not dead code.
#![allow(dead_code)]

// What search costs on the corpus, and what it must be true of after every phase.
//
// Two latencies are printed because two different things are worth knowing. One is the
// whole command, which is what an operator waits for and which carries the cost of opening
// a database and an index in a fresh process. The other is the query itself, measured
// beside the graph in `infrastructure::search::cost_tests`, where a process is already
// running. Neither replaces the other, and neither is called by the name of the other.
use std::time::Instant;

use super::cli::{Bench, megabytes};
use super::corpus::Corpus;

/// Words the generated corpus is built from, in both of its languages, plus two forms
/// nothing was written in — so a run shows both that matches are found and that the
/// absence of a match is answered as quickly.
const QUERIES: &[&str] = &[
    "graph",
    "evidence",
    "reconcile",
    "location",
    "locations",
    "chunk",
    "policy",
    "watcher",
    "заметка",
    "заметки",
    "индекс",
    "индексы",
    "источник",
    "источников",
    "проверка",
    "хранилище",
    "ZZQQNOTHINGHERE",
    "несуществующее",
];

/// The index holds a document per chunk of a current version, and says so itself.
pub fn assert_search_is_in_step(bench: &Bench, expected_chunks: usize) {
    let status = bench.search_status();
    assert_eq!(
        status.documents, expected_chunks as u64,
        "the search index must hold one document per chunk of a current version: {status:?}"
    );
    assert_eq!(status.chunks, expected_chunks as u64, "{status:?}");
    assert!(status.in_step, "{status:?}");
    assert!(status.matches_this_build, "{status:?}");
}

/// Index size against the text it was built from, the latency of a whole `search` command,
/// and what a rebuild of the same index costs.
pub fn measure_search(bench: &Bench, corpus: &Corpus) {
    let on_disk = bench.search_index_bytes();
    println!(
        "search index: {} against {} of indexed text — {:.2}x",
        megabytes(on_disk),
        megabytes(corpus.text_bytes),
        on_disk as f64 / corpus.text_bytes as f64
    );

    let mut samples = Vec::with_capacity(QUERIES.len());
    let mut answered = 0;
    for query in QUERIES {
        let started = Instant::now();
        let stdout = bench.run_ok(&["search", query, "--limit", "10"]);
        samples.push(started.elapsed().as_micros() as u64);
        if !stdout.contains("Results: 0") {
            answered += 1;
        }
    }
    samples.sort_unstable();
    println!(
        "search command: {} queries ({answered} with results), median {:.1} ms, p90 {:.1} ms — a whole process, including opening the database and the index",
        samples.len(),
        samples[samples.len() / 2] as f64 / 1000.0,
        samples[samples.len() * 9 / 10] as f64 / 1000.0
    );

    let rebuilt = bench.measured("search index rebuild", &["index", "rebuild", "--search"]);
    assert_eq!(
        rebuilt.exit_code,
        Some(0),
        "rebuilding the search index failed: {}",
        rebuilt.program_stderr()
    );
    println!(
        "search index rebuild: {:.2?}, {} on disk",
        rebuilt.duration,
        megabytes(bench.search_index_bytes())
    );
}
