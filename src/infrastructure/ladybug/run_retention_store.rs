// Retention of closed reconciliation runs: an explicit maintenance pass, never a step of
// `index reconcile`.
//
// Roots and runs are both read in keyset pages, so a long history never lands in memory
// at once. Per root, `running` runs and closed runs that still carry historical `OBSERVED`
// edges are only counted as protected; the remaining closed runs are walked newest first,
// the first `keep` are retained and the rest removed. Nothing but `ReconcileRun` rows is
// touched: the epoch, locations and evidence stay as they are. Must run inside the
// caller's transaction, so a failure on any page leaves every run in place.
use lbug::{Connection, Value};

use crate::application::RepositoryError;
use crate::domain::{RetentionMode, RunRetention, RunRetentionReport};

use super::reconcile_store::{COMPLETED, INCOMPLETE, RUNNING};
use super::store_support::{execute_statement, fetch_count, fetch_rows};

pub(super) fn prune_closed_runs(
    connection: &Connection<'_>,
    retention: RunRetention,
    mode: RetentionMode,
    page_size: usize,
) -> Result<RunRetentionReport, RepositoryError> {
    let mut report = RunRetentionReport::empty(mode);
    let mut after_root = String::new();
    loop {
        let roots = root_page(connection, &after_root, page_size)?;
        let Some(last) = roots.last() else {
            break;
        };
        after_root = last.clone();
        for root in &roots {
            prune_root(connection, root, retention, mode, page_size, &mut report)?;
        }
    }
    Ok(report)
}

fn prune_root(
    connection: &Connection<'_>,
    root: &str,
    retention: RunRetention,
    mode: RetentionMode,
    page_size: usize,
    report: &mut RunRetentionReport,
) -> Result<(), RepositoryError> {
    report.protect_running(count_running_runs(connection, root)?);
    report.protect_legacy(count_legacy_closed_runs(connection, root)?);

    let mut retained = 0;
    let mut after_sequence = i64::MAX;
    loop {
        let page = removable_runs_page(connection, root, after_sequence, page_size)?;
        let Some(last) = page.last() else {
            break;
        };
        after_sequence = last.sequence;
        for run in page {
            if retained < retention.keep() {
                retained += 1;
                report.retain_run();
                continue;
            }
            if mode == RetentionMode::Apply {
                delete_run(connection, &run.id)?;
            }
            report.remove_run();
        }
    }
    Ok(())
}

/// One removable closed run, as read from a page.
struct RemovableRun {
    id: String,
    sequence: i64,
}

/// `page_size` is an internal constant, never user input, so it may be spliced in.
fn root_page(
    connection: &Connection<'_>,
    after_root: &str,
    page_size: usize,
) -> Result<Vec<String>, RepositoryError> {
    let query = format!(
        "MATCH (run:ReconcileRun) WHERE run.root > $after_root RETURN DISTINCT run.root ORDER BY run.root LIMIT {page_size};"
    );
    let rows = fetch_rows(
        connection,
        &query,
        vec![("after_root", Value::String(after_root.to_string()))],
        "reconcile root page",
    )?;
    rows.into_iter()
        .map(|row| match row.as_slice() {
            [Value::String(root)] => Ok(root.clone()),
            values => Err(RepositoryError::InvariantViolation(format!(
                "reconcile root page returned invalid values: {values:?}"
            ))),
        })
        .collect()
}

/// Closed runs without `OBSERVED` evidence, newest first, older than `after_sequence`.
fn removable_runs_page(
    connection: &Connection<'_>,
    root: &str,
    after_sequence: i64,
    page_size: usize,
) -> Result<Vec<RemovableRun>, RepositoryError> {
    let query = format!(
        "MATCH (run:ReconcileRun {{root: $root}}) WHERE (run.status = $completed OR run.status = $incomplete) AND NOT EXISTS {{ MATCH (run)-[:OBSERVED]->(:FileLocation) }} AND run.sequence < $after_sequence RETURN run.id, run.sequence ORDER BY run.sequence DESC LIMIT {page_size};"
    );
    let rows = fetch_rows(
        connection,
        &query,
        vec![
            ("root", Value::String(root.to_string())),
            ("completed", Value::String(COMPLETED.to_string())),
            ("incomplete", Value::String(INCOMPLETE.to_string())),
            ("after_sequence", Value::Int64(after_sequence)),
        ],
        "removable reconcile run page",
    )?;
    rows.into_iter()
        .map(|row| match row.as_slice() {
            [Value::String(id), Value::Int64(sequence)] => Ok(RemovableRun {
                id: id.clone(),
                sequence: *sequence,
            }),
            values => Err(RepositoryError::InvariantViolation(format!(
                "removable reconcile run page returned invalid values: {values:?}"
            ))),
        })
        .collect()
}

fn count_running_runs(connection: &Connection<'_>, root: &str) -> Result<usize, RepositoryError> {
    fetch_count(
        connection,
        "MATCH (run:ReconcileRun {root: $root, status: $running}) RETURN COUNT(run);",
        vec![
            ("root", Value::String(root.to_string())),
            ("running", Value::String(RUNNING.to_string())),
        ],
        "running reconcile run count",
    )
}

/// Closed runs of the historical v6 model that still point at locations through
/// `OBSERVED`; that evidence is kept as applied, so its runs are never removed.
fn count_legacy_closed_runs(
    connection: &Connection<'_>,
    root: &str,
) -> Result<usize, RepositoryError> {
    fetch_count(
        connection,
        "MATCH (run:ReconcileRun {root: $root}) WHERE (run.status = $completed OR run.status = $incomplete) AND EXISTS { MATCH (run)-[:OBSERVED]->(:FileLocation) } RETURN COUNT(run);",
        vec![
            ("root", Value::String(root.to_string())),
            ("completed", Value::String(COMPLETED.to_string())),
            ("incomplete", Value::String(INCOMPLETE.to_string())),
        ],
        "legacy reconcile run count",
    )
}

/// A plain `DELETE`, never `DETACH DELETE`: Ladybug refuses to remove a node that still
/// has an edge, so even a run wrongly selected here could not take its `OBSERVED`
/// evidence with it; the whole pass would roll back instead.
fn delete_run(connection: &Connection<'_>, run_id: &str) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (run:ReconcileRun {id: $id}) DELETE run;",
        vec![("id", Value::String(run_id.to_string()))],
        "reconcile run removal",
    )
}
