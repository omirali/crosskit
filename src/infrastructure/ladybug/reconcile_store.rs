// Reconciliation runs, the shared sighting epoch and the retirement of unseen locations.
//
// The epoch is one durable counter. Starting a run advances it and the run keeps the new
// value as its `sequence`; every location write, whether from `index init` or from a run,
// stamps the current value as `seen_in_epoch`. A run may therefore retire only locations
// last seen before it started, whoever saw them, and no per-run marks accumulate.
use lbug::{Connection, Value};

use crate::application::{RepositoryError, RetirementReport};
use crate::domain::{NormalizedPath, ReconcileRun};

use super::location_store::file_remains_present_elsewhere;
use super::store_support::{execute_statement, fetch_first_row, storage_error};

pub(super) const RUNNING: &str = "running";
pub(super) const COMPLETED: &str = "completed";
pub(super) const INCOMPLETE: &str = "incomplete";
/// A pass that was attempted and never walked. Unlike `incomplete`, which is a walk that
/// happened and may not retire, this is a walk that did not begin.
pub(super) const FAILED: &str = "failed";
const EPOCH_LOOKUP: &str = "MATCH (epoch:ReconcileEpoch {id: 'current'}) RETURN epoch.sequence;";

/// The epoch to stamp on a location sighting made inside the current transaction.
pub(super) fn current_epoch(connection: &Connection<'_>) -> Result<i64, RepositoryError> {
    let row = fetch_first_row(connection, EPOCH_LOOKUP, vec![], "reconcile epoch lookup")?;
    match row.as_deref() {
        Some([Value::Int64(sequence)]) => Ok(*sequence),
        Some(values) => Err(RepositoryError::InvariantViolation(format!(
            "reconcile epoch has invalid values: {values:?}"
        ))),
        None => Err(RepositoryError::InvariantViolation(
            "reconcile epoch row is missing; schema migration 7 did not complete".to_string(),
        )),
    }
}

/// Advances the epoch and records the run as `running`. Must run inside the caller's
/// transaction, which also serializes the increment.
pub(super) fn begin_run(
    connection: &Connection<'_>,
    root: &NormalizedPath,
    started_at_unix_ms: i64,
) -> Result<ReconcileRun, RepositoryError> {
    let sequence = current_epoch(connection)?.checked_add(1).ok_or_else(|| {
        RepositoryError::InvariantViolation("reconcile epoch exceeds Ladybug INT64".to_string())
    })?;
    execute_statement(
        connection,
        "MATCH (epoch:ReconcileEpoch {id: 'current'}) SET epoch.sequence = $sequence;",
        vec![("sequence", Value::Int64(sequence))],
        "reconcile epoch advance",
    )?;

    let run = ReconcileRun::new(
        format!("reconcile-run:{sequence}"),
        sequence,
        root.clone(),
        started_at_unix_ms,
    )
    .map_err(|error| RepositoryError::InvariantViolation(error.to_string()))?;
    execute_statement(
        connection,
        "CREATE (:ReconcileRun {id: $id, sequence: $sequence, root: $root, status: $status, started_at_unix_ms: $started_at});",
        vec![
            ("id", Value::String(run.id().to_string())),
            ("sequence", Value::Int64(run.sequence())),
            ("root", Value::String(run.root().as_str().to_string())),
            ("status", Value::String(RUNNING.to_string())),
            ("started_at", Value::Int64(run.started_at_unix_ms())),
        ],
        "reconcile run insert",
    )?;
    Ok(run)
}

/// The caller's run must be exactly the persisted run and still open. A run value with a
/// real id but another root or sequence, or a run already closed, must never write.
pub(super) fn require_running_run(
    connection: &Connection<'_>,
    run: &ReconcileRun,
) -> Result<(), RepositoryError> {
    let row = fetch_first_row(
        connection,
        "MATCH (run:ReconcileRun {id: $id}) RETURN run.sequence, run.root, run.status;",
        vec![("id", Value::String(run.id().to_string()))],
        "reconcile run lookup",
    )?;
    match row.as_deref() {
        Some(
            [
                Value::Int64(sequence),
                Value::String(root),
                Value::String(status),
            ],
        ) if *sequence == run.sequence() && root == run.root().as_str() => {
            if status != RUNNING {
                return Err(RepositoryError::InvariantViolation(format!(
                    "reconcile run {} is already {status}",
                    run.id()
                )));
            }
            Ok(())
        }
        Some(values) => Err(RepositoryError::InvariantViolation(format!(
            "reconcile run {} does not match its stored identity: {values:?}",
            run.id()
        ))),
        None => Err(RepositoryError::InvariantViolation(format!(
            "reconcile run {} is unknown",
            run.id()
        ))),
    }
}

/// Retires every `present` location under the run's root that was last seen before the
/// run started and that `is_visible` accepts, classifies each retirement, then completes
/// the run. `is_visible` states which paths the run could inspect: inside its root and
/// not excluded by its policy. It bounds both what may be retired and what may count as
/// a relocation destination. Candidates are read in keyset pages of `page_size` ordered
/// by id, each page materialized before its writes because a query result borrows the
/// connection. Must run inside the caller's transaction, so a failure on any page leaves
/// every status untouched and no report escapes.
pub(super) fn finalize_run(
    connection: &Connection<'_>,
    run: &ReconcileRun,
    finished_at_unix_ms: i64,
    is_visible: &dyn Fn(&NormalizedPath) -> bool,
    page_size: usize,
) -> Result<RetirementReport, RepositoryError> {
    require_running_run(connection, run)?;
    let mut report = RetirementReport::default();
    let mut after_id = String::new();
    loop {
        let page = unseen_present_locations(connection, run, &after_id, page_size)?;
        let Some(last) = page.last() else {
            break;
        };
        after_id = last.id.clone();
        for candidate in page {
            if !is_visible(&candidate.path) {
                continue;
            }
            retire_location(connection, &candidate.id)?;
            let relocated = file_remains_present_elsewhere(
                connection,
                &candidate.file_id,
                &candidate.id,
                run.sequence(),
                is_visible,
                page_size,
            )?;
            report.record(relocated);
        }
    }
    close_run(connection, run, COMPLETED, finished_at_unix_ms)?;
    Ok(report)
}

/// One `present` location the run did not see, as read from a page.
struct UnseenLocation {
    id: String,
    file_id: String,
    path: NormalizedPath,
}

pub(super) fn abandon_run(
    connection: &Connection<'_>,
    run: &ReconcileRun,
    finished_at_unix_ms: i64,
) -> Result<(), RepositoryError> {
    require_running_run(connection, run)?;
    close_run(connection, run, INCOMPLETE, finished_at_unix_ms)
}

/// Scope is a whole-segment boundary: the root itself or anything under `root/`. The
/// filesystem root is the only path whose descendant prefix is already `/`. `page_size`
/// is an internal constant, never user input, so it may be spliced into the statement.
fn unseen_present_locations(
    connection: &Connection<'_>,
    run: &ReconcileRun,
    after_id: &str,
    page_size: usize,
) -> Result<Vec<UnseenLocation>, RepositoryError> {
    let root = run.root().as_str();
    let descendant_prefix = if root == "/" {
        "/".to_string()
    } else {
        format!("{root}/")
    };
    let query = format!(
        "MATCH (location:FileLocation) WHERE location.status = $present AND (location.normalized_path = $root OR starts_with(location.normalized_path, $prefix)) AND location.seen_in_epoch < $sequence AND location.id > $after_id RETURN location.id, location.file_id, location.normalized_path ORDER BY location.id LIMIT {page_size};"
    );
    let mut statement = connection
        .prepare(&query)
        .map_err(|error| storage_error("prepare unseen location page", error))?;
    let mut result = connection
        .execute(
            &mut statement,
            vec![
                ("present", Value::String("present".to_string())),
                ("root", Value::String(root.to_string())),
                ("prefix", Value::String(descendant_prefix)),
                ("sequence", Value::Int64(run.sequence())),
                ("after_id", Value::String(after_id.to_string())),
            ],
        )
        .map_err(|error| storage_error("unseen location page", error))?;

    let mut page = Vec::with_capacity(page_size);
    for row in result.by_ref() {
        match row.as_slice() {
            [
                Value::String(id),
                Value::String(file_id),
                Value::String(path),
            ] => {
                let path = NormalizedPath::from_absolute(path).map_err(|_| {
                    RepositoryError::InvariantViolation(format!(
                        "a file location of {file_id} has an invalid stored path"
                    ))
                })?;
                page.push(UnseenLocation {
                    id: id.clone(),
                    file_id: file_id.clone(),
                    path,
                });
            }
            values => {
                return Err(RepositoryError::InvariantViolation(format!(
                    "unseen location page returned invalid values: {values:?}"
                )));
            }
        }
    }
    Ok(page)
}

fn retire_location(connection: &Connection<'_>, location_id: &str) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (location:FileLocation {id: $id}) SET location.status = $missing;",
        vec![
            ("id", Value::String(location_id.to_string())),
            ("missing", Value::String("missing".to_string())),
        ],
        "file location retirement",
    )
}

/// Writes an attempt on `root` that ended before a walk, in status `failed` and under the
/// category of the failure that stopped it.
///
/// It goes through `begin_run` so that a failed attempt is a run like any other: same table,
/// same id scheme, same ordering. The epoch it advances costs nothing — the epoch bounds
/// what a later run may retire, and a run that retires nothing only leaves a gap in the
/// numbering.
pub(super) fn record_failed_run(
    connection: &Connection<'_>,
    root: &NormalizedPath,
    failure: &str,
    at_unix_ms: i64,
) -> Result<String, RepositoryError> {
    let run = begin_run(connection, root, at_unix_ms)?;
    execute_statement(
        connection,
        "MATCH (run:ReconcileRun {id: $id}) SET run.status = $status, run.failure = $failure, run.finished_at_unix_ms = $finished_at;",
        vec![
            ("id", Value::String(run.id().to_string())),
            ("status", Value::String(FAILED.to_string())),
            ("failure", Value::String(failure.to_string())),
            ("finished_at", Value::Int64(at_unix_ms)),
        ],
        "failed pass record",
    )?;
    Ok(run.id().to_string())
}

fn close_run(
    connection: &Connection<'_>,
    run: &ReconcileRun,
    status: &str,
    finished_at_unix_ms: i64,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (run:ReconcileRun {id: $id}) SET run.status = $status, run.finished_at_unix_ms = $finished_at;",
        vec![
            ("id", Value::String(run.id().to_string())),
            ("status", Value::String(status.to_string())),
            ("finished_at", Value::Int64(finished_at_unix_ms)),
        ],
        "reconcile run close",
    )
}
