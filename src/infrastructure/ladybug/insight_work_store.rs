// The worker's half of the insight outbox: claiming a job under a lease, recording what it
// produced, and freeing what no worker holds any more.
//
// Every function here runs inside the caller's transaction, so each transition is whole or
// absent. Every read is bounded by a `LIMIT`, so no statement materializes the queue. The
// state a job is in is part of the pattern that moves it, so a transition that no longer
// applies changes nothing and is reported rather than silently skipped.
//
// Nothing here reads a file name or chunk text, and nothing here decides anything about a
// path: a job names a revision and a content hash, and a failure names a category. A claim
// does read the paths its revision's file is still present at, because the caller cannot
// ask the policy about a job without them — the decision stays with the caller.
use std::collections::HashMap;

use lbug::{Connection, LogicalType, Value};

use crate::application::{InsightQueueSummary, JobScope, RepositoryError};
use crate::domain::{
    ClaimedInsightJob, FailureDisposition, InsightFailureCategory, InsightJobState, InsightLease,
    InsightRecovery, InsightRecoveryOutcome, InsightResult, InsightRetryPolicy, NormalizedPath,
    QueuedInsightJob,
};

use super::knowledge_store;
use super::store_support::{
    execute_ids, execute_statement, fetch_count, fetch_rows, fetch_rows_for_ids, number, text,
    u64_to_i64,
};

/// How many jobs one recovery statement moves at a time. Internal, so recovering a queue
/// that a long outage filled never holds more than this many ids at once.
const RECOVERY_PAGE: usize = 500;

/// One count per state. States with no job are simply absent from the grouped result, so
/// the summary starts at zero and only the states that exist are filled in.
pub(super) fn count_jobs_by_state(
    connection: &Connection<'_>,
) -> Result<InsightQueueSummary, RepositoryError> {
    const CONTEXT: &str = "insight job state count";
    let rows = fetch_rows(
        connection,
        "MATCH (job:InsightJob) RETURN job.state, COUNT(job);",
        vec![],
        CONTEXT,
    )?;
    let mut summary = InsightQueueSummary::default();
    for row in rows {
        let label = text(&row, 0, CONTEXT)?;
        let state = InsightJobState::from_label(&label).ok_or_else(|| {
            RepositoryError::InvariantViolation(format!(
                "{CONTEXT} found jobs in a state this program never wrote"
            ))
        })?;
        let count = number(&row, 1, CONTEXT)?;
        summary.set(
            state,
            usize::try_from(count).map_err(|_| {
                RepositoryError::InvariantViolation(format!("{CONTEXT} is negative: {count}"))
            })?,
        );
    }
    Ok(summary)
}

/// Frees every job no worker holds: a claim whose lease ran out and a job that failed. Each
/// goes back in the queue, or is retired when `policy` says its attempts are spent. Both
/// sweeps walk in pages until nothing is left, which terminates because every page leaves
/// the state it was read from.
pub(super) fn recover_jobs(
    connection: &Connection<'_>,
    policy: InsightRetryPolicy,
    now_unix_ms: i64,
) -> Result<InsightRecovery, RepositoryError> {
    let mut recovery = InsightRecovery::default();
    // A claim that ran out says nothing about whether the job is sound, so only the attempt
    // count decides.
    sweep(
        connection,
        &mut recovery,
        |connection| {
            Ok(expired_claim_page(connection, now_unix_ms)?
                .into_iter()
                .map(|(id, attempts)| (id, policy.after(attempts)))
                .collect())
        },
        release_expired_claim,
    )?;
    // A job that failed says exactly why, and a reason another attempt cannot change is
    // retired now rather than after spending the attempts to reach the same end.
    sweep(
        connection,
        &mut recovery,
        |connection| {
            Ok(failed_job_page(connection)?
                .into_iter()
                .map(|(id, attempts, category)| {
                    let outcome = match category.disposition() {
                        FailureDisposition::Permanent => InsightRecoveryOutcome::Retire,
                        FailureDisposition::Retry => policy.after(attempts),
                    };
                    (id, outcome)
                })
                .collect())
        },
        release_failed_job,
    )
    .map(|()| recovery)
}

/// One page of the queue, oldest revision first. Bounded by `LIMIT`, so the engine keeps a
/// heap of that many rows and nothing here ever holds the queue.
pub(super) const CLAIM_PAGE_BY_OLDEST_REVISION: &str = "MATCH (job:InsightJob {state: $state}) RETURN job.id, job.source_revision_id, job.input_content_hash ORDER BY job.revision_observed_at_unix_ms, job.id LIMIT $limit;";

/// The same page among chosen jobs: each reached by its primary key, so the scope costs what
/// it holds and not what the queue holds, and the order is the queue's own.
/// The page of the whole queue but the jobs held back.
const CLAIM_PAGE_EXCEPT: &str = "MATCH (job:InsightJob {state: $state}) WHERE NOT list_contains($ids, job.id) RETURN job.id, job.source_revision_id, job.input_content_hash ORDER BY job.revision_observed_at_unix_ms, job.id LIMIT $limit;";
const CLAIM_PAGE_AMONG: &str = "UNWIND $ids AS id MATCH (job:InsightJob {id: id, state: $state}) RETURN job.id, job.source_revision_id, job.input_content_hash ORDER BY job.revision_observed_at_unix_ms, job.id LIMIT $limit;";

/// The same page read by joining each job to its revision instead of by the moment the job
/// copied from it. Kept so the reason it was not chosen stays checkable rather than
/// remembered: the planner answers it with a full scan of `FileVersion` and a hash join, on
/// every claimed page, which is the shape stage 18 took off the write path. A test asserts
/// it still scans; if the planner ever stops, the choice can be revisited rather than
/// silently inherited.
#[cfg(test)]
pub(super) const CLAIM_PAGE_BY_JOINED_REVISION: &str = "MATCH (job:InsightJob {state: $state}) MATCH (version:FileVersion {id: job.source_revision_id}) RETURN job.id, job.source_revision_id, job.input_content_hash ORDER BY version.created_at_unix_ms, job.id LIMIT $limit;";

/// Claims at most `limit` queued jobs, **oldest revision first**, and holds each for
/// `lease`.
///
/// The order is by when the revision was observed, not by when its row in the queue was
/// written. The two differ whenever work is asked for after the fact: approving an
/// analysis mask queues jobs today for revisions seen weeks ago, and those are legitimately
/// older than a document first seen this morning. The queue reflects the age of the work,
/// not the accident of when somebody finished writing the rule that asked for it. The
/// owner reads the queue a portion at a time, so this decides what is seen today and what
/// in a week. ADR 0030.
///
/// `revision_observed_at_unix_ms` is the job's copy of that moment, taken once when the job
/// was created; ordering by joining `FileVersion` here was measured and plans as a full
/// scan of every revision the graph holds, on every page. Ties go to the job id, so two
/// runs over one queue hand out the same order.
///
/// The attempt is counted here rather than when the job finishes: a worker that dies
/// mid-job has still spent one, which is what lets a job that kills its worker reach its
/// limit instead of being retried for ever.
pub(super) fn claim_jobs(
    connection: &Connection<'_>,
    limit: usize,
    lease: InsightLease,
    now_unix_ms: i64,
    scope: JobScope<'_>,
) -> Result<Vec<ClaimedInsightJob>, RepositoryError> {
    const CONTEXT: &str = "queued insight job page";
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut parameters = vec![
        ("state", state(InsightJobState::Queued)),
        (
            "limit",
            Value::Int64(u64_to_i64(limit as u64, "claim limit")?),
        ),
    ];
    let query = match scope {
        JobScope::Everything => CLAIM_PAGE_BY_OLDEST_REVISION,
        JobScope::Among([]) => return Ok(Vec::new()),
        JobScope::Except([]) => CLAIM_PAGE_BY_OLDEST_REVISION,
        JobScope::Among(ids) | JobScope::Except(ids) => {
            parameters.push((
                "ids",
                Value::List(
                    LogicalType::String,
                    ids.iter().cloned().map(Value::String).collect(),
                ),
            ));
            match scope {
                JobScope::Among(_) => CLAIM_PAGE_AMONG,
                _ => CLAIM_PAGE_EXCEPT,
            }
        }
    };
    let rows = fetch_rows(connection, query, parameters, CONTEXT)?;

    let revision_ids: Vec<String> = rows
        .iter()
        .map(|row| text(row, 1, CONTEXT))
        .collect::<Result<_, _>>()?;
    let present = present_paths_of(connection, revision_ids)?;

    let mut claimed = Vec::with_capacity(rows.len());
    for row in &rows {
        let revision_id = text(row, 1, CONTEXT)?;
        let paths = present.get(&revision_id).cloned().unwrap_or_default();
        claimed.push(ClaimedInsightJob::new(
            text(row, 0, CONTEXT)?,
            revision_id,
            text(row, 2, CONTEXT)?,
            paths,
        ));
    }

    execute_ids(
        connection,
        "UNWIND $ids AS id MATCH (job:InsightJob {id: id, state: $from}) SET job.state = $to, job.attempts = job.attempts + 1, job.claimed_at_unix_ms = $claimed_at, job.lease_expires_at_unix_ms = $expires_at RETURN COUNT(job);",
        claimed.iter().map(|job| job.id().to_string()).collect(),
        vec![
            ("from", state(InsightJobState::Queued)),
            ("to", state(InsightJobState::Claimed)),
            ("claimed_at", Value::Int64(now_unix_ms)),
            ("expires_at", Value::Int64(lease.expires_at(now_unix_ms))),
        ],
        "insight job claim",
    )?;
    Ok(claimed)
}

/// Where the file of each revision is still present, so the worker can ask the policy
/// before it spends anything. A revision whose file is at no present location answers with
/// none, which is the same answer the policy gives about a path it no longer admits.
fn present_paths_of(
    connection: &Connection<'_>,
    revision_ids: Vec<String>,
) -> Result<HashMap<String, Vec<NormalizedPath>>, RepositoryError> {
    const CONTEXT: &str = "present paths of claimed revisions";
    let rows = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (version:FileVersion {id: id}) MATCH (file:File {id: version.file_id})-[:LOCATED_AT]->(location:FileLocation {status: 'present'}) RETURN version.id, location.normalized_path;",
        revision_ids,
        CONTEXT,
    )?;
    let mut paths: HashMap<String, Vec<NormalizedPath>> = HashMap::new();
    for row in &rows {
        let revision_id = text(row, 0, CONTEXT)?;
        let path = NormalizedPath::from_absolute(&text(row, 1, CONTEXT)?)
            .map_err(|error| RepositoryError::Storage(format!("{CONTEXT}: {error}")))?;
        paths.entry(revision_id).or_default().push(path);
    }
    Ok(paths)
}

/// One page of jobs still waiting, keyset by id, with the paths their revisions' files are
/// still present at. Bounded by `LIMIT` like every other page here.
///
/// Keyset by id and not by the queue's order, because the sweep that reads this withdraws
/// what it finds: a job that moves out of `queued` is gone from every later page, so
/// ordering by the id the pages advance through is the only order that cannot skip one.
pub(super) const QUEUED_PAGE_BY_ID: &str = "MATCH (job:InsightJob {state: $state}) WHERE job.id > $after RETURN job.id, job.source_revision_id ORDER BY job.id LIMIT $limit;";

pub(super) fn queued_job_page(
    connection: &Connection<'_>,
    after: &str,
    limit: usize,
) -> Result<Vec<QueuedInsightJob>, RepositoryError> {
    const CONTEXT: &str = "waiting insight job page";
    let rows = fetch_rows(
        connection,
        QUEUED_PAGE_BY_ID,
        vec![
            ("state", state(InsightJobState::Queued)),
            ("after", Value::String(after.to_string())),
            (
                "limit",
                Value::Int64(u64_to_i64(limit as u64, "queued page limit")?),
            ),
        ],
        CONTEXT,
    )?;

    let revision_ids: Vec<String> = rows
        .iter()
        .map(|row| text(row, 1, CONTEXT))
        .collect::<Result<_, _>>()?;
    let present = present_paths_of(connection, revision_ids)?;

    let mut queued = Vec::with_capacity(rows.len());
    for row in &rows {
        let revision_id = text(row, 1, CONTEXT)?;
        let paths = present.get(&revision_id).cloned().unwrap_or_default();
        queued.push(QueuedInsightJob::new(text(row, 0, CONTEXT)?, paths));
    }
    Ok(queued)
}

/// Retires the named jobs that are still queued, and answers how many moved. The state is
/// part of the pattern, so a job a worker claimed between the read and this write is left
/// alone rather than taken out of its hands.
pub(super) fn withdraw_queued_jobs(
    connection: &Connection<'_>,
    job_ids: &[String],
) -> Result<usize, RepositoryError> {
    if job_ids.is_empty() {
        return Ok(0);
    }
    fetch_count(
        connection,
        "UNWIND $ids AS id MATCH (job:InsightJob {id: id, state: $from}) SET job.state = $to RETURN COUNT(job);",
        vec![
            (
                "ids",
                Value::List(
                    LogicalType::String,
                    job_ids.iter().cloned().map(Value::String).collect(),
                ),
            ),
            ("from", state(InsightJobState::Queued)),
            ("to", state(InsightJobState::Skipped)),
        ],
        "insight job withdrawal",
    )
}

/// Retires a job the policy no longer asks for. No result, no attempt held against it, and
/// no error: the question was withdrawn, which is not the same as work that went wrong.
pub(super) fn skip_job(connection: &Connection<'_>, job_id: &str) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (job:InsightJob {id: $id, state: $from}) SET job.state = $to, job.claimed_at_unix_ms = NULL, job.lease_expires_at_unix_ms = NULL;",
        vec![
            ("id", Value::String(job_id.to_string())),
            ("from", state(InsightJobState::Claimed)),
            ("to", state(InsightJobState::Skipped)),
        ],
        "insight job skip",
    )
}

/// Stores `result` and marks its job done, in the caller's one transaction. A job that
/// already has a result cannot gain a second: the result's id is derived from the job's, so
/// the primary key refuses it, and `HAS_INSIGHT_RESULT` is `ONE_ONE`, so the link is
/// refused too. Neither refusal is caught here; both roll the transition back.
pub(super) fn complete_job(
    connection: &Connection<'_>,
    result: &InsightResult,
) -> Result<(), RepositoryError> {
    let finding = result.finding();
    let provenance = finding.provenance();
    // A finding with no topic stores an empty one at confidence zero: that is what "nothing
    // was concluded" looks like, and it is never mistaken for a weak conclusion, because a
    // conclusion can never have an empty label.
    let (topic, confidence) = match finding.topic() {
        Some(topic) => (topic.label().to_string(), topic.confidence().value()),
        None => (String::new(), 0.0),
    };
    let tally = finding
        .knowledge()
        .map(|knowledge| knowledge.tally())
        .unwrap_or_default();
    let count = |value: usize| Value::Int64(i64::try_from(value).unwrap_or(i64::MAX));
    execute_statement(
        connection,
        "CREATE (:InsightResult {id: $id, job_id: $job_id, source_revision_id: $source_revision_id, input_content_hash: $input_content_hash, chunk_count: $chunk_count, text_bytes: $text_bytes, produced_by: $produced_by, model_id: $model_id, prompt_version: $prompt_version, topic: $topic, confidence: $confidence, input_tokens: $input_tokens, output_tokens: $output_tokens, cost_micro_usd: $cost_micro_usd, knowledge_checked: $knowledge_checked, facts_kept: $facts_kept, facts_rejected_by_quote: $facts_rejected_by_quote, facts_rejected_unnamed: $facts_rejected_unnamed, facts_rejected_otherwise: $facts_rejected_otherwise, entities_kept: $entities_kept, answered_from_cache: $answered_from_cache, produced_at_unix_ms: $produced_at});",
        vec![
            ("id", Value::String(result.id().to_string())),
            ("job_id", Value::String(result.job_id().to_string())),
            (
                "source_revision_id",
                Value::String(result.source_revision_id().to_string()),
            ),
            (
                "input_content_hash",
                Value::String(result.input_content_hash().to_string()),
            ),
            ("chunk_count", Value::Int64(finding.input().chunk_count())),
            ("text_bytes", Value::Int64(finding.input().text_bytes())),
            (
                "produced_by",
                Value::String(provenance.produced_by().to_string()),
            ),
            ("model_id", Value::String(provenance.model_id().to_string())),
            (
                "prompt_version",
                Value::String(provenance.prompt_version().to_string()),
            ),
            ("topic", Value::String(topic)),
            ("confidence", Value::Double(confidence)),
            ("input_tokens", Value::Int64(provenance.input_tokens())),
            ("output_tokens", Value::Int64(provenance.output_tokens())),
            ("cost_micro_usd", Value::Int64(provenance.cost_micro_usd())),
            (
                "knowledge_checked",
                Value::Bool(finding.knowledge().is_some()),
            ),
            ("facts_kept", count(tally.facts_kept)),
            (
                "facts_rejected_by_quote",
                count(tally.facts_rejected_by_quote),
            ),
            (
                "facts_rejected_unnamed",
                count(tally.facts_rejected_unnamed),
            ),
            (
                "facts_rejected_otherwise",
                count(tally.facts_rejected_otherwise),
            ),
            ("entities_kept", count(tally.entities_kept)),
            (
                "answered_from_cache",
                Value::Bool(finding.answered_from_cache()),
            ),
            ("produced_at", Value::Int64(result.produced_at_unix_ms())),
        ],
        "insight result insert",
    )?;
    // In the same transaction as the result: a job is done with its knowledge written, or not
    // done at all.
    if let Some(knowledge) = finding.knowledge() {
        knowledge_store::replace_revision_knowledge(
            connection,
            result.source_revision_id(),
            knowledge,
            provenance,
            result.produced_at_unix_ms(),
        )?;
    }
    execute_statement(
        connection,
        "MATCH (job:InsightJob {id: $job_id}) MATCH (result:InsightResult {id: $result_id}) CREATE (job)-[:HAS_INSIGHT_RESULT]->(result);",
        vec![
            ("job_id", Value::String(result.job_id().to_string())),
            ("result_id", Value::String(result.id().to_string())),
        ],
        "insight result link",
    )?;
    leave_claim(
        connection,
        result.job_id(),
        InsightJobState::Done,
        None,
        "insight job completion",
    )
}

/// Marks the job failed and records the category of its failure. Nothing else is written,
/// so a failed job never leaves a partial result behind.
pub(super) fn fail_job(
    connection: &Connection<'_>,
    job: &ClaimedInsightJob,
    category: InsightFailureCategory,
) -> Result<(), RepositoryError> {
    leave_claim(
        connection,
        job.id(),
        InsightJobState::Failed,
        Some(category),
        "insight job failure",
    )
}

/// Moves one job out of `claimed` and clears the claim it held. A job that is done has no
/// last error, so the category is cleared with it; how many attempts it cost stays.
///
/// The state is part of the pattern, so a job that is no longer claimed is not moved, and
/// the count says so instead of the transition passing silently. It identifies the state,
/// not the worker: telling one claim from another would need a token on the claim, and a
/// moment is not one, since two claims can share a millisecond. That is enough here,
/// because the database has a single writer and a job can hold only one result whatever
/// order two workers arrive in — a lost race duplicates work, never a result. ADR 0009
/// records when it stops being enough: a background service running several workers over
/// one database needs a claim token, and this pattern has to carry it.
fn leave_claim(
    connection: &Connection<'_>,
    job_id: &str,
    to: InsightJobState,
    category: Option<InsightFailureCategory>,
    context: &'static str,
) -> Result<(), RepositoryError> {
    let category = category.map_or(String::new(), |category| category.label().to_string());
    let moved = fetch_count(
        connection,
        "MATCH (job:InsightJob {id: $id, state: $from}) SET job.state = $to, job.claimed_at_unix_ms = 0, job.lease_expires_at_unix_ms = 0, job.last_error_category = $category RETURN COUNT(job);",
        vec![
            ("id", Value::String(job_id.to_string())),
            ("from", state(InsightJobState::Claimed)),
            ("to", state(to)),
            ("category", Value::String(category)),
        ],
        context,
    )?;
    if moved != 1 {
        return Err(RepositoryError::InvariantViolation(format!(
            "{context} found the job no longer claimed"
        )));
    }
    Ok(())
}

/// Reads pages of jobs to free, each already carrying where it goes, and applies `release`
/// to the two groups. Stops when a page comes back empty, which it must, because every page
/// leaves the state it was read from.
fn sweep(
    connection: &Connection<'_>,
    recovery: &mut InsightRecovery,
    page: impl Fn(&Connection<'_>) -> Result<Vec<(String, InsightRecoveryOutcome)>, RepositoryError>,
    release: impl Fn(
        &Connection<'_>,
        Vec<String>,
        InsightRecoveryOutcome,
    ) -> Result<(), RepositoryError>,
) -> Result<(), RepositoryError> {
    loop {
        let jobs = page(connection)?;
        if jobs.is_empty() {
            return Ok(());
        }
        for outcome in [
            InsightRecoveryOutcome::Requeue,
            InsightRecoveryOutcome::Retire,
        ] {
            let ids: Vec<String> = jobs
                .iter()
                .filter(|(_, decided)| *decided == outcome)
                .map(|(id, _)| id.clone())
                .collect();
            recovery.record(outcome, ids.len());
            release(connection, ids, outcome)?;
        }
    }
}

fn expired_claim_page(
    connection: &Connection<'_>,
    now_unix_ms: i64,
) -> Result<Vec<(String, i64)>, RepositoryError> {
    job_page(
        connection,
        "MATCH (job:InsightJob {state: $state}) WHERE job.lease_expires_at_unix_ms <= $now RETURN job.id, job.attempts ORDER BY job.revision_observed_at_unix_ms, job.id LIMIT $limit;",
        vec![
            ("state", state(InsightJobState::Claimed)),
            ("now", Value::Int64(now_unix_ms)),
        ],
        "expired insight claim page",
    )
}

/// Failed jobs with what they failed at, because the category decides as much as the
/// attempt count does: a failure another attempt cannot change is retired at once.
fn failed_job_page(
    connection: &Connection<'_>,
) -> Result<Vec<(String, i64, InsightFailureCategory)>, RepositoryError> {
    const CONTEXT: &str = "failed insight job page";
    let rows = fetch_rows(
        connection,
        "MATCH (job:InsightJob {state: $state}) RETURN job.id, job.attempts, job.last_error_category ORDER BY job.revision_observed_at_unix_ms, job.id LIMIT $limit;",
        vec![
            ("state", state(InsightJobState::Failed)),
            (
                "limit",
                Value::Int64(u64_to_i64(RECOVERY_PAGE as u64, "recovery page")?),
            ),
        ],
        CONTEXT,
    )?;
    rows.iter()
        .map(|row| {
            let label = text(row, 2, CONTEXT)?;
            // A category this program never wrote would decide a job's fate from something
            // nothing here understands, so it is refused instead.
            let category = InsightFailureCategory::from_label(&label).ok_or_else(|| {
                RepositoryError::InvariantViolation(format!(
                    "{CONTEXT} found a job that failed for a reason this program never wrote"
                ))
            })?;
            Ok((text(row, 0, CONTEXT)?, number(row, 1, CONTEXT)?, category))
        })
        .collect()
}

/// A claim that ran out is the last thing that happened to the job, so the category records
/// it whether the job is queued again or retired.
fn release_expired_claim(
    connection: &Connection<'_>,
    ids: Vec<String>,
    outcome: InsightRecoveryOutcome,
) -> Result<(), RepositoryError> {
    execute_ids(
        connection,
        "UNWIND $ids AS id MATCH (job:InsightJob {id: id, state: $from}) SET job.state = $to, job.claimed_at_unix_ms = 0, job.lease_expires_at_unix_ms = 0, job.last_error_category = $category RETURN COUNT(job);",
        ids,
        vec![
            ("from", state(InsightJobState::Claimed)),
            ("to", state(recovered_state(outcome))),
            (
                "category",
                Value::String(InsightFailureCategory::LeaseExpired.label().to_string()),
            ),
        ],
        "expired insight claim release",
    )
}

/// A failed job already carries the category its failure wrote, so recovery only moves it.
fn release_failed_job(
    connection: &Connection<'_>,
    ids: Vec<String>,
    outcome: InsightRecoveryOutcome,
) -> Result<(), RepositoryError> {
    execute_ids(
        connection,
        "UNWIND $ids AS id MATCH (job:InsightJob {id: id, state: $from}) SET job.state = $to RETURN COUNT(job);",
        ids,
        vec![
            ("from", state(InsightJobState::Failed)),
            ("to", state(recovered_state(outcome))),
        ],
        "failed insight job release",
    )
}

fn recovered_state(outcome: InsightRecoveryOutcome) -> InsightJobState {
    match outcome {
        InsightRecoveryOutcome::Requeue => InsightJobState::Queued,
        InsightRecoveryOutcome::Retire => InsightJobState::Dead,
    }
}

fn job_page(
    connection: &Connection<'_>,
    query: &str,
    mut parameters: Vec<(&'static str, Value)>,
    context: &'static str,
) -> Result<Vec<(String, i64)>, RepositoryError> {
    parameters.push((
        "limit",
        Value::Int64(u64_to_i64(RECOVERY_PAGE as u64, "recovery page")?),
    ));
    let rows = fetch_rows(connection, query, parameters, context)?;
    rows.iter()
        .map(|row| Ok((text(row, 0, context)?, number(row, 1, context)?)))
        .collect()
}

fn state(state: InsightJobState) -> Value {
    Value::String(state.label().to_string())
}
