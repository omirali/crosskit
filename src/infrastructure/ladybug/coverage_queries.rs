// Stage 34: what one context's graph holds under one configured root, read and nothing
// else.
//
// It lives inside the storage adapter because that is where a Cypher statement is allowed
// to be written at all, and it is the only part of the coverage audit that needs one. The
// audit itself is `infrastructure::coverage`; this file answers the two questions the audit
// cannot answer from disk — what the graph kept of a root, and whether a pass over that
// root ever ran.
//
// Every root travels as a parameter. A path is untrusted input, and a prefix pasted into a
// statement would be a rule written by the tree rather than by this program.
use lbug::{Connection, Value};

use crate::application::RepositoryError;
use crate::domain::NormalizedPath;

use super::LadybugDatabaseManager;
use super::store_support::{fetch_count, fetch_rows, prepare_error};

/// What the graph of one context holds under one root.
///
/// `files` counts `File` nodes with at least one location under the root, present or not,
/// because a file that moved away still belongs to the root that indexed it. The rest
/// narrows that set: how many of them the graph considers current, and how much text their
/// current versions hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GraphCoverage {
    files: usize,
    files_with_current_version: usize,
    locations_present: usize,
    locations_missing: usize,
    chunks_of_current_versions: usize,
    passes: usize,
}

impl GraphCoverage {
    pub(crate) fn files(&self) -> usize {
        self.files
    }

    pub(crate) fn files_with_current_version(&self) -> usize {
        self.files_with_current_version
    }

    pub(crate) fn locations_present(&self) -> usize {
        self.locations_present
    }

    pub(crate) fn locations_missing(&self) -> usize {
        self.locations_missing
    }

    /// Chunks of the current versions of those files. The search index of a context holds
    /// one document per such chunk, which is the invariant `doctor` and `status` report, so
    /// this is how many documents of the index belong to this root.
    pub(crate) fn chunks_of_current_versions(&self) -> usize {
        self.chunks_of_current_versions
    }

    /// Reconcile runs recorded against exactly this root. Zero means no `index reconcile`
    /// ever ran here; it does not mean no `index init` did, because an initial scan records
    /// no run.
    pub(crate) fn passes(&self) -> usize {
        self.passes
    }
}

/// The prefix a root's descendants share. The separator is part of it, so `/tree/ab` never
/// answers for `/tree/abc`.
fn descendant_prefix(root: &NormalizedPath) -> String {
    match root.as_str() {
        "/" => "/".to_string(),
        path => format!("{path}/"),
    }
}

pub(crate) fn coverage_of_root(
    manager: &LadybugDatabaseManager,
    root: &NormalizedPath,
) -> Result<GraphCoverage, RepositoryError> {
    let connection = connection_to(manager)?;
    let prefix = descendant_prefix(root);
    let parameter = || vec![("prefix", Value::String(prefix.clone()))];

    Ok(GraphCoverage {
        files: fetch_count(
            &connection,
            "MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation) \
             WHERE location.normalized_path STARTS WITH $prefix \
             RETURN COUNT(DISTINCT file);",
            parameter(),
            "count files under a root",
        )?,
        files_with_current_version: fetch_count(
            &connection,
            "MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation) \
             WHERE location.normalized_path STARTS WITH $prefix \
             MATCH (file)-[:CURRENT_VERSION]->(:FileVersion) \
             RETURN COUNT(DISTINCT file);",
            parameter(),
            "count current files under a root",
        )?,
        locations_present: fetch_count(
            &connection,
            "MATCH (:File)-[:LOCATED_AT]->(location:FileLocation) \
             WHERE location.normalized_path STARTS WITH $prefix AND location.status = 'present' \
             RETURN COUNT(*);",
            parameter(),
            "count present locations under a root",
        )?,
        locations_missing: fetch_count(
            &connection,
            "MATCH (:File)-[:LOCATED_AT]->(location:FileLocation) \
             WHERE location.normalized_path STARTS WITH $prefix AND location.status = 'missing' \
             RETURN COUNT(*);",
            parameter(),
            "count missing locations under a root",
        )?,
        chunks_of_current_versions: fetch_count(
            &connection,
            "MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation) \
             WHERE location.normalized_path STARTS WITH $prefix \
             MATCH (file)-[:CURRENT_VERSION]->(:FileVersion)-[:HAS_CHUNK]->(chunk:ContentChunk) \
             RETURN COUNT(DISTINCT chunk);",
            parameter(),
            "count current chunks under a root",
        )?,
        passes: fetch_count(
            &connection,
            "MATCH (run:ReconcileRun) WHERE run.root = $root RETURN COUNT(*);",
            vec![("root", Value::String(root.as_str().to_string()))],
            "count reconcile runs of a root",
        )?,
    })
}

/// How many locations the graph holds under a root at all. Asked before the paths
/// themselves, so a root far larger than the ceiling is reported rather than half-read.
pub(crate) fn locations_under(
    manager: &LadybugDatabaseManager,
    root: &NormalizedPath,
) -> Result<usize, RepositoryError> {
    fetch_count(
        &connection_to(manager)?,
        "MATCH (:File)-[:LOCATED_AT]->(location:FileLocation) \
         WHERE location.normalized_path STARTS WITH $prefix \
         RETURN COUNT(*);",
        vec![("prefix", Value::String(descendant_prefix(root)))],
        "count locations under a root",
    )
}

/// Every path the graph knows under a root, with the status it holds it under. The caller
/// bounds the answer: a query result borrows the connection, so the page is read whole.
pub(crate) fn known_paths_under(
    manager: &LadybugDatabaseManager,
    root: &NormalizedPath,
    ceiling: usize,
) -> Result<Vec<(String, String)>, RepositoryError> {
    let rows = fetch_rows(
        &connection_to(manager)?,
        "MATCH (:File)-[:LOCATED_AT]->(location:FileLocation) \
         WHERE location.normalized_path STARTS WITH $prefix \
         RETURN location.normalized_path, location.status ORDER BY location.normalized_path \
         LIMIT $ceiling;",
        vec![
            ("prefix", Value::String(descendant_prefix(root))),
            (
                "ceiling",
                Value::Int64(i64::try_from(ceiling).unwrap_or(i64::MAX)),
            ),
        ],
        "list known paths under a root",
    )?;
    rows.iter()
        .map(|row| match row.as_slice() {
            [Value::String(path), Value::String(status)] => Ok((path.clone(), status.clone())),
            values => Err(RepositoryError::InvariantViolation(format!(
                "a location row is not a path and a status: {values:?}"
            ))),
        })
        .collect()
}

/// Every `File` of a context, counted whole. The audit subtracts what the configured roots
/// account for: what is left is indexed under a path no context root covers any more.
pub(crate) fn files_in_graph(manager: &LadybugDatabaseManager) -> Result<usize, RepositoryError> {
    fetch_count(
        &connection_to(manager)?,
        "MATCH (:File) RETURN COUNT(*);",
        vec![],
        "count files of a context",
    )
}

/// How many nodes of each kind a context's graph holds.
///
/// Every statement is written out rather than built from a label, because a label cannot be
/// a parameter and a statement assembled from a string is the one thing this program does
/// not do with Cypher. The list is what the coverage audit needs to say where a database's
/// size comes from, and it is fixed at compile time.
pub(crate) fn node_counts(
    manager: &LadybugDatabaseManager,
) -> Result<Vec<(&'static str, usize)>, RepositoryError> {
    const COUNTS: [(&str, &str); 6] = [
        ("File", "MATCH (:File) RETURN COUNT(*);"),
        ("FileVersion", "MATCH (:FileVersion) RETURN COUNT(*);"),
        ("FileLocation", "MATCH (:FileLocation) RETURN COUNT(*);"),
        (
            "FileObservation",
            "MATCH (:FileObservation) RETURN COUNT(*);",
        ),
        ("ContentChunk", "MATCH (:ContentChunk) RETURN COUNT(*);"),
        ("ReconcileRun", "MATCH (:ReconcileRun) RETURN COUNT(*);"),
    ];
    let connection = connection_to(manager)?;
    COUNTS
        .into_iter()
        .map(|(label, query)| {
            fetch_count(&connection, query, vec![], "count nodes of one kind")
                .map(|count| (label, count))
        })
        .collect()
}

/// The roots of every reconcile run the graph recorded, with how many runs each had and the
/// state of the last one. A root that never had a pass is simply absent.
pub(crate) fn recorded_pass_roots(
    manager: &LadybugDatabaseManager,
) -> Result<Vec<(String, usize)>, RepositoryError> {
    let rows = fetch_rows(
        &connection_to(manager)?,
        "MATCH (run:ReconcileRun) RETURN run.root, COUNT(*) ORDER BY run.root;",
        vec![],
        "list reconcile run roots",
    )?;
    rows.iter()
        .map(|row| match row.as_slice() {
            [Value::String(root), Value::Int64(runs)] => {
                Ok((root.clone(), usize::try_from(*runs).unwrap_or(0)))
            }
            values => Err(RepositoryError::InvariantViolation(format!(
                "a reconcile run row is not a root and a count: {values:?}"
            ))),
        })
        .collect()
}

fn connection_to(manager: &LadybugDatabaseManager) -> Result<Connection<'_>, RepositoryError> {
    Connection::new(&manager.owner.database)
        .map_err(|error| prepare_error("open a connection for the coverage audit", error))
}
