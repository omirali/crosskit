// Reading every current conversation back for stitching into tasks (ADR 0052).
//
// Every statement is fixed and takes its values as parameters, and reads go outside any
// transaction (ADR 0034). What is read is records: which chunk is whose and which
// conversations make one task is decided in `application::task_candidates`.
use std::collections::HashMap;

use crate::application::{ConversationLog, ConversationLogReader, RepositoryError};
use crate::domain::conversation::{Speaker, TouchVerb};

use super::conversation_store::{TIME_NOT_RECORDED, speaker_of};
use super::repository::LadybugGraphRepository;
use super::search_store::locations_of;
use super::store_support::{fetch_rows, fetch_rows_for_ids, number, text};

/// Every conversation whose revision is its file's current version. A transcript that grew
/// left a conversation at every revision; only the last saw all of it.
const CURRENT_CONVERSATIONS: &str = "MATCH (conversation:Conversation) MATCH (version:FileVersion {id: conversation.source_revision_id}) MATCH (file:File {id: version.file_id}) MATCH (file)-[:CURRENT_VERSION]->(current:FileVersion) WHERE current.id = version.id RETURN conversation.id, conversation.session_id, conversation.name, conversation.named_by, version.id, version.file_id;";

const REPLIES: &str = "UNWIND $ids AS id MATCH (conversation:Conversation {id: id})-[:HAS_MESSAGE]->(message:Message) RETURN conversation.id, message.ordinal, message.role, message.at_unix_ms;";

const CHUNKS: &str = "UNWIND $ids AS id MATCH (version:FileVersion {id: id})-[:HAS_CHUNK]->(chunk:ContentChunk) RETURN version.id, chunk.ordinal, chunk.text;";

const TOUCHES: &str = "UNWIND $ids AS id MATCH (conversation:Conversation {id: id})-[:TOUCHED_FILE]->(touch:FileTouch) RETURN conversation.id, touch.path, touch.verb, touch.at_unix_ms;";

/// How `named_by` says a conversation has no name, so chunk zero is a reply.
const NAMED_BY_NOBODY: &str = "nobody";

impl ConversationLogReader for LadybugGraphRepository {
    fn conversation_logs(&self) -> Result<Vec<ConversationLog>, RepositoryError> {
        const CONTEXT: &str = "conversations for stitching";
        let connection = self.read_connection()?;
        let rows = fetch_rows(&connection, CURRENT_CONVERSATIONS, Vec::new(), CONTEXT)?;
        let mut logs: Vec<(String, String, String, ConversationLog)> =
            Vec::with_capacity(rows.len());
        for values in rows {
            let name = text(&values, 2, CONTEXT)?;
            let named = text(&values, 3, CONTEXT)? != NAMED_BY_NOBODY;
            logs.push((
                text(&values, 0, CONTEXT)?,
                text(&values, 4, CONTEXT)?,
                text(&values, 5, CONTEXT)?,
                ConversationLog {
                    session_id: text(&values, 1, CONTEXT)?,
                    name: (!name.is_empty()).then_some(name),
                    named,
                    folder: None,
                    replies: Vec::new(),
                    chunks: Vec::new(),
                    touches: Vec::new(),
                },
            ));
        }
        let conversations: Vec<String> = logs.iter().map(|(id, ..)| id.clone()).collect();
        let revisions: Vec<String> = logs
            .iter()
            .map(|(_, revision, ..)| revision.clone())
            .collect();
        let files: Vec<String> = logs.iter().map(|(_, _, file, _)| file.clone()).collect();

        let mut replies = replies_of(&connection, conversations.clone())?;
        let mut chunks = chunks_of(&connection, revisions)?;
        let mut touches = touches_of(&connection, conversations)?;
        let folders: HashMap<String, String> = locations_of(&connection, files)?
            .into_iter()
            .filter_map(|(file, places)| {
                let (path, _) = places.into_iter().next()?;
                Some((file, path.parent()?.as_str().to_string()))
            })
            .collect();
        Ok(logs
            .into_iter()
            .map(|(conversation, revision, file, mut log)| {
                log.replies = replies.remove(&conversation).unwrap_or_default();
                log.chunks = chunks.remove(&revision).unwrap_or_default();
                log.touches = touches.remove(&conversation).unwrap_or_default();
                log.folder = folders.get(&file).cloned();
                log
            })
            .collect())
    }
}

/// A reply: who said it and when.
type Reply = (Speaker, Option<i64>);
type Replies = Vec<Reply>;
/// A touch: the path, the verb and when.
type Touch = (String, TouchVerb, Option<i64>);

/// Every reply of each conversation, by ordinal.
fn replies_of(
    connection: &lbug::Connection<'_>,
    conversations: Vec<String>,
) -> Result<HashMap<String, Replies>, RepositoryError> {
    const CONTEXT: &str = "replies for stitching";
    let mut found: HashMap<String, Vec<(i64, Reply)>> = HashMap::new();
    for values in fetch_rows_for_ids(connection, REPLIES, conversations, CONTEXT)? {
        let at = number(&values, 3, CONTEXT)?;
        found.entry(text(&values, 0, CONTEXT)?).or_default().push((
            number(&values, 1, CONTEXT)?,
            (
                speaker_of(&text(&values, 2, CONTEXT)?),
                (at != TIME_NOT_RECORDED).then_some(at),
            ),
        ));
    }
    Ok(found
        .into_iter()
        .map(|(conversation, mut replies)| {
            replies.sort_by_key(|(ordinal, _)| *ordinal);
            (
                conversation,
                replies.into_iter().map(|(_, reply)| reply).collect(),
            )
        })
        .collect())
}

/// Every chunk of each revision, with its ordinal and text.
fn chunks_of(
    connection: &lbug::Connection<'_>,
    revisions: Vec<String>,
) -> Result<HashMap<String, Vec<(u32, String)>>, RepositoryError> {
    const CONTEXT: &str = "chunks for stitching";
    let mut found: HashMap<String, Vec<(u32, String)>> = HashMap::new();
    for values in fetch_rows_for_ids(connection, CHUNKS, revisions, CONTEXT)? {
        let ordinal = u32::try_from(number(&values, 1, CONTEXT)?).map_err(|_| {
            RepositoryError::InvariantViolation(format!("{CONTEXT} returned a negative ordinal"))
        })?;
        found
            .entry(text(&values, 0, CONTEXT)?)
            .or_default()
            .push((ordinal, text(&values, 2, CONTEXT)?));
    }
    Ok(found)
}

/// Every touch of each conversation. A verb this build does not know is left out, the way
/// every other reader of touches leaves it.
fn touches_of(
    connection: &lbug::Connection<'_>,
    conversations: Vec<String>,
) -> Result<HashMap<String, Vec<Touch>>, RepositoryError> {
    const CONTEXT: &str = "touches for stitching";
    let mut found: HashMap<String, Vec<Touch>> = HashMap::new();
    for values in fetch_rows_for_ids(connection, TOUCHES, conversations, CONTEXT)? {
        let Some(verb) = TouchVerb::parse(&text(&values, 2, CONTEXT)?) else {
            continue;
        };
        let at = number(&values, 3, CONTEXT)?;
        found.entry(text(&values, 0, CONTEXT)?).or_default().push((
            text(&values, 1, CONTEXT)?,
            verb,
            (at != TIME_NOT_RECORDED).then_some(at),
        ));
    }
    Ok(found)
}
