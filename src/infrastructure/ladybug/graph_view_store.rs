// Reading the neighbourhood of a node for the graph view (UI-4).
//
// Every statement is fixed and takes its values as parameters; nothing a person typed or a
// transcript wrote is spliced into one. Reads go outside any transaction and without the write
// lock: the command that asks writes nothing and opens nothing for writing (ADR 0034).
//
// What is read here is records. Which pairs are drawn, which reply a touch belongs to and what
// fits in the bound are decided in `application::graph_explorer`.
use std::collections::HashMap;

use lbug::{Connection, LogicalType, Value};

use crate::application::{
    GraphReader, RepositoryError, StoredConversation, StoredFile, StoredReply, StoredTopic,
    StoredTouch,
};
use crate::domain::conversation::Speaker;

use super::conversation_store::{TIME_NOT_RECORDED, message_id_of, speaker_of};
use super::period_store::files_at;
use super::repository::LadybugGraphRepository;
use super::search_store::locations_of;
use super::store_support::{fetch_rows, fetch_rows_for_ids, number, struct_list, text};
use super::touch_store::touches_of_rows;

/// Every row of these sessions, with the file of the transcript each was read from. A scan of
/// `Conversation`, which holds a row per transcript revision and not per reply.
const CONVERSATIONS_OF_SESSIONS: &str = "MATCH (conversation:Conversation) WHERE list_contains($sessions, conversation.session_id) MATCH (version:FileVersion {id: conversation.source_revision_id}) RETURN conversation.id, conversation.session_id, conversation.name, conversation.message_count, conversation.last_message_at_unix_ms, version.file_id, version.id;";

/// Which version each named file currently has.
const CURRENT_VERSIONS: &str = "UNWIND $ids AS id MATCH (file:File {id: id})-[:CURRENT_VERSION]->(version:FileVersion) RETURN file.id, version.id;";

/// What the analysis concluded about each named revision, with the result it is read from. An
/// empty topic is how "nothing was concluded" is stored, and it is not a topic.
const TOPICS_OF_REVISIONS: &str = "UNWIND $ids AS id MATCH (result:InsightResult {source_revision_id: id}) WHERE result.topic <> '' RETURN result.source_revision_id, result.id, result.topic;";

/// The touches of these sessions, in the column order `touch_store` reads.
const TOUCHES_OF_SESSIONS: &str = "MATCH (conversation:Conversation)-[:TOUCHED_FILE]->(touch:FileTouch) WHERE list_contains($sessions, conversation.session_id) RETURN conversation.session_id, conversation.name, touch.path, touch.verb, touch.naming, touch.times, touch.at_unix_ms;";

/// The touches at exactly these paths, in the same order.
const TOUCHES_AT_PATHS: &str = "MATCH (conversation:Conversation)-[:TOUCHED_FILE]->(touch:FileTouch) WHERE list_contains($paths, touch.path) RETURN conversation.session_id, conversation.name, touch.path, touch.verb, touch.naming, touch.times, touch.at_unix_ms;";

/// For each `(conversation, moment)`, the ordinal of the latest reply said at or before it. A
/// reply whose time was not recorded is never placed: it could stand anywhere.
const LATEST_REPLY_BEFORE: &str = "UNWIND $rows AS row MATCH (conversation:Conversation {id: row.conversation_id}) MATCH (conversation)-[:HAS_MESSAGE]->(message:Message) WHERE message.at_unix_ms <> $not_recorded AND message.at_unix_ms <= row.at RETURN row.conversation_id, row.at, MAX(message.ordinal);";

/// The replies named, by their keys.
const REPLIES: &str = "UNWIND $ids AS id MATCH (message:Message {id: id}) RETURN message.id, message.role, message.at_unix_ms;";

impl GraphReader for LadybugGraphRepository {
    fn conversations_of_sessions(
        &self,
        sessions: &[String],
    ) -> Result<Vec<StoredConversation>, RepositoryError> {
        const CONTEXT: &str = "conversations of sessions";
        let connection = self.read_connection()?;
        let rows = fetch_rows(
            &connection,
            CONVERSATIONS_OF_SESSIONS,
            vec![("sessions", strings(sessions))],
            CONTEXT,
        )?;
        let mut read = Vec::with_capacity(rows.len());
        for values in rows {
            let last = number(&values, 4, CONTEXT)?;
            read.push((
                StoredConversation {
                    id: text(&values, 0, CONTEXT)?,
                    session_id: text(&values, 1, CONTEXT)?,
                    name: text(&values, 2, CONTEXT)?,
                    message_count: u64::try_from(number(&values, 3, CONTEXT)?).unwrap_or(0),
                    last_message_at_unix_ms: (last != TIME_NOT_RECORDED).then_some(last),
                    transcript_file_id: text(&values, 5, CONTEXT)?,
                    current: false,
                    topic: None,
                },
                text(&values, 6, CONTEXT)?,
            ));
        }
        let current = current_versions(
            &connection,
            read.iter()
                .map(|(row, _)| row.transcript_file_id.clone())
                .collect(),
        )?;
        let mut topics = topics_of(
            &connection,
            read.iter().map(|(_, revision)| revision.clone()).collect(),
        )?;
        Ok(read
            .into_iter()
            .map(|(mut row, revision)| {
                row.current = current.get(&row.transcript_file_id) == Some(&revision);
                row.topic = topics.remove(&revision);
                row
            })
            .collect())
    }

    fn touches_of_sessions(
        &self,
        sessions: &[String],
    ) -> Result<Vec<StoredTouch>, RepositoryError> {
        const CONTEXT: &str = "touches of sessions";
        let rows = fetch_rows(
            &self.read_connection()?,
            TOUCHES_OF_SESSIONS,
            vec![("sessions", strings(sessions))],
            CONTEXT,
        )?;
        touches_of_rows(rows, CONTEXT)
    }

    fn touches_at_paths(&self, paths: &[String]) -> Result<Vec<StoredTouch>, RepositoryError> {
        const CONTEXT: &str = "touches at paths";
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let rows = fetch_rows(
            &self.read_connection()?,
            TOUCHES_AT_PATHS,
            vec![("paths", strings(paths))],
            CONTEXT,
        )?;
        touches_of_rows(rows, CONTEXT)
    }

    fn files_at_paths(&self, paths: &[String]) -> Result<Vec<(String, String)>, RepositoryError> {
        let found = files_at(&self.read_connection()?, paths.to_vec())?;
        let mut pairs: Vec<(String, String)> = found
            .into_iter()
            .flat_map(|(path, files)| files.into_iter().map(move |file| (path.clone(), file)))
            .collect();
        pairs.sort();
        Ok(pairs)
    }

    fn files(&self, file_ids: &[String]) -> Result<Vec<StoredFile>, RepositoryError> {
        let connection = self.read_connection()?;
        let mut locations = locations_of(&connection, file_ids.to_vec())?;
        let current = current_versions(&connection, file_ids.to_vec())?;
        let mut topics = topics_of(&connection, current.values().cloned().collect())?;
        let mut files: Vec<StoredFile> = file_ids
            .iter()
            .filter_map(|file_id| {
                let mut held = locations.remove(file_id)?;
                held.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
                Some(StoredFile {
                    file_id: file_id.clone(),
                    locations: held,
                    topic: current
                        .get(file_id)
                        .and_then(|revision| topics.remove(revision)),
                })
            })
            .collect();
        files.sort_by(|left, right| left.file_id.cmp(&right.file_id));
        Ok(files)
    }

    fn replies_before(
        &self,
        moments: &[(String, i64)],
    ) -> Result<Vec<StoredReply>, RepositoryError> {
        const CONTEXT: &str = "replies before touches";
        if moments.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.read_connection()?;
        let rows = moments
            .iter()
            .map(|(conversation_id, at)| {
                Value::Struct(vec![
                    (
                        "conversation_id".to_string(),
                        Value::String(conversation_id.clone()),
                    ),
                    ("at".to_string(), Value::Int64(*at)),
                ])
            })
            .collect();
        let latest = fetch_rows(
            &connection,
            LATEST_REPLY_BEFORE,
            vec![
                ("rows", struct_list(rows)),
                ("not_recorded", Value::Int64(TIME_NOT_RECORDED)),
            ],
            CONTEXT,
        )?;
        let mut placed: Vec<(String, i64, u64)> = Vec::with_capacity(latest.len());
        for values in latest {
            // A moment with no reply before it matches no message, and so has no row here.
            placed.push((
                text(&values, 0, CONTEXT)?,
                number(&values, 1, CONTEXT)?,
                u64::try_from(number(&values, 2, CONTEXT)?).unwrap_or(0),
            ));
        }

        let ids: Vec<String> = placed
            .iter()
            .map(|(conversation_id, _, ordinal)| message_id_of(conversation_id, *ordinal))
            .collect();
        let mut replies: HashMap<String, (Speaker, i64)> = HashMap::new();
        for values in fetch_rows_for_ids(&connection, REPLIES, ids, CONTEXT)? {
            let speaker = speaker_of(&text(&values, 1, CONTEXT)?);
            replies.insert(
                text(&values, 0, CONTEXT)?,
                (speaker, number(&values, 2, CONTEXT)?),
            );
        }
        Ok(placed
            .into_iter()
            .filter_map(|(conversation_id, touched_at_unix_ms, ordinal)| {
                let message_id = message_id_of(&conversation_id, ordinal);
                let (speaker, at_unix_ms) = *replies.get(&message_id)?;
                Some(StoredReply {
                    conversation_id,
                    touched_at_unix_ms,
                    message_id,
                    ordinal,
                    speaker,
                    at_unix_ms,
                })
            })
            .collect())
    }
}

fn strings(values: &[String]) -> Value {
    Value::List(
        LogicalType::String,
        values.iter().cloned().map(Value::String).collect(),
    )
}

fn current_versions(
    connection: &Connection<'_>,
    file_ids: Vec<String>,
) -> Result<HashMap<String, String>, RepositoryError> {
    const CONTEXT: &str = "current versions of viewed files";
    fetch_rows_for_ids(connection, CURRENT_VERSIONS, file_ids, CONTEXT)?
        .into_iter()
        .map(|values| Ok((text(&values, 0, CONTEXT)?, text(&values, 1, CONTEXT)?)))
        .collect()
}

/// The topic of each revision that has one. Two results for one revision keep the first by
/// id, so a run reads the same topic as the one before it.
fn topics_of(
    connection: &Connection<'_>,
    revision_ids: Vec<String>,
) -> Result<HashMap<String, StoredTopic>, RepositoryError> {
    const CONTEXT: &str = "topics of viewed revisions";
    let mut topics: HashMap<String, StoredTopic> = HashMap::new();
    for values in fetch_rows_for_ids(connection, TOPICS_OF_REVISIONS, revision_ids, CONTEXT)? {
        let revision = text(&values, 0, CONTEXT)?;
        let topic = StoredTopic {
            result_id: text(&values, 1, CONTEXT)?,
            label: text(&values, 2, CONTEXT)?.trim().to_string(),
        };
        if topic.label.is_empty() {
            continue;
        }
        let keep = topics
            .get(&revision)
            .is_none_or(|held| topic.result_id < held.result_id);
        if keep {
            topics.insert(revision, topic);
        }
    }
    Ok(topics)
}
