// Writing the conversation a revision held: the conversation, its messages and the files it
// touched.
//
// One rule governs all three tables and it is the rule the chunks already follow: a row is
// identified by the revision it came from, so publishing the same revision twice writes
// nothing the second time, and a revision that is new brings its conversation with it inside
// the same transaction. Nothing here is updated in place — a transcript that grew is a new
// revision and gets a new conversation, exactly as it gets new chunks.
use std::collections::HashMap;

use lbug::{Connection, Value};

use crate::application::RepositoryError;
use crate::domain::conversation::Speaker;

use super::batch_projection::{ConversationRow, MessageRow, TouchRow};
use super::store_support::{execute_rows, fetch_rows_for_ids, text};

/// What the graph stores for a line the transcript gave no time for.
///
/// A sentinel and not a zero: zero is the first of January 1970, which is a date, and a
/// column that cannot tell "midnight in 1970" from "not recorded" is the `und` mistake of
/// Stage 8 in a numeric column. No transcript carries a negative timestamp, so this value
/// means one thing.
pub(super) const TIME_NOT_RECORDED: i64 = -1;

/// The id of the conversation a revision was read as.
///
/// Derived from the revision and from nothing else, which is what makes it a key both a
/// writer and a reader can compute: the writer to publish the same revision twice and write
/// nothing the second time, a reader to ask for the conversation of a revision by its key.
pub(super) fn conversation_id_of(revision_id: &str) -> String {
    format!("conversation:{revision_id}")
}

/// Who said a stored reply, by its `role`. Only the owner's replies are stored as `owner`; any
/// other value is the model's, the way the writer has only two to write.
pub(super) fn speaker_of(role: &str) -> Speaker {
    match role {
        "owner" => Speaker::Owner,
        _ => Speaker::Model,
    }
}

/// The id of the reply at `ordinal` of a conversation, derived the same way for the same
/// reason as the conversation's own.
pub(super) fn message_id_of(conversation_id: &str, ordinal: u64) -> String {
    format!("{conversation_id}:m{ordinal}")
}

/// Persists the conversations of one batch, with their messages and touches.
pub(super) fn persist_conversation_batch(
    connection: &Connection<'_>,
    conversations: &[ConversationRow<'_>],
    messages: &[MessageRow<'_>],
    touches: &[TouchRow<'_>],
) -> Result<(), RepositoryError> {
    if conversations.is_empty() {
        return Ok(());
    }
    const CONTEXT: &str = "conversation batch lookup";
    let stored = fetch_rows_for_ids(
        connection,
        "UNWIND $ids AS id MATCH (conversation:Conversation {id: id}) RETURN conversation.id;",
        conversations.iter().map(|row| row.id.clone()).collect(),
        CONTEXT,
    )?;
    let mut known = HashMap::with_capacity(stored.len());
    for values in stored {
        known.insert(text(&values, 0, CONTEXT)?, ());
    }

    let fresh: Vec<&ConversationRow<'_>> = conversations
        .iter()
        .filter(|row| !known.contains_key(&row.id))
        .collect();
    if fresh.is_empty() {
        return Ok(());
    }

    execute_rows(
        connection,
        CONVERSATION_INSERT,
        fresh.iter().map(|row| conversation_value(row)).collect(),
        "conversation batch insert",
    )?;
    execute_rows(
        connection,
        EVIDENCED_BY_CREATE,
        fresh
            .iter()
            .map(|row| {
                Value::Struct(vec![
                    ("id".to_string(), Value::String(row.id.clone())),
                    (
                        "source_revision_id".to_string(),
                        Value::String(row.source_revision_id.to_string()),
                    ),
                ])
            })
            .collect(),
        "conversation evidence link",
    )?;

    // Only the messages and touches of conversations this batch created. A conversation
    // already stored brought its own, and re-publishing them would either duplicate rows or
    // cost a lookup per row to find out that it would.
    let publishing: Vec<&str> = fresh.iter().map(|row| row.id.as_str()).collect();
    let mine = |conversation_id: &str| publishing.contains(&conversation_id);

    execute_rows(
        connection,
        MESSAGE_INSERT,
        messages
            .iter()
            .filter(|row| mine(&row.conversation_id))
            .map(message_value)
            .collect(),
        "message batch insert",
    )?;
    execute_rows(
        connection,
        HAS_MESSAGE_CREATE,
        messages
            .iter()
            .filter(|row| mine(&row.conversation_id))
            .map(|row| {
                Value::Struct(vec![
                    ("id".to_string(), Value::String(row.id.clone())),
                    (
                        "conversation_id".to_string(),
                        Value::String(row.conversation_id.clone()),
                    ),
                ])
            })
            .collect(),
        "message link",
    )?;
    execute_rows(
        connection,
        TOUCH_INSERT,
        touches
            .iter()
            .filter(|row| mine(&row.conversation_id))
            .map(touch_value)
            .collect(),
        "file touch batch insert",
    )?;
    execute_rows(
        connection,
        TOUCHED_FILE_CREATE,
        touches
            .iter()
            .filter(|row| mine(&row.conversation_id))
            .map(|row| {
                Value::Struct(vec![
                    ("id".to_string(), Value::String(row.id.clone())),
                    (
                        "conversation_id".to_string(),
                        Value::String(row.conversation_id.clone()),
                    ),
                ])
            })
            .collect(),
        "file touch link",
    )?;
    Ok(())
}

fn conversation_value(row: &ConversationRow<'_>) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.clone())),
        (
            "source_revision_id".to_string(),
            Value::String(row.source_revision_id.to_string()),
        ),
        (
            "session_id".to_string(),
            Value::String(row.session_id.to_string()),
        ),
        ("name".to_string(), Value::String(row.name.to_string())),
        (
            "named_by".to_string(),
            Value::String(row.named_by.to_string()),
        ),
        (
            "first_message_at_unix_ms".to_string(),
            Value::Int64(row.first_message_at_unix_ms),
        ),
        (
            "last_message_at_unix_ms".to_string(),
            Value::Int64(row.last_message_at_unix_ms),
        ),
        ("message_count".to_string(), Value::Int64(row.message_count)),
        (
            "created_at_unix_ms".to_string(),
            Value::Int64(row.created_at_unix_ms),
        ),
    ])
}

fn message_value(row: &MessageRow<'_>) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.clone())),
        (
            "conversation_id".to_string(),
            Value::String(row.conversation_id.clone()),
        ),
        ("role".to_string(), Value::String(row.role.to_string())),
        ("ordinal".to_string(), Value::Int64(row.ordinal)),
        ("at_unix_ms".to_string(), Value::Int64(row.at_unix_ms)),
    ])
}

fn touch_value(row: &TouchRow<'_>) -> Value {
    Value::Struct(vec![
        ("id".to_string(), Value::String(row.id.clone())),
        (
            "conversation_id".to_string(),
            Value::String(row.conversation_id.clone()),
        ),
        ("path".to_string(), Value::String(row.path.to_string())),
        ("verb".to_string(), Value::String(row.verb.to_string())),
        ("source".to_string(), Value::String(row.source.to_string())),
        ("at_unix_ms".to_string(), Value::Int64(row.at_unix_ms)),
        ("naming".to_string(), Value::String(row.naming.to_string())),
        ("times".to_string(), Value::Int64(row.times)),
    ])
}

const CONVERSATION_INSERT: &str = "UNWIND $rows AS row CREATE (conversation:Conversation {id: row.id, source_revision_id: row.source_revision_id, session_id: row.session_id, name: row.name, named_by: row.named_by, first_message_at_unix_ms: row.first_message_at_unix_ms, last_message_at_unix_ms: row.last_message_at_unix_ms, message_count: row.message_count, created_at_unix_ms: row.created_at_unix_ms}) RETURN COUNT(conversation);";

/// Section 4.3 of the architecture: an observation names the revision it was derived from.
/// Without this edge a conversation is a claim with no source, and the revision it belongs to
/// could be retired while it stayed behind.
const EVIDENCED_BY_CREATE: &str = "UNWIND $rows AS row MATCH (conversation:Conversation {id: row.id}) MATCH (version:FileVersion {id: row.source_revision_id}) CREATE (conversation)-[:EVIDENCED_BY]->(version) RETURN COUNT(*);";

const MESSAGE_INSERT: &str = "UNWIND $rows AS row CREATE (message:Message {id: row.id, conversation_id: row.conversation_id, role: row.role, ordinal: row.ordinal, at_unix_ms: row.at_unix_ms}) RETURN COUNT(message);";

const HAS_MESSAGE_CREATE: &str = "UNWIND $rows AS row MATCH (conversation:Conversation {id: row.conversation_id}) MATCH (message:Message {id: row.id}) CREATE (conversation)-[:HAS_MESSAGE]->(message) RETURN COUNT(*);";

const TOUCH_INSERT: &str = "UNWIND $rows AS row CREATE (touch:FileTouch {id: row.id, conversation_id: row.conversation_id, path: row.path, verb: row.verb, source: row.source, at_unix_ms: row.at_unix_ms, naming: row.naming, times: row.times}) RETURN COUNT(touch);";

const TOUCHED_FILE_CREATE: &str = "UNWIND $rows AS row MATCH (conversation:Conversation {id: row.conversation_id}) MATCH (touch:FileTouch {id: row.id}) CREATE (conversation)-[:TOUCHED_FILE]->(touch) RETURN COUNT(*);";
