// Stage 19, step one: what actually accumulates in a pass, before anything is changed.
//
// Stage 18 measured that Crosskit's own allocations follow the number of files in one
// pass — 32.1 MB at ten thousand, 116.8 MB at a hundred thousand — and named the search
// index's deletion queue as the source. That name was read out of the code, not measured,
// and the code says the writer's arena is fixed at 15 MB with one thread, which cannot
// account for eighty-five megabytes. So the first thing this stage does is take the number
// apart.
//
// It is measured with the allocation counter rather than with resident memory, because the
// question is what is live and held, not what the process touched: the counter is exact to
// the byte and comparable between runs, which resident memory is not.
//
// Each shape publishes the same documents through the production writer, in batches the
// size a scan publishes, and reads what is still live afterwards — with no commit, which is
// what a pass does until it ends.
//
//   cargo test --release --offline --features allocation-profile --bins \
//     search::writer_memory_tests -- --ignored --nocapture --test-threads=1
use std::path::PathBuf;

use crate::application::{SearchDocument, SearchIndexWriter};
use crate::domain::NormalizationSet;
use crate::infrastructure::allocation_profile::profile;

use super::context_index::ContextSearchIndex;
use super::language_pack::built_in_normalizations;

/// How many sources one batch carries, so the writer is driven the way a scan drives it.
const BATCH_SOURCES: usize = 100;
/// One chunk per file, which is what the scale corpus holds for all but every fortieth file.
const CHUNKS_PER_FILE: usize = 1;
/// The corpus sizes the growth was measured at.
const POINTS: [usize; 5] = [10_000, 20_000, 30_000, 40_000, 50_000];

/// What one shape does with a batch, so the two halves of `publish` can be told apart.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Exactly what a pass does: delete the file's old documents, then add its new ones.
    Publish,
    /// Only the deletions — the queue Stage 18 accused without measuring.
    DeleteOnly,
    /// Only the documents, with nothing deleted.
    AddOnly,
    /// Deletions again, but the term is as short as an identity can be. If what the queue
    /// costs follows the term, this is cheaper; if it does not, the cost is the entry
    /// around the term and shortening identities would buy nothing.
    DeleteShortTerm,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Self::Publish => "publish (delete + add)",
            Self::DeleteOnly => "delete only",
            Self::AddOnly => "add only",
            Self::DeleteShortTerm => "delete only, 3-byte term",
        }
    }
}

#[test]
#[ignore = "what a pass accumulates: cargo test --release --offline --features allocation-profile --bins search::writer_memory_tests -- --ignored --nocapture --test-threads=1"]
fn what_the_index_writer_holds_until_the_pass_ends() {
    println!();
    println!("live bytes held by the writer after publishing, before any commit:");
    println!("shape | files | live after | per file");
    for shape in [
        Shape::Publish,
        Shape::DeleteOnly,
        Shape::AddOnly,
        Shape::DeleteShortTerm,
    ] {
        let mut rows = Vec::new();
        for files in POINTS {
            rows.push((files, held_after_publishing(shape, files)));
        }
        for (files, held) in &rows {
            println!(
                "{} | {files} | {} | {:.0} B",
                shape.name(),
                megabytes(*held),
                *held as f64 / *files as f64,
            );
        }
        // The slope between the smallest and the largest point is the whole question: a
        // writer that holds a fixed arena has none.
        let ((small, small_held), (large, large_held)) = (rows[0], rows[rows.len() - 1]);
        println!(
            "{} | slope: {:.0} B per file between {small} and {large} files",
            shape.name(),
            (large_held as f64 - small_held as f64) / (large as f64 - small as f64),
        );
    }
}

/// Publishes `files` files through the production writer in scan-sized batches and returns
/// how many bytes are still live when it is done.
///
/// The index directory and everything built around it stay alive until after the reading,
/// so what the reading shows is what the writer holds, not what dropping it released.
fn held_after_publishing(shape: Shape, files: usize) -> u64 {
    let directory = scratch_directory(shape, files);
    let index = ContextSearchIndex::open(&directory, &normalizations())
        .expect("the index directory is this test's own");

    let before = profile().live_bytes;
    for batch in 0..files.div_ceil(BATCH_SOURCES) {
        let first = batch * BATCH_SOURCES;
        let last = (first + BATCH_SOURCES).min(files);
        let file_ids: Vec<String> = match shape {
            Shape::DeleteShortTerm => (first..last).map(|file| format!("{file:x}")).collect(),
            _ => (first..last).map(file_id).collect(),
        };
        let documents: Vec<SearchDocument> = (first..last)
            .flat_map(|file| {
                (0..CHUNKS_PER_FILE).map(move |chunk| {
                    // No language: this measures what the writer holds, and a language
                    // would add a field to every document without changing that.
                    SearchDocument::new(
                        format!("{}#{chunk}", file_id(file)),
                        file_id(file),
                        None,
                        chunk_text(file),
                    )
                })
            })
            .collect();
        match shape {
            Shape::Publish => index.publish(&file_ids, documents),
            Shape::DeleteOnly | Shape::DeleteShortTerm => index.publish(&file_ids, Vec::new()),
            Shape::AddOnly => index.append(documents),
        }
        .expect("publishing to this test's own index cannot fail");
    }
    let held = profile().live_bytes.saturating_sub(before);

    // Read first, then let go: dropping the writer waits for its threads and frees the
    // arena, and a reading taken after that would be a reading of nothing.
    drop(index);
    let _ = std::fs::remove_dir_all(&directory);
    held
}

/// A file identity the shape of a real one: device, inode and birth time.
fn file_id(file: usize) -> String {
    format!(
        "file:16777232:{:09}:{}",
        100_000 + file,
        1_700_000_000 + file
    )
}

/// A chunk of prose about the size the scale corpus writes, so the documents weigh what
/// real ones weigh.
fn chunk_text(file: usize) -> String {
    let mut text = String::with_capacity(900);
    let subject = file % 97;
    while text.len() < 900 {
        text.push_str(&format!(
            "note {file} about subject {subject} with evidence reconcile chunk policy watcher "
        ));
    }
    text
}

fn normalizations() -> NormalizationSet {
    built_in_normalizations()
}

fn scratch_directory(shape: Shape, files: usize) -> PathBuf {
    let stem = match shape {
        Shape::Publish => "publish",
        Shape::DeleteOnly => "delete",
        Shape::AddOnly => "add",
        Shape::DeleteShortTerm => "delete-short",
    };
    let directory = std::env::temp_dir().join(format!(
        "crosskit-writer-memory-{stem}-{files}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    directory
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}
