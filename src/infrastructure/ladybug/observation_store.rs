// Immutable FileObservation facts, their durable ObservationOrder and the links
// HAS_OBSERVATION, OBSERVES and ORDERS.
use std::collections::HashMap;

use lbug::{Connection, Value};

use crate::application::RepositoryError;
use crate::domain::FileObservation;

use super::batch_projection::ObservationRow;
use super::store_support::{
    execute_ids, execute_rows, execute_statement, fetch_first_row, fetch_rows_for_ids, number, text,
};

/// Strict order of a persisted observation. The derived ordering compares
/// `observed_at_unix_ms` first and `sequence` second, so observations recorded within
/// the same millisecond are still totally ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ObservationOrderKey {
    observed_at_unix_ms: i64,
    sequence: i64,
}

impl ObservationOrderKey {
    pub(super) fn new(observed_at_unix_ms: i64, sequence: i64) -> Self {
        Self {
            observed_at_unix_ms,
            sequence,
        }
    }
}

/// Persists the observation with its order and links, returning the durable order key.
/// A retry of an already stored observation reuses its sequence instead of allocating a
/// new one, so the order key of an observation never changes. Must run inside the
/// caller's transaction.
pub(super) fn persist_observation(
    connection: &Connection<'_>,
    observation: &FileObservation,
) -> Result<ObservationOrderKey, RepositoryError> {
    ensure_observation(connection, observation)?;
    let sequence = ensure_observation_order(connection, observation)?;
    link_file_to_observation(connection, observation)?;
    link_observation_to_version(connection, observation)?;
    link_order_to_observation(connection, observation)?;
    Ok(ObservationOrderKey::new(
        observation.observed_at_unix_ms(),
        sequence,
    ))
}

fn ensure_observation(
    connection: &Connection<'_>,
    observation: &FileObservation,
) -> Result<(), RepositoryError> {
    let stored = fetch_first_row(
        connection,
        "MATCH (observation:FileObservation {id: $id}) RETURN observation.file_id, observation.file_version_id, observation.observed_at_unix_ms;",
        vec![("id", Value::String(observation.id().to_string()))],
        "file observation lookup",
    )?;
    if let Some(row) = stored {
        return verify_observation(&row, observation);
    }

    execute_statement(
        connection,
        "CREATE (:FileObservation {id: $id, file_id: $file_id, file_version_id: $version_id, observed_at_unix_ms: $observed_at});",
        vec![
            ("id", Value::String(observation.id().to_string())),
            ("file_id", Value::String(observation.file_id().to_string())),
            (
                "version_id",
                Value::String(observation.file_version_id().to_string()),
            ),
            (
                "observed_at",
                Value::Int64(observation.observed_at_unix_ms()),
            ),
        ],
        "file observation insert",
    )
}

/// A stored observation is an immutable fact: file, version and timestamp must all match.
fn verify_observation(row: &[Value], observation: &FileObservation) -> Result<(), RepositoryError> {
    match row {
        [
            Value::String(file_id),
            Value::String(version_id),
            Value::Int64(observed_at),
        ] if file_id == observation.file_id()
            && version_id == observation.file_version_id()
            && *observed_at == observation.observed_at_unix_ms() =>
        {
            Ok(())
        }
        values => Err(RepositoryError::InvariantViolation(format!(
            "file observation {} conflicts with immutable stored values: {values:?}",
            observation.id()
        ))),
    }
}

/// Returns the durable sequence of the observation, allocating one only when the
/// observation is stored for the first time.
fn ensure_observation_order(
    connection: &Connection<'_>,
    observation: &FileObservation,
) -> Result<i64, RepositoryError> {
    let stored = fetch_first_row(
        connection,
        "MATCH (observation_order:ObservationOrder {id: $id}) RETURN observation_order.file_id, observation_order.observed_at_unix_ms, observation_order.sequence;",
        vec![("id", Value::String(observation.id().to_string()))],
        "observation order lookup",
    )?;
    if let Some(row) = stored {
        return verify_observation_order(&row, observation);
    }

    let sequence = next_observation_sequence(connection)?;
    execute_statement(
        connection,
        "CREATE (:ObservationOrder {id: $id, file_id: $file_id, observed_at_unix_ms: $observed_at, sequence: $sequence});",
        vec![
            ("id", Value::String(observation.id().to_string())),
            ("file_id", Value::String(observation.file_id().to_string())),
            (
                "observed_at",
                Value::Int64(observation.observed_at_unix_ms()),
            ),
            ("sequence", Value::Int64(sequence)),
        ],
        "observation order insert",
    )?;
    Ok(sequence)
}

fn verify_observation_order(
    row: &[Value],
    observation: &FileObservation,
) -> Result<i64, RepositoryError> {
    match row {
        [
            Value::String(file_id),
            Value::Int64(observed_at),
            Value::Int64(stored_sequence),
        ] if file_id == observation.file_id()
            && *observed_at == observation.observed_at_unix_ms() =>
        {
            Ok(*stored_sequence)
        }
        values => Err(RepositoryError::InvariantViolation(format!(
            "observation order {} conflicts with immutable stored values: {values:?}",
            observation.id()
        ))),
    }
}

/// The sequence is derived from the stored maximum, so it survives process restarts and
/// keeps growing across reopened databases.
fn next_observation_sequence(connection: &Connection<'_>) -> Result<i64, RepositoryError> {
    let row = fetch_first_row(
        connection,
        "MATCH (observation_order:ObservationOrder) RETURN MAX(observation_order.sequence);",
        vec![],
        "observation sequence lookup",
    )?;
    match row.as_deref() {
        None | Some([Value::Null(_)]) => Ok(0),
        Some([Value::Int64(sequence)]) => sequence.checked_add(1).ok_or_else(|| {
            RepositoryError::InvariantViolation(
                "observation sequence exceeds Ladybug INT64".to_string(),
            )
        }),
        Some(values) => Err(RepositoryError::InvariantViolation(format!(
            "observation sequence lookup returned invalid values: {values:?}"
        ))),
    }
}

fn link_file_to_observation(
    connection: &Connection<'_>,
    observation: &FileObservation,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (file:File {id: $file_id}), (observation:FileObservation {id: $observation_id}) MERGE (file)-[:HAS_OBSERVATION]->(observation);",
        vec![
            ("file_id", Value::String(observation.file_id().to_string())),
            (
                "observation_id",
                Value::String(observation.id().to_string()),
            ),
        ],
        "file observation link",
    )
}

fn link_observation_to_version(
    connection: &Connection<'_>,
    observation: &FileObservation,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (observation:FileObservation {id: $observation_id}), (version:FileVersion {id: $version_id}) MERGE (observation)-[:OBSERVES]->(version);",
        vec![
            (
                "observation_id",
                Value::String(observation.id().to_string()),
            ),
            (
                "version_id",
                Value::String(observation.file_version_id().to_string()),
            ),
        ],
        "observation version link",
    )
}

fn link_order_to_observation(
    connection: &Connection<'_>,
    observation: &FileObservation,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (observation_order:ObservationOrder {id: $id}), (observation:FileObservation {id: $id}) MERGE (observation_order)-[:ORDERS]->(observation);",
        vec![("id", Value::String(observation.id().to_string()))],
        "observation order link",
    )
}

/// Persists the observations of one batch with their durable order, and reports the order
/// key of every observation so the current state can be decided without reading it back.
///
/// A retried observation keeps the sequence it was first given. The observations that are
/// new to the database receive one contiguous range, allocated once from the stored
/// maximum inside this transaction and handed out strictly by the order the sources were
/// read: the same sequences a one-by-one run would have produced.
pub(super) fn persist_observation_batch(
    connection: &Connection<'_>,
    rows: &[ObservationRow<'_>],
) -> Result<HashMap<String, ObservationOrderKey>, RepositoryError> {
    let stored = stored_observations(connection, rows)?;
    let stored_orders = stored_observation_orders(connection, rows)?;
    let mut observation_inserts = Vec::new();
    let mut order_inserts = Vec::new();
    let mut keys = HashMap::with_capacity(rows.len());
    let mut next_sequence: Option<i64> = None;
    // An observation and its order are written with their links in one transaction, so a
    // row whose observation and order are both new cannot have a link yet: its links are
    // created outright, by primary key. Anything else has been here before, or is here
    // only in part, and takes the repairing path below.
    let mut created: Vec<String> = Vec::new();
    let mut repaired: Vec<&ObservationRow<'_>> = Vec::new();
    // Sequences are handed out strictly by the index of the commit each observation came
    // from, whatever order the projection happened to collect them in.
    let mut ordered: Vec<&ObservationRow<'_>> = rows.iter().collect();
    ordered.sort_by_key(|row| row.input_index);

    for row in ordered {
        let new_observation = match stored.get(row.id) {
            Some(stored) => {
                verify_stored_observation(row, stored)?;
                false
            }
            None => {
                observation_inserts.push(observation_value(row));
                true
            }
        };

        let mut new_order = false;
        let sequence = match stored_orders.get(row.id) {
            Some(stored) => {
                verify_stored_order(row, stored)?;
                stored.sequence
            }
            None => {
                new_order = true;
                let base = match next_sequence {
                    Some(next) => next,
                    None => next_observation_sequence(connection)?,
                };
                next_sequence = Some(base.checked_add(1).ok_or_else(|| {
                    RepositoryError::InvariantViolation(
                        "observation sequence exceeds Ladybug INT64".to_string(),
                    )
                })?);
                order_inserts.push(order_value(row, base));
                base
            }
        };
        keys.insert(
            row.id.to_string(),
            ObservationOrderKey::new(row.observed_at_unix_ms, sequence),
        );
        if new_observation && new_order {
            created.push(row.id.to_string());
        } else {
            repaired.push(row);
        }
    }

    execute_rows(
        connection,
        "UNWIND $rows AS row CREATE (observation:FileObservation {id: row.id, file_id: row.file_id, file_version_id: row.file_version_id, observed_at_unix_ms: row.observed_at_unix_ms}) RETURN COUNT(observation);",
        observation_inserts,
        "file observation batch insert",
    )?;
    execute_rows(
        connection,
        "UNWIND $rows AS row CREATE (observation_order:ObservationOrder {id: row.id, file_id: row.file_id, observed_at_unix_ms: row.observed_at_unix_ms, sequence: row.sequence}) RETURN COUNT(observation_order);",
        order_inserts,
        "observation order batch insert",
    )?;
    create_links(connection, created)?;
    repair_links(connection, &repaired)?;
    Ok(keys)
}

/// Creates the three links of observations that are new to the database. Both endpoints of
/// every link are reached by primary key, which keeps the cost of a batch independent of
/// how much history the database already holds. A link that somehow existed would make
/// `CREATE` fail and roll the batch back, so this can never quietly duplicate one.
fn create_links(connection: &Connection<'_>, ids: Vec<String>) -> Result<(), RepositoryError> {
    for (context, query) in NEW_LINK_STATEMENTS {
        execute_ids(connection, query, ids.clone(), vec![], context)?;
    }
    Ok(())
}

/// The statements that link an observation that is new to the database, named so the
/// capability tests can hold the plan of exactly what runs here. Each one reaches its two
/// endpoints through separate `MATCH` clauses: written as one comma-separated pattern the
/// planner joins two table scans instead of looking both up by primary key.
pub(super) const NEW_LINK_STATEMENTS: [(&str, &str); 3] = [
    (
        "file observation batch link",
        "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}) MATCH (file:File {id: observation.file_id}) CREATE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);",
    ),
    (
        "observation version batch link",
        "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}) MATCH (version:FileVersion {id: observation.file_version_id}) CREATE (observation)-[:OBSERVES]->(version) RETURN COUNT(*);",
    ),
    (
        "observation order batch link",
        "UNWIND $ids AS id MATCH (observation_order:ObservationOrder {id: id}) MATCH (observation:FileObservation {id: id}) CREATE (observation_order)-[:ORDERS]->(observation) RETURN COUNT(*);",
    ),
];

/// Restores whatever is missing for an observation that has been written before. A stored
/// observation normally has all three links already, because they were written in the same
/// transaction as the observation itself; a link can only be missing if something outside
/// this program removed it. `MERGE` therefore repairs rather than refuses, which is the
/// contract this store has always had. It costs more than the creating path, and only
/// rows that were here before pay it.
fn repair_links(
    connection: &Connection<'_>,
    rows: &[&ObservationRow<'_>],
) -> Result<(), RepositoryError> {
    if rows.is_empty() {
        return Ok(());
    }
    let owned: Vec<ObservationRow<'_>> = rows
        .iter()
        .map(|row| ObservationRow {
            input_index: row.input_index,
            id: row.id,
            file_id: row.file_id,
            file_version_id: row.file_version_id,
            observed_at_unix_ms: row.observed_at_unix_ms,
        })
        .collect();
    link_observation_batch(connection, &owned)
}

struct StoredObservation {
    file_id: String,
    file_version_id: String,
    observed_at_unix_ms: i64,
}

struct StoredOrder {
    file_id: String,
    observed_at_unix_ms: i64,
    sequence: i64,
}

fn stored_observations(
    connection: &Connection<'_>,
    rows: &[ObservationRow<'_>],
) -> Result<HashMap<String, StoredObservation>, RepositoryError> {
    const CONTEXT: &str = "file observation batch lookup";
    let found = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (observation:FileObservation {id: id}) RETURN observation.id, observation.file_id, observation.file_version_id, observation.observed_at_unix_ms;",
        rows.iter().map(|row| row.id.to_string()).collect(),
        CONTEXT,
    )?;
    let mut stored = HashMap::with_capacity(found.len());
    for values in found {
        stored.insert(
            text(&values, 0, CONTEXT)?,
            StoredObservation {
                file_id: text(&values, 1, CONTEXT)?,
                file_version_id: text(&values, 2, CONTEXT)?,
                observed_at_unix_ms: number(&values, 3, CONTEXT)?,
            },
        );
    }
    Ok(stored)
}

fn stored_observation_orders(
    connection: &Connection<'_>,
    rows: &[ObservationRow<'_>],
) -> Result<HashMap<String, StoredOrder>, RepositoryError> {
    const CONTEXT: &str = "observation order batch lookup";
    let found = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (observation_order:ObservationOrder {id: id}) RETURN observation_order.id, observation_order.file_id, observation_order.observed_at_unix_ms, observation_order.sequence;",
        rows.iter().map(|row| row.id.to_string()).collect(),
        CONTEXT,
    )?;
    let mut stored = HashMap::with_capacity(found.len());
    for values in found {
        stored.insert(
            text(&values, 0, CONTEXT)?,
            StoredOrder {
                file_id: text(&values, 1, CONTEXT)?,
                observed_at_unix_ms: number(&values, 2, CONTEXT)?,
                sequence: number(&values, 3, CONTEXT)?,
            },
        );
    }
    Ok(stored)
}

fn verify_stored_observation(
    row: &ObservationRow<'_>,
    stored: &StoredObservation,
) -> Result<(), RepositoryError> {
    if stored.file_id == row.file_id
        && stored.file_version_id == row.file_version_id
        && stored.observed_at_unix_ms == row.observed_at_unix_ms
    {
        return Ok(());
    }
    Err(RepositoryError::InvariantViolation(format!(
        "file observation {} conflicts with immutable stored values",
        row.id
    )))
}

fn verify_stored_order(
    row: &ObservationRow<'_>,
    stored: &StoredOrder,
) -> Result<(), RepositoryError> {
    if stored.file_id == row.file_id && stored.observed_at_unix_ms == row.observed_at_unix_ms {
        return Ok(());
    }
    Err(RepositoryError::InvariantViolation(format!(
        "observation order {} conflicts with immutable stored values",
        row.id
    )))
}

fn link_observation_batch(
    connection: &Connection<'_>,
    rows: &[ObservationRow<'_>],
) -> Result<(), RepositoryError> {
    let links: Vec<Value> = rows.iter().map(observation_value).collect();
    execute_rows(
        connection,
        "UNWIND $rows AS row MATCH (file:File {id: row.file_id}), (observation:FileObservation {id: row.id}) MERGE (file)-[:HAS_OBSERVATION]->(observation) RETURN COUNT(*);",
        links.clone(),
        "file observation batch link",
    )?;
    execute_rows(
        connection,
        "UNWIND $rows AS row MATCH (observation:FileObservation {id: row.id}), (version:FileVersion {id: row.file_version_id}) MERGE (observation)-[:OBSERVES]->(version) RETURN COUNT(*);",
        links.clone(),
        "observation version batch link",
    )?;
    execute_rows(
        connection,
        "UNWIND $rows AS row MATCH (observation_order:ObservationOrder {id: row.id}), (observation:FileObservation {id: row.id}) MERGE (observation_order)-[:ORDERS]->(observation) RETURN COUNT(*);",
        links,
        "observation order batch link",
    )
}

fn observation_value(row: &ObservationRow<'_>) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.to_string())),
        (
            "file_id".to_string(),
            Value::String(row.file_id.to_string()),
        ),
        (
            "file_version_id".to_string(),
            Value::String(row.file_version_id.to_string()),
        ),
        (
            "observed_at_unix_ms".to_string(),
            Value::Int64(row.observed_at_unix_ms),
        ),
    ])
}

fn order_value(row: &ObservationRow<'_>, sequence: i64) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.to_string())),
        (
            "file_id".to_string(),
            Value::String(row.file_id.to_string()),
        ),
        (
            "observed_at_unix_ms".to_string(),
            Value::Int64(row.observed_at_unix_ms),
        ),
        ("sequence".to_string(), Value::Int64(sequence)),
    ])
}
