// The graph side of search, read from the context's own database.
//
// Four reads, and every one of them is about current versions only. A file's earlier
// revisions stay in the graph and are never indexed: that is what bounds the index by the
// number of files instead of by the number of times they were edited, and it is why the
// count these reads return is the count `doctor` compares the index against.
use std::collections::HashMap;
use std::sync::Arc;

use lbug::{Connection, Value};

use crate::application::{
    MatchedConversation, RepositoryError, ResolvedChunk, SearchDocument, SearchGraphReader,
};
use crate::domain::{ExtractionProfile, LocationStatus, NormalizedPath, SourceLanguage};

use super::conversation_store::{TIME_NOT_RECORDED, conversation_id_of};
use super::location_store::status_from_label;
use super::manager::LadybugDatabaseManager;
use super::store_support::{fetch_count, fetch_rows, fetch_rows_for_ids, number, text};

/// Every chunk of every current version, ordered by chunk id so a rebuild can walk them in
/// keyset pages.
const CURRENT_DOCUMENT_PAGE: &str = "MATCH (:File)-[:CURRENT_VERSION]->(version:FileVersion)-[:HAS_CHUNK]->(chunk:ContentChunk) WHERE chunk.id > $after_id RETURN chunk.id, version.file_id, chunk.text, chunk.extractor_id, chunk.ordinal ORDER BY chunk.id LIMIT";

/// Which version each named file currently has. Primary key lookups, one statement.
const CURRENT_VERSIONS: &str = "UNWIND $ids AS id MATCH (file:File {id: id})-[:CURRENT_VERSION]->(version:FileVersion) RETURN file.id, version.id;";

/// How many chunks the current versions hold: what an index in step with the graph has a
/// document for.
const CURRENT_CHUNK_COUNT: &str = "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion)-[:HAS_CHUNK]->(chunk:ContentChunk) RETURN COUNT(chunk);";

/// The text of each matched chunk and which file it belongs to.
///
/// Both keys are bound in a `MATCH` of their own, which is Stage 2N's lesson on a new shape
/// of query: written as one pattern — `(version:FileVersion)-[:HAS_CHUNK]->(chunk {id: id})`
/// — the planner reaches the chunk through its version instead of by its own key. Measured
/// together with the statement below, the two forms differ by 9.9 ms against 3.9 ms for one
/// page of ten results.
const MATCHED_CHUNKS: &str = "UNWIND $ids AS id MATCH (chunk:ContentChunk {id: id}) MATCH (version:FileVersion {id: chunk.source_revision_id}) RETURN chunk.id, version.file_id, chunk.text, chunk.source_revision_id;";

/// What the analysis concluded about each named revision, for the revisions that were
/// analysed. A revision with no result is simply absent from the answer: most revisions are,
/// and that is the ordinary state rather than a gap to be filled.
///
/// The empty label is filtered here rather than downstream, because "nothing was concluded"
/// is stored as an empty topic at confidence zero and is not a topic anybody should read.
const TOPICS_OF_REVISIONS: &str = "UNWIND $ids AS id MATCH (result:InsightResult {source_revision_id: id}) RETURN result.source_revision_id, result.topic;";

/// The conversation each named revision was read as, for the revisions that were read as one.
/// Asked by the conversation's own key, which is derived from the revision's.
const CONVERSATIONS_OF_REVISIONS: &str = "UNWIND $ids AS id MATCH (conversation:Conversation {id: id}) RETURN conversation.source_revision_id, conversation.session_id, conversation.name, conversation.message_count, conversation.last_message_at_unix_ms;";

/// Every place the named files are known to be, with the status of each. The file is bound
/// by its key and the edge expanded from it, for the same reason as above.
const LOCATIONS_OF_FILES: &str = "UNWIND $ids AS id MATCH (file:File {id: id}) MATCH (file)-[:LOCATED_AT]->(location:FileLocation) RETURN file.id, location.normalized_path, location.status;";

pub struct LadybugSearchGraph {
    manager: Arc<LadybugDatabaseManager>,
}

impl LadybugSearchGraph {
    pub fn new(manager: Arc<LadybugDatabaseManager>) -> Self {
        Self { manager }
    }

    /// Reads outside any transaction and without the write lock, like every other read of
    /// what is already durable.
    pub(super) fn connect(&self) -> Result<Connection<'_>, RepositoryError> {
        Connection::new(&self.manager.owner.database)
            .map_err(|error| RepositoryError::Storage(format!("open search connection: {error}")))
    }
}

impl SearchGraphReader for LadybugSearchGraph {
    /// The names of the files this page belongs to are read in a second statement rather than
    /// joined onto the first. A file may be at several paths, and a join would then hand back
    /// one row per path per chunk — the same chunk indexed twice, and a keyset page that can
    /// split a chunk's rows across two pages. Asked separately, the paths of at most one
    /// page's files are read once and the language decided per file.
    fn current_documents(
        &self,
        after_chunk_id: &str,
        limit: usize,
    ) -> Result<Vec<SearchDocument>, RepositoryError> {
        const CONTEXT: &str = "search index document page";
        let connection = self.connect()?;
        let rows = fetch_rows(
            &connection,
            &format!("{CURRENT_DOCUMENT_PAGE} {limit};"),
            vec![("after_id", Value::String(after_chunk_id.to_string()))],
            CONTEXT,
        )?;

        let mut chunks = Vec::with_capacity(rows.len());
        let mut file_ids: Vec<String> = Vec::new();
        for values in rows {
            let chunk_id = text(&values, 0, CONTEXT)?;
            let file_id = text(&values, 1, CONTEXT)?;
            let chunk_text = text(&values, 2, CONTEXT)?;
            let extractor_id = text(&values, 3, CONTEXT)?;
            let ordinal = u32::try_from(number(&values, 4, CONTEXT)?).unwrap_or(u32::MAX);
            if !file_ids.contains(&file_id) {
                file_ids.push(file_id.clone());
            }
            chunks.push((chunk_id, file_id, chunk_text, extractor_id, ordinal));
        }
        let languages = self.languages_of(&connection, file_ids)?;

        Ok(chunks
            .into_iter()
            .map(|(chunk_id, file_id, chunk_text, extractor_id, ordinal)| {
                let document = SearchDocument::new(
                    &chunk_id,
                    &file_id,
                    languages.get(&file_id).copied().flatten(),
                    &chunk_text,
                );
                // The same rule the pass applies, so a rebuilt index and a freshly written
                // one hold the same name documents.
                match ExtractionProfile::of_extractor_id(&extractor_id)
                    .is_some_and(|profile| profile.chunk_is_the_conversation_name(ordinal))
                {
                    true => document.naming_a_conversation(chunk_text),
                    false => document,
                }
            })
            .collect())
    }

    fn current_versions(
        &self,
        file_ids: &[String],
    ) -> Result<HashMap<String, String>, RepositoryError> {
        const CONTEXT: &str = "search index current versions";
        let connection = self.connect()?;
        let rows = fetch_rows_for_ids(&connection, CURRENT_VERSIONS, file_ids.to_vec(), CONTEXT)?;
        rows.into_iter()
            .map(|values| Ok((text(&values, 0, CONTEXT)?, text(&values, 1, CONTEXT)?)))
            .collect()
    }

    fn current_chunk_count(&self) -> Result<u64, RepositoryError> {
        let connection = self.connect()?;
        let count = fetch_count(
            &connection,
            CURRENT_CHUNK_COUNT,
            vec![],
            "search index chunk count",
        )?;
        Ok(count as u64)
    }

    fn resolve(&self, chunk_ids: &[String]) -> Result<Vec<ResolvedChunk>, RepositoryError> {
        const CONTEXT: &str = "search match resolution";
        let connection = self.connect()?;
        let matched = fetch_rows_for_ids(&connection, MATCHED_CHUNKS, chunk_ids.to_vec(), CONTEXT)?;

        let mut chunks = Vec::with_capacity(matched.len());
        let mut file_ids = Vec::with_capacity(matched.len());
        let mut revision_ids = Vec::with_capacity(matched.len());
        for values in matched {
            let chunk_id = text(&values, 0, CONTEXT)?;
            let file_id = text(&values, 1, CONTEXT)?;
            let chunk_text = text(&values, 2, CONTEXT)?;
            let revision_id = text(&values, 3, CONTEXT)?;
            if !file_ids.contains(&file_id) {
                file_ids.push(file_id.clone());
            }
            if !revision_ids.contains(&revision_id) {
                revision_ids.push(revision_id.clone());
            }
            chunks.push((chunk_id, file_id, chunk_text, revision_id));
        }

        let locations = locations_of(&connection, file_ids)?;
        let conversations = self.conversations_of(&connection, &revision_ids)?;
        let topics = self.topics_of(&connection, revision_ids)?;
        Ok(chunks
            .into_iter()
            .map(|(chunk_id, file_id, chunk_text, revision_id)| {
                let chunk = ResolvedChunk::new(
                    chunk_id,
                    chunk_text,
                    locations.get(&file_id).cloned().unwrap_or_default(),
                    topics.get(&revision_id).cloned(),
                );
                match conversations.get(&revision_id) {
                    Some(conversation) => chunk.in_conversation(conversation.clone()),
                    None => chunk,
                }
            })
            .collect())
    }
}

impl LadybugSearchGraph {
    /// What the names of each of these files declare them to be written in.
    ///
    /// Every location counts, present or missing. A file whose last name on disk is gone
    /// keeps its documents — that is what `--include-missing` shows — and labelling it by
    /// the names that still exist would give a rebuilt index a different answer from the
    /// scan that first wrote it, for no reason a reader could see.
    fn languages_of(
        &self,
        connection: &Connection<'_>,
        file_ids: Vec<String>,
    ) -> Result<HashMap<String, Option<SourceLanguage>>, RepositoryError> {
        let located = locations_of(connection, file_ids)?;
        Ok(located
            .into_iter()
            .map(|(file_id, locations)| {
                let language = SourceLanguage::of_paths(locations.iter().map(|(path, _)| path));
                (file_id, language)
            })
            .collect())
    }

    /// The conversation each of these revisions was read as, keyed by revision.
    fn conversations_of(
        &self,
        connection: &Connection<'_>,
        revision_ids: &[String],
    ) -> Result<HashMap<String, MatchedConversation>, RepositoryError> {
        const CONTEXT: &str = "conversations of matched revisions";
        let ids = revision_ids
            .iter()
            .map(|revision_id| conversation_id_of(revision_id))
            .collect();
        let rows = fetch_rows_for_ids(connection, CONVERSATIONS_OF_REVISIONS, ids, CONTEXT)?;
        rows.into_iter()
            .map(|values| {
                let revision_id = text(&values, 0, CONTEXT)?;
                let name = text(&values, 2, CONTEXT)?;
                let last_said = number(&values, 4, CONTEXT)?;
                Ok((
                    revision_id,
                    MatchedConversation::new(
                        text(&values, 1, CONTEXT)?,
                        Some(name),
                        u64::try_from(number(&values, 3, CONTEXT)?).unwrap_or(0),
                        (last_said != TIME_NOT_RECORDED).then_some(last_said),
                    ),
                ))
            })
            .collect()
    }

    /// What was concluded about each of these revisions, for those that were analysed.
    fn topics_of(
        &self,
        connection: &Connection<'_>,
        revision_ids: Vec<String>,
    ) -> Result<HashMap<String, String>, RepositoryError> {
        const CONTEXT: &str = "revision topics";
        let rows = fetch_rows_for_ids(connection, TOPICS_OF_REVISIONS, revision_ids, CONTEXT)?;
        rows.into_iter()
            .map(|values| Ok((text(&values, 0, CONTEXT)?, text(&values, 1, CONTEXT)?)))
            .collect()
    }
}

/// Every location of every named file, grouped by file.
///
/// A path the graph cannot read back as a normalized path is left out rather than guessed at:
/// a result must name a path the policy can be applied to, and a path this program cannot
/// parse is one it cannot decide about. The graph view reads the same, for the same reason.
pub(super) fn locations_of(
    connection: &Connection<'_>,
    file_ids: Vec<String>,
) -> Result<HashMap<String, Vec<(NormalizedPath, LocationStatus)>>, RepositoryError> {
    const CONTEXT: &str = "search match locations";
    let rows = fetch_rows_for_ids(connection, LOCATIONS_OF_FILES, file_ids, CONTEXT)?;
    let mut locations: HashMap<String, Vec<(NormalizedPath, LocationStatus)>> = HashMap::new();
    for values in rows {
        let file_id = text(&values, 0, CONTEXT)?;
        let path = text(&values, 1, CONTEXT)?;
        let status = text(&values, 2, CONTEXT)?;
        let (Ok(path), Some(status)) = (
            NormalizedPath::from_absolute(&path),
            status_from_label(&status),
        ) else {
            continue;
        };
        locations.entry(file_id).or_default().push((path, status));
    }
    Ok(locations)
}
