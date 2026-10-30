// The revision a job names, read from the context's own graph, and the answers already
// bought for it.
//
// `summarize` counts what the chunks amount to without touching a byte of their text, which
// is what lets an over-budget revision be refused before its content is anywhere near
// memory. `read_parts` is the one function in this program that pulls file content back out
// of the graph for a model, and it takes the context's permit, so it cannot be reached on
// behalf of a context that refuses to let its content leave.
//
// The cache lives here too, in the same database as everything else the context owns. That
// is not an optimization detail but the boundary again: an answer bought for one context's
// file is stored where only that context can find it.
use std::sync::Arc;

use lbug::{Connection, Value};

use crate::application::{InsightCache, RevisionInputReader};
use crate::domain::knowledge::SourcePart;
use crate::domain::{
    ClaimedInsightJob, ContentEgressPermit, InsightCacheKey, InsightFailureCategory,
    InsightProvenance, ModelAnswer, RevisionSummary, RevisionTopic,
};

use super::super::analysis::knowledge_answer::{proposal_from_stored, stored_proposal};
use super::manager::LadybugDatabaseManager;
use super::store_support::{current_unix_ms, fetch_first_row, fetch_rows};

/// How many chunks one page of text carries. The text of a revision is bounded by the
/// content limit the policy enforces, but it is still read a page at a time so one
/// statement never materializes a whole source.
const TEXT_PAGE: usize = 100;

pub struct LadybugRevisionInput {
    manager: Arc<LadybugDatabaseManager>,
}

impl LadybugRevisionInput {
    pub fn new(manager: Arc<LadybugDatabaseManager>) -> Self {
        Self { manager }
    }

    /// Reads outside any transaction and without the write lock, like every other read of
    /// what is already durable.
    fn connect(&self) -> Result<Connection<'_>, InsightFailureCategory> {
        Connection::new(&self.manager.owner.database).map_err(|_| InsightFailureCategory::Handler)
    }
}

impl RevisionInputReader for LadybugRevisionInput {
    /// The two categories this can answer with are different problems. `Input` means the
    /// graph does not hold what the job names — the revision is gone, or is not the content
    /// the job was queued for — and no retry changes that. `Handler` means the graph would
    /// not answer, or answered in a shape this code does not know. The engine's own words
    /// never leave this function.
    fn summarize(
        &self,
        job: &ClaimedInsightJob,
    ) -> Result<RevisionSummary, InsightFailureCategory> {
        const CONTEXT: &str = "insight job revision summary";
        let connection = self.connect()?;
        let row = fetch_first_row(
            &connection,
            concat!("MATCH (version:FileVersion {id: $revision_id}) OPTIONAL MATCH (version)-[:HAS_CHUNK]->(chunk:ContentChunk) RETURN version.content_hash, COUNT(chunk), SUM(", chunk_bytes!(), ");"),
            vec![(
                "revision_id",
                Value::String(job.source_revision_id().to_string()),
            )],
            CONTEXT,
        )
        .map_err(|_| InsightFailureCategory::Handler)?;

        // No row at all: the job names a revision this graph does not hold.
        let Some(values) = row else {
            return Err(InsightFailureCategory::Input);
        };
        let [
            Value::String(content_hash),
            Value::Int64(chunk_count),
            text_bytes,
        ] = values.as_slice()
        else {
            return Err(InsightFailureCategory::Handler);
        };
        if content_hash != job.input_content_hash() {
            return Err(InsightFailureCategory::Input);
        }
        // `SUM` widens INT64 to INT128, and a revision with no chunk sums to nothing.
        let text_bytes = match text_bytes {
            Value::Int128(bytes) => {
                i64::try_from(*bytes).map_err(|_| InsightFailureCategory::Handler)?
            }
            Value::Null(_) => 0,
            _ => return Err(InsightFailureCategory::Handler),
        };
        Ok(RevisionSummary::new(*chunk_count, text_bytes))
    }

    /// The chunks of the revision in their own order, each numbered from one. `permit` is not
    /// inspected: holding one is the permission, and a denying context has none to pass.
    fn read_parts(
        &self,
        job: &ClaimedInsightJob,
        _permit: ContentEgressPermit,
    ) -> Result<Vec<SourcePart>, InsightFailureCategory> {
        const CONTEXT: &str = "insight job revision parts";
        let connection = self.connect()?;
        let mut parts = Vec::new();
        let mut after_ordinal: i64 = -1;
        loop {
            let query = format!(
                "MATCH (:FileVersion {{id: $revision_id}})-[:HAS_CHUNK]->(chunk:ContentChunk) WHERE chunk.ordinal > $after_ordinal RETURN chunk.ordinal, chunk.id, chunk.text ORDER BY chunk.ordinal LIMIT {TEXT_PAGE};"
            );
            let rows = fetch_rows(
                &connection,
                &query,
                vec![
                    (
                        "revision_id",
                        Value::String(job.source_revision_id().to_string()),
                    ),
                    ("after_ordinal", Value::Int64(after_ordinal)),
                ],
                CONTEXT,
            )
            .map_err(|_| InsightFailureCategory::Handler)?;
            if rows.is_empty() {
                return Ok(parts);
            }
            for row in &rows {
                let [
                    Value::Int64(ordinal),
                    Value::String(id),
                    Value::String(text),
                ] = row.as_slice()
                else {
                    return Err(InsightFailureCategory::Handler);
                };
                parts.push(SourcePart::new(parts.len() + 1, id.clone(), text.clone()));
                after_ordinal = *ordinal;
            }
        }
    }
}

/// The answers this context has already bought, keyed by the question rather than by the
/// job that asked it.
pub struct LadybugInsightCache {
    manager: Arc<LadybugDatabaseManager>,
}

impl LadybugInsightCache {
    pub fn new(manager: Arc<LadybugDatabaseManager>) -> Self {
        Self { manager }
    }
}

impl InsightCache for LadybugInsightCache {
    fn lookup(&self, key: &InsightCacheKey) -> Result<Option<ModelAnswer>, InsightFailureCategory> {
        const CONTEXT: &str = "insight cache lookup";
        let connection = Connection::new(&self.manager.owner.database)
            .map_err(|_| InsightFailureCategory::Handler)?;
        let row = fetch_first_row(
            &connection,
            "MATCH (entry:InsightCacheEntry {key: $key}) RETURN entry.topic, entry.confidence, entry.produced_by, entry.model_id, entry.prompt_version, entry.input_tokens, entry.output_tokens, entry.cost_micro_usd, entry.knowledge;",
            vec![("key", Value::String(key.as_str().to_string()))],
            CONTEXT,
        )
        .map_err(|_| InsightFailureCategory::Handler)?;
        let Some(values) = row else {
            return Ok(None);
        };
        let [
            Value::String(topic),
            Value::Double(confidence),
            Value::String(produced_by),
            Value::String(model_id),
            Value::String(prompt_version),
            Value::Int64(input_tokens),
            Value::Int64(output_tokens),
            Value::Int64(cost_micro_usd),
            Value::String(knowledge),
        ] = values.as_slice()
        else {
            return Err(InsightFailureCategory::Handler);
        };
        // A stored answer passes the same constructors a fresh one does: whatever is on disk
        // has to still be a topic and a proposal within the limits, or it is not served.
        let topic =
            RevisionTopic::parse(topic, *confidence).map_err(|_| InsightFailureCategory::Schema)?;
        Ok(Some(
            ModelAnswer::new(
                topic,
                InsightProvenance::from_model(
                    produced_by,
                    model_id,
                    prompt_version,
                    *input_tokens,
                    *output_tokens,
                )
                .costing(*cost_micro_usd),
            )
            .with_knowledge(proposal_from_stored(knowledge)?),
        ))
    }

    /// Stores the answer in its own transaction under the write lock. It is not part of the
    /// job's transaction on purpose: a cache entry without its result costs one repeated
    /// question, while joining the two would put a paid answer at the mercy of a job that
    /// fails afterwards.
    fn store(
        &self,
        key: &InsightCacheKey,
        answer: &ModelAnswer,
    ) -> Result<(), InsightFailureCategory> {
        let created_at = current_unix_ms().map_err(|_| InsightFailureCategory::Handler)?;
        let provenance = answer.provenance();
        let _write_guard = self
            .manager
            .owner
            .write_guard()
            .map_err(|_| InsightFailureCategory::Handler)?;
        let connection = Connection::new(&self.manager.owner.database)
            .map_err(|_| InsightFailureCategory::Handler)?;
        let owner = &self.manager.owner;
        super::migration::in_transaction_committing(&connection, |connection| {
            super::store_support::execute_statement(
                connection,
                "MERGE (entry:InsightCacheEntry {key: $key}) ON CREATE SET entry.topic = $topic, entry.confidence = $confidence, entry.produced_by = $produced_by, entry.model_id = $model_id, entry.prompt_version = $prompt_version, entry.input_tokens = $input_tokens, entry.output_tokens = $output_tokens, entry.cost_micro_usd = $cost_micro_usd, entry.knowledge = $knowledge, entry.created_at_unix_ms = $created_at;",
                vec![
                    ("key", Value::String(key.as_str().to_string())),
                    ("topic", Value::String(answer.topic().label().to_string())),
                    (
                        "confidence",
                        Value::Double(answer.topic().confidence().value()),
                    ),
                    (
                        "produced_by",
                        Value::String(provenance.produced_by().to_string()),
                    ),
                    ("model_id", Value::String(provenance.model_id().to_string())),
                    (
                        "prompt_version",
                        Value::String(provenance.prompt_version().to_string()),
                    ),
                    ("input_tokens", Value::Int64(provenance.input_tokens())),
                    ("output_tokens", Value::Int64(provenance.output_tokens())),
                    (
                        "cost_micro_usd",
                        Value::Int64(provenance.cost_micro_usd()),
                    ),
                    (
                        "knowledge",
                        Value::String(stored_proposal(answer.knowledge())),
                    ),
                    ("created_at", Value::Int64(created_at)),
                ],
                "insight cache insert",
            )
        }, || owner.readers_kept_out())
        .map_err(|_| InsightFailureCategory::Handler)
    }
}
