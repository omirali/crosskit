// Reading a Claude Code transcript: one `.jsonl` file, one line at a time.
//
// The file is untrusted input in the ordinary sense — it is somebody else's format, written
// by a program that is updated weekly, and nothing in it is guaranteed. So every line is read
// on its own and a line that cannot be read is reported rather than skipped: a reader that
// quietly drops what it does not recognise keeps reporting the same numbers after the format
// moves under it.
//
// A line is never held in the conversation whole. What is kept is the text of `text` blocks
// under a `user` or `assistant` message, and the name lines. Everything else is named, counted
// and dropped, which is most of the file: tool output is carried twice over (once in the
// envelope as `toolUseResult`, once in the body as a `tool_result` block), attachments carry
// whole files, and the model's own deliberation is not something anybody said.
#[cfg(test)]
use std::fs::File;
#[cfg(test)]
use std::io::BufReader;
use std::io::{BufRead, Read};
use std::ops::ControlFlow;
#[cfg(test)]
use std::path::Path;

use serde_json::Value;

use crate::domain::conversation::{
    ConversationName, FileTouch, NamedBy, Reply, SkippedLine, Speaker, TouchSource, TouchVerb,
    TranscriptLine, UnreadableLine,
};

/// How much of one line is read before it is refused.
///
/// A transcript line holding a pasted dump can be tens of megabytes, and the reader is asked
/// to work on a corpus without holding it. The ceiling is generous next to any line that
/// carries speech — the longest reply in the owner's corpus is under 100 KB — and it means a
/// single malformed file cannot make the reader allocate without bound.
const MOST_BYTES_PER_LINE: u64 = 64 * 1024 * 1024;

/// Reads `path`, hands each line's meaning to `line`, and returns how many lines the file
/// held.
///
/// The count of lines is returned rather than tallied, because it is the one number only the
/// reader can know: above it a line has become several observations — a message holding three
/// deliberations and a sentence is four — and no count of those is a count of lines.
///
/// The bytes are never decoded twice and no line outlives the call: the caller keeps what it
/// wants from each `TranscriptLine` and the buffer is reused.
/// Used by Stage 31's measurement, which walks a corpus by path. The pass reads from the
/// descriptor it already holds — see `read_transcript_from`.
#[cfg(test)]
pub fn read_transcript(
    path: &Path,
    mut line: impl FnMut(TranscriptLine),
) -> Result<u64, std::io::Error> {
    read_transcript_from(BufReader::new(File::open(path)?), |met| {
        line(met);
        ControlFlow::Continue(())
    })
}

/// The same, over a reader somebody else opened.
///
/// This is the one the indexing pass uses: it holds a descriptor it opened relative to the
/// directory it walked, with `O_NOFOLLOW`, and re-opening the same name by path would give
/// back whatever that name points at now — which is the whole reason the walk is
/// descriptor-relative.
///
/// The caller can stop it. A transcript is read to find out what is in it, and a caller that
/// has already found more than it is allowed to hold must be able to put the book down rather
/// than finish it and then refuse — the peak it was avoiding would have happened by then.
pub fn read_transcript_from(
    mut reader: impl BufRead,
    mut line: impl FnMut(TranscriptLine) -> ControlFlow<()>,
) -> Result<u64, std::io::Error> {
    let mut buffer = Vec::new();
    let mut lines = 0;
    loop {
        buffer.clear();
        let read = (&mut reader)
            .take(MOST_BYTES_PER_LINE)
            .read_until(b'\n', &mut buffer)?;
        if read == 0 {
            return Ok(lines);
        }
        lines += 1;
        let carry_on = match std::str::from_utf8(&buffer) {
            Ok(text) => read_line(text, &mut line),
            Err(_) => line(TranscriptLine::Unreadable(UnreadableLine::NotUtf8)),
        };
        if carry_on.is_break() {
            return Ok(lines);
        }
    }
}

/// What one line of a transcript means. Split out from the file so a test can hand it a line
/// without writing one to disk, which is also what keeps the fixtures small.
pub fn read_line(
    text: &str,
    mut line: impl FnMut(TranscriptLine) -> ControlFlow<()>,
) -> ControlFlow<()> {
    let text = text.trim();
    if text.is_empty() {
        return ControlFlow::Continue(());
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return line(TranscriptLine::Unreadable(UnreadableLine::NotJson));
    };
    let Some(object) = value.as_object() else {
        return line(TranscriptLine::Unreadable(UnreadableLine::NotAnObject));
    };
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return line(TranscriptLine::Unreadable(UnreadableLine::Untyped));
    };
    if let Some(session) = object.get("sessionId").and_then(Value::as_str)
        && !session.is_empty()
    {
        line(TranscriptLine::Session(session.to_string()))?;
    }

    match kind {
        "ai-title" => named(object, "aiTitle", NamedBy::Model, line),
        "custom-title" => named(object, "customTitle", NamedBy::Owner, line),
        "user" => message(object, Speaker::Owner, line),
        "assistant" => message(object, Speaker::Model, line),
        "attachment" => line(TranscriptLine::Skipped(SkippedLine::Attachment)),
        "file-history-snapshot" => file_history(object, line),
        _ => line(TranscriptLine::Skipped(SkippedLine::Bookkeeping)),
    }
}

/// The snapshot the tool writes before it edits: one entry per file it is about to change.
///
/// A fact of the same rank as a tool argument — the tool had the file open and copied it —
/// and the verb is known from what a backup is for. The line carries no timestamp of its
/// own, so the touch has none rather than borrowing one.
fn file_history(
    object: &serde_json::Map<String, Value>,
    mut line: impl FnMut(TranscriptLine) -> ControlFlow<()>,
) -> ControlFlow<()> {
    let backups = object
        .get("snapshot")
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("trackedFileBackups"))
        .and_then(Value::as_object);
    let Some(backups) = backups.filter(|backups| !backups.is_empty()) else {
        return line(TranscriptLine::Skipped(SkippedLine::Bookkeeping));
    };
    let at_unix_ms = object
        .get("snapshot")
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("timestamp"))
        .and_then(Value::as_str)
        .and_then(unix_ms_of_rfc3339);
    for path in backups.keys() {
        // A touch is not speech, so a caller's stop does not apply to it; the read ends on a
        // reply, never on bookkeeping.
        let _stop = match FileTouch::new(
            path,
            TouchVerb::Edited,
            TouchSource::FileHistory,
            at_unix_ms,
        ) {
            Some(touch) => line(TranscriptLine::Touched(touch)),
            None => line(TranscriptLine::Skipped(SkippedLine::Bookkeeping)),
        };
    }
    ControlFlow::Continue(())
}

/// A name line. A name field that is missing or empty leaves the transcript unnamed rather
/// than naming it with nothing.
fn named(
    object: &serde_json::Map<String, Value>,
    field: &str,
    by: NamedBy,
    mut line: impl FnMut(TranscriptLine) -> ControlFlow<()>,
) -> ControlFlow<()> {
    match object
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| ConversationName::new(value, by))
    {
        Some(name) => line(TranscriptLine::Named(name)),
        None => line(TranscriptLine::Skipped(SkippedLine::Bookkeeping)),
    }
}

/// A `user` or `assistant` line: whatever text survives the blocks that are not speech.
///
/// `isMeta` marks a message the harness put under a party's role — a skill's own text, an
/// image placeholder, a note about the session. It is written as if the owner said it and
/// they did not, so it is counted as injected and dropped: leaving it in would let a skill
/// file's vocabulary count as the owner's words in every measurement below.
fn message(
    object: &serde_json::Map<String, Value>,
    speaker: Speaker,
    mut line: impl FnMut(TranscriptLine) -> ControlFlow<()>,
) -> ControlFlow<()> {
    if object.contains_key("toolUseResult") {
        line(TranscriptLine::Skipped(SkippedLine::ToolUseResult))?;
    }
    if object.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return line(TranscriptLine::Skipped(SkippedLine::Injected));
    }
    let Some(content) = object
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
    else {
        return line(TranscriptLine::Unreadable(UnreadableLine::MessageMissing));
    };
    let at_unix_ms = object
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(unix_ms_of_rfc3339);

    let said = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let mut said = String::new();
            for block in blocks {
                match block_text(block, at_unix_ms, &mut line) {
                    Some(text) if !text.trim().is_empty() => {
                        if !said.is_empty() {
                            said.push('\n');
                        }
                        said.push_str(text);
                    }
                    _ => continue,
                }
            }
            said
        }
        _ => {
            return line(TranscriptLine::Unreadable(UnreadableLine::ContentUnknown));
        }
    };

    match Reply::new(speaker, said, at_unix_ms) {
        Some(reply) => line(TranscriptLine::Spoke(reply)),
        None => line(TranscriptLine::Skipped(SkippedLine::NoTextLeft)),
    }
}

/// The file a tool call was handed, when the tool is one whose verb this build knows.
///
/// The path is the argument the tool was called with, which is the top rung of section 5's
/// scale: the tool read it off the disk. A tool carrying a `file_path` that this build has no
/// verb for produces nothing and is counted — the set of tool names moves between releases of
/// Claude Code, and the count is how anybody finds out that it moved.
fn tool_use(
    block: &serde_json::Map<String, Value>,
    at_unix_ms: Option<i64>,
    line: &mut impl FnMut(TranscriptLine) -> ControlFlow<()>,
) {
    let Some(path) = block
        .get("input")
        .and_then(Value::as_object)
        .and_then(|input| input.get("file_path"))
        .and_then(Value::as_str)
    else {
        return;
    };
    let tool = block
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(verb) = TouchVerb::of_tool(tool) else {
        // Ignored rather than propagated: a stop is about how much speech is held, and a
        // touch is not speech. Breaking here would end the read on a bookkeeping line.
        let _stop = line(TranscriptLine::Skipped(SkippedLine::UnknownTool));
        return;
    };
    if let Some(touch) = FileTouch::new(path, verb, TouchSource::ToolInput, at_unix_ms) {
        let _stop = line(TranscriptLine::Touched(touch));
    }
}

/// The text of one content block, and nothing for the blocks that are not speech — each of
/// which is counted by what it was.
fn block_text<'a>(
    block: &'a Value,
    at_unix_ms: Option<i64>,
    line: &mut impl FnMut(TranscriptLine) -> ControlFlow<()>,
) -> Option<&'a str> {
    let block = block.as_object()?;
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block.get("text").and_then(Value::as_str),
        Some("thinking") => {
            let _stop = line(TranscriptLine::Skipped(SkippedLine::Thinking));
            None
        }
        Some("tool_use") => {
            tool_use(block, at_unix_ms, line);
            // A tool call is not speech, so a caller's stop does not apply to it.
            let _stop = line(TranscriptLine::Skipped(SkippedLine::ToolUse));
            None
        }
        Some("tool_result") => {
            let _stop = line(TranscriptLine::Skipped(SkippedLine::ToolResult));
            None
        }
        Some("image") => {
            let _stop = line(TranscriptLine::Skipped(SkippedLine::Image));
            None
        }
        _ => None,
    }
}

/// `2026-09-03T07:09:25.457Z` as milliseconds since the epoch, and nothing for a stamp this
/// does not recognise.
///
/// Written out rather than reached for in a crate: the transcript writes one shape, the
/// measurement needs the ordering of replies and nothing about calendars, and a dependency
/// added for one field of one format is a dependency the product then carries.
pub(crate) fn unix_ms_of_rfc3339(stamp: &str) -> Option<i64> {
    let (date, rest) = stamp.split_once('T')?;
    let time = rest.strip_suffix('Z')?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    if date.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, fraction) = match time.split_once('.') {
        Some((clock, fraction)) => (clock, fraction),
        None => (time, "0"),
    };
    let mut clock = clock.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if clock.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let milli: i64 = fraction.get(..3).unwrap_or(fraction).parse().ok()?;
    let seconds = days_since_epoch(year, month, day)? * 86_400 + hour * 3600 + minute * 60 + second;
    Some(seconds * 1000 + milli)
}

/// Days from 1970-01-01 to `year-month-day`, by the civil-from-days algorithm: the calendar
/// is arithmetic, and this is the arithmetic.
fn days_since_epoch(year: i64, month: i64, day: i64) -> Option<i64> {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}
