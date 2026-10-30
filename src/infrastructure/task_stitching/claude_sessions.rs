// Every Claude Code transcript as session facts.
//
// What was said, what was touched and what the conversation is called come from the product's
// own reader (`read_transcript`), so the owner's text here is the owner's by the product's rules
// and not the harness's. Where and on which branch it ran is one more pass over the same file
// that keeps two fields of each line and nothing else of it.
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::Deserialize;

use crate::domain::conversation::{NamedBy, Speaker, TouchVerb, TranscriptLine};
use crate::domain::task_stitching::mentions::file_key;
use crate::domain::task_stitching::session::{Platform, SessionFacts};
use crate::infrastructure::chats::transcript_reader::read_transcript;
use crate::infrastructure::machine_survey::entries_in;

const TRANSCRIPT_SUFFIX: &str = ".jsonl";
const MOST_BYTES_PER_LINE: u64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
struct PlaceOnly {
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
}

/// Every transcript one level under every folder of `root`. A transcript that cannot be read
/// at all is left out and counted.
pub(super) fn claude_sessions(root: &Path, home: &str) -> (Vec<SessionFacts>, usize) {
    let mut sessions = Vec::new();
    let mut unreadable = 0;
    for folder in entries_in(root) {
        if !folder.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        for entry in entries_in(&folder.path()) {
            let path = entry.path();
            let is_transcript = entry.file_type().is_ok_and(|kind| kind.is_file())
                && path.to_string_lossy().ends_with(TRANSCRIPT_SUFFIX);
            if !is_transcript {
                continue;
            }
            match session_of(&path, home) {
                Some(session) => sessions.push(session),
                None => unreadable += 1,
            }
        }
    }
    (sessions, unreadable)
}

fn session_of(path: &Path, home: &str) -> Option<SessionFacts> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let mut session = SessionFacts::new(id, Platform::ClaudeCode);
    let (working_directory, branch) = place_of(path);
    session.place = working_directory.clone();
    session.working_directory = working_directory;
    session.branch = branch;

    let mut owner_title: Option<String> = None;
    let mut model_title: Option<String> = None;
    read_transcript(path, |line| match line {
        TranscriptLine::Named(name) => match name.named_by() {
            NamedBy::Owner => owner_title = Some(name.value().to_string()),
            NamedBy::Model => model_title = Some(name.value().to_string()),
        },
        TranscriptLine::Spoke(reply) => {
            if let Some(at) = reply.at_unix_ms() {
                session.saw(at);
            }
            if reply.speaker() == Speaker::Owner {
                session.take_owner_words(reply.text(), home);
            }
        }
        TranscriptLine::Touched(touch) => {
            if let Some(at) = touch.at_unix_ms() {
                session.saw(at);
            }
            let wd = session.working_directory.clone();
            let Some(key) = file_key(touch.path(), wd.as_deref(), home) else {
                return;
            };
            match touch.verb() {
                TouchVerb::Read => session.take_reference(key),
                TouchVerb::Wrote | TouchVerb::Edited => {
                    session.written.insert(key);
                }
            }
        }
        TranscriptLine::Session(_) | TranscriptLine::Skipped(_) | TranscriptLine::Unreadable(_) => {
        }
    })
    .ok()?;
    session.title = owner_title.or(model_title);
    Some(session)
}

/// The first working directory and the first named branch the transcript records.
fn place_of(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(file) = std::fs::File::open(path) else {
        return (None, None);
    };
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    let (mut working_directory, mut branch) = (None, None);
    while working_directory.is_none() || branch.is_none() {
        buffer.clear();
        match (&mut reader)
            .take(MOST_BYTES_PER_LINE)
            .read_until(b'\n', &mut buffer)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(line) = serde_json::from_slice::<PlaceOnly>(&buffer) else {
            continue;
        };
        working_directory = working_directory.or(line.cwd);
        branch = branch.or(line.git_branch.filter(|name| !name.is_empty()));
    }
    (working_directory, branch)
}
