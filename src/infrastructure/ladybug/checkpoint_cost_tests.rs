// What a checkpoint at the end of a pass costs, and what it changes about a stop.
//
// The engine folds committed batches into the database when its log grows past a threshold
// of its own and when the database is closed. Between those, the work lives in the log —
// and a process stopped without closing leaves a log the engine will not replay. A pass
// that ends by checkpointing pays for that fold earlier; this measures how much.
use std::path::PathBuf;
use std::time::Instant;

use super::LadybugDatabaseManager;
use super::test_support::TestDatabase;
use crate::application::{GraphCheckpoint, SourceBatchWriter};
use crate::domain::{
    AnalysisRequest, ChunkScript, ContentChunk, EvidenceCommit, FileKind, FileLocation,
    FileObservation, FileVersion, NormalizedPath, SourceCommit,
};

/// A source of about the size the scale corpus has: one chunk of roughly 4 KiB.
fn source(ordinal: usize) -> SourceCommit {
    let text = "обход индекса сверка хранилище файл версия чанк локация ".repeat(40);
    let file_id = format!("file:{ordinal}");
    let content_hash = format!("hash:{ordinal}");
    let version = FileVersion::new(
        format!("{file_id}@{content_hash}"),
        &file_id,
        &content_hash,
        100,
    )
    .unwrap();
    let observation = FileObservation::new(
        format!("observation:{ordinal}"),
        &file_id,
        version.id(),
        100,
    )
    .unwrap();
    let chunk = ContentChunk::new(
        format!("chunk:{ordinal}"),
        version.id(),
        0,
        0,
        text.len() as u64,
        format!("text:{ordinal}"),
        &text,
        ChunkScript::Cyrillic,
        "plain-text",
        "plain-text",
        "1",
        "config:1",
        version.content_hash(),
        100,
    )
    .unwrap();
    SourceCommit::with_evidence(
        FileLocation::observed(
            &file_id,
            NormalizedPath::from_absolute(&format!("/corpus/{ordinal:06}.md")).unwrap(),
            FileKind::RegularFile,
            text.len() as u64,
            100,
            100,
        )
        .unwrap(),
        EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

/// A pass of this many sources, in batches of the production size.
const SOURCES: usize = 10_000;
const BATCH: usize = 100;

#[test]
#[ignore = "storage measurement: cargo test --release --offline --bins the_cost_of_a_checkpoint_after_a_pass -- --ignored --nocapture"]
fn the_cost_of_a_checkpoint_after_a_pass() {
    let database = TestDatabase::new("checkpoint-cost");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let log = PathBuf::from(format!("{}.wal", database.path().display()));

    let started = Instant::now();
    for batch in 0..SOURCES / BATCH {
        let commits = (0..BATCH)
            .map(|index| source(batch * BATCH + index))
            .collect();
        repository.commit_batch(commits).unwrap();
    }
    let pass = started.elapsed();
    let log_bytes = log.metadata().map(|meta| meta.len()).unwrap_or(0);
    let database_bytes = database.path().metadata().unwrap().len();

    let started = Instant::now();
    repository.checkpoint().unwrap();
    let checkpoint = started.elapsed();

    println!(
        "pass of {SOURCES} sources: {pass:.2?}; log before the checkpoint {:.1} MiB, database {:.1} MiB",
        log_bytes as f64 / 1024.0 / 1024.0,
        database_bytes as f64 / 1024.0 / 1024.0,
    );
    println!(
        "checkpoint: {checkpoint:.2?} ({:.2}% of the pass); log after {:.1} MiB, database {:.1} MiB",
        checkpoint.as_secs_f64() / pass.as_secs_f64() * 100.0,
        log.metadata().map(|meta| meta.len()).unwrap_or(0) as f64 / 1024.0 / 1024.0,
        database.path().metadata().unwrap().len() as f64 / 1024.0 / 1024.0,
    );

    // And the second one, right after it, which is what a pass that changed nothing pays.
    let started = Instant::now();
    repository.checkpoint().unwrap();
    println!("checkpoint with nothing to fold: {:.2?}", started.elapsed());
}
