// What the set-based stores rely on, proven against the real LadybugDB rather than
// assumed: parameterised `UNWIND $rows` over struct rows, every property type the schema
// uses, MERGE and relationship creation in bulk, a verifiable count of processed rows,
// and a failure inside one row rolling the whole statement and transaction back.
use lbug::{Connection, LogicalType, Value};

use super::store_support::{fetch_count, struct_list};
use super::test_support::{TestDatabase, count};
use super::{LadybugDatabaseManager, migration};
use crate::application::RepositoryError;

fn version_row(index: usize) -> Value {
    Value::Struct(vec![
        (
            "id".to_string(),
            Value::String(format!("file:{index}@hash")),
        ),
        (
            "file_id".to_string(),
            Value::String(format!("file:{index}")),
        ),
        (
            "content_hash".to_string(),
            Value::String("hash".to_string()),
        ),
        ("created_at".to_string(), Value::Int64(100 + index as i64)),
    ])
}

fn run(
    connection: &Connection<'_>,
    query: &str,
    parameters: Vec<(&str, Value)>,
) -> Result<Vec<Vec<Value>>, RepositoryError> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| RepositoryError::Storage(error.to_string()))?;
    let result = connection
        .execute(&mut statement, parameters)
        .map_err(|error| RepositoryError::Storage(error.to_string()))?;
    Ok(result.collect())
}

#[test]
fn unwind_creates_nodes_merges_and_relationships_and_reports_how_many_rows_it_processed() {
    let database = TestDatabase::new("unwind-capabilities");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();
    let rows: Vec<Value> = (0..4).map(version_row).collect();

    migration::in_transaction(&connection, |connection| {
        // MERGE over a batch: the File nodes the versions will hang from.
        let merged = run(
            connection,
            "UNWIND $rows AS row MERGE (file:File {id: row.file_id}) RETURN COUNT(file);",
            vec![("rows", struct_list(rows.clone()))],
        )?;
        assert_eq!(merged, [[Value::Int64(4)]], "MERGE must report its rows");

        // CREATE over a batch, with STRING and INT64 properties from the same struct.
        let created = run(
            connection,
            "UNWIND $rows AS row CREATE (version:FileVersion {id: row.id, file_id: row.file_id, content_hash: row.content_hash, created_at_unix_ms: row.created_at}) RETURN COUNT(version);",
            vec![("rows", struct_list(rows.clone()))],
        )?;
        assert_eq!(created, [[Value::Int64(4)]]);

        // Relationships over a batch, matching both endpoints by primary key.
        let linked = run(
            connection,
            "UNWIND $rows AS row MATCH (file:File {id: row.file_id}), (version:FileVersion {id: row.id}) MERGE (file)-[:HAS_VERSION]->(version) RETURN COUNT(*);",
            vec![("rows", struct_list(rows.clone()))],
        )?;
        assert_eq!(linked, [[Value::Int64(4)]]);
        Ok(())
    })
    .unwrap();

    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 4);
    assert_eq!(
        count(
            &manager,
            "MATCH (:File)-[:HAS_VERSION]->(:FileVersion) RETURN COUNT(*);"
        ),
        4
    );
}

#[test]
fn a_batch_read_returns_only_the_rows_the_batch_asked_about() {
    let database = TestDatabase::new("unwind-read");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();
    migration::in_transaction(&connection, |connection| {
        run(
            connection,
            "UNWIND $rows AS row CREATE (:FileVersion {id: row.id, file_id: row.file_id, content_hash: row.content_hash, created_at_unix_ms: row.created_at});",
            vec![("rows", struct_list((0..5).map(version_row).collect()))],
        )
        .map(|_| ())
    })
    .unwrap();

    // Asking about three ids, one of which does not exist, returns exactly the two stored.
    let ids = Value::List(
        LogicalType::String,
        vec![
            Value::String("file:1@hash".to_string()),
            Value::String("file:3@hash".to_string()),
            Value::String("file:99@hash".to_string()),
        ],
    );
    let found = run(
        &connection,
        "UNWIND $ids AS id MATCH (version:FileVersion {id: id}) RETURN version.id, version.file_id, version.content_hash, version.created_at_unix_ms;",
        vec![("ids", ids)],
    )
    .unwrap();

    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(
        found[0],
        [
            Value::String("file:1@hash".to_string()),
            Value::String("file:1".to_string()),
            Value::String("hash".to_string()),
            Value::Int64(101),
        ]
    );
}

#[test]
fn a_failing_row_rolls_back_its_whole_statement_and_transaction() {
    let database = TestDatabase::new("unwind-rollback");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();
    migration::in_transaction(&connection, |connection| {
        run(
            connection,
            "UNWIND $rows AS row CREATE (:FileVersion {id: row.id, file_id: row.file_id, content_hash: row.content_hash, created_at_unix_ms: row.created_at});",
            vec![("rows", struct_list(vec![version_row(0)]))],
        )
        .map(|_| ())
    })
    .unwrap();
    assert_eq!(count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"), 1);

    // The second batch is valid except for one row that repeats a stored primary key.
    let doomed = vec![version_row(1), version_row(0), version_row(2)];
    let error = migration::in_transaction(&connection, |connection| {
        run(
            connection,
            "UNWIND $rows AS row CREATE (:FileVersion {id: row.id, file_id: row.file_id, content_hash: row.content_hash, created_at_unix_ms: row.created_at});",
            vec![("rows", struct_list(doomed))],
        )
        .map(|_| ())
    })
    .unwrap_err();

    assert!(matches!(error, RepositoryError::Storage(_)), "{error:?}");
    assert_eq!(
        count(&manager, "MATCH (:FileVersion) RETURN COUNT(*);"),
        1,
        "no row of the failed statement may survive"
    );
}

#[test]
fn counting_helpers_agree_with_what_the_batch_wrote() {
    let database = TestDatabase::new("unwind-count");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let counted = migration::in_transaction(&connection, |connection| {
        run(
            connection,
            "UNWIND $rows AS row CREATE (:FileVersion {id: row.id, file_id: row.file_id, content_hash: row.content_hash, created_at_unix_ms: row.created_at});",
            vec![("rows", struct_list((0..7).map(version_row).collect()))],
        )?;
        fetch_count(
            connection,
            "MATCH (version:FileVersion) RETURN COUNT(version);",
            vec![],
            "file version count",
        )
    })
    .unwrap();

    assert_eq!(counted, 7);
}
