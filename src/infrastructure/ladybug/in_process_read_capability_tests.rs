// The evidence base of ADR 0050: what a reading connection inside the writer's own process may
// and may not do.
//
// It may read beside an open write transaction: it answers at once and sees what was committed
// before it. It may not be inside the engine while pages move — a commit (which may run the
// engine's own checkpoint) or a checkpoint — which crashed the whole process twice, with a
// SIGSEGV in `ColumnChunk::scan` on the reader while another thread checkpointed. A tiny
// database does not show that, which is how the first version of this file concluded the
// opposite; the last test here is the one at a scale that did, with the hold that prevents it.
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use lbug::{Connection, Database, SystemConfig, Value};

/// Longer than any read below may take; a read that waits for the writer waits this long.
const WRITER_HOLDS: Duration = Duration::from_secs(2);

fn count(connection: &Connection<'_>) -> i64 {
    let mut result = connection.query("MATCH (t:Item) RETURN COUNT(*);").unwrap();
    match result.next().unwrap()[0] {
        Value::Int64(count) => count,
        ref other => panic!("unexpected count {other:?}"),
    }
}

fn database(name: &str) -> (std::path::PathBuf, Database) {
    let directory = std::env::temp_dir().join(format!("crosskit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let database = Database::new(directory.join("graph.lbug"), SystemConfig::default()).unwrap();
    {
        let connection = Connection::new(&database).unwrap();
        connection
            .query("CREATE NODE TABLE Item(id INT64, PRIMARY KEY(id));")
            .unwrap();
        connection.query("CREATE (:Item {id: 0});").unwrap();
    }
    (directory, database)
}

/// A read beside an open write transaction answers at once, from what was committed before
/// it: the rows the writer has written and not committed are not there.
#[test]
fn a_read_beside_an_open_write_transaction_answers_at_once_from_the_committed_snapshot() {
    let (directory, database) = database("in-process-read");
    let (written, read_done) = (mpsc::channel(), mpsc::channel::<()>());
    let (written_tx, written_rx) = written;
    let (done_tx, done_rx) = read_done;

    thread::scope(|scope| {
        let database = &database;
        scope.spawn(move || {
            let writer = Connection::new(database).unwrap();
            writer.query("BEGIN TRANSACTION;").unwrap();
            writer
                .query("UNWIND range(1, 5000) AS i CREATE (:Item {id: i});")
                .unwrap();
            written_tx.send(()).unwrap();
            // Held open until the reader is done, or for at most WRITER_HOLDS.
            let _ = done_rx.recv_timeout(WRITER_HOLDS);
            writer.query("COMMIT;").unwrap();
        });

        written_rx.recv().unwrap();
        let reader = Connection::new(database).unwrap();
        let started = Instant::now();
        let seen = count(&reader);
        let took = started.elapsed();
        done_tx.send(()).unwrap();

        assert_eq!(seen, 1, "the read saw rows the writer had not committed");
        assert!(
            took < WRITER_HOLDS / 2,
            "the read waited for the writer: {took:?}"
        );
    });

    {
        let after = Connection::new(&database).unwrap();
        assert_eq!(count(&after), 5001);
    }
    drop(database);
    std::fs::remove_dir_all(&directory).unwrap();
}

/// A checkpoint beside a reader is not refused, and on a database this small the reader is not
/// hurt — which is exactly why this test proves nothing about a real one; see below.
#[test]
fn a_checkpoint_beside_a_reader_neither_refuses_nor_is_refused() {
    let (directory, database) = database("in-process-checkpoint");
    {
        let writer = Connection::new(&database).unwrap();
        writer
            .query("UNWIND range(1, 5000) AS i CREATE (:Item {id: i});")
            .unwrap();
    }

    thread::scope(|scope| {
        let database = &database;
        let reading = scope.spawn(move || {
            let reader = Connection::new(database).unwrap();
            (0..50).map(|_| count(&reader)).collect::<Vec<_>>()
        });
        let folder = Connection::new(database).unwrap();
        let started = Instant::now();
        folder.query("CHECKPOINT;").unwrap();
        let took = started.elapsed();
        let counts = reading.join().unwrap();
        assert!(counts.iter().all(|seen| *seen == 5001), "{counts:?}");
        println!("checkpoint beside a reader: {took:?}");
    });
    drop(database);
    std::fs::remove_dir_all(&directory).unwrap();
}

/// The production path at a scale where unguarded reads crashed: a writer that commits batch
/// after batch and folds the log after each, and four readers asking all the while — each
/// under `reading()`, as the service's socket asks. Every reader answers, every batch lands.
#[test]
fn readers_holding_off_commits_and_checkpoints_survive_a_writer_that_folds_after_every_batch() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::LadybugDatabaseManager;
    use super::test_support::{TestDatabase, count as count_rows, evidence_commit};
    use crate::application::{GraphCheckpoint, SourceBatchWriter};

    const ROUNDS: usize = 40;
    const FILES: usize = 50;
    let test = TestDatabase::new("reads-beside-checkpoints");
    let manager = LadybugDatabaseManager::open(test.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let writing = AtomicBool::new(true);
    let answered = AtomicUsize::new(0);

    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                while writing.load(Ordering::SeqCst) {
                    let _hold = manager.reading();
                    count_rows(
                        &manager,
                        "MATCH (l:FileLocation) WHERE l.normalized_path STARTS WITH '/data' RETURN COUNT(*);",
                    );
                    answered.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
        for round in 0..ROUNDS {
            let batch = (0..FILES)
                .map(|file| {
                    evidence_commit(
                        &format!("file-{round}-{file}"),
                        &format!("/data/r{round}/f{file}.md"),
                    )
                })
                .collect();
            repository.commit_batch(batch).unwrap();
            repository.checkpoint().unwrap();
        }
        writing.store(false, Ordering::SeqCst);
    });

    assert!(answered.load(Ordering::SeqCst) > 0);
    assert_eq!(
        count_rows(&manager, "MATCH (l:FileLocation) RETURN COUNT(*);"),
        (ROUNDS * FILES) as i64
    );
}
