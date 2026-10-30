//! What one process pays for doing many transactions, separated from what it pays for the
//! data those transactions write.
//!
//! The curve says the growth follows the number of jobs one process works and not the size
//! of the graph. This narrows it further, against the engine directly: the same number of
//! equally small transactions, written two ways — a connection per transaction, which is
//! what the repository does, and one connection reused. Each shape runs in a process of its
//! own, because peak resident set size is a high-water mark and two shapes in one process
//! would share it.
use std::path::Path;

use lbug::{Connection, Database, SystemConfig, Value};

/// Enough transactions for the growth to be unmistakable, few enough that one shape runs in
/// under a minute.
pub const TRANSACTIONS: usize = 20_000;

/// Which shape a worker process runs.
pub const SHAPE_VARIABLE: &str = "CROSSKIT_TRANSACTION_SHAPE";
pub const FRESH_CONNECTION: &str = "one node, a connection per transaction";
pub const ONE_CONNECTION: &str = "one node, one connection";
/// What a finished job actually writes: a node, and a relationship from a node that already
/// exists to it. Both ends are matched by primary key, as the store matches them.
pub const WITH_RELATIONSHIP: &str = "a node and a relationship";
/// The same, with the engine asked to fold its log into the database every so often. The
/// narrowest possible remedy if what grows is state the engine holds until a checkpoint.
pub const CHECKPOINTED: &str = "a node and a relationship, checkpointed every 1000";

/// How often the checkpointed shape checkpoints.
const CHECKPOINT_EVERY: usize = 1_000;

/// Writes `TRANSACTIONS` rows, one transaction each, in the shape named. Every row is the
/// same size, so nothing about the data changes between the two shapes or along the run.
pub fn run_worker(database_path: &Path, shape: &str) {
    let database = Database::new(database_path, SystemConfig::default()).unwrap();
    {
        let connection = Connection::new(&database).unwrap();
        connection
            .query("CREATE NODE TABLE Row(id STRING, payload STRING, PRIMARY KEY (id));")
            .unwrap();
        connection
            .query("CREATE NODE TABLE Holder(id STRING, state STRING, PRIMARY KEY (id));")
            .unwrap();
        connection
            .query("CREATE REL TABLE HAS_ROW(FROM Holder TO Row, ONE_ONE);")
            .unwrap();
    }

    match shape {
        FRESH_CONNECTION => {
            for row in 0..TRANSACTIONS {
                let connection = Connection::new(&database).unwrap();
                write_row(&connection, row);
            }
        }
        ONE_CONNECTION => {
            let connection = Connection::new(&database).unwrap();
            for row in 0..TRANSACTIONS {
                write_row(&connection, row);
            }
        }
        WITH_RELATIONSHIP | CHECKPOINTED => {
            let checkpointing = shape == CHECKPOINTED;
            let connection = Connection::new(&database).unwrap();
            // The holders exist before the measured loop, the way a job exists before the
            // result that is linked to it: the growth being looked for is in the writing of
            // the link, not in the making of its ends.
            for row in 0..TRANSACTIONS {
                write_holder(&connection, row);
            }
            for row in 0..TRANSACTIONS {
                write_row_and_link(&connection, row);
                if checkpointing && (row + 1).is_multiple_of(CHECKPOINT_EVERY) {
                    connection.query("CHECKPOINT;").unwrap();
                }
            }
        }
        other => panic!("unknown transaction shape {other:?}"),
    }
    println!("wrote {TRANSACTIONS} rows as {shape}");
}

/// One transaction that creates one node and nothing else.
fn write_row(connection: &Connection<'_>, row: usize) {
    connection.query("BEGIN TRANSACTION;").unwrap();
    let mut statement = connection
        .prepare("CREATE (:Row {id: $id, payload: $payload});")
        .unwrap();
    connection
        .execute(
            &mut statement,
            vec![
                ("id", Value::String(format!("row:{row}"))),
                ("payload", Value::String("a payload of a fixed size".into())),
            ],
        )
        .unwrap();
    connection.query("COMMIT;").unwrap();
}

fn write_holder(connection: &Connection<'_>, row: usize) {
    connection.query("BEGIN TRANSACTION;").unwrap();
    let mut statement = connection
        .prepare("CREATE (:Holder {id: $id, state: 'queued'});")
        .unwrap();
    connection
        .execute(
            &mut statement,
            vec![("id", Value::String(format!("holder:{row}")))],
        )
        .unwrap();
    connection.query("COMMIT;").unwrap();
}

/// One transaction shaped like a finished job's: the result node, the link from the job to
/// it, and the job's new state.
fn write_row_and_link(connection: &Connection<'_>, row: usize) {
    connection.query("BEGIN TRANSACTION;").unwrap();
    let mut create = connection
        .prepare("CREATE (:Row {id: $id, payload: $payload});")
        .unwrap();
    connection
        .execute(
            &mut create,
            vec![
                ("id", Value::String(format!("row:{row}"))),
                ("payload", Value::String("a payload of a fixed size".into())),
            ],
        )
        .unwrap();
    let mut link = connection
        .prepare(
            "MATCH (holder:Holder {id: $holder}) MATCH (target:Row {id: $target}) CREATE (holder)-[:HAS_ROW]->(target);",
        )
        .unwrap();
    connection
        .execute(
            &mut link,
            vec![
                ("holder", Value::String(format!("holder:{row}"))),
                ("target", Value::String(format!("row:{row}"))),
            ],
        )
        .unwrap();
    let mut done = connection
        .prepare("MATCH (holder:Holder {id: $holder}) SET holder.state = 'done';")
        .unwrap();
    connection
        .execute(
            &mut done,
            vec![("holder", Value::String(format!("holder:{row}")))],
        )
        .unwrap();
    connection.query("COMMIT;").unwrap();
}
