// Every Codex session file as session facts — the measurement's reader, not the product's
// (ADR 0051, decision 3: the product reads Codex through its app-server).
//
// A file holds more than Codex's own conversations. Codex Desktop imports the history of Claude
// Code, and an imported conversation is a file here too: its turns are named
// `external-import-turn-…` and it carries the replies without the tool calls. Such a file is a
// Claude Code conversation read from a copy, and it is recorded as one. Codex also starts
// sessions of its own accord — workers a conversation spawned, and the reviewer that judges a
// command before it runs — and those are recorded by role, not as conversations.
//
// What is kept of a line: its time, the owner's words, the files a patch changes, the paths a
// command names. Compressed sessions (`.jsonl.zst`) are counted and not opened.
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde_json::Value;

use crate::domain::task_stitching::mentions::{file_key, patched_files};
use crate::domain::task_stitching::session::{Origin, Platform, SessionFacts, SessionRole};
use crate::infrastructure::chats::transcript_reader::unix_ms_of_rfc3339;
use crate::infrastructure::machine_survey::entries_in;

const SESSION_SUFFIX: &str = ".jsonl";
const COMPRESSED_SUFFIX: &str = ".jsonl.zst";
const MOST_BYTES_PER_LINE: u64 = 64 * 1024 * 1024;
/// How the turns of an imported conversation are named.
const IMPORTED_TURN: &[u8] = b"external-import-turn";
/// How the command reviewer's request opens.
const REVIEW_REQUEST: &str = "The following is the Codex agent history";
/// Blocks Codex puts into the owner's message that the owner did not write. The IDE notes are
/// kept for their paths: a file the owner had open is a file the conversation referred to.
const INSTRUCTION_OPENINGS: [&str; 4] = [
    "# AGENTS.md",
    "<user_instructions>",
    "<environment_context>",
    "<permissions",
];

/// What the walk found besides sessions.
#[derive(Debug, Default)]
pub(super) struct CodexWalk {
    pub(super) compressed: usize,
    pub(super) unreadable: usize,
}

/// Every session file under each of `roots`, recursively.
pub(super) fn codex_sessions(
    roots: &[std::path::PathBuf],
    home: &str,
) -> (Vec<SessionFacts>, CodexWalk) {
    let mut sessions = Vec::new();
    let mut walk = CodexWalk::default();
    for root in roots {
        collect(root, home, &mut sessions, &mut walk);
    }
    (sessions, walk)
}

fn collect(directory: &Path, home: &str, sessions: &mut Vec<SessionFacts>, walk: &mut CodexWalk) {
    for entry in entries_in(directory) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            collect(&path, home, sessions, walk);
            continue;
        }
        let name = path.to_string_lossy();
        if !kind.is_file() {
            continue;
        }
        if name.ends_with(COMPRESSED_SUFFIX) {
            walk.compressed += 1;
        } else if name.ends_with(SESSION_SUFFIX) {
            match session_of(&path, home) {
                Some(session) => sessions.push(session),
                None => walk.unreadable += 1,
            }
        }
    }
}

fn session_of(path: &Path, home: &str) -> Option<SessionFacts> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    let mut session: Option<SessionFacts> = None;
    let mut imported = false;
    loop {
        buffer.clear();
        match (&mut reader)
            .take(MOST_BYTES_PER_LINE)
            .read_until(b'\n', &mut buffer)
        {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return None,
        }
        imported |= buffer
            .windows(IMPORTED_TURN.len())
            .any(|window| window == IMPORTED_TURN);
        let Ok(line) = serde_json::from_slice::<Value>(&buffer) else {
            continue;
        };
        let payload = &line["payload"];
        if line["type"] == "session_meta" {
            session = Some(described(payload));
            continue;
        }
        let Some(session) = session.as_mut() else {
            continue;
        };
        if let Some(at) = line["timestamp"].as_str().and_then(unix_ms_of_rfc3339) {
            session.saw(at);
        }
        if line["type"] == "response_item" {
            take_item(session, payload, home);
        }
    }
    let mut session = session?;
    if imported {
        session.platform = Platform::ClaudeCode;
        session.origin = Origin::ImportedCopy;
    }
    Some(session)
}

/// The session as its first line describes it: id, directory, branch, and whether Codex
/// started it on its own.
fn described(payload: &Value) -> SessionFacts {
    let id = payload["id"].as_str().unwrap_or_default();
    let mut session = SessionFacts::new(id, Platform::Codex);
    session.working_directory = payload["cwd"].as_str().map(str::to_string);
    session.place = session.working_directory.clone();
    session.branch = payload["git"]["branch"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    if payload["source"]["subagent"].is_object() {
        session.role = SessionRole::Worker;
    }
    session
}

fn take_item(session: &mut SessionFacts, item: &Value, home: &str) {
    match item["type"].as_str() {
        Some("message") if item["role"] == "user" => {
            for part in item["content"].as_array().into_iter().flatten() {
                if let Some(text) = part["text"].as_str() {
                    take_message(session, text, home);
                }
            }
        }
        Some("custom_tool_call") => take_call(session, item["input"].as_str(), home),
        Some("function_call") => take_call(session, item["arguments"].as_str(), home),
        _ => {}
    }
}

fn take_message(session: &mut SessionFacts, text: &str, home: &str) {
    if text.starts_with(REVIEW_REQUEST) {
        session.role = SessionRole::AutoReview;
        return;
    }
    if INSTRUCTION_OPENINGS
        .iter()
        .any(|opening| text.trim_start().starts_with(opening))
    {
        return;
    }
    let (notes, words) = split_leading_notes(text);
    session.take_mentions(notes, home);
    if !words.trim().is_empty() {
        session.take_owner_words(words, home);
    }
}

/// The IDE notes Codex or the import put before the owner's words, and the words.
fn split_leading_notes(text: &str) -> (&str, &str) {
    let mut rest = text.trim_start();
    let mut consumed = text.len() - rest.len();
    while rest.starts_with('<') {
        let Some(tag_end) = rest.find('>') else {
            break;
        };
        let name = &rest[1..tag_end];
        let closing = format!("</{name}>");
        let Some(close) = rest.find(&closing) else {
            break;
        };
        let after = close + closing.len();
        consumed += after;
        rest = &rest[after..];
        let trimmed = rest.trim_start();
        consumed += rest.len() - trimmed.len();
        rest = trimmed;
    }
    (&text[..consumed], rest)
}

fn take_call(session: &mut SessionFacts, text: Option<&str>, home: &str) {
    let Some(text) = text else {
        return;
    };
    for file in patched_files(text) {
        if let Some(key) = file_key(&file, session.working_directory.as_deref(), home) {
            session.written.insert(key);
        }
    }
    session.take_mentions(text, home);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ide_notes_are_split_from_the_owner_words() {
        let text = "<ide_opened_file>The user opened /w/payments-api/plan.md</ide_opened_file>\n\nПрочитай план";
        let (notes, words) = split_leading_notes(text);
        assert!(notes.contains("plan.md"));
        assert_eq!(words, "Прочитай план");
    }

    #[test]
    fn a_message_without_notes_is_all_words() {
        assert_eq!(split_leading_notes("Привет"), ("", "Привет"));
    }
}
