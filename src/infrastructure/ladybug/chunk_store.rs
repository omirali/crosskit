// Immutable ContentChunk nodes, their Extractor provenance and the links HAS_CHUNK and
// GENERATED_BY.
use std::collections::HashMap;

use lbug::{Connection, Value};

use crate::application::RepositoryError;
use crate::domain::{ChunkScript, ContentChunk, FileVersion};

use super::batch_projection::{ChunkRow, ExtractorRow};
use super::store_support::{
    execute_rows, execute_statement, fetch_first_row, fetch_rows, fetch_rows_for_ids, text,
    u64_to_i64,
};

/// Persists every chunk of the version with its extractor and links. Must run inside the
/// caller's transaction.
pub(super) fn persist_chunks(
    connection: &Connection<'_>,
    version: &FileVersion,
    chunks: &[ContentChunk],
) -> Result<(), RepositoryError> {
    for chunk in chunks {
        ensure_extractor(connection, chunk)?;
        ensure_chunk(connection, chunk)?;
        link_version_to_chunk(connection, version, chunk)?;
        link_chunk_to_extractor(connection, chunk)?;
    }
    Ok(())
}

fn ensure_extractor(
    connection: &Connection<'_>,
    chunk: &ContentChunk,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MERGE (extractor:Extractor {id: $id}) ON CREATE SET extractor.extractor_id = $extractor_id, extractor.extractor_version = $extractor_version;",
        vec![
            ("id", Value::String(extractor_key(chunk))),
            (
                "extractor_id",
                Value::String(chunk.extractor_id().to_string()),
            ),
            (
                "extractor_version",
                Value::String(chunk.extractor_version().to_string()),
            ),
        ],
        "extractor upsert",
    )
}

fn ensure_chunk(connection: &Connection<'_>, chunk: &ContentChunk) -> Result<(), RepositoryError> {
    let stored = fetch_first_row(
        connection,
        "MATCH (chunk:ContentChunk {id: $id}) RETURN chunk.source_revision_id, chunk.ordinal, chunk.start_byte, chunk.end_byte, chunk.text_hash, chunk.text, chunk.script, chunk.extraction_method, chunk.extractor_id, chunk.extractor_version, chunk.config_hash, chunk.input_content_hash, chunk.created_at_unix_ms;",
        vec![("id", Value::String(chunk.id().to_string()))],
        "content chunk lookup",
    )?;
    if let Some(row) = stored {
        return verify_chunk(&row, chunk);
    }

    execute_statement(
        connection,
        "CREATE (:ContentChunk {id: $id, source_revision_id: $source_revision_id, ordinal: $ordinal, start_byte: $start_byte, end_byte: $end_byte, text_hash: $text_hash, text: $text, script: $script, extraction_method: $extraction_method, extractor_id: $extractor_id, extractor_version: $extractor_version, config_hash: $config_hash, input_content_hash: $input_content_hash, created_at_unix_ms: $created_at_unix_ms});",
        chunk_parameters(chunk)?,
        "content chunk insert",
    )
}

/// A stored chunk is immutable: text, byte range and the whole provenance contract must
/// match the retried chunk. `created_at_unix_ms` records the first write and is only
/// checked for its type.
fn verify_chunk(row: &[Value], chunk: &ContentChunk) -> Result<(), RepositoryError> {
    let expected = vec![
        Value::String(chunk.source_revision_id().to_string()),
        Value::Int64(i64::from(chunk.ordinal())),
        Value::Int64(u64_to_i64(chunk.start_byte(), "chunk start byte")?),
        Value::Int64(u64_to_i64(chunk.end_byte(), "chunk end byte")?),
        Value::String(chunk.text_hash().to_string()),
        Value::String(chunk.text().to_string()),
        Value::String(chunk.script().as_str().to_string()),
        Value::String(chunk.extraction_method().to_string()),
        Value::String(chunk.extractor_id().to_string()),
        Value::String(chunk.extractor_version().to_string()),
        Value::String(chunk.config_hash().to_string()),
        Value::String(chunk.input_content_hash().to_string()),
    ];

    let matches_stored = row.len() == expected.len() + 1
        && row[..expected.len()] == expected[..]
        && matches!(row.last(), Some(Value::Int64(_)));
    if matches_stored {
        return Ok(());
    }

    Err(RepositoryError::InvariantViolation(format!(
        "content chunk {} conflicts with immutable stored values",
        chunk.id()
    )))
}

fn link_version_to_chunk(
    connection: &Connection<'_>,
    version: &FileVersion,
    chunk: &ContentChunk,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (version:FileVersion {id: $version_id}), (chunk:ContentChunk {id: $chunk_id}) MERGE (version)-[:HAS_CHUNK]->(chunk);",
        vec![
            ("version_id", Value::String(version.id().to_string())),
            ("chunk_id", Value::String(chunk.id().to_string())),
        ],
        "version chunk link",
    )
}

fn link_chunk_to_extractor(
    connection: &Connection<'_>,
    chunk: &ContentChunk,
) -> Result<(), RepositoryError> {
    execute_statement(
        connection,
        "MATCH (chunk:ContentChunk {id: $chunk_id}), (extractor:Extractor {id: $extractor_id}) MERGE (chunk)-[:GENERATED_BY]->(extractor);",
        vec![
            ("chunk_id", Value::String(chunk.id().to_string())),
            ("extractor_id", Value::String(extractor_key(chunk))),
        ],
        "chunk extractor link",
    )
}

fn chunk_parameters(chunk: &ContentChunk) -> Result<Vec<(&'static str, Value)>, RepositoryError> {
    Ok(vec![
        ("id", Value::String(chunk.id().to_string())),
        (
            "source_revision_id",
            Value::String(chunk.source_revision_id().to_string()),
        ),
        ("ordinal", Value::Int64(i64::from(chunk.ordinal()))),
        (
            "start_byte",
            Value::Int64(u64_to_i64(chunk.start_byte(), "chunk start byte")?),
        ),
        (
            "end_byte",
            Value::Int64(u64_to_i64(chunk.end_byte(), "chunk end byte")?),
        ),
        ("text_hash", Value::String(chunk.text_hash().to_string())),
        ("text", Value::String(chunk.text().to_string())),
        ("script", Value::String(chunk.script().as_str().to_string())),
        (
            "extraction_method",
            Value::String(chunk.extraction_method().to_string()),
        ),
        (
            "extractor_id",
            Value::String(chunk.extractor_id().to_string()),
        ),
        (
            "extractor_version",
            Value::String(chunk.extractor_version().to_string()),
        ),
        (
            "config_hash",
            Value::String(chunk.config_hash().to_string()),
        ),
        (
            "input_content_hash",
            Value::String(chunk.input_content_hash().to_string()),
        ),
        (
            "created_at_unix_ms",
            Value::Int64(chunk.created_at_unix_ms()),
        ),
    ])
}

/// How many chunks one statement carries. Stage 2Q measured the peak following the size of
/// a single `UNWIND`, not the transaction, so this bounds it; the page is internal and has
/// no configuration. Stage 2R chose 100 by measurement on the largest source the policy
/// allows: 1 000 and 250 rows hold 525-537 MiB, 100 rows hold 481 MiB, and only the last
/// is under the 509 MiB bar. Time does not follow the page size within the spread.
const CHUNK_PAGE: usize = 100;

/// Production has exactly one page size, compiled in above. A test build lets the source
/// cost diagnostic vary it, because telling what paging is worth from what the link shape
/// is worth needs the same write measured both ways; no released binary reads this.
#[cfg(not(test))]
fn chunk_page() -> usize {
    CHUNK_PAGE
}

#[cfg(test)]
fn chunk_page() -> usize {
    std::env::var("CROSSKIT_DIAGNOSTIC_CHUNK_PAGE")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|page| *page > 0)
        .unwrap_or(CHUNK_PAGE)
}

/// The length prefix keeps the key unambiguous when an extractor id itself ends with
/// characters that could be read as the start of a version.
fn extractor_key(chunk: &ContentChunk) -> String {
    format!(
        "{}:{}{}",
        chunk.extractor_id().len(),
        chunk.extractor_id(),
        chunk.extractor_version()
    )
}

/// The statements that link a chunk that is new to the database. Each reaches its two
/// endpoints through a separate `MATCH`: written as one comma-separated pattern the planner
/// joins table scans instead of using the keys it was given, and `MERGE` scans the
/// relationship table even when both ends are known (ADR 0006, Stage 2R).
#[cfg(test)]
pub(super) const NEW_CHUNK_STATEMENTS: &[(&str, &str)] = &[
    ("chunk insert", CHUNK_INSERT),
    ("has_chunk create", HAS_CHUNK_CREATE),
    ("generated_by create", GENERATED_BY_CREATE),
];

/// The extractor upsert. It scans, and it stays: a batch carries at most a handful of
/// extractor rows against a table that holds one row per extractor version, so the scan is
/// of something that does not grow with the corpus. Every other scanning shape was taken
/// off the write path, because the rows reaching it did grow.
#[cfg(test)]
pub(super) const SCANNING_UPSERT: &[(&str, &str)] = &[("extractor upsert", EXTRACTOR_UPSERT)];

/// The repair path a chunk already in the database used to take, kept so the reason it was
/// removed stays checkable.
///
/// Stage 2R added it reading "only rows that were here before pay for it", which sounded
/// rare. It is not rare: a reconcile of an unchanged tree re-reads every file, so every
/// chunk of the corpus is already stored and every one of them took this path — two
/// relationship scans each, on every pass, for ever. What it repaired cannot happen either:
/// a chunk and its links are written by one transaction, `HAS_CHUNK` is `ONE_MANY` and
/// `GENERATED_BY` is `MANY_ONE`, so a stored chunk has exactly the links it was created
/// with unless something outside this program removed them.
#[cfg(test)]
pub(super) const REMOVED_REPAIR_STATEMENTS: &[(&str, &str)] = &[
    ("has_chunk repair", HAS_CHUNK_REPAIR),
    ("generated_by repair", GENERATED_BY_REPAIR),
];

/// The shapes Stage 2R replaced, kept so the reason for replacing them stays checkable.
#[cfg(test)]
pub(super) const REPLACED_LINK_SHAPES: &[(&str, &str)] = &[
    (
        "has_chunk, comma-separated merge",
        "UNWIND $rows AS row MATCH (version:FileVersion {id: row.source_revision_id}), (chunk:ContentChunk {id: row.id}) MERGE (version)-[:HAS_CHUNK]->(chunk) RETURN COUNT(*);",
    ),
    (
        "generated_by, comma-separated merge",
        "UNWIND $rows AS row MATCH (chunk:ContentChunk {id: row.id}), (extractor:Extractor {id: row.extractor_key}) MERGE (chunk)-[:GENERATED_BY]->(extractor) RETURN COUNT(*);",
    ),
];

const EXTRACTOR_UPSERT: &str = "UNWIND $rows AS row MERGE (extractor:Extractor {id: row.id}) ON CREATE SET extractor.extractor_id = row.extractor_id, extractor.extractor_version = row.extractor_version RETURN COUNT(extractor);";

const CHUNK_INSERT: &str = "UNWIND $rows AS row CREATE (chunk:ContentChunk {id: row.id, source_revision_id: row.source_revision_id, ordinal: row.ordinal, start_byte: row.start_byte, end_byte: row.end_byte, text_hash: row.text_hash, text: row.text, script: row.script, extraction_method: row.extraction_method, extractor_id: row.extractor_id, extractor_version: row.extractor_version, config_hash: row.config_hash, input_content_hash: row.input_content_hash, created_at_unix_ms: row.created_at_unix_ms}) RETURN COUNT(chunk);";

const HAS_CHUNK_CREATE: &str = "UNWIND $rows AS row MATCH (chunk:ContentChunk {id: row.id}) MATCH (version:FileVersion {id: row.source_revision_id}) CREATE (version)-[:HAS_CHUNK]->(chunk) RETURN COUNT(*);";

const GENERATED_BY_CREATE: &str = "UNWIND $rows AS row MATCH (chunk:ContentChunk {id: row.id}) MATCH (extractor:Extractor {id: row.extractor_key}) CREATE (chunk)-[:GENERATED_BY]->(extractor) RETURN COUNT(*);";

#[cfg(test)]
const HAS_CHUNK_REPAIR: &str = "UNWIND $rows AS row MATCH (version:FileVersion {id: row.source_revision_id}), (chunk:ContentChunk {id: row.id}) MERGE (version)-[:HAS_CHUNK]->(chunk) RETURN COUNT(*);";

#[cfg(test)]
const GENERATED_BY_REPAIR: &str = "UNWIND $rows AS row MATCH (chunk:ContentChunk {id: row.id}), (extractor:Extractor {id: row.extractor_key}) MERGE (chunk)-[:GENERATED_BY]->(extractor) RETURN COUNT(*);";

/// Persists the chunks and extractors of one batch with their links. A chunk already
/// stored is immutable: every provenance field must match before the batch may continue.
pub(super) fn persist_chunk_batch(
    connection: &Connection<'_>,
    chunks: &[ChunkRow<'_>],
    extractors: &[ExtractorRow],
) -> Result<(), RepositoryError> {
    execute_rows(
        connection,
        EXTRACTOR_UPSERT,
        extractors
            .iter()
            .map(|row| {
                Value::Struct(vec![
                    ("id".to_string(), Value::String(row.id.clone())),
                    (
                        "extractor_id".to_string(),
                        Value::String(row.extractor_id.clone()),
                    ),
                    (
                        "extractor_version".to_string(),
                        Value::String(row.extractor_version.clone()),
                    ),
                ])
            })
            .collect(),
        "extractor batch upsert",
    )?;

    for page in chunks.chunks(chunk_page()) {
        persist_chunk_page(connection, page)?;
    }
    Ok(())
}

/// One page of chunks, written whole: the same transaction holds every page, so a source
/// is still published entirely or not at all. Paging bounds what one statement carries,
/// which is what the peak follows (ADR 0006, Stage 2Q).
fn persist_chunk_page(
    connection: &Connection<'_>,
    chunks: &[ChunkRow<'_>],
) -> Result<(), RepositoryError> {
    let stored = stored_chunks(connection, chunks)?;
    let mut new_rows = Vec::new();
    for row in chunks {
        match stored.get(row.id) {
            Some(stored) => verify_stored_chunk(row, stored)?,
            None => new_rows.push(row),
        }
    }

    // A chunk that is new to the database cannot already have links: its node and its links
    // are written by this one transaction. The fast path is chosen from that reading, never
    // by attempting a write and catching the refusal.
    let inserts: Vec<Value> = new_rows.iter().map(|row| chunk_value(row)).collect();
    let new_links: Vec<Value> = new_rows.iter().map(|row| chunk_link_value(row)).collect();
    execute_rows(
        connection,
        CHUNK_INSERT,
        inserts,
        "content chunk batch insert",
    )?;
    execute_rows(
        connection,
        HAS_CHUNK_CREATE,
        new_links.clone(),
        "version chunk batch link",
    )?;
    execute_rows(
        connection,
        GENERATED_BY_CREATE,
        new_links,
        "chunk extractor batch link",
    )
}

/// The immutable contract of a stored chunk, in the order the lookup returns it.
struct StoredChunk {
    values: Vec<Value>,
}

fn stored_chunks(
    connection: &Connection<'_>,
    chunks: &[ChunkRow<'_>],
) -> Result<HashMap<String, StoredChunk>, RepositoryError> {
    const CONTEXT: &str = "content chunk batch lookup";
    let found = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (chunk:ContentChunk {id: id}) RETURN chunk.id, chunk.source_revision_id, chunk.ordinal, chunk.start_byte, chunk.end_byte, chunk.text_hash, chunk.text, chunk.script, chunk.extraction_method, chunk.extractor_id, chunk.extractor_version, chunk.config_hash, chunk.input_content_hash;",
        chunks.iter().map(|row| row.id.to_string()).collect(),
        CONTEXT,
    )?;
    let mut stored = HashMap::with_capacity(found.len());
    for values in found {
        stored.insert(
            text(&values, 0, CONTEXT)?,
            StoredChunk {
                values: values[1..].to_vec(),
            },
        );
    }
    Ok(stored)
}

/// The error names the chunk, never its text.
fn verify_stored_chunk(row: &ChunkRow<'_>, stored: &StoredChunk) -> Result<(), RepositoryError> {
    let expected = vec![
        Value::String(row.source_revision_id.to_string()),
        Value::Int64(row.ordinal),
        Value::Int64(row.start_byte),
        Value::Int64(row.end_byte),
        Value::String(row.text_hash.to_string()),
        Value::String(row.text.to_string()),
        Value::String(row.script.to_string()),
        Value::String(row.extraction_method.to_string()),
        Value::String(row.extractor_id.to_string()),
        Value::String(row.extractor_version.to_string()),
        Value::String(row.config_hash.to_string()),
        Value::String(row.input_content_hash.to_string()),
    ];
    if stored.values == expected {
        return Ok(());
    }
    Err(RepositoryError::InvariantViolation(format!(
        "content chunk {} conflicts with immutable stored values",
        row.id
    )))
}

pub(super) fn chunk_value(row: &ChunkRow<'_>) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.to_string())),
        (
            "source_revision_id".to_string(),
            Value::String(row.source_revision_id.to_string()),
        ),
        ("ordinal".to_string(), Value::Int64(row.ordinal)),
        ("start_byte".to_string(), Value::Int64(row.start_byte)),
        ("end_byte".to_string(), Value::Int64(row.end_byte)),
        (
            "text_hash".to_string(),
            Value::String(row.text_hash.to_string()),
        ),
        ("text".to_string(), Value::String(row.text.to_string())),
        ("script".to_string(), Value::String(row.script.to_string())),
        (
            "extraction_method".to_string(),
            Value::String(row.extraction_method.to_string()),
        ),
        (
            "extractor_id".to_string(),
            Value::String(row.extractor_id.to_string()),
        ),
        (
            "extractor_version".to_string(),
            Value::String(row.extractor_version.to_string()),
        ),
        (
            "config_hash".to_string(),
            Value::String(row.config_hash.to_string()),
        ),
        (
            "input_content_hash".to_string(),
            Value::String(row.input_content_hash.to_string()),
        ),
        (
            "created_at_unix_ms".to_string(),
            Value::Int64(row.created_at_unix_ms),
        ),
    ])
}

fn chunk_link_value(row: &ChunkRow<'_>) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.to_string())),
        (
            "source_revision_id".to_string(),
            Value::String(row.source_revision_id.to_string()),
        ),
        (
            "extractor_key".to_string(),
            Value::String(row.extractor_key.clone()),
        ),
    ])
}

/// Recomputes `ContentChunk.script` from the text the graph already holds.
///
/// The rule is `ChunkScript::of` and nothing else. A migrated graph and a freshly indexed
/// one have to write the same value for the same text, and two implementations of one rule
/// — one in Rust, one in Cypher — are two rules that agree only until they don't.
///
/// Keyset pages, so upgrading a large graph never materializes every chunk at once.
pub(super) fn backfill_chunk_scripts(
    connection: &Connection<'_>,
    page_size: usize,
) -> Result<(), RepositoryError> {
    const CONTEXT: &str = "content chunk script backfill";
    // `page_size` is an internal constant, never user input, so it may be spliced in.
    let page_query = format!(
        "MATCH (chunk:ContentChunk) WHERE chunk.id > $after_id RETURN chunk.id, chunk.text ORDER BY chunk.id LIMIT {page_size};"
    );

    let mut after_id = String::new();
    loop {
        let page = fetch_rows(
            connection,
            &page_query,
            vec![("after_id", Value::String(after_id.clone()))],
            CONTEXT,
        )?;
        if page.is_empty() {
            return Ok(());
        }

        let mut rows = Vec::with_capacity(page.len());
        for values in &page {
            let [Value::String(id), Value::String(text)] = values.as_slice() else {
                return Err(RepositoryError::InvariantViolation(format!(
                    "{CONTEXT} page returned invalid values: {values:?}"
                )));
            };
            after_id = id.clone();
            rows.push(Value::Struct(vec![
                ("id".to_string(), Value::String(id.clone())),
                (
                    "script".to_string(),
                    Value::String(ChunkScript::of(text).as_str().to_string()),
                ),
            ]));
        }
        execute_rows(connection, CHUNK_SCRIPT_UPDATE, rows, CONTEXT)?;
    }
}

const CHUNK_SCRIPT_UPDATE: &str = "UNWIND $rows AS row MATCH (chunk:ContentChunk {id: row.id}) SET chunk.script = row.script RETURN COUNT(chunk);";
