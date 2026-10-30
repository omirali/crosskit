// The reads behind `index jobs run --under` and `--touching`: which current revisions one
// directory selects, and how many of their jobs are waiting.
use lbug::{Connection, LogicalType, Value};

use crate::application::{RepositoryError, Requeued, RevisionSize};
use crate::domain::{HeldJob, NormalizedPath};

use super::store_support::{
    execute_ids, execute_statement, fetch_count, fetch_rows, fetch_rows_for_ids, number, text,
};

/// The current revision of every file with a present location under a prefix. A scan of the
/// locations by a string predicate: it reads the locations once, which is what a pass reads
/// every time, and it runs once per command.
const CURRENT_REVISIONS_UNDER: &str = "MATCH (file:File)-[:LOCATED_AT]->(location:FileLocation {status: 'present'}) WHERE location.normalized_path STARTS WITH $prefix MATCH (file)-[:CURRENT_VERSION]->(version:FileVersion) RETURN DISTINCT version.id;";

/// The current revision of the transcript of each session: a session is stored again at every
/// revision of its transcript, and only the current one is worth asking about.
const CURRENT_REVISIONS_OF_SESSIONS: &str = "UNWIND $ids AS session MATCH (conversation:Conversation {session_id: session}) MATCH (version:FileVersion {id: conversation.source_revision_id}) MATCH (:File {id: version.file_id})-[:CURRENT_VERSION]->(version) RETURN DISTINCT version.id;";

/// Queued jobs whose revision has more parts than a run may send unasked, oldest first.
const QUEUED_OVER_PARTS: &str = concat!(
    "MATCH (job:InsightJob {state: 'queued'}) MATCH (version:FileVersion {id: job.source_revision_id})-[:HAS_CHUNK]->(chunk:ContentChunk) WITH job, COUNT(chunk) AS parts, SUM(",
    chunk_bytes!(),
    ") AS bytes WHERE parts > $most RETURN job.id, parts, bytes, job.revision_observed_at_unix_ms ORDER BY job.revision_observed_at_unix_ms, job.id;"
);

const EXISTING_AMONG: &str = "UNWIND $ids AS id MATCH (job:InsightJob {id: id}) RETURN COUNT(job);";
const REVISION_SIZES: &str = concat!(
    "UNWIND $ids AS id MATCH (version:FileVersion {id: id}) OPTIONAL MATCH (version)-[:HAS_CHUNK]->(chunk:ContentChunk) RETURN version.id, COUNT(chunk), SUM(",
    chunk_bytes!(),
    ");"
);
/// Analysed jobs of current revisions a model answered under another question.
const ASKED_OTHERWISE: &str = "MATCH (job:InsightJob {state: 'done'})-[:HAS_INSIGHT_RESULT]->(result:InsightResult) WHERE result.produced_by = $described AND result.prompt_version <> $current MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion {id: job.source_revision_id}) RETURN job.id, job.source_revision_id;";
const FORGET_RESULTS: &str = "UNWIND $ids AS id MATCH (:InsightJob {id: id})-[:HAS_INSIGHT_RESULT]->(result:InsightResult) DETACH DELETE result;";
const BACK_TO_QUEUE: &str = "UNWIND $ids AS id MATCH (job:InsightJob {id: id, state: 'done'}) SET job.state = 'queued', job.attempts = 0, job.claimed_at_unix_ms = 0, job.lease_expires_at_unix_ms = 0, job.last_error_category = '' RETURN COUNT(job);";

const QUEUED_AMONG: &str =
    "UNWIND $ids AS id MATCH (job:InsightJob {id: id, state: 'queued'}) RETURN COUNT(job);";

/// The directory and the separator after it, so `/work/reg` does not select `/work/registry`.
pub(super) fn prefix_of(directory: &NormalizedPath) -> String {
    let path = directory.as_str();
    match path.ends_with('/') {
        true => path.to_string(),
        false => format!("{path}/"),
    }
}

pub(super) fn current_revisions_under(
    connection: &Connection<'_>,
    directory: &NormalizedPath,
) -> Result<Vec<String>, RepositoryError> {
    const CONTEXT: &str = "current revisions under a directory";
    fetch_rows(
        connection,
        CURRENT_REVISIONS_UNDER,
        vec![("prefix", Value::String(prefix_of(directory)))],
        CONTEXT,
    )?
    .iter()
    .map(|row| text(row, 0, CONTEXT))
    .collect()
}

pub(super) fn current_revisions_of_sessions(
    connection: &Connection<'_>,
    sessions: &[String],
) -> Result<Vec<String>, RepositoryError> {
    const CONTEXT: &str = "current revisions of conversations";
    fetch_rows_for_ids(
        connection,
        CURRENT_REVISIONS_OF_SESSIONS,
        sessions.to_vec(),
        CONTEXT,
    )?
    .iter()
    .map(|row| text(row, 0, CONTEXT))
    .collect()
}

pub(super) fn queued_over_parts(
    connection: &Connection<'_>,
    most: usize,
) -> Result<Vec<HeldJob>, RepositoryError> {
    const CONTEXT: &str = "queued jobs over the part limit";
    fetch_rows(
        connection,
        QUEUED_OVER_PARTS,
        vec![(
            "most",
            Value::Int64(i64::try_from(most).unwrap_or(i64::MAX)),
        )],
        CONTEXT,
    )?
    .iter()
    .map(|row| {
        let bytes = summed(row.get(2));
        Ok(HeldJob::new(
            text(row, 0, CONTEXT)?,
            usize::try_from(number(row, 1, CONTEXT)?).unwrap_or(0),
            bytes,
        ))
    })
    .collect()
}

pub(super) fn existing_among(
    connection: &Connection<'_>,
    job_ids: &[String],
) -> Result<usize, RepositoryError> {
    if job_ids.is_empty() {
        return Ok(0);
    }
    fetch_count(
        connection,
        EXISTING_AMONG,
        vec![("ids", ids_value(job_ids))],
        "existing jobs among",
    )
}

/// Parts and bytes of each revision; the paths are read by the knowledge sources, one place.
pub(super) fn revision_sizes(
    connection: &Connection<'_>,
    revision_ids: &[String],
) -> Result<Vec<RevisionSize>, RepositoryError> {
    const CONTEXT: &str = "revision sizes";
    let sources = super::knowledge_store::revision_sources(connection, revision_ids)?;
    fetch_rows_for_ids(connection, REVISION_SIZES, revision_ids.to_vec(), CONTEXT)?
        .iter()
        .map(|row| {
            let revision_id = text(row, 0, CONTEXT)?;
            let present_paths = sources
                .iter()
                .find(|source| source.revision_id == revision_id)
                .map(|source| source.present_paths.clone())
                .unwrap_or_default();
            Ok(RevisionSize {
                revision_id,
                present_paths,
                parts: usize::try_from(number(row, 1, CONTEXT)?).unwrap_or(0),
                bytes: summed(row.get(2)),
            })
        })
        .collect()
}

/// Puts the jobs a model answered under another question back in the queue, in the caller's
/// transaction, and says how much asking them again sends.
pub(super) fn requeue_asked_otherwise(
    connection: &Connection<'_>,
    described_by: &str,
    current_prompt: &str,
    among: Option<&[String]>,
) -> Result<Requeued, RepositoryError> {
    const CONTEXT: &str = "jobs answered under another question";
    let mut jobs = Vec::new();
    let mut revisions = Vec::new();
    for row in fetch_rows(
        connection,
        ASKED_OTHERWISE,
        vec![
            ("described", Value::String(described_by.to_string())),
            ("current", Value::String(current_prompt.to_string())),
        ],
        CONTEXT,
    )? {
        let job = text(&row, 0, CONTEXT)?;
        if among.is_some_and(|ids| !ids.contains(&job)) {
            continue;
        }
        jobs.push(job);
        revisions.push(text(&row, 1, CONTEXT)?);
    }
    if jobs.is_empty() {
        return Ok(Requeued::default());
    }
    let sizes = revision_sizes(connection, &revisions)?;
    execute_statement(
        connection,
        FORGET_RESULTS,
        vec![("ids", ids_value(&jobs))],
        "forget results answered under another question",
    )?;
    execute_ids(
        connection,
        BACK_TO_QUEUE,
        jobs.clone(),
        vec![],
        "jobs back to the queue",
    )?;
    Ok(Requeued {
        jobs: jobs.len(),
        parts: sizes.iter().map(|size| size.parts).sum(),
        bytes: sizes.iter().map(|size| size.bytes).sum(),
    })
}

fn ids_value(ids: &[String]) -> Value {
    Value::List(
        LogicalType::String,
        ids.iter().cloned().map(Value::String).collect(),
    )
}

/// A `SUM` the engine widens to INT128, and answers with nothing over no rows.
fn summed(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Int128(sum)) => i64::try_from(*sum).unwrap_or(i64::MAX),
        Some(Value::Int64(sum)) => *sum,
        _ => 0,
    }
}

pub(super) fn queued_among(
    connection: &Connection<'_>,
    job_ids: &[String],
) -> Result<usize, RepositoryError> {
    if job_ids.is_empty() {
        return Ok(0);
    }
    let ids = Value::List(
        LogicalType::String,
        job_ids.iter().cloned().map(Value::String).collect(),
    );
    fetch_count(
        connection,
        QUEUED_AMONG,
        vec![("ids", ids)],
        "queued jobs among",
    )
}
