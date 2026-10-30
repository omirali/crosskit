// The physical layer: File identity, FileLocation observations and the LOCATED_AT link.
use std::collections::{HashMap, HashSet};

use lbug::{Connection, Value};

use crate::application::RepositoryError;
use crate::domain::{FileKind, FileLocation, KnownIdentity, LocationStatus, NormalizedPath};

use super::batch_projection::LocationRow;
use super::store_support::{
    execute_rows, execute_rows_unchecked, execute_statement, fetch_first_row, fetch_rows_for_ids,
    number, storage_error, text, u64_to_i64,
};

/// Persists the file identity and the location observation. A location seen before keeps
/// its identity and `first_seen`; size, modification time, `last_seen`, status and the
/// reconciliation epoch of this sighting are refreshed. Any other `present` location at
/// the same path is retired as `missing`, because one path holds one file at a time.
/// Must run inside the caller's transaction.
pub(super) fn persist_location(
    connection: &Connection<'_>,
    location: &FileLocation,
    seen_in_epoch: i64,
) -> Result<(), RepositoryError> {
    ensure_file(
        connection,
        location.file_id(),
        location.first_seen_at_unix_ms(),
    )?;
    let stored = fetch_first_row(
        connection,
        "MATCH (location:FileLocation {id: $id}) RETURN location.file_id, location.normalized_path, location.kind, location.last_seen_at_unix_ms;",
        vec![("id", Value::String(location.id().to_string()))],
        "file location lookup",
    )?;
    match stored {
        Some(row) => {
            let stored_last_seen = verify_location(&row, location)?;
            refresh_location(connection, location, stored_last_seen, seen_in_epoch)?;
        }
        None => insert_location(connection, location, seen_in_epoch)?,
    }
    link_file_to_location(connection, location)?;
    retire_other_locations_at_path(connection, location)
}

/// What the graph holds under the kernel's device-and-inode identity: when it first saw a
/// file carrying it. The scan runs this for every file it meets, so it reaches the file by
/// its primary key and reads one stored number; taking the earliest of the file's locations
/// instead was measured to plan as a scan of the relationship table, which would cost a
/// table scan per source.
pub(super) const KNOWN_IDENTITY: &str =
    "MATCH (file:File {id: $id}) RETURN file.first_seen_at_unix_ms;";

pub(super) fn known_identity(
    connection: &Connection<'_>,
    base_id: &str,
) -> Result<KnownIdentity, RepositoryError> {
    let row = fetch_first_row(
        connection,
        KNOWN_IDENTITY,
        vec![("id", Value::String(base_id.to_string()))],
        "file identity lookup",
    )?;
    match row.as_deref() {
        None => Ok(KnownIdentity::Unseen),
        // Zero is what a File no location ever dated keeps: a database written before
        // locations existed. Nothing places an observed file against it.
        Some([Value::Int64(0)]) => Ok(KnownIdentity::Undated),
        Some([Value::Int64(first_seen_at_unix_ms)]) => {
            Ok(KnownIdentity::FirstSeenAt(*first_seen_at_unix_ms))
        }
        Some(values) => Err(RepositoryError::InvariantViolation(format!(
            "a file identity lookup returned invalid values: {values:?}"
        ))),
    }
}

/// Whether the file identified by `file_id` still has a `present` location other than
/// `location_id` that was confirmed in `epoch` or later and that `is_visible` accepts.
/// Only a sighting made since the caller's run started counts, and only at a path that
/// run could have inspected: a stale `present`, a sibling outside the run's root or one
/// under a path its policy excludes proves nothing, however fresh. Siblings are read in
/// keyset pages so a file with many hard links never lands in memory at once.
pub(super) fn file_remains_present_elsewhere(
    connection: &Connection<'_>,
    file_id: &str,
    location_id: &str,
    epoch: i64,
    is_visible: &dyn Fn(&NormalizedPath) -> bool,
    page_size: usize,
) -> Result<bool, RepositoryError> {
    let mut after_id = String::new();
    loop {
        let page = fresh_present_siblings(
            connection,
            file_id,
            location_id,
            epoch,
            &after_id,
            page_size,
        )?;
        let Some(last) = page.last() else {
            return Ok(false);
        };
        after_id = last.0.clone();
        if page.iter().any(|(_, path)| is_visible(path)) {
            return Ok(true);
        }
    }
}

/// `page_size` is an internal constant, never user input, so it may be spliced in.
fn fresh_present_siblings(
    connection: &Connection<'_>,
    file_id: &str,
    location_id: &str,
    epoch: i64,
    after_id: &str,
    page_size: usize,
) -> Result<Vec<(String, NormalizedPath)>, RepositoryError> {
    let query = format!(
        "MATCH (:File {{id: $file_id}})-[:LOCATED_AT]->(other:FileLocation) WHERE other.id <> $location_id AND other.status = $present AND other.seen_in_epoch >= $epoch AND other.id > $after_id RETURN other.id, other.normalized_path ORDER BY other.id LIMIT {page_size};"
    );
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| storage_error("prepare sibling location page", error))?;
    let mut result = connection
        .execute(
            &mut statement,
            vec![
                ("file_id", Value::String(file_id.to_string())),
                ("location_id", Value::String(location_id.to_string())),
                (
                    "present",
                    Value::String(status_label(LocationStatus::Present).to_string()),
                ),
                ("epoch", Value::Int64(epoch)),
                ("after_id", Value::String(after_id.to_string())),
            ],
        )
        .map_err(|error| storage_error("sibling location page", error))?;

    let mut page = Vec::with_capacity(page_size);
    for row in result.by_ref() {
        match row.as_slice() {
            [Value::String(id), Value::String(path)] => {
                let path = NormalizedPath::from_absolute(path).map_err(|_| {
                    RepositoryError::InvariantViolation(format!(
                        "a file location of {file_id} has an invalid stored path"
                    ))
                })?;
                page.push((id.clone(), path));
            }
            values => {
                return Err(RepositoryError::InvariantViolation(format!(
                    "sibling location page returned invalid values: {values:?}"
                )));
            }
        }
    }
    Ok(page)
}

/// The identity is dated by the first location that ever carried it and never redated: a
/// later sighting of the same file, at a new path or as a second hard link, is still that
/// file. That date is what a newly observed birth time is judged against.
fn ensure_file(
    connection: &Connection<'_>,
    file_id: &str,
    first_seen_at_unix_ms: i64,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MERGE (file:File {id: $file_id}) ON CREATE SET file.first_seen_at_unix_ms = $first_seen_at_unix_ms;",
        vec![
            ("file_id", Value::String(file_id.to_string())),
            ("first_seen_at_unix_ms", Value::Int64(first_seen_at_unix_ms)),
        ],
        "file upsert",
    )
}

/// Identity columns never change for a location id; returns the stored `last_seen` so the
/// refresh can keep the later of the two timestamps.
fn verify_location(row: &[Value], location: &FileLocation) -> Result<i64, RepositoryError> {
    match row {
        [
            Value::String(file_id),
            Value::String(normalized_path),
            Value::String(kind),
            Value::Int64(last_seen),
        ] if file_id == location.file_id()
            && normalized_path == location.path().as_str()
            && kind == kind_label(location.kind()) =>
        {
            Ok(*last_seen)
        }
        _ => Err(location_conflict(location.file_id())),
    }
}

/// A location identity embeds its normalized path, and so do the stored columns, so
/// neither may reach an error a person reads. The file identity is device and inode and
/// names no path.
pub(super) fn location_conflict(file_id: &str) -> RepositoryError {
    RepositoryError::InvariantViolation(format!(
        "a file location of {file_id} conflicts with immutable stored values"
    ))
}

fn insert_location(
    connection: &Connection<'_>,
    location: &FileLocation,
    seen_in_epoch: i64,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "CREATE (:FileLocation {id: $id, file_id: $file_id, normalized_path: $normalized_path, display_name: $display_name, kind: $kind, size_bytes: $size_bytes, modified_at_unix_ms: $modified_at_unix_ms, first_seen_at_unix_ms: $first_seen_at_unix_ms, last_seen_at_unix_ms: $last_seen_at_unix_ms, status: $status, seen_in_epoch: $seen_in_epoch});",
        vec![
            ("seen_in_epoch", Value::Int64(seen_in_epoch)),
            ("id", Value::String(location.id().to_string())),
            ("file_id", Value::String(location.file_id().to_string())),
            (
                "normalized_path",
                Value::String(location.path().as_str().to_string()),
            ),
            (
                "display_name",
                Value::String(location.display_name().to_string()),
            ),
            (
                "kind",
                Value::String(kind_label(location.kind()).to_string()),
            ),
            (
                "size_bytes",
                Value::Int64(u64_to_i64(location.size_bytes(), "location size")?),
            ),
            (
                "modified_at_unix_ms",
                Value::Int64(location.modified_at_unix_ms()),
            ),
            (
                "first_seen_at_unix_ms",
                Value::Int64(location.first_seen_at_unix_ms()),
            ),
            (
                "last_seen_at_unix_ms",
                Value::Int64(location.last_seen_at_unix_ms()),
            ),
            (
                "status",
                Value::String(status_label(location.status()).to_string()),
            ),
        ],
        "file location insert",
    )
}

fn refresh_location(
    connection: &Connection<'_>,
    location: &FileLocation,
    stored_last_seen: i64,
    seen_in_epoch: i64,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (location:FileLocation {id: $id}) SET location.size_bytes = $size_bytes, location.modified_at_unix_ms = $modified_at_unix_ms, location.last_seen_at_unix_ms = $last_seen_at_unix_ms, location.status = $status, location.seen_in_epoch = $seen_in_epoch;",
        vec![
            ("seen_in_epoch", Value::Int64(seen_in_epoch)),
            ("id", Value::String(location.id().to_string())),
            (
                "size_bytes",
                Value::Int64(u64_to_i64(location.size_bytes(), "location size")?),
            ),
            (
                "modified_at_unix_ms",
                Value::Int64(location.modified_at_unix_ms()),
            ),
            (
                "last_seen_at_unix_ms",
                Value::Int64(stored_last_seen.max(location.last_seen_at_unix_ms())),
            ),
            (
                "status",
                Value::String(status_label(location.status()).to_string()),
            ),
        ],
        "file location refresh",
    )
}

fn link_file_to_location(
    connection: &Connection<'_>,
    location: &FileLocation,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (file:File {id: $file_id}), (location:FileLocation {id: $location_id}) MERGE (file)-[:LOCATED_AT]->(location);",
        vec![
            ("file_id", Value::String(location.file_id().to_string())),
            ("location_id", Value::String(location.id().to_string())),
        ],
        "file location link",
    )
}

fn retire_other_locations_at_path(
    connection: &Connection<'_>,
    location: &FileLocation,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (other:FileLocation {normalized_path: $normalized_path}) WHERE other.id <> $id AND other.status = $present SET other.status = $missing;",
        vec![
            (
                "normalized_path",
                Value::String(location.path().as_str().to_string()),
            ),
            ("id", Value::String(location.id().to_string())),
            (
                "present",
                Value::String(status_label(LocationStatus::Present).to_string()),
            ),
            (
                "missing",
                Value::String(status_label(LocationStatus::Missing).to_string()),
            ),
        ],
        "file location retirement",
    )
}

fn kind_label(kind: FileKind) -> &'static str {
    match kind {
        FileKind::RegularFile => "file",
        FileKind::Symlink => "symlink",
        FileKind::Special => "special",
    }
}

pub(super) fn status_label(status: LocationStatus) -> &'static str {
    match status {
        LocationStatus::Present => "present",
        LocationStatus::Missing => "missing",
    }
}

/// The same mapping read back. A label this program did not write is nothing rather than a
/// guess: the two statuses mean different things to a reader, and inventing one would turn
/// a file that may still be there into a claim that it is.
pub(super) fn status_from_label(label: &str) -> Option<LocationStatus> {
    match label {
        "present" => Some(LocationStatus::Present),
        "missing" => Some(LocationStatus::Missing),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------
// Set-based writes for a bounded batch.
// ---------------------------------------------------------------------------------

/// Persists the files and locations of one batch in a fixed number of statements, with
/// the same meaning as writing them one after another: identity fields of a stored
/// location are immutable, `first_seen` belongs to the first sighting, `last_seen` never
/// moves backwards, and any other `present` location at the same path is retired.
pub(super) fn persist_location_batch(
    connection: &Connection<'_>,
    files: &[&str],
    rows: &[LocationRow<'_>],
    seen_in_epoch: i64,
) -> Result<(), RepositoryError> {
    ensure_files(connection, files, rows)?;
    let stored = stored_locations(connection, rows)?;
    let mut inserts = Vec::new();
    let mut updates = Vec::new();
    // Locations that are new to the graph, and locations that are taking a path they did
    // not already hold. Only those two can displace whatever else sits at their path, and
    // only the first needs its link written; everything else is a location this pass has
    // simply seen again. Both lists are almost always empty on a reconcile of an unchanged
    // tree, and both statements below cost a scan, which is the whole point of separating
    // them.
    let mut linked = Vec::new();
    let mut claiming_path = Vec::new();
    let present = status_label(LocationStatus::Present);
    for row in rows {
        match stored.get(row.id) {
            Some(stored) => {
                verify_stored_location(row, stored)?;
                // Arriving at a path it did not hold: the graph records it as gone and
                // this batch publishes it as present again. Stated as the two statuses
                // rather than as "they differ", so a batch that ever published something
                // other than a present location could not turn this into a claim.
                if row.status == present && stored.status != present {
                    claiming_path.push(link_value(row));
                }
                updates.push(update_value(
                    row,
                    stored.last_seen_at_unix_ms.max(row.last_seen_at_unix_ms),
                    seen_in_epoch,
                ));
            }
            None => {
                inserts.push(insert_value(row, seen_in_epoch));
                linked.push(link_value(row));
                claiming_path.push(link_value(row));
            }
        }
    }

    execute_rows(
        connection,
        "UNWIND $rows AS row CREATE (location:FileLocation {id: row.id, file_id: row.file_id, normalized_path: row.normalized_path, display_name: row.display_name, kind: row.kind, size_bytes: row.size_bytes, modified_at_unix_ms: row.modified_at_unix_ms, first_seen_at_unix_ms: row.first_seen_at_unix_ms, last_seen_at_unix_ms: row.last_seen_at_unix_ms, status: row.status, seen_in_epoch: row.seen_in_epoch}) RETURN COUNT(location);",
        inserts,
        "file location batch insert",
    )?;
    execute_rows(
        connection,
        "UNWIND $rows AS row MATCH (location:FileLocation {id: row.id}) SET location.size_bytes = row.size_bytes, location.modified_at_unix_ms = row.modified_at_unix_ms, location.last_seen_at_unix_ms = row.last_seen_at_unix_ms, location.status = row.status, location.seen_in_epoch = row.seen_in_epoch RETURN COUNT(location);",
        updates,
        "file location batch refresh",
    )?;
    execute_rows(
        connection,
        LOCATED_AT_CREATE,
        linked,
        "file location batch link",
    )?;
    retire_other_locations(connection, claiming_path)
}

/// The link of a location that is new to the graph. Each endpoint is reached through a
/// `MATCH` of its own, and the edge is created rather than merged: `MERGE` scans the
/// relationship table even when both ends are already found by key, and that scan grows
/// with every location the graph has ever held (ADR 0006, Stage 2R).
///
/// A location already stored already has this link. The node and its link are written in
/// one transaction, and `LOCATED_AT` is `ONE_MANY`, so a location cannot acquire a second
/// one. Re-merging it on every pass would buy nothing this program can break, and would
/// cost a relationship scan per file per pass — which is what Stage 18 measured.
pub(super) const LOCATED_AT_CREATE: &str = "UNWIND $rows AS row MATCH (file:File {id: row.file_id}) MATCH (location:FileLocation {id: row.id}) CREATE (file)-[:LOCATED_AT]->(location) RETURN COUNT(*);";

/// The one `MERGE` this replaced, kept so the reason for replacing it stays checkable.
#[cfg(test)]
pub(super) const REPLACED_LOCATION_LINK_SHAPE: &str = "UNWIND $rows AS row MATCH (file:File {id: row.file_id}), (location:FileLocation {id: row.id}) MERGE (file)-[:LOCATED_AT]->(location) RETURN COUNT(*);";

/// A file new to the graph is dated by the earliest sighting this batch carries for it; one
/// already there keeps the date it was given, whatever this batch says.
fn ensure_files(
    connection: &Connection<'_>,
    files: &[&str],
    rows: &[LocationRow<'_>],
) -> Result<(), RepositoryError> {
    // `MERGE (file:File {id: row.id})` scans the File table although `id` is its primary
    // key, and that scan was paid for every file of every batch of every pass. Asked by key
    // and created only where the answer was nothing, it scans nothing; the rule is written
    // out in `store_support`.
    const CONTEXT: &str = "file batch lookup";
    let known = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (file:File {id: id}) RETURN file.id;",
        files.iter().map(|file_id| (*file_id).to_string()).collect(),
        CONTEXT,
    )?;
    let mut stored = HashSet::with_capacity(known.len());
    for values in known {
        stored.insert(text(&values, 0, CONTEXT)?);
    }

    let mut earliest: HashMap<&str, i64> = HashMap::with_capacity(files.len());
    for row in rows {
        earliest
            .entry(row.file_id)
            .and_modify(|first_seen| *first_seen = (*first_seen).min(row.first_seen_at_unix_ms))
            .or_insert(row.first_seen_at_unix_ms);
    }
    let mut inserts = Vec::with_capacity(files.len());
    for file_id in files {
        // Every file of a batch comes from one of its locations. A file without one would
        // be dated zero, which reads as "nothing dates it" and would cost the file its
        // history on the next pass, so it is refused instead. Checked for every file of the
        // batch, stored or not: a batch that cannot date a file is malformed either way.
        let first_seen_at_unix_ms = earliest.get(file_id).copied().ok_or_else(|| {
            RepositoryError::InvariantViolation(format!(
                "the batch carries the file {file_id} without a location to date it"
            ))
        })?;
        // A file already in the graph keeps the date it was given, which is exactly what
        // `ON CREATE SET` used to say: it is dated by the first location that ever carried
        // it and never redated.
        if stored.contains(*file_id) {
            continue;
        }
        inserts.push(Value::Struct(vec![
            ("id".to_string(), Value::String((*file_id).to_string())),
            (
                "first_seen_at_unix_ms".to_string(),
                Value::Int64(first_seen_at_unix_ms),
            ),
        ]));
    }
    execute_rows(
        connection,
        "UNWIND $rows AS row CREATE (file:File {id: row.id, first_seen_at_unix_ms: row.first_seen_at_unix_ms}) RETURN COUNT(file);",
        inserts,
        "file batch insert",
    )
}

/// The immutable identity and the recorded `last_seen` of the locations already stored.
struct StoredLocation {
    file_id: String,
    normalized_path: String,
    kind: String,
    last_seen_at_unix_ms: i64,
    /// What the graph already records here. A location that is already `present` at the
    /// path it is being published at cannot have taken that path from anything, which is
    /// what lets the batch skip asking.
    status: String,
}

fn stored_locations(
    connection: &Connection<'_>,
    rows: &[LocationRow<'_>],
) -> Result<HashMap<String, StoredLocation>, RepositoryError> {
    const CONTEXT: &str = "file location batch lookup";
    let found = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (location:FileLocation {id: id}) RETURN location.id, location.file_id, location.normalized_path, location.kind, location.last_seen_at_unix_ms, location.status;",
        rows.iter().map(|row| row.id.to_string()).collect(),
        CONTEXT,
    )?;
    let mut stored = HashMap::with_capacity(found.len());
    for values in found {
        stored.insert(
            text(&values, 0, CONTEXT)?,
            StoredLocation {
                file_id: text(&values, 1, CONTEXT)?,
                normalized_path: text(&values, 2, CONTEXT)?,
                kind: text(&values, 3, CONTEXT)?,
                last_seen_at_unix_ms: number(&values, 4, CONTEXT)?,
                status: text(&values, 5, CONTEXT)?,
            },
        );
    }
    Ok(stored)
}

fn verify_stored_location(
    row: &LocationRow<'_>,
    stored: &StoredLocation,
) -> Result<(), RepositoryError> {
    if stored.file_id == row.file_id
        && stored.normalized_path == row.normalized_path
        && stored.kind == row.kind
    {
        return Ok(());
    }
    Err(location_conflict(row.file_id))
}

/// One path holds one file at a time, so anything else still `present` there is retired.
///
/// `claiming` are the locations of this batch that are arriving at their path: new to the
/// graph, or stored but not `present` until now. A location the graph already records as
/// `present` at this path took it when it arrived and cannot have taken it again, so asking
/// on its behalf would only repeat an answer already acted on — at the price of a node
/// table scan, because a path is not a primary key.
fn retire_other_locations(
    connection: &Connection<'_>,
    claiming: Vec<Value>,
) -> Result<(), RepositoryError> {
    execute_rows_unchecked(
        connection,
        RETIRE_OTHER_LOCATIONS_AT_PATH,
        claiming,
        vec![
            (
                "present",
                Value::String(status_label(LocationStatus::Present).to_string()),
            ),
            (
                "missing",
                Value::String(status_label(LocationStatus::Missing).to_string()),
            ),
        ],
        "file location batch retirement",
    )
}

/// Reaches `FileLocation` by a path, which is not its primary key, so it scans the node
/// table. Only the locations arriving at a path are asked about.
pub(super) const RETIRE_OTHER_LOCATIONS_AT_PATH: &str = "UNWIND $rows AS row MATCH (other:FileLocation {normalized_path: row.normalized_path}) WHERE other.id <> row.id AND other.status = $present SET other.status = $missing;";

fn insert_value(row: &LocationRow<'_>, seen_in_epoch: i64) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.to_string())),
        (
            "file_id".to_string(),
            Value::String(row.file_id.to_string()),
        ),
        (
            "normalized_path".to_string(),
            Value::String(row.normalized_path.to_string()),
        ),
        (
            "display_name".to_string(),
            Value::String(row.display_name.to_string()),
        ),
        ("kind".to_string(), Value::String(row.kind.to_string())),
        ("size_bytes".to_string(), Value::Int64(row.size_bytes)),
        (
            "modified_at_unix_ms".to_string(),
            Value::Int64(row.modified_at_unix_ms),
        ),
        (
            "first_seen_at_unix_ms".to_string(),
            Value::Int64(row.first_seen_at_unix_ms),
        ),
        (
            "last_seen_at_unix_ms".to_string(),
            Value::Int64(row.last_seen_at_unix_ms),
        ),
        ("status".to_string(), Value::String(row.status.to_string())),
        ("seen_in_epoch".to_string(), Value::Int64(seen_in_epoch)),
    ])
}

fn update_value(row: &LocationRow<'_>, last_seen_at_unix_ms: i64, seen_in_epoch: i64) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.to_string())),
        ("size_bytes".to_string(), Value::Int64(row.size_bytes)),
        (
            "modified_at_unix_ms".to_string(),
            Value::Int64(row.modified_at_unix_ms),
        ),
        (
            "last_seen_at_unix_ms".to_string(),
            Value::Int64(last_seen_at_unix_ms),
        ),
        ("status".to_string(), Value::String(row.status.to_string())),
        ("seen_in_epoch".to_string(), Value::Int64(seen_in_epoch)),
    ])
}

fn link_value(row: &LocationRow<'_>) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.to_string())),
        (
            "file_id".to_string(),
            Value::String(row.file_id.to_string()),
        ),
        (
            "normalized_path".to_string(),
            Value::String(row.normalized_path.to_string()),
        ),
    ])
}
