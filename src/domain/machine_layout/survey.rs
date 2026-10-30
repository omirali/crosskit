// What the censuses found beside the walk, in the shape the grouping rule reads.
//
// Both are facts about the disk and nothing more: which folders Claude Code filed transcripts
// under and where each conversation in them started, and which folders without git lie where
// a person keeps documents. What they mean is `grouping`'s question.
use crate::domain::NormalizedPath;

/// One conversation of a transcript folder: the working directories its lines record, in the
/// order they first appear. The first is where the session was started, which is what the
/// folder is named after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationStart {
    working_directories: Vec<NormalizedPath>,
}

impl ConversationStart {
    pub fn new(working_directories: Vec<NormalizedPath>) -> Self {
        Self {
            working_directories,
        }
    }

    pub fn first_working_directory(&self) -> Option<&NormalizedPath> {
        self.working_directories.first()
    }

    pub fn working_directories(&self) -> &[NormalizedPath] {
        &self.working_directories
    }
}

/// One folder under `~/.claude/projects` and the conversations lying directly in it. What lies
/// deeper — a session's subagents and saved tool output — is not a conversation and is not
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptFolderSurvey {
    path: NormalizedPath,
    conversations: Vec<ConversationStart>,
}

impl TranscriptFolderSurvey {
    pub fn new(path: NormalizedPath, conversations: Vec<ConversationStart>) -> Self {
        Self {
            path,
            conversations,
        }
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    /// The folder's own name, which is the encoding of a directory.
    pub fn name(&self) -> &str {
        self.path.file_name()
    }

    pub fn conversations(&self) -> &[ConversationStart] {
        &self.conversations
    }
}

/// One folder without git where documents are kept, with how many files lie under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentFolderSurvey {
    path: NormalizedPath,
    files: u64,
}

impl DocumentFolderSurvey {
    pub fn new(path: NormalizedPath, files: u64) -> Self {
        Self { path, files }
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn files(&self) -> u64 {
        self.files
    }
}
