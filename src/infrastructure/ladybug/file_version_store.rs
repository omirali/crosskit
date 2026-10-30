// Immutable FileVersion nodes and the HAS_VERSION link from their File.
use std::collections::HashMap;

use lbug::{Connection, Value};

use crate::application::RepositoryError;
use crate::domain::FileVersion;

use super::batch_projection::VersionRow;
use super::store_support::{
    execute_rows, execute_statement, fetch_first_row, fetch_rows_for_ids, text,
};

/// Persists the immutable version and links it to its File, which the location store has
/// already created for this commit. Must run inside the caller's transaction.
pub(super) fn persist_file_version(
    connection: &Connection<'_>,
    version: &FileVersion,
) -> Result<(), RepositoryError> {
    ensure_file_version(connection, version)?;
    link_file_to_version(connection, version)
}

fn ensure_file_version(
    connection: &Connection<'_>,
    version: &FileVersion,
) -> Result<(), RepositoryError> {
    let stored = fetch_first_row(
        connection,
        "MATCH (version:FileVersion {id: $id}) RETURN version.file_id, version.content_hash, version.created_at_unix_ms;",
        vec![("id", Value::String(version.id().to_string()))],
        "file version lookup",
    )?;
    if let Some(row) = stored {
        return verify_file_version(&row, version);
    }

    execute_statement(
        connection,
        "CREATE (:FileVersion {id: $id, file_id: $file_id, content_hash: $content_hash, created_at_unix_ms: $created_at_unix_ms});",
        vec![
            ("id", Value::String(version.id().to_string())),
            ("file_id", Value::String(version.file_id().to_string())),
            (
                "content_hash",
                Value::String(version.content_hash().to_string()),
            ),
            (
                "created_at_unix_ms",
                Value::Int64(version.created_at_unix_ms()),
            ),
        ],
        "file version insert",
    )
}

/// A stored version is immutable: a retry must carry the same file and content hash.
/// `created_at_unix_ms` records the first write and is allowed to differ on retry.
fn verify_file_version(row: &[Value], version: &FileVersion) -> Result<(), RepositoryError> {
    match row {
        [
            Value::String(file_id),
            Value::String(content_hash),
            Value::Int64(_),
        ] if file_id == version.file_id() && content_hash == version.content_hash() => Ok(()),
        values => Err(RepositoryError::InvariantViolation(format!(
            "file version {} conflicts with immutable stored values: {values:?}",
            version.id()
        ))),
    }
}

fn link_file_to_version(
    connection: &Connection<'_>,
    version: &FileVersion,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (file:File {id: $file_id}), (version:FileVersion {id: $version_id}) MERGE (file)-[:HAS_VERSION]->(version);",
        vec![
            ("file_id", Value::String(version.file_id().to_string())),
            ("version_id", Value::String(version.id().to_string())),
        ],
        "file version link",
    )
}

/// The link of a version that is new to the graph: each endpoint through a `MATCH` of its
/// own and `CREATE` rather than `MERGE`, which scans three node tables and the relationship
/// table even when both ends were handed to it (see `store_support`).
const HAS_VERSION_CREATE: &str = "UNWIND $rows AS row MATCH (file:File {id: row.file_id}) MATCH (version:FileVersion {id: row.id}) CREATE (file)-[:HAS_VERSION]->(version) RETURN COUNT(*);";

/// Persists the immutable versions of one batch and links each to its File. A version
/// already stored is left as it is, after its file and content hash are confirmed to
/// match; `created_at_unix_ms` records the first write and may differ on a retry.
pub(super) fn persist_file_version_batch(
    connection: &Connection<'_>,
    rows: &[VersionRow<'_>],
) -> Result<(), RepositoryError> {
    const CONTEXT: &str = "file version batch lookup";
    let stored = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (version:FileVersion {id: id}) RETURN version.id, version.file_id, version.content_hash;",
        rows.iter().map(|row| row.id.to_string()).collect(),
        CONTEXT,
    )?;
    let mut known = HashMap::with_capacity(stored.len());
    for values in stored {
        known.insert(
            text(&values, 0, CONTEXT)?,
            (text(&values, 1, CONTEXT)?, text(&values, 2, CONTEXT)?),
        );
    }

    // Only the versions this batch creates are inserted and linked. A version already
    // stored was linked by the transaction that wrote it, and `HAS_VERSION` is `ONE_MANY`,
    // so it can neither be missing a link nor gain a second one; re-merging cost a scan of
    // the relationship table for every version of every batch. The rule is in
    // `store_support`.
    let mut inserts = Vec::new();
    let mut links = Vec::new();
    for row in rows {
        if let Some((file_id, content_hash)) = known.get(row.id) {
            if file_id != row.file_id || content_hash != row.content_hash {
                return Err(RepositoryError::InvariantViolation(format!(
                    "file version {} conflicts with immutable stored values",
                    row.id
                )));
            }
            continue;
        }
        inserts.push(Value::Struct(vec![
            ("id".to_string(), Value::String(row.id.to_string())),
            (
                "file_id".to_string(),
                Value::String(row.file_id.to_string()),
            ),
            (
                "content_hash".to_string(),
                Value::String(row.content_hash.to_string()),
            ),
            (
                "created_at_unix_ms".to_string(),
                Value::Int64(row.created_at_unix_ms),
            ),
        ]));
        links.push(Value::Struct(vec![
            ("id".to_string(), Value::String(row.id.to_string())),
            (
                "file_id".to_string(),
                Value::String(row.file_id.to_string()),
            ),
        ]));
    }

    execute_rows(
        connection,
        "UNWIND $rows AS row CREATE (version:FileVersion {id: row.id, file_id: row.file_id, content_hash: row.content_hash, created_at_unix_ms: row.created_at_unix_ms}) RETURN COUNT(version);",
        inserts,
        "file version batch insert",
    )?;
    execute_rows(
        connection,
        HAS_VERSION_CREATE,
        links,
        "file version batch link",
    )
}
