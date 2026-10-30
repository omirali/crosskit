// Reading the events of a period out of one context's graph: replies said in it, and files
// touched in it.
//
// Every read here is of the conversations of *current* transcript revisions. A transcript is
// appended to, so the same session is stored again at every revision that saw it grow, and
// the earlier copies hold the same replies with the same moments; reading them too would
// count each reply once per revision and admit chunks no index holds.
//
// Nothing here decides what is inside the period. The window narrows in the database, and the
// rules — which chunk is which reply, which touch is deliberate — are applied above, where
// they can be read.
use std::collections::{HashMap, HashSet};

use lbug::{Connection, LogicalType, Value};

use crate::application::{ConversationTimeline, PeriodEventReader, PeriodTouch, RepositoryError};
use crate::domain::conversation::TouchVerb;
use crate::domain::{ExtractionProfile, LocationStatus, OwnerNaming};

use super::conversation_store::TIME_NOT_RECORDED;
use super::location_store::status_label;
use super::search_store::LadybugSearchGraph;
use super::store_support::{fetch_rows, fetch_rows_for_ids, number, text};

/// The conversations that said anything in the window, of any revision. Narrowed to current
/// revisions by the next statement: a scan of `Message` is one pass over one column, and the
/// join to the current version is cheaper on the few ids that survive it.
const CONVERSATIONS_SPEAKING: &str = "MATCH (message:Message) WHERE message.at_unix_ms >= $from_ms AND message.at_unix_ms < $before_ms RETURN DISTINCT message.conversation_id;";

/// Of the named conversations, the ones whose revision is the current version of its file,
/// with the file and whether the conversation has a name. Each key is bound in a `MATCH` of
/// its own, which is Stage 2N's lesson on reaching a row by its key.
const CURRENT_CONVERSATIONS: &str = "UNWIND $ids AS id MATCH (conversation:Conversation {id: id}) MATCH (version:FileVersion {id: conversation.source_revision_id}) MATCH (file:File {id: version.file_id}) MATCH (file)-[:CURRENT_VERSION]->(current:FileVersion) WHERE current.id = version.id RETURN conversation.id, version.file_id, version.id, conversation.named_by;";

const REPLY_MOMENTS: &str = "UNWIND $ids AS id MATCH (conversation:Conversation {id: id}) MATCH (conversation)-[:HAS_MESSAGE]->(message:Message) RETURN conversation.id, message.ordinal, message.at_unix_ms;";

const REVISION_CHUNKS: &str = "UNWIND $ids AS id MATCH (version:FileVersion {id: id}) MATCH (version)-[:HAS_CHUNK]->(chunk:ContentChunk) RETURN version.id, chunk.ordinal, chunk.id;";

const TOUCHES_IN_WINDOW: &str = "MATCH (touch:FileTouch) WHERE touch.at_unix_ms >= $from_ms AND touch.at_unix_ms < $before_ms RETURN touch.conversation_id, touch.path, touch.verb, touch.naming, touch.at_unix_ms;";

/// The files at these paths in this context. A touch names a path as a string (ADR 0037), and
/// a path no location of this graph holds is a file of another context or of none.
const FILES_AT_PATHS: &str = "MATCH (location:FileLocation) WHERE list_contains($paths, location.normalized_path) RETURN location.normalized_path, location.file_id;";

/// Transcripts at a present path written on disk at or after the period's start, with the time
/// they were written. Narrowed by suffix in the database; the suffix is the extraction
/// profile's own rule, passed in rather than written here a second time.
const TRANSCRIPTS_WRITTEN_SINCE: &str = "MATCH (location:FileLocation) WHERE location.status = $present AND location.modified_at_unix_ms >= $from_ms AND location.normalized_path ENDS WITH $suffix RETURN location.file_id, location.normalized_path, location.modified_at_unix_ms;";

/// When the graph last read each of these files: the moment of its current observation.
const LAST_READ: &str = "UNWIND $ids AS id MATCH (file:File {id: id}) MATCH (file)-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN file.id, observation.observed_at_unix_ms;";

/// One current conversation: its file, its revision and whether chunk zero is its name.
struct CurrentConversation {
    file_id: String,
    revision_id: String,
    named: bool,
}

impl PeriodEventReader for LadybugSearchGraph {
    fn conversations_speaking_between(
        &self,
        start_unix_ms: i64,
        end_unix_ms: i64,
    ) -> Result<Vec<ConversationTimeline>, RepositoryError> {
        const CONTEXT: &str = "conversations of a period";
        let connection = self.connect()?;
        let speaking = fetch_rows(
            &connection,
            CONVERSATIONS_SPEAKING,
            window(start_unix_ms, end_unix_ms),
            CONTEXT,
        )?
        .iter()
        .map(|values| text(values, 0, CONTEXT))
        .collect::<Result<Vec<_>, _>>()?;
        let current = current_conversations(&connection, speaking)?;
        if current.is_empty() {
            return Ok(Vec::new());
        }

        let mut moments: HashMap<String, Vec<(i64, Option<i64>)>> = HashMap::new();
        for values in fetch_rows_for_ids(
            &connection,
            REPLY_MOMENTS,
            current.keys().cloned().collect(),
            CONTEXT,
        )? {
            let at = match number(&values, 2, CONTEXT)? {
                TIME_NOT_RECORDED => None,
                at => Some(at),
            };
            moments
                .entry(text(&values, 0, CONTEXT)?)
                .or_default()
                .push((number(&values, 1, CONTEXT)?, at));
        }

        let mut chunks: HashMap<String, Vec<(u32, String)>> = HashMap::new();
        for values in fetch_rows_for_ids(
            &connection,
            REVISION_CHUNKS,
            current
                .values()
                .map(|conversation| conversation.revision_id.clone())
                .collect(),
            CONTEXT,
        )? {
            let ordinal = u32::try_from(number(&values, 1, CONTEXT)?).map_err(|_| {
                RepositoryError::InvariantViolation(format!(
                    "{CONTEXT} returned a chunk ordinal outside its range"
                ))
            })?;
            chunks
                .entry(text(&values, 0, CONTEXT)?)
                .or_default()
                .push((ordinal, text(&values, 2, CONTEXT)?));
        }

        Ok(current
            .into_iter()
            .map(|(conversation_id, conversation)| {
                let mut replies = moments.remove(&conversation_id).unwrap_or_default();
                replies.sort_unstable_by_key(|(ordinal, _)| *ordinal);
                ConversationTimeline::new(
                    conversation.file_id,
                    conversation.named,
                    replies.into_iter().map(|(_, at)| at).collect(),
                    chunks.remove(&conversation.revision_id).unwrap_or_default(),
                )
            })
            .collect())
    }

    fn touches_between(
        &self,
        start_unix_ms: i64,
        end_unix_ms: i64,
    ) -> Result<Vec<PeriodTouch>, RepositoryError> {
        const CONTEXT: &str = "touches of a period";
        let connection = self.connect()?;
        let mut touches = Vec::new();
        for values in fetch_rows(
            &connection,
            TOUCHES_IN_WINDOW,
            window(start_unix_ms, end_unix_ms),
            CONTEXT,
        )? {
            // A verb this build does not know is not a touch it can answer with, exactly as
            // `touches` leaves it out.
            let Some(verb) = TouchVerb::parse(&text(&values, 2, CONTEXT)?) else {
                continue;
            };
            touches.push((
                text(&values, 0, CONTEXT)?,
                text(&values, 1, CONTEXT)?,
                verb,
                OwnerNaming::parse(&text(&values, 3, CONTEXT)?),
                number(&values, 4, CONTEXT)?,
            ));
        }
        if touches.is_empty() {
            return Ok(Vec::new());
        }

        let conversations: HashSet<String> = touches.iter().map(|touch| touch.0.clone()).collect();
        let current = current_conversations(&connection, conversations.into_iter().collect())?;
        touches.retain(|touch| current.contains_key(&touch.0));
        let paths: HashSet<String> = touches.iter().map(|touch| touch.1.clone()).collect();
        let files = files_at(&connection, paths.into_iter().collect())?;

        let mut admitted = Vec::new();
        for (_, path, verb, naming, at_unix_ms) in touches {
            for file_id in files.get(&path).into_iter().flatten() {
                admitted.push(PeriodTouch::new(file_id, at_unix_ms, verb, naming));
            }
        }
        Ok(admitted)
    }

    /// The modification time is the one the last pass recorded on the location, and the read
    /// time is the current observation's. A pass that read the new bytes observes them after
    /// they were written, so a location written later than its last observation is a change
    /// the pass saw and published no revision for.
    fn transcripts_behind_since(&self, start_unix_ms: i64) -> Result<Vec<String>, RepositoryError> {
        const CONTEXT: &str = "transcripts behind a period";
        let connection = self.connect()?;
        let mut written: Vec<(String, String, i64)> = Vec::new();
        for values in fetch_rows(
            &connection,
            TRANSCRIPTS_WRITTEN_SINCE,
            vec![
                (
                    "present",
                    Value::String(status_label(LocationStatus::Present).to_string()),
                ),
                ("from_ms", Value::Int64(start_unix_ms)),
                (
                    "suffix",
                    Value::String(ExtractionProfile::transcript_suffix().to_string()),
                ),
            ],
            CONTEXT,
        )? {
            written.push((
                text(&values, 0, CONTEXT)?,
                text(&values, 1, CONTEXT)?,
                number(&values, 2, CONTEXT)?,
            ));
        }
        let mut last_read: HashMap<String, i64> = HashMap::new();
        for values in fetch_rows_for_ids(
            &connection,
            LAST_READ,
            written
                .iter()
                .map(|(file_id, _, _)| file_id.clone())
                .collect(),
            CONTEXT,
        )? {
            last_read.insert(text(&values, 0, CONTEXT)?, number(&values, 1, CONTEXT)?);
        }
        Ok(written
            .into_iter()
            .filter(|(file_id, _, modified)| {
                last_read.get(file_id).is_none_or(|read| modified > read)
            })
            .map(|(_, path, _)| path)
            .collect())
    }
}

fn window(start_unix_ms: i64, end_unix_ms: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("from_ms", Value::Int64(start_unix_ms)),
        ("before_ms", Value::Int64(end_unix_ms)),
    ]
}

/// The conversations among `ids` that belong to the current version of their transcript,
/// by conversation id.
fn current_conversations(
    connection: &Connection<'_>,
    ids: Vec<String>,
) -> Result<HashMap<String, CurrentConversation>, RepositoryError> {
    const CONTEXT: &str = "current conversations";
    let mut current = HashMap::new();
    for values in fetch_rows_for_ids(connection, CURRENT_CONVERSATIONS, ids, CONTEXT)? {
        current.insert(
            text(&values, 0, CONTEXT)?,
            CurrentConversation {
                file_id: text(&values, 1, CONTEXT)?,
                revision_id: text(&values, 2, CONTEXT)?,
                // `nobody` is how the graph stores a conversation with no name, and then chunk
                // zero is its first reply (`batch_projection`).
                named: text(&values, 3, CONTEXT)? != "nobody",
            },
        );
    }
    Ok(current)
}

/// The files of this context at each of these paths, by path.
pub(super) fn files_at(
    connection: &Connection<'_>,
    paths: Vec<String>,
) -> Result<HashMap<String, Vec<String>>, RepositoryError> {
    const CONTEXT: &str = "files at touched paths";
    if paths.is_empty() {
        return Ok(HashMap::new());
    }
    let list = Value::List(
        LogicalType::String,
        paths.into_iter().map(Value::String).collect(),
    );
    let mut files: HashMap<String, Vec<String>> = HashMap::new();
    for values in fetch_rows(connection, FILES_AT_PATHS, vec![("paths", list)], CONTEXT)? {
        let file_id = text(&values, 1, CONTEXT)?;
        let held = files.entry(text(&values, 0, CONTEXT)?).or_default();
        if !held.contains(&file_id) {
            held.push(file_id);
        }
    }
    Ok(files)
}
