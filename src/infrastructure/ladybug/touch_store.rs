// Reading the files a conversation touched, from either side.
//
// Both queries narrow in the database and neither of them decides anything: the boundary
// rule for a path tail and the rule for what belongs in a strict answer live in
// `application::file_touches`, because they are what the answer means rather than how it is
// fetched.
//
// The touch carries `conversation_id`, so the conversation comes from a join on the id and
// not from the edge: the edge exists, and walking it would cost the same and read less
// plainly. Both sides are the same row in the same table.
use lbug::{Connection, Value};

use crate::application::{RepositoryError, StoredTouch};
use crate::domain::OwnerNaming;
use crate::domain::conversation::TouchVerb;

use super::conversation_store::TIME_NOT_RECORDED;
use super::store_support::{fetch_rows, number, text};

/// Every touch whose path ends with the text given, with the conversation that made it.
///
/// `ENDS WITH` and not `CONTAINS`: a person naming a file names its tail, and a path that
/// merely holds the text somewhere in the middle is a different file. The segment boundary is
/// applied above this, which costs a row and keeps the rule where it can be read.
const TOUCHES_ENDING_WITH: &str = "MATCH (conversation:Conversation)-[:TOUCHED_FILE]->(touch:FileTouch) WHERE touch.path ENDS WITH $needle RETURN conversation.session_id, conversation.name, touch.path, touch.verb, touch.naming, touch.times, touch.at_unix_ms;";

/// Every touch of the conversations whose session is the text given, or whose name holds it.
///
/// One statement and not two, because a person types one word and does not say which of the
/// two it is. A session id is exact — it is a UUID nobody types by accident — and a name is
/// held loosely, since the point of naming a conversation is to find it later by part of it.
const TOUCHES_OF_CONVERSATIONS: &str = "MATCH (conversation:Conversation)-[:TOUCHED_FILE]->(touch:FileTouch) WHERE conversation.session_id = $needle OR conversation.name CONTAINS $needle RETURN conversation.session_id, conversation.name, touch.path, touch.verb, touch.naming, touch.times, touch.at_unix_ms;";

/// Every touch whose path starts with a directory and the separator after it: the touches a
/// conversation made of the files under that directory. A path written relative to the
/// conversation's own directory is not matched — the string is all this query reads.
const TOUCHES_UNDER: &str = "MATCH (conversation:Conversation)-[:TOUCHED_FILE]->(touch:FileTouch) WHERE touch.path STARTS WITH $needle RETURN conversation.session_id, conversation.name, touch.path, touch.verb, touch.naming, touch.times, touch.at_unix_ms;";

pub(super) fn touches_under(
    connection: &Connection<'_>,
    prefix: &str,
) -> Result<Vec<StoredTouch>, RepositoryError> {
    read(
        connection,
        TOUCHES_UNDER,
        prefix,
        "touches under a directory",
    )
}

pub(super) fn touches_ending_with(
    connection: &Connection<'_>,
    needle: &str,
) -> Result<Vec<StoredTouch>, RepositoryError> {
    read(connection, TOUCHES_ENDING_WITH, needle, "touches of a path")
}

pub(super) fn touches_of_conversations_matching(
    connection: &Connection<'_>,
    needle: &str,
) -> Result<Vec<StoredTouch>, RepositoryError> {
    read(
        connection,
        TOUCHES_OF_CONVERSATIONS,
        needle,
        "touches of a conversation",
    )
}

fn read(
    connection: &Connection<'_>,
    query: &str,
    needle: &str,
    context: &'static str,
) -> Result<Vec<StoredTouch>, RepositoryError> {
    let rows = fetch_rows(
        connection,
        query,
        vec![("needle", Value::String(needle.to_string()))],
        context,
    )?;
    touches_of_rows(rows, context)
}

/// Touch rows in the column order every touch read returns: session, name, path, verb,
/// naming, times, moment. The graph view reads touches in this shape too, so a row is turned
/// into a touch in one place.
pub(super) fn touches_of_rows(
    rows: Vec<Vec<Value>>,
    context: &'static str,
) -> Result<Vec<StoredTouch>, RepositoryError> {
    let mut touches = Vec::with_capacity(rows.len());
    for values in rows {
        let verb = text(&values, 3, context)?;
        // A verb this build does not know is not a touch it can answer with: the closed set
        // is what makes "which conversations wrote this file" mean one thing. A row carrying
        // one is left out rather than guessed at, exactly as the reader leaves out a tool it
        // has no verb for.
        let Some(verb) = TouchVerb::parse(&verb) else {
            continue;
        };
        let at_unix_ms = match number(&values, 6, context)? {
            TIME_NOT_RECORDED => None,
            at => Some(at),
        };
        touches.push(StoredTouch::new(
            text(&values, 0, context)?,
            text(&values, 1, context)?,
            text(&values, 2, context)?,
            verb,
            OwnerNaming::parse(&text(&values, 4, context)?),
            // Zero is the sentinel of a row written before the count existed, and it is kept
            // as one: a reader that turned it into "once" would report a number nobody
            // counted. Every touch this build writes is at least one.
            u32::try_from(number(&values, 5, context)?).unwrap_or(0),
            at_unix_ms,
        ));
    }
    Ok(touches)
}
