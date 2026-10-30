// CURRENT_VERSION and CURRENT_OBSERVATION pointers of a file, replaced together.
use std::collections::HashMap;

use lbug::{Connection, Value};

use crate::application::RepositoryError;
use crate::domain::{FileObservation, FileVersion};

use super::current_state_statements::{
    CURRENT_ORDER_LOOKUP, REMOVE_CURRENT_OBSERVATION, REMOVE_CURRENT_OBSERVATION_FOR_FILE,
    REMOVE_CURRENT_VERSION, REMOVE_CURRENT_VERSION_FOR_FILE, SET_CURRENT_OBSERVATION,
    SET_CURRENT_OBSERVATION_FOR_FILE, SET_CURRENT_VERSION, SET_CURRENT_VERSION_FOR_FILE,
};
use super::observation_store::ObservationOrderKey;
use super::store_support::{
    execute_rows, execute_rows_unchecked, execute_statement, fetch_first_row, fetch_rows_for_ids,
    number, text,
};

/// Moves both current pointers of the file to `version` and `observation` when the
/// candidate is strictly newer than the stored current observation. An older or retried
/// observation leaves the current state untouched, so a late retry never rolls it back.
/// Both pointers are replaced inside the caller's transaction and never disagree.
pub(super) fn advance_current_state(
    connection: &Connection<'_>,
    version: &FileVersion,
    observation: &FileObservation,
    candidate: ObservationOrderKey,
) -> Result<(), RepositoryError> {
    let file_id = version.file_id();
    let is_newer =
        current_order_key(connection, file_id)?.is_none_or(|current| candidate > current);
    if !is_newer {
        return Ok(());
    }

    remove_current_pointers(connection, file_id)?;
    set_current_pointers(connection, version, observation)
}

fn current_order_key(
    connection: &Connection<'_>,
    file_id: &str,
) -> Result<Option<ObservationOrderKey>, RepositoryError> {
    let row = fetch_first_row(
        connection,
        "MATCH (:File {id: $file_id})-[:CURRENT_OBSERVATION]->(:FileObservation)<-[:ORDERS]-(observation_order:ObservationOrder) RETURN observation_order.observed_at_unix_ms, observation_order.sequence;",
        vec![("file_id", Value::String(file_id.to_string()))],
        "current observation order lookup",
    )?;
    match row.as_deref() {
        None => Ok(None),
        Some([Value::Int64(observed_at), Value::Int64(sequence)]) => {
            Ok(Some(ObservationOrderKey::new(*observed_at, *sequence)))
        }
        Some(values) => Err(RepositoryError::InvariantViolation(format!(
            "current version for file {file_id} has invalid ordering data: {values:?}"
        ))),
    }
}

/// Both relationships are ONE_ONE, so the previous pointers must go before new ones
/// are created.
fn remove_current_pointers(
    connection: &Connection<'_>,
    file_id: &str,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        REMOVE_CURRENT_VERSION_FOR_FILE,
        vec![("file_id", Value::String(file_id.to_string()))],
        "current version removal",
    )?;
    execute_statement(
        connection,
        REMOVE_CURRENT_OBSERVATION_FOR_FILE,
        vec![("file_id", Value::String(file_id.to_string()))],
        "current observation removal",
    )
}

fn set_current_pointers(
    connection: &Connection<'_>,
    version: &FileVersion,
    observation: &FileObservation,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        SET_CURRENT_VERSION_FOR_FILE,
        vec![
            ("file_id", Value::String(version.file_id().to_string())),
            ("version_id", Value::String(version.id().to_string())),
        ],
        "current version set",
    )?;
    execute_statement(
        connection,
        SET_CURRENT_OBSERVATION_FOR_FILE,
        vec![
            ("file_id", Value::String(observation.file_id().to_string())),
            (
                "observation_id",
                Value::String(observation.id().to_string()),
            ),
        ],
        "current observation set",
    )
}

/// The observation a batch proposes as the current state of one file.
pub(super) struct CurrentCandidate {
    pub(super) file_id: String,
    pub(super) version_id: String,
    pub(super) observation_id: String,
    pub(super) key: ObservationOrderKey,
}

/// Moves both current pointers of every file whose candidate is strictly newer than the
/// stored current observation, and leaves the rest alone. One candidate per file, already
/// chosen by `(observed_at_unix_ms, sequence)`, so the outcome is the one a one-by-one run
/// would reach.
pub(super) fn advance_current_state_batch(
    connection: &Connection<'_>,
    candidates: &[CurrentCandidate],
) -> Result<(), RepositoryError> {
    let current = current_order_keys(connection, candidates)?;
    let winners: Vec<&CurrentCandidate> = candidates
        .iter()
        .filter(|candidate| {
            current
                .get(&candidate.file_id)
                .is_none_or(|stored| candidate.key > *stored)
        })
        .collect();
    if winners.is_empty() {
        return Ok(());
    }

    let files: Vec<Value> = winners
        .iter()
        .map(|candidate| {
            Value::Struct(vec![(
                "file_id".to_string(),
                Value::String(candidate.file_id.clone()),
            )])
        })
        .collect();
    // Both relationships are ONE_ONE, so the previous pointers must go first.
    execute_rows_unchecked(
        connection,
        REMOVE_CURRENT_VERSION,
        files.clone(),
        vec![],
        "current version batch removal",
    )?;
    execute_rows_unchecked(
        connection,
        REMOVE_CURRENT_OBSERVATION,
        files,
        vec![],
        "current observation batch removal",
    )?;

    let pointers: Vec<Value> = winners
        .iter()
        .map(|candidate| {
            Value::Struct(vec![
                (
                    "file_id".to_string(),
                    Value::String(candidate.file_id.clone()),
                ),
                (
                    "version_id".to_string(),
                    Value::String(candidate.version_id.clone()),
                ),
                (
                    "observation_id".to_string(),
                    Value::String(candidate.observation_id.clone()),
                ),
            ])
        })
        .collect();
    execute_rows(
        connection,
        SET_CURRENT_VERSION,
        pointers.clone(),
        "current version batch set",
    )?;
    execute_rows(
        connection,
        SET_CURRENT_OBSERVATION,
        pointers,
        "current observation batch set",
    )
}

fn current_order_keys(
    connection: &Connection<'_>,
    candidates: &[CurrentCandidate],
) -> Result<HashMap<String, ObservationOrderKey>, RepositoryError> {
    const CONTEXT: &str = "current observation order batch lookup";
    let found = fetch_rows_for_ids(
        connection,
        CURRENT_ORDER_LOOKUP,
        candidates
            .iter()
            .map(|candidate| candidate.file_id.clone())
            .collect(),
        CONTEXT,
    )?;
    let mut current = HashMap::with_capacity(found.len());
    for values in found {
        current.insert(
            text(&values, 0, CONTEXT)?,
            ObservationOrderKey::new(number(&values, 1, CONTEXT)?, number(&values, 2, CONTEXT)?),
        );
    }
    Ok(current)
}
