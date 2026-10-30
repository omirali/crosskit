// The insight outbox: one queued InsightJob per immutable FileVersion, linked through
// REQUESTS_INSIGHT, written in the same transaction as the revision it enriches.
use std::collections::HashMap;

use lbug::{Connection, Value};

use crate::application::InventoriedRevision;
use crate::application::RepositoryError;
use crate::domain::{FileVersion, InsightJob, InsightJobState, NormalizedPath};

use super::batch_projection::JobRow;
use super::store_support::{
    execute_ids, execute_rows, execute_statement, fetch_first_row, fetch_rows, fetch_rows_for_ids,
    number, text, u64_to_i64,
};

/// Persists the job unless it already exists, then links it to its revision. An existing
/// job is left exactly as it is: its state belongs to the worker that will own it and its
/// place in the queue was fixed when it was created, so a retry of the same revision must
/// reset neither. Its identity fields must agree with the derived job, otherwise the whole
/// commit rolls back. Must run inside the caller's transaction.
///
/// A new job takes its place in the queue from the revision as the graph holds it, not
/// from the one the caller is carrying: see `revisions_observed_at`.
///
/// One state is the exception, and it is the exception for a reason that does not weaken
/// the rule. `skipped` means the policy stopped asking — no attempt was made, no result was
/// produced, nothing was spent — and this function runs only because a pass published the
/// revision again, which it does only for a file the policy admits. So the condition that
/// retired the job is exactly the condition that has just been reversed, observed rather
/// than assumed. Every other state stands for work: resetting it would throw work away or
/// tear a job out of a worker's hands.
pub(super) fn persist_insight_job(
    connection: &Connection<'_>,
    job: &InsightJob,
) -> Result<(), RepositoryError> {
    let stored = fetch_first_row(
        connection,
        "MATCH (job:InsightJob {id: $id}) RETURN job.source_revision_id, job.input_content_hash, job.pipeline_version, job.state;",
        vec![("id", Value::String(job.id().to_string()))],
        "insight job lookup",
    )?;
    match stored {
        Some(row) => {
            verify_insight_job(&row, job)?;
            if text(&row, 3, "insight job lookup")? == InsightJobState::Skipped.label() {
                requeue_skipped(connection, vec![job.id().to_string()])?;
            }
        }
        None => {
            let observed_at =
                revisions_observed_at(connection, vec![job.source_revision_id().to_string()])?
                    .remove(job.source_revision_id())
                    .ok_or_else(|| {
                        RepositoryError::InvariantViolation(format!(
                            "insight job {} names a revision the graph does not hold",
                            job.id()
                        ))
                    })?;
            insert_insight_job(connection, job, observed_at)?
        }
    }
    link_version_to_job(connection, job)
}

/// Puts jobs the policy had withdrawn back in the queue, now that the policy admits their
/// files again. Matched on the state as well as the id, so a job that moved on between the
/// read and the write is not dragged backwards.
///
/// Both write paths — one revision at a time when an approved mask reaches back over
/// revisions that already exist, a batch at a time for a pass — go through this one
/// function. There are two of them, and a rule written into one is a rule half applied.
fn requeue_skipped(
    connection: &Connection<'_>,
    job_ids: Vec<String>,
) -> Result<(), RepositoryError> {
    if job_ids.is_empty() {
        return Ok(());
    }
    execute_ids(
        connection,
        "UNWIND $ids AS id MATCH (job:InsightJob {id: id, state: $from}) SET job.state = $to RETURN COUNT(job);",
        job_ids,
        vec![
            (
                "from",
                Value::String(InsightJobState::Skipped.label().to_string()),
            ),
            (
                "to",
                Value::String(InsightJobState::Queued.label().to_string()),
            ),
        ],
        "insight job requeue",
    )
}

/// Every existing revision gets its queued job, in keyset pages by revision id so the
/// graph is never held in memory at once. Revisions that already have their job keep it
/// untouched, which makes a retried migration idempotent. Must run inside the caller's
/// transaction, so the schema marker only appears together with the complete backfill.
///
/// It writes the outbox row as version 8 shaped it, rather than through the live store:
/// a migration writes the schema of its own moment, and this one runs on a database that
/// has not yet reached the rename of migration 13. The column it fills holds the
/// revision's own moment either way, and migration 13 rewrites every job's from the
/// revision again.
pub(super) fn backfill_insight_jobs(
    connection: &Connection<'_>,
    page_size: usize,
) -> Result<(), RepositoryError> {
    let mut after_id = String::new();
    loop {
        let page = file_version_page(connection, &after_id, page_size)?;
        let Some(last) = page.last() else {
            return Ok(());
        };
        after_id = last.id().to_string();
        for version in &page {
            let job = InsightJob::queued_for(version);
            match fetch_first_row(
                connection,
                "MATCH (job:InsightJob {id: $id}) RETURN job.source_revision_id, job.input_content_hash, job.pipeline_version, job.state;",
                vec![("id", Value::String(job.id().to_string()))],
                "insight job lookup",
            )? {
                Some(row) => verify_insight_job(&row, &job)?,
                None => insert_v8_insight_job(connection, &job, version.created_at_unix_ms())?,
            }
            link_version_to_job(connection, &job)?;
        }
    }
}

/// The insert of migration 8, kept at that migration's column name. See
/// `backfill_insight_jobs` for why it is not the store's insert.
fn insert_v8_insight_job(
    connection: &Connection<'_>,
    job: &InsightJob,
    created_at_unix_ms: i64,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "CREATE (:InsightJob {id: $id, source_revision_id: $source_revision_id, input_content_hash: $input_content_hash, pipeline_version: $pipeline_version, state: $state, created_at_unix_ms: $created_at});",
        vec![
            ("id", Value::String(job.id().to_string())),
            (
                "source_revision_id",
                Value::String(job.source_revision_id().to_string()),
            ),
            (
                "input_content_hash",
                Value::String(job.input_content_hash().to_string()),
            ),
            (
                "pipeline_version",
                Value::String(job.pipeline_version().to_string()),
            ),
            ("state", Value::String(job.state().label().to_string())),
            ("created_at", Value::Int64(created_at_unix_ms)),
        ],
        "insight job insert",
    )
}

/// `page_size` is an internal constant, never user input, so it may be spliced in.
fn file_version_page(
    connection: &Connection<'_>,
    after_id: &str,
    page_size: usize,
) -> Result<Vec<FileVersion>, RepositoryError> {
    let query = format!(
        "MATCH (version:FileVersion) WHERE version.id > $after_id RETURN version.id, version.file_id, version.content_hash, version.created_at_unix_ms ORDER BY version.id LIMIT {page_size};"
    );
    let rows = fetch_rows(
        connection,
        &query,
        vec![("after_id", Value::String(after_id.to_string()))],
        "file version page",
    )?;
    rows.into_iter()
        .map(|row| match row.as_slice() {
            [
                Value::String(id),
                Value::String(file_id),
                Value::String(content_hash),
                Value::Int64(created_at),
            ] => FileVersion::new(
                id.clone(),
                file_id.clone(),
                content_hash.clone(),
                *created_at,
            )
            .map_err(|error| {
                RepositoryError::InvariantViolation(format!(
                    "file version {id} cannot be reconstructed for its insight job: {error}"
                ))
            }),
            values => Err(RepositoryError::InvariantViolation(format!(
                "file version page returned invalid values: {values:?}"
            ))),
        })
        .collect()
}

/// The identity of a stored job is immutable; state and creation time are not compared
/// because they are owned by the outbox, not by the commit that meets the job again.
/// The stored row carries the job's state last, which is not one of its immutable fields:
/// the state is the worker's, and it is read here only so a withdrawn job can be put back.
fn verify_insight_job(row: &[Value], job: &InsightJob) -> Result<(), RepositoryError> {
    match row {
        [
            Value::String(source_revision_id),
            Value::String(input_content_hash),
            Value::String(pipeline_version),
            Value::String(_state),
        ] if source_revision_id == job.source_revision_id()
            && input_content_hash == job.input_content_hash()
            && pipeline_version == job.pipeline_version() =>
        {
            Ok(())
        }
        values => Err(RepositoryError::InvariantViolation(format!(
            "insight job {} conflicts with immutable stored values: {values:?}",
            job.id()
        ))),
    }
}

fn insert_insight_job(
    connection: &Connection<'_>,
    job: &InsightJob,
    revision_observed_at_unix_ms: i64,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "CREATE (:InsightJob {id: $id, source_revision_id: $source_revision_id, input_content_hash: $input_content_hash, pipeline_version: $pipeline_version, state: $state, revision_observed_at_unix_ms: $revision_observed_at});",
        vec![
            ("id", Value::String(job.id().to_string())),
            (
                "source_revision_id",
                Value::String(job.source_revision_id().to_string()),
            ),
            (
                "input_content_hash",
                Value::String(job.input_content_hash().to_string()),
            ),
            (
                "pipeline_version",
                Value::String(job.pipeline_version().to_string()),
            ),
            ("state", Value::String(job.state().label().to_string())),
            (
                "revision_observed_at",
                Value::Int64(revision_observed_at_unix_ms),
            ),
        ],
        "insight job insert",
    )
}

/// When each named revision was first observed, as the graph holds it.
///
/// This is the one place the queue's order comes from. A revision is identified by its
/// content, so a pass that re-reads an unchanged file builds the same revision stamped
/// with the clock of *that* read; the stored node keeps the first write and is the only
/// record of when the revision was actually observed. A job copies that moment once, when
/// it is created, and the claim orders by the copy — ordering by joining `FileVersion`
/// instead was measured and plans as a full scan of every revision the graph holds, on
/// every claimed page (ADR 0030).
///
/// One primary-key lookup per revision and no scan, per the rule in `store_support`. The
/// revision is always there: a job is written in the same transaction as its revision and
/// after it, and a `FileVersion` is never deleted.
fn revisions_observed_at(
    connection: &Connection<'_>,
    revision_ids: Vec<String>,
) -> Result<HashMap<String, i64>, RepositoryError> {
    const CONTEXT: &str = "revision observed-at lookup";
    let rows = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (version:FileVersion {id: id}) RETURN version.id, version.created_at_unix_ms;",
        revision_ids,
        CONTEXT,
    )?;
    let mut observed = HashMap::with_capacity(rows.len());
    for row in &rows {
        observed.insert(text(row, 0, CONTEXT)?, number(row, 1, CONTEXT)?);
    }
    Ok(observed)
}

fn link_version_to_job(
    connection: &Connection<'_>,
    job: &InsightJob,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (version:FileVersion {id: $version_id}), (job:InsightJob {id: $job_id}) MERGE (version)-[:REQUESTS_INSIGHT]->(job);",
        vec![
            (
                "version_id",
                Value::String(job.source_revision_id().to_string()),
            ),
            ("job_id", Value::String(job.id().to_string())),
        ],
        "insight job link",
    )
}

/// The link of a job that is new to the graph. `MERGE` here scans three node tables and the
/// relationship table even with both endpoints handed to it, and a job is created once and
/// never again, so the link is created with the node (see `store_support`).
const REQUESTS_INSIGHT_CREATE: &str = "UNWIND $rows AS row MATCH (version:FileVersion {id: row.source_revision_id}) MATCH (job:InsightJob {id: row.id}) CREATE (version)-[:REQUESTS_INSIGHT]->(job) RETURN COUNT(*);";

/// Persists the outbox entries of one batch and links each to its revision. A job that
/// already exists is left exactly as it is: its state belongs to the worker that will own
/// it, and its place in the queue was fixed when it was created, so a replayed batch must
/// reset neither.
pub(super) fn persist_insight_job_batch(
    connection: &Connection<'_>,
    rows: &[JobRow<'_>],
) -> Result<(), RepositoryError> {
    const CONTEXT: &str = "insight job batch lookup";
    let found = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (job:InsightJob {id: id}) RETURN job.id, job.source_revision_id, job.input_content_hash, job.pipeline_version, job.state;",
        rows.iter().map(|row| row.id.clone()).collect(),
        CONTEXT,
    )?;
    let mut stored = HashMap::with_capacity(found.len());
    for values in found {
        stored.insert(
            text(&values, 0, CONTEXT)?,
            (
                text(&values, 1, CONTEXT)?,
                text(&values, 2, CONTEXT)?,
                text(&values, 3, CONTEXT)?,
                text(&values, 4, CONTEXT)?,
            ),
        );
    }

    // Only the jobs this batch creates are inserted and linked: `REQUESTS_INSIGHT` is
    // `ONE_MANY` and a job is written with its link in one transaction, so a stored job
    // already has exactly one.
    let mut new_rows = Vec::new();
    let mut returning = Vec::new();
    for row in rows {
        if let Some((source_revision_id, input_content_hash, pipeline_version, state)) =
            stored.get(&row.id)
        {
            if source_revision_id != row.source_revision_id
                || input_content_hash != row.input_content_hash
                || pipeline_version != &row.pipeline_version
            {
                return Err(RepositoryError::InvariantViolation(format!(
                    "insight job {} conflicts with immutable stored values",
                    row.id
                )));
            }
            // This batch is publishing the revision again, which happens only for a file
            // the policy admits — so a job the policy had withdrawn is asked for once more.
            if state == InsightJobState::Skipped.label() {
                returning.push(row.id.clone());
            }
            continue;
        }
        new_rows.push(row);
    }

    // One lookup for the whole batch, and none at all when the batch creates no job —
    // which is every pass over a tree that did not change.
    let observed = revisions_observed_at(
        connection,
        new_rows
            .iter()
            .map(|row| row.source_revision_id.to_string())
            .collect(),
    )?;

    let mut inserts = Vec::with_capacity(new_rows.len());
    let mut links = Vec::with_capacity(new_rows.len());
    for row in new_rows {
        let observed_at = observed.get(row.source_revision_id).ok_or_else(|| {
            RepositoryError::InvariantViolation(format!(
                "insight job {} names a revision the graph does not hold",
                row.id
            ))
        })?;
        inserts.push(Value::Struct(vec![
            ("id".to_string(), Value::String(row.id.clone())),
            (
                "source_revision_id".to_string(),
                Value::String(row.source_revision_id.to_string()),
            ),
            (
                "input_content_hash".to_string(),
                Value::String(row.input_content_hash.to_string()),
            ),
            (
                "pipeline_version".to_string(),
                Value::String(row.pipeline_version.clone()),
            ),
            ("state".to_string(), Value::String(row.state.to_string())),
            (
                "revision_observed_at_unix_ms".to_string(),
                Value::Int64(*observed_at),
            ),
        ]));
        links.push(Value::Struct(vec![
            ("id".to_string(), Value::String(row.id.clone())),
            (
                "source_revision_id".to_string(),
                Value::String(row.source_revision_id.to_string()),
            ),
        ]));
    }

    execute_rows(
        connection,
        "UNWIND $rows AS row CREATE (job:InsightJob {id: row.id, source_revision_id: row.source_revision_id, input_content_hash: row.input_content_hash, pipeline_version: row.pipeline_version, state: row.state, revision_observed_at_unix_ms: row.revision_observed_at_unix_ms}) RETURN COUNT(job);",
        inserts,
        "insight job batch insert",
    )?;
    execute_rows(
        connection,
        REQUESTS_INSIGHT_CREATE,
        links,
        "insight job batch link",
    )?;
    requeue_skipped(connection, returning)
}

/// Current revisions with the path their file is present at, in keyset pages by revision id.
/// Bounded by `LIMIT`, so no statement here materializes the graph.
const CURRENT_REVISION_PAGE: &str = "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion) MATCH (file)-[:LOCATED_AT]->(location:FileLocation {status: 'present'}) WHERE version.id > $after RETURN version.id, location.normalized_path ORDER BY version.id LIMIT $limit;";

pub(super) fn current_revision_page(
    connection: &Connection<'_>,
    after: &str,
    limit: usize,
) -> Result<Vec<InventoriedRevision>, RepositoryError> {
    const CONTEXT: &str = "current revision page";
    let rows = fetch_rows(
        connection,
        CURRENT_REVISION_PAGE,
        vec![
            ("after", Value::String(after.to_string())),
            (
                "limit",
                Value::Int64(u64_to_i64(limit as u64, "revision page limit")?),
            ),
        ],
        CONTEXT,
    )?;
    let mut revisions = Vec::with_capacity(rows.len());
    for row in &rows {
        let path = NormalizedPath::from_absolute(&text(row, 1, CONTEXT)?)
            .map_err(|error| RepositoryError::Storage(format!("{CONTEXT}: {error}")))?;
        revisions.push(InventoriedRevision::new(text(row, 0, CONTEXT)?, path));
    }
    Ok(revisions)
}

/// The versions among `revision_ids` that have no outbox entry, as the jobs they would get.
///
/// Read before writing rather than written with a conflict clause, because a job that
/// already exists belongs to whatever worked it: an approval is not a reason to reset a
/// finished job, and stage 21 already gave the one state that comes back on its own.
pub(super) fn queue_missing_insight_jobs(
    connection: &Connection<'_>,
    revision_ids: &[String],
) -> Result<usize, RepositoryError> {
    const CONTEXT: &str = "backfill revisions without a job";
    if revision_ids.is_empty() {
        return Ok(0);
    }
    let rows = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (version:FileVersion {id: id}) WHERE NOT EXISTS { MATCH (version)-[:REQUESTS_INSIGHT]->(:InsightJob) } RETURN version.id, version.file_id, version.content_hash, version.created_at_unix_ms;",
        revision_ids.to_vec(),
        CONTEXT,
    )?;
    let mut queued = 0;
    for row in &rows {
        let version = FileVersion::new(
            text(row, 0, CONTEXT)?,
            text(row, 1, CONTEXT)?,
            text(row, 2, CONTEXT)?,
            number(row, 3, CONTEXT)?,
        )
        .map_err(|error| RepositoryError::Storage(format!("{CONTEXT}: {error}")))?;
        persist_insight_job(connection, &InsightJob::queued_for(&version))?;
        queued += 1;
    }
    Ok(queued)
}
