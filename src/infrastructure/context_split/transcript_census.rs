// Every folder Claude Code files transcripts under, and of every conversation in it the three
// things the split reads: the working directories recorded on its lines, its name, and the files
// it touched.
//
// The name and the touches come from the product's own reader and assembly — `read_transcript`
// and `ConversationAssembly` — so they are the ones a pass would write. The working directories
// come from the reader `setup` uses since Stage 39 (`machine_survey`): one field of each line's
// object, and nothing else of the line is kept.
//
// A folder is walked one level deep. Its `.jsonl` files are the conversations; what lies in
// the directories beside them — a session's subagents and its saved tool output — is counted and
// not read, the way the `chats` context's exclusions leave it.
use std::collections::BTreeSet;
use std::path::Path;

use crate::application::conversation_assembly::ConversationAssembly;
use crate::domain::NormalizedPath;
use crate::infrastructure::chats::transcript_reader::read_transcript;
pub(super) use crate::infrastructure::machine_survey::entries_in;
use crate::infrastructure::machine_survey::working_directories_in;

const TRANSCRIPT_SUFFIX: &str = ".jsonl";

/// One transcript folder.
#[derive(Debug, Clone)]
pub(super) struct TranscriptFolder {
    pub(super) name: String,
    pub(super) conversations: Vec<ConversationRecord>,
    /// `.jsonl` files one level further down: the transcripts of a session's subagents.
    pub(super) nested_transcripts: usize,
}

/// One conversation, as far as the split reads it.
#[derive(Debug, Clone)]
pub(super) struct ConversationRecord {
    pub(super) file: NormalizedPath,
    /// Every distinct working directory, in the order the lines first name it.
    pub(super) working_directories: Vec<NormalizedPath>,
    /// What the conversation is called — the owner's name over the model's — if anything.
    pub(super) title: Option<String>,
    /// The distinct paths it read, wrote or edited through a file tool.
    pub(super) touched: Vec<NormalizedPath>,
    /// Whether the file could be read at all. A live transcript can be mid-write.
    pub(super) readable: bool,
}

impl ConversationRecord {
    pub(super) fn first_working_directory(&self) -> Option<&NormalizedPath> {
        self.working_directories.first()
    }
}

/// Every transcript folder under `root`, sorted by name. A root that is not there is an empty
/// census, and the report says so by its zero.
pub(super) fn census_of_transcripts(root: &NormalizedPath) -> Vec<TranscriptFolder> {
    let mut folders: Vec<TranscriptFolder> = directories_in(Path::new(root.as_str()))
        .into_iter()
        .filter_map(|(name, path)| {
            let normalized = root.join(&name).ok()?;
            Some(read_folder(name, &path, &normalized))
        })
        .collect();
    folders.sort_by(|left, right| left.name.cmp(&right.name));
    folders
}

fn read_folder(name: String, path: &Path, normalized: &NormalizedPath) -> TranscriptFolder {
    let mut conversations = Vec::new();
    let mut nested_transcripts = 0;
    for entry in entries_in(path) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if kind.is_dir() {
            nested_transcripts += count_transcripts_below(&entry.path());
            continue;
        }
        if !kind.is_file() || !file_name.ends_with(TRANSCRIPT_SUFFIX) {
            continue;
        }
        let Ok(file) = normalized.join(&file_name) else {
            continue;
        };
        conversations.push(read_conversation(&entry.path(), file, &file_name));
    }
    conversations.sort_by(|left, right| left.file.as_str().cmp(right.file.as_str()));
    TranscriptFolder {
        name,
        conversations,
        nested_transcripts,
    }
}

fn read_conversation(path: &Path, file: NormalizedPath, file_name: &str) -> ConversationRecord {
    let session = file_name
        .strip_suffix(TRANSCRIPT_SUFFIX)
        .unwrap_or(file_name);
    let mut assembly = ConversationAssembly::for_session(session, u64::MAX);
    let read = read_transcript(path, |line| {
        // No ceiling on speech here, so the assembly never asks to stop.
        let _stop = assembly.absorb(line);
    });
    let conversation = assembly.finish().into_conversation();
    let touched: BTreeSet<String> = conversation
        .touches()
        .iter()
        .map(|touch| touch.path().to_string())
        .collect();
    ConversationRecord {
        file,
        working_directories: working_directories_in(path),
        title: conversation.name().map(|name| name.value().to_string()),
        touched: touched
            .iter()
            .filter_map(|path| NormalizedPath::from_absolute(path).ok())
            .collect(),
        readable: read.is_ok(),
    }
}

fn count_transcripts_below(directory: &Path) -> usize {
    let mut count = 0;
    for entry in entries_in(directory) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            count += count_transcripts_below(&entry.path());
        } else if kind.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(TRANSCRIPT_SUFFIX))
        {
            count += 1;
        }
    }
    count
}

/// The directories directly in `directory` whose names are text, links not followed.
pub(super) fn directories_in(directory: &Path) -> Vec<(String, std::path::PathBuf)> {
    entries_in(directory)
        .into_iter()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            Some((name, entry.path()))
        })
        .collect()
}
