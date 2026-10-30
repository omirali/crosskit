// What one root of a context holds, read for a report.
//
// Three questions, and the first two are deliberately asked of the locations rather than of
// the run records: `index init` writes no run, so a root indexed by it would look like a
// root nobody had ever walked. What the graph holds under a path is the fact; a run record
// is only a note about how it got there.
use lbug::{Connection, Value};

use crate::application::{FailedPass, RepositoryError, RootCoverage};
use crate::domain::NormalizedPath;

use super::reconcile_store::FAILED;
use super::store_support::{fetch_count, fetch_first_row};

/// The prefix a root's descendants share. The separator is part of it, so `/tree/ab` never
/// answers for `/tree/abc`.
fn descendant_prefix(root: &NormalizedPath) -> String {
    match root.as_str() {
        "/" => "/".to_string(),
        path => format!("{path}/"),
    }
}

pub(super) fn coverage_of_root(
    connection: &Connection<'_>,
    root: &NormalizedPath,
) -> Result<RootCoverage, RepositoryError> {
    let prefix = Value::String(descendant_prefix(root));
    let files = fetch_count(
        connection,
        "MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation) \
         WHERE location.normalized_path STARTS WITH $prefix \
         RETURN COUNT(DISTINCT file);",
        vec![("prefix", prefix.clone())],
        "files under a root",
    )?;
    Ok(RootCoverage::new(
        files as u64,
        last_seen_under(connection, prefix)?,
        last_failed_pass(connection, root)?,
    ))
}

/// When a pass last confirmed anything under this root.
fn last_seen_under(
    connection: &Connection<'_>,
    prefix: Value,
) -> Result<Option<i64>, RepositoryError> {
    let row = fetch_first_row(
        connection,
        "MATCH (:File)-[:LOCATED_AT]->(location:FileLocation) \
         WHERE location.normalized_path STARTS WITH $prefix \
         RETURN MAX(location.last_seen_at_unix_ms);",
        vec![("prefix", prefix)],
        "last sighting under a root",
    )?;
    match row.as_deref() {
        Some([Value::Int64(at)]) => Ok(Some(*at)),
        // No location under the root: the aggregate has nothing to take a maximum of.
        Some([Value::Null(_)]) | None => Ok(None),
        Some(values) => Err(RepositoryError::InvariantViolation(format!(
            "last sighting under a root returned invalid values: {values:?}"
        ))),
    }
}

/// The newest attempt on exactly this root that ended without a walk.
fn last_failed_pass(
    connection: &Connection<'_>,
    root: &NormalizedPath,
) -> Result<Option<FailedPass>, RepositoryError> {
    let row = fetch_first_row(
        connection,
        "MATCH (run:ReconcileRun) WHERE run.root = $root AND run.status = $failed \
         RETURN run.failure, run.started_at_unix_ms ORDER BY run.sequence DESC LIMIT 1;",
        vec![
            ("root", Value::String(root.as_str().to_string())),
            ("failed", Value::String(FAILED.to_string())),
        ],
        "last failed pass of a root",
    )?;
    match row.as_deref() {
        Some([Value::String(category), Value::Int64(at)]) => {
            Ok(Some(FailedPass::new(category.clone(), *at)))
        }
        None => Ok(None),
        Some(values) => Err(RepositoryError::InvariantViolation(format!(
            "failed pass row is not a category and a time: {values:?}"
        ))),
    }
}
