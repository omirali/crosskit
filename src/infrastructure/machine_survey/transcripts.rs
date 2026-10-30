// Every folder Claude Code filed conversations under, and where each conversation worked.
//
// A folder is read one level deep. Its `.jsonl` files are the conversations; the directories
// beside them hold a session's subagents and its saved tool output, which are not the
// conversation and are not opened here. Of a conversation, one field of each line is read — its
// `cwd` — and every other field is left to the parser to skip, so nothing said in it is held.
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::Deserialize;

use crate::domain::{ConversationStart, NormalizedPath, TranscriptFolderSurvey};

/// Where the tool keeps its transcript folders, under a home directory.
const TRANSCRIPTS: &str = ".claude/projects";
const TRANSCRIPT_SUFFIX: &str = ".jsonl";
/// How much of one line is read before the line is given up — the ceiling the product's
/// transcript reader puts on a line, for its reason: one pasted dump must not make a read
/// allocate without bound.
const MOST_BYTES_PER_LINE: u64 = 64 * 1024 * 1024;

/// The directory holding the transcript folders of `home`, whether or not it exists.
pub fn transcripts_root(home: &NormalizedPath) -> Option<NormalizedPath> {
    NormalizedPath::from_absolute(&format!("{}/{TRANSCRIPTS}", home.as_str())).ok()
}

/// Every transcript folder under `root`, sorted by name. A root that is not there is no
/// folder at all, which is what a machine without the tool looks like.
pub fn transcript_folders(root: &NormalizedPath) -> Vec<TranscriptFolderSurvey> {
    let mut folders: Vec<TranscriptFolderSurvey> = entries_in(Path::new(root.as_str()))
        .into_iter()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let path = root.join(&name).ok()?;
            Some(TranscriptFolderSurvey::new(
                path,
                conversations_in(&entry.path()),
            ))
        })
        .collect();
    folders.sort_by(|first, second| first.path().as_str().cmp(second.path().as_str()));
    folders
}

/// The conversations lying directly in one folder, in the order of their file names.
fn conversations_in(folder: &Path) -> Vec<ConversationStart> {
    let mut files: Vec<(String, std::path::PathBuf)> = entries_in(folder)
        .into_iter()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            name.ends_with(TRANSCRIPT_SUFFIX)
                .then(|| (name, entry.path()))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|(_, path)| ConversationStart::new(working_directories_in(&path)))
        .collect()
}

#[derive(Deserialize)]
struct WorkingDirectoryOnly {
    cwd: Option<String>,
}

/// Every distinct `cwd` the lines of one transcript carry, in order of first appearance. A
/// line that is not an object with a string `cwd`, or that names no absolute directory, names
/// none; a file that cannot be read names none either.
pub(crate) fn working_directories_in(path: &Path) -> Vec<NormalizedPath> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    working_directories(path)
        .filter(|directory| seen.insert(directory.as_str().to_string()))
        .collect()
}

/// Where a conversation was started: the first `cwd` its transcript carries. Reading stops at
/// it, because a transcript can run to tens of megabytes and the start is in its first lines.
pub fn first_working_directory_in(path: &Path) -> Option<NormalizedPath> {
    working_directories(path).next()
}

/// The `cwd` of every line of a transcript that names one, read as the lines are asked for.
fn working_directories(path: &Path) -> impl Iterator<Item = NormalizedPath> {
    let reader = fs::File::open(path).ok().map(BufReader::new);
    let mut buffer = Vec::new();
    std::iter::from_fn({
        let mut reader = reader;
        move || -> Option<Option<NormalizedPath>> {
            let reader = reader.as_mut()?;
            buffer.clear();
            match reader
                .take(MOST_BYTES_PER_LINE)
                .read_until(b'\n', &mut buffer)
            {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            let named = serde_json::from_slice::<WorkingDirectoryOnly>(&buffer)
                .ok()
                .and_then(|line| line.cwd)
                .and_then(|cwd| NormalizedPath::from_absolute(&cwd).ok());
            Some(named)
        }
    })
    .flatten()
}

/// What `directory` lists, or nothing when it cannot be listed. `file_type` of an entry is the
/// entry's own, so a link is a link and never what it points at.
pub(crate) fn entries_in(directory: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(directory)
        .map(|entries| entries.flatten().collect())
        .unwrap_or_default()
}
