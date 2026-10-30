// The knowledge layer in the graph: writing what a revision proved, retiring what a newer
// revision replaced, and reading it back for the map.
//
// A file's knowledge is the knowledge of its current revision and of nothing else. Writing a
// revision's facts therefore deletes every fact and mention of the same file first, in the same
// transaction as the result, so the graph never holds two revisions' facts about one file. A
// revision that is no longer current when its job finishes — a newer one was published while
// the model was answering — writes no knowledge at all: what it states is already not what the
// file says. And a revision that was replaced before anyone asked about its successor keeps its
// facts only until the next sweep retires them; readers skip them before that.
//
// Writing follows the rule of `store_support`: what exists is asked by primary key in one
// statement, and only what is missing is created. An entity is shared by every file that
// mentions it, so it is created by the first and deleted with the last.
use std::collections::BTreeSet;

use lbug::{Connection, LogicalType, Value};

use crate::application::{
    KnowledgeTotals, PaidCall, RepositoryError, RevisionSource, SourceConversation,
};
use crate::domain::knowledge::{RevisionKnowledge, StoredFact, StoredMention};
use crate::domain::{ClaimedInsightJob, InsightFinding, InsightProvenance, NormalizedPath};

use super::conversation_store::conversation_id_of;
use super::store_support::{
    execute_rows, execute_statement, fetch_count, fetch_first_row, fetch_rows, fetch_rows_for_ids,
    number, text,
};

/// How many stale facts or mentions one retiring statement removes at a time.
const RETIRE_PAGE: usize = 500;

const CURRENT_FILE_OF_REVISION: &str = "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion {id: $revision_id}) RETURN file.id;";
const ENTITIES_MENTIONED_BY_FILE: &str =
    "MATCH (mention:EntityMention {file_id: $file_id}) RETURN DISTINCT mention.entity_id;";
const DELETE_FACTS_OF_FILE: &str = "MATCH (fact:Fact {file_id: $file_id}) DETACH DELETE fact;";
const DELETE_MENTIONS_OF_FILE: &str =
    "MATCH (mention:EntityMention {file_id: $file_id}) DETACH DELETE mention;";
const EXISTING_ENTITIES: &str =
    "UNWIND $ids AS id MATCH (entity:Entity {id: id}) RETURN entity.id;";
const ENTITY_INSERT: &str = "UNWIND $rows AS row CREATE (entity:Entity {id: row.id, key: row.key, created_at_unix_ms: row.at}) RETURN COUNT(entity);";
const MENTION_INSERT: &str = "UNWIND $rows AS row CREATE (mention:EntityMention {id: row.id, entity_id: row.entity_id, file_id: row.file_id, source_revision_id: row.revision_id, name: row.name, entity_type: row.entity_type, description: row.description}) RETURN COUNT(mention);";
const MENTION_LINKS: &str = "UNWIND $rows AS row MATCH (entity:Entity {id: row.entity_id}) MATCH (mention:EntityMention {id: row.id}) MATCH (version:FileVersion {id: row.revision_id}) CREATE (entity)-[:MENTIONED_AS]->(mention) CREATE (mention)-[:MENTIONED_IN]->(version) RETURN COUNT(mention);";
const FACT_INSERT: &str = "UNWIND $rows AS row CREATE (fact:Fact {id: row.id, file_id: row.file_id, source_revision_id: row.revision_id, source_chunk_id: row.chunk_id, subject_id: row.subject_id, predicate: row.predicate, object_id: row.object_id, description: row.description, weight: row.weight, quote: row.quote, model_id: row.model_id, prompt_version: row.prompt_version, created_at_unix_ms: row.at}) RETURN COUNT(fact);";
const FACT_LINKS: &str = "UNWIND $rows AS row MATCH (fact:Fact {id: row.id}) MATCH (subject:Entity {id: row.subject_id}) MATCH (object:Entity {id: row.object_id}) MATCH (chunk:ContentChunk {id: row.chunk_id}) CREATE (fact)-[:FACT_SUBJECT]->(subject) CREATE (fact)-[:FACT_OBJECT]->(object) CREATE (fact)-[:STATED_IN]->(chunk) RETURN COUNT(fact);";
/// An entity no mention names any more. Asked only of the entities a write or a retirement
/// just touched, so it never walks the whole table.
const DELETE_UNMENTIONED_ENTITIES: &str = "UNWIND $ids AS id MATCH (entity:Entity {id: id}) WHERE NOT EXISTS { MATCH (entity)-[:MENTIONED_AS]->(:EntityMention) } DETACH DELETE entity;";
const STALE_FACTS: &str = "MATCH (fact:Fact) MATCH (file:File {id: fact.file_id})-[:CURRENT_VERSION]->(current:FileVersion) WHERE current.id <> fact.source_revision_id RETURN fact.id LIMIT $limit;";
const STALE_MENTIONS: &str = "MATCH (mention:EntityMention) MATCH (file:File {id: mention.file_id})-[:CURRENT_VERSION]->(current:FileVersion) WHERE current.id <> mention.source_revision_id RETURN mention.id, mention.entity_id LIMIT $limit;";
const DELETE_FACTS: &str = "UNWIND $ids AS id MATCH (fact:Fact {id: id}) DETACH DELETE fact;";
const DELETE_MENTIONS: &str =
    "UNWIND $ids AS id MATCH (mention:EntityMention {id: id}) DETACH DELETE mention;";
const SPELLINGS: &str =
    "MATCH (mention:EntityMention) RETURN DISTINCT mention.entity_id, mention.name;";
const CURRENT_MENTIONS: &str = "MATCH (mention:EntityMention) MATCH (file:File {id: mention.file_id})-[:CURRENT_VERSION]->(current:FileVersion) WHERE current.id = mention.source_revision_id RETURN mention.entity_id, mention.source_revision_id, mention.name, mention.entity_type, mention.description, mention.file_id ORDER BY mention.id;";
const CURRENT_FACTS: &str = "MATCH (fact:Fact)-[:STATED_IN]->(chunk:ContentChunk) MATCH (file:File {id: fact.file_id})-[:CURRENT_VERSION]->(current:FileVersion) WHERE current.id = fact.source_revision_id RETURN fact.id, fact.subject_id, fact.predicate, fact.object_id, fact.description, fact.weight, fact.quote, fact.file_id, fact.source_revision_id, chunk.ordinal ORDER BY fact.id;";
const PRESENT_PATHS_OF_REVISIONS: &str = "UNWIND $ids AS id MATCH (version:FileVersion {id: id}) MATCH (:File {id: version.file_id})-[:LOCATED_AT]->(location:FileLocation {status: 'present'}) RETURN version.id, location.normalized_path ORDER BY version.id, location.normalized_path;";
const CONVERSATIONS_OF_REVISIONS: &str = "UNWIND $ids AS id MATCH (conversation:Conversation {id: id}) MATCH (version:FileVersion {id: conversation.source_revision_id}) OPTIONAL MATCH (version)-[:HAS_CHUNK]->(chunk:ContentChunk) RETURN conversation.source_revision_id, conversation.session_id, conversation.name, conversation.named_by, conversation.message_count, COUNT(chunk);";
const CHECKED_RESULTS: &str = "MATCH (result:InsightResult) WHERE result.knowledge_checked RETURN COUNT(result), SUM(result.chunk_count), SUM(result.text_bytes), SUM(result.facts_kept), SUM(result.facts_rejected_by_quote), SUM(result.facts_rejected_otherwise), SUM(result.facts_rejected_unnamed);";
/// What was paid: only the results whose answer was bought. One served from the cache carries
/// the provenance of the call that bought it, and counting it again would bill that call twice.
const PAID_CALLS: &str = "MATCH (result:InsightResult) WHERE result.knowledge_checked AND NOT result.answered_from_cache RETURN result.text_bytes, result.input_tokens, result.output_tokens, result.cost_micro_usd;";
const QUEUED_WORK: &str = concat!(
    "MATCH (job:InsightJob {state: 'queued'}) MATCH (version:FileVersion {id: job.source_revision_id}) OPTIONAL MATCH (version)-[:HAS_CHUNK]->(chunk:ContentChunk) RETURN COUNT(DISTINCT job.id), COUNT(chunk), SUM(",
    chunk_bytes!(),
    ");"
);

/// Analysed jobs whose revision is still current: what a recheck may apply new rules to.
const ANALYSED_CURRENT_JOBS: &str = "MATCH (job:InsightJob {state: 'done'})-[:HAS_INSIGHT_RESULT]->(result:InsightResult) WHERE result.knowledge_checked MATCH (:File)-[:CURRENT_VERSION]->(version:FileVersion {id: job.source_revision_id}) RETURN job.id, job.source_revision_id, job.input_content_hash ORDER BY job.id;";
const RESULT_COUNTS_UPDATE: &str = "MATCH (result:InsightResult {id: $id}) SET result.facts_kept = $kept, result.facts_rejected_by_quote = $by_quote, result.facts_rejected_unnamed = $unnamed, result.facts_rejected_otherwise = $otherwise, result.entities_kept = $entities;";

pub(super) fn analysed_current_jobs(
    connection: &Connection<'_>,
) -> Result<Vec<ClaimedInsightJob>, RepositoryError> {
    const CONTEXT: &str = "analysed current jobs";
    fetch_rows(connection, ANALYSED_CURRENT_JOBS, vec![], CONTEXT)?
        .iter()
        .map(|row| {
            Ok(ClaimedInsightJob::new(
                text(row, 0, CONTEXT)?,
                text(row, 1, CONTEXT)?,
                text(row, 2, CONTEXT)?,
                Vec::new(),
            ))
        })
        .collect()
}

/// Writes a rechecked revision's knowledge and the counts on its result, in the caller's
/// transaction. The result keeps what it cost: a recheck asked nobody.
pub(super) fn rewrite_knowledge(
    connection: &Connection<'_>,
    job: &ClaimedInsightJob,
    finding: &InsightFinding,
    at_unix_ms: i64,
) -> Result<(), RepositoryError> {
    let Some(knowledge) = finding.knowledge() else {
        return Ok(());
    };
    replace_revision_knowledge(
        connection,
        job.source_revision_id(),
        knowledge,
        finding.provenance(),
        at_unix_ms,
    )?;
    let tally = knowledge.tally();
    let count = |value: usize| Value::Int64(i64::try_from(value).unwrap_or(i64::MAX));
    execute_statement(
        connection,
        RESULT_COUNTS_UPDATE,
        vec![
            ("id", Value::String(format!("insight-result:{}", job.id()))),
            ("kept", count(tally.facts_kept)),
            ("by_quote", count(tally.facts_rejected_by_quote)),
            ("unnamed", count(tally.facts_rejected_unnamed)),
            ("otherwise", count(tally.facts_rejected_otherwise)),
            ("entities", count(tally.entities_kept)),
        ],
        "rechecked result counts",
    )
}

/// Writes what `revision_id` proved, replacing every fact and mention its file had, in the
/// caller's transaction. Nothing is written when the revision is no longer its file's current
/// one.
pub(super) fn replace_revision_knowledge(
    connection: &Connection<'_>,
    revision_id: &str,
    knowledge: &RevisionKnowledge,
    provenance: &InsightProvenance,
    at_unix_ms: i64,
) -> Result<(), RepositoryError> {
    const CONTEXT: &str = "knowledge revision lookup";
    let Some(row) = fetch_first_row(
        connection,
        CURRENT_FILE_OF_REVISION,
        vec![("revision_id", Value::String(revision_id.to_string()))],
        CONTEXT,
    )?
    else {
        return Ok(());
    };
    let file_id = text(&row, 0, CONTEXT)?;

    let mut touched: BTreeSet<String> = fetch_rows(
        connection,
        ENTITIES_MENTIONED_BY_FILE,
        vec![("file_id", Value::String(file_id.clone()))],
        "knowledge entities of a file",
    )?
    .iter()
    .map(|row| text(row, 0, "knowledge entities of a file"))
    .collect::<Result<_, _>>()?;
    for statement in [DELETE_FACTS_OF_FILE, DELETE_MENTIONS_OF_FILE] {
        execute_statement(
            connection,
            statement,
            vec![("file_id", Value::String(file_id.clone()))],
            "knowledge of a replaced revision",
        )?;
    }

    create_missing_entities(connection, knowledge, at_unix_ms)?;
    let mentions = mention_rows(knowledge, &file_id, revision_id);
    execute_rows(
        connection,
        MENTION_INSERT,
        mentions.clone(),
        "entity mention insert",
    )?;
    execute_rows(connection, MENTION_LINKS, mentions, "entity mention links")?;
    let facts = fact_rows(knowledge, &file_id, revision_id, provenance, at_unix_ms);
    execute_rows(connection, FACT_INSERT, facts.clone(), "fact insert")?;
    execute_rows(connection, FACT_LINKS, facts, "fact links")?;

    for entity in knowledge.entities() {
        touched.remove(&entity.key().entity_id());
    }
    delete_unmentioned(connection, touched.into_iter().collect())
}

fn create_missing_entities(
    connection: &Connection<'_>,
    knowledge: &RevisionKnowledge,
    at_unix_ms: i64,
) -> Result<(), RepositoryError> {
    const CONTEXT: &str = "knowledge entity lookup";
    let wanted: Vec<String> = knowledge
        .entities()
        .iter()
        .map(|entity| entity.key().entity_id())
        .collect();
    let existing: BTreeSet<String> =
        fetch_rows_for_ids(connection, EXISTING_ENTITIES, wanted, CONTEXT)?
            .iter()
            .map(|row| text(row, 0, CONTEXT))
            .collect::<Result<_, _>>()?;
    let rows = knowledge
        .entities()
        .iter()
        .filter(|entity| !existing.contains(&entity.key().entity_id()))
        .map(|entity| {
            Value::Struct(vec![
                ("id".to_string(), Value::String(entity.key().entity_id())),
                (
                    "key".to_string(),
                    Value::String(entity.key().as_str().to_string()),
                ),
                ("at".to_string(), Value::Int64(at_unix_ms)),
            ])
        })
        .collect();
    execute_rows(connection, ENTITY_INSERT, rows, "knowledge entity insert")
}

/// One row per spelling: the spelling is what the source wrote, and the map shows the one
/// written most often.
fn mention_rows(knowledge: &RevisionKnowledge, file_id: &str, revision_id: &str) -> Vec<Value> {
    let mut rows = Vec::new();
    for (entity_number, entity) in knowledge.entities().iter().enumerate() {
        for (spelling_number, spelling) in entity.spellings().iter().enumerate() {
            rows.push(Value::Struct(vec![
                (
                    "id".to_string(),
                    Value::String(format!(
                        "mention:{revision_id}:{entity_number}:{spelling_number}"
                    )),
                ),
                (
                    "entity_id".to_string(),
                    Value::String(entity.key().entity_id()),
                ),
                ("file_id".to_string(), Value::String(file_id.to_string())),
                (
                    "revision_id".to_string(),
                    Value::String(revision_id.to_string()),
                ),
                ("name".to_string(), Value::String(spelling.clone())),
                (
                    "entity_type".to_string(),
                    Value::String(entity.entity_type().to_string()),
                ),
                (
                    "description".to_string(),
                    Value::String(entity.description().to_string()),
                ),
            ]));
        }
    }
    rows
}

fn fact_rows(
    knowledge: &RevisionKnowledge,
    file_id: &str,
    revision_id: &str,
    provenance: &InsightProvenance,
    at_unix_ms: i64,
) -> Vec<Value> {
    knowledge
        .facts()
        .iter()
        .enumerate()
        .map(|(number, fact)| {
            Value::Struct(vec![
                (
                    "id".to_string(),
                    Value::String(format!("fact:{revision_id}:{number}")),
                ),
                ("file_id".to_string(), Value::String(file_id.to_string())),
                (
                    "revision_id".to_string(),
                    Value::String(revision_id.to_string()),
                ),
                (
                    "chunk_id".to_string(),
                    Value::String(fact.chunk_id().to_string()),
                ),
                (
                    "subject_id".to_string(),
                    Value::String(fact.subject().entity_id()),
                ),
                (
                    "predicate".to_string(),
                    Value::String(fact.predicate().to_string()),
                ),
                (
                    "object_id".to_string(),
                    Value::String(fact.object().entity_id()),
                ),
                (
                    "description".to_string(),
                    Value::String(fact.description().to_string()),
                ),
                ("weight".to_string(), Value::Int64(i64::from(fact.weight()))),
                ("quote".to_string(), Value::String(fact.quote().to_string())),
                (
                    "model_id".to_string(),
                    Value::String(provenance.model_id().to_string()),
                ),
                (
                    "prompt_version".to_string(),
                    Value::String(provenance.prompt_version().to_string()),
                ),
                ("at".to_string(), Value::Int64(at_unix_ms)),
            ])
        })
        .collect()
}

fn delete_unmentioned(
    connection: &Connection<'_>,
    ids: Vec<String>,
) -> Result<(), RepositoryError> {
    delete_by_ids(
        connection,
        DELETE_UNMENTIONED_ENTITIES,
        ids,
        "knowledge entity retirement",
    )
}

/// A delete over a list of ids. How many rows it removed is not counted: a detached delete
/// returns nothing to count, and every id here was read in the same transaction.
fn delete_by_ids(
    connection: &Connection<'_>,
    query: &str,
    ids: Vec<String>,
    context: &'static str,
) -> Result<(), RepositoryError> {
    if ids.is_empty() {
        return Ok(());
    }
    let ids = Value::List(
        LogicalType::String,
        ids.into_iter().map(Value::String).collect(),
    );
    execute_statement(connection, query, vec![("ids", ids)], context)
}

/// Retires the facts and mentions of every revision a newer one replaced, a page at a time,
/// and the entities that leaves unnamed. Returns how many facts went.
pub(super) fn retire_stale_knowledge(
    connection: &Connection<'_>,
) -> Result<usize, RepositoryError> {
    let limit = vec![("limit", Value::Int64(RETIRE_PAGE as i64))];
    let mut retired_facts = 0;
    loop {
        let ids: Vec<String> = fetch_rows(connection, STALE_FACTS, limit.clone(), "stale facts")?
            .iter()
            .map(|row| text(row, 0, "stale facts"))
            .collect::<Result<_, _>>()?;
        if ids.is_empty() {
            break;
        }
        retired_facts += ids.len();
        delete_by_ids(connection, DELETE_FACTS, ids, "stale fact retirement")?;
    }
    loop {
        let rows = fetch_rows(connection, STALE_MENTIONS, limit.clone(), "stale mentions")?;
        if rows.is_empty() {
            break;
        }
        let mut ids = Vec::with_capacity(rows.len());
        let mut entities = BTreeSet::new();
        for row in &rows {
            ids.push(text(row, 0, "stale mentions")?);
            entities.insert(text(row, 1, "stale mentions")?);
        }
        delete_by_ids(connection, DELETE_MENTIONS, ids, "stale mention retirement")?;
        delete_unmentioned(connection, entities.into_iter().collect())?;
    }
    Ok(retired_facts)
}

pub(super) fn current_mentions(
    connection: &Connection<'_>,
) -> Result<Vec<StoredMention>, RepositoryError> {
    const CONTEXT: &str = "current entity mentions";
    fetch_rows(connection, CURRENT_MENTIONS, vec![], CONTEXT)?
        .iter()
        .map(|row| {
            Ok(StoredMention {
                entity_id: text(row, 0, CONTEXT)?,
                revision_id: text(row, 1, CONTEXT)?,
                name: text(row, 2, CONTEXT)?,
                entity_type: text(row, 3, CONTEXT)?,
                description: text(row, 4, CONTEXT)?,
                file_id: text(row, 5, CONTEXT)?,
            })
        })
        .collect()
}

pub(super) fn current_facts(
    connection: &Connection<'_>,
) -> Result<Vec<StoredFact>, RepositoryError> {
    const CONTEXT: &str = "current facts";
    fetch_rows(connection, CURRENT_FACTS, vec![], CONTEXT)?
        .iter()
        .map(|row| {
            let weight = number(row, 5, CONTEXT)?;
            Ok(StoredFact {
                id: text(row, 0, CONTEXT)?,
                subject_id: text(row, 1, CONTEXT)?,
                predicate: text(row, 2, CONTEXT)?,
                object_id: text(row, 3, CONTEXT)?,
                description: text(row, 4, CONTEXT)?,
                weight: u8::try_from(weight).map_err(|_| {
                    RepositoryError::InvariantViolation(format!(
                        "{CONTEXT} returned a weight of {weight}"
                    ))
                })?,
                quote: text(row, 6, CONTEXT)?,
                file_id: text(row, 7, CONTEXT)?,
                revision_id: text(row, 8, CONTEXT)?,
                chunk_ordinal: number(row, 9, CONTEXT)?,
            })
        })
        .collect()
}

pub(super) fn revision_sources(
    connection: &Connection<'_>,
    revision_ids: &[String],
) -> Result<Vec<RevisionSource>, RepositoryError> {
    const PATHS: &str = "knowledge source paths";
    const CONVERSATIONS: &str = "knowledge source conversations";
    let mut sources: Vec<RevisionSource> = revision_ids
        .iter()
        .map(|revision_id| RevisionSource {
            revision_id: revision_id.clone(),
            present_paths: Vec::new(),
            conversation: None,
        })
        .collect();
    let position = |sources: &[RevisionSource], revision: &str| {
        sources
            .iter()
            .position(|source| source.revision_id == revision)
    };
    for row in fetch_rows_for_ids(
        connection,
        PRESENT_PATHS_OF_REVISIONS,
        revision_ids.to_vec(),
        PATHS,
    )? {
        let revision = text(&row, 0, PATHS)?;
        let Ok(path) = NormalizedPath::from_absolute(&text(&row, 1, PATHS)?) else {
            continue;
        };
        if let Some(at) = position(&sources, &revision) {
            sources[at].present_paths.push(path);
        }
    }
    let conversation_ids = revision_ids
        .iter()
        .map(|revision| conversation_id_of(revision))
        .collect();
    for row in fetch_rows_for_ids(
        connection,
        CONVERSATIONS_OF_REVISIONS,
        conversation_ids,
        CONVERSATIONS,
    )? {
        let revision = text(&row, 0, CONVERSATIONS)?;
        let conversation = SourceConversation {
            session_id: text(&row, 1, CONVERSATIONS)?,
            name: text(&row, 2, CONVERSATIONS)?,
            named: text(&row, 3, CONVERSATIONS)? != "nobody",
            message_count: u64::try_from(number(&row, 4, CONVERSATIONS)?).unwrap_or(0),
            chunk_count: u64::try_from(number(&row, 5, CONVERSATIONS)?).unwrap_or(0),
        };
        if let Some(at) = position(&sources, &revision) {
            sources[at].conversation = Some(conversation);
        }
    }
    Ok(sources)
}

pub(super) fn knowledge_totals(
    connection: &Connection<'_>,
) -> Result<KnowledgeTotals, RepositoryError> {
    let count =
        |query: &str, context: &'static str| fetch_count(connection, query, vec![], context);
    let mut totals = KnowledgeTotals {
        entities: count(
            "MATCH (entity:Entity) RETURN COUNT(entity);",
            "entity count",
        )?,
        facts: count("MATCH (fact:Fact) RETURN COUNT(fact);", "fact count")?,
        mentions: count(
            "MATCH (mention:EntityMention) RETURN COUNT(mention);",
            "mention count",
        )?,
        spellings: fetch_rows(connection, SPELLINGS, vec![], "spelling count")?.len(),
        ..KnowledgeTotals::default()
    };

    const CHECKED: &str = "checked results";
    if let Some(row) = fetch_first_row(connection, CHECKED_RESULTS, vec![], CHECKED)? {
        totals.checked_revisions = usize::try_from(number(&row, 0, CHECKED)?).unwrap_or_default();
        totals.checked_chunks = summed(&row, 1, CHECKED)?;
        totals.checked_bytes = summed(&row, 2, CHECKED)?;
        totals.facts_kept = summed(&row, 3, CHECKED)?;
        totals.facts_rejected_by_quote = summed(&row, 4, CHECKED)?;
        totals.facts_rejected_otherwise = summed(&row, 5, CHECKED)?;
        totals.facts_rejected_unnamed = summed(&row, 6, CHECKED)?;
    }
    const PAID: &str = "paid calls";
    for row in fetch_rows(connection, PAID_CALLS, vec![], PAID)? {
        let call = PaidCall {
            bytes: number(&row, 0, PAID)?,
            cost_micro_usd: number(&row, 3, PAID)?,
        };
        totals.input_tokens += number(&row, 1, PAID)?;
        totals.output_tokens += number(&row, 2, PAID)?;
        totals.cost_micro_usd += call.cost_micro_usd;
        totals.paid_calls.push(call);
    }
    const QUEUED: &str = "queued knowledge work";
    if let Some(row) = fetch_first_row(connection, QUEUED_WORK, vec![], QUEUED)? {
        totals.queued_jobs = usize::try_from(number(&row, 0, QUEUED)?).unwrap_or_default();
        totals.queued_chunks = number(&row, 1, QUEUED)?;
        totals.queued_bytes = summed(&row, 2, QUEUED)?;
    }
    Ok(totals)
}

/// A `SUM`, which the engine widens to INT128 and answers with nothing over no rows.
fn summed(values: &[Value], index: usize, context: &'static str) -> Result<i64, RepositoryError> {
    match values.get(index) {
        Some(Value::Int128(sum)) => i64::try_from(*sum).map_err(|_| {
            RepositoryError::InvariantViolation(format!("{context} sum exceeds INT64"))
        }),
        Some(Value::Int64(sum)) => Ok(*sum),
        Some(Value::Null(_)) | None => Ok(0),
        Some(_) => Err(RepositoryError::InvariantViolation(format!(
            "{context} returned a row without a sum at position {index}"
        ))),
    }
}
