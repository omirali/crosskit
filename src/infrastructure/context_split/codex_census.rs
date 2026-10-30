// Every Codex session, and of each only its working directory.
//
// Codex files its sessions by date, not by project, and Crosskit has no reader for them; the
// stage asks for a count and nothing more. So one line is read of each file — the first, which
// is the session's own description — and one field of it kept. Nothing said in the session is
// looked at.
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::Deserialize;

use super::transcript_census::entries_in;
use crate::domain::NormalizedPath;

/// The first line carries the session's instructions in full, which run to tens of kilobytes.
/// A first line longer than this is not the description this reads.
const MOST_BYTES_OF_THE_FIRST_LINE: u64 = 4 * 1024 * 1024;
const SESSION_SUFFIX: &str = ".jsonl";
const DESCRIPTION: &str = "session_meta";

#[derive(Deserialize)]
struct FirstLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<Description>,
}

#[derive(Deserialize)]
struct Description {
    cwd: Option<String>,
}

/// The working directory of every session under `root`, `None` for a session whose first
/// line does not say.
pub(super) fn census_of_codex_sessions(root: &NormalizedPath) -> Vec<Option<NormalizedPath>> {
    let mut sessions = Vec::new();
    collect(Path::new(root.as_str()), &mut sessions);
    sessions
}

fn collect(directory: &Path, sessions: &mut Vec<Option<NormalizedPath>>) {
    for entry in entries_in(directory) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(&entry.path(), sessions);
            continue;
        }
        let is_session = kind.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(SESSION_SUFFIX));
        if is_session {
            sessions.push(working_directory_of(&entry.path()));
        }
    }
}

fn working_directory_of(path: &Path) -> Option<NormalizedPath> {
    let file = std::fs::File::open(path).ok()?;
    let mut line = Vec::new();
    BufReader::new(file)
        .take(MOST_BYTES_OF_THE_FIRST_LINE)
        .read_until(b'\n', &mut line)
        .ok()?;
    let first: FirstLine = serde_json::from_slice(&line).ok()?;
    if first.kind.as_deref() != Some(DESCRIPTION) {
        return None;
    }
    NormalizedPath::from_absolute(&first.payload?.cwd?).ok()
}
