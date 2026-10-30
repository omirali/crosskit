// Conversations, folders without git and Codex sessions, placed by the domain's rules over the
// repositories and folder names `placement` resolved.
use std::collections::BTreeMap;

use super::document_census::DocumentFolder;
use super::placement::{FolderIndex, FolderKind, Repositories, place_of};
use super::transcript_census::{ConversationRecord, TranscriptFolder};
use crate::domain::NormalizedPath;
use crate::domain::context_split::{
    ContextSide, FolderPlace, SplitDecision, WorkingDirectoryEvidence, is_temporary_place,
    place_conversation, place_folder_without_git, transcript_folder_name,
};

/// One conversation and everything the rule saw of it.
#[derive(Debug, Clone)]
pub(super) struct PlacedConversation {
    pub(super) folder_kind: FolderKind,
    pub(super) record: ConversationRecord,
    pub(super) directories: WorkingDirectoryEvidence,
    /// The answer: the folder first, the working directories as the check.
    pub(super) decision: SplitDecision,
    /// What the working directories alone would have said, with the folder unread.
    pub(super) by_directories_alone: SplitDecision,
    /// Whether the first working directory the file records encodes to the folder it lies in.
    /// `None` for a transcript that records none.
    pub(super) first_directory_names_the_folder: Option<bool>,
    /// Whether a working directory lies outside the project the folder names — the session
    /// moved.
    pub(super) left_the_folders_project: bool,
}

pub(super) fn place_conversations(
    folders: &[TranscriptFolder],
    index: &FolderIndex,
    repositories: &Repositories,
) -> Vec<PlacedConversation> {
    let mut placed = Vec::new();
    for folder in folders {
        let kind = index.kind_of(&folder.name);
        let place = place_of(&kind, repositories);
        for record in &folder.conversations {
            let directories = evidence_of(&record.working_directories, repositories);
            let left_the_folders_project = match &kind {
                FolderKind::Project(root) => record
                    .working_directories
                    .iter()
                    .any(|directory| !directory.is_within(root) && !is_temporary_place(directory)),
                _ => false,
            };
            placed.push(PlacedConversation {
                folder_kind: kind.clone(),
                record: record.clone(),
                decision: place_conversation(place, &directories),
                by_directories_alone: place_conversation(FolderPlace::Unknown, &directories),
                directories,
                first_directory_names_the_folder: record
                    .first_working_directory()
                    .map(|first| transcript_folder_name(first.as_str()) == folder.name),
                left_the_folders_project,
            });
        }
    }
    placed
}

/// Where each working directory lies: a temporary place, a repository with its answer, or
/// nowhere the rule knows.
fn evidence_of(
    directories: &[NormalizedPath],
    repositories: &Repositories,
) -> WorkingDirectoryEvidence {
    let mut evidence = WorkingDirectoryEvidence::default();
    for directory in directories {
        if is_temporary_place(directory) {
            evidence.record_temporary_place();
            continue;
        }
        match repositories.holding(directory) {
            Some(repository) => evidence.record_project(repository.effective),
            None => evidence.record_outside_projects(),
        }
    }
    evidence
}

/// One folder without git, and what decided it.
#[derive(Debug, Clone)]
pub(super) struct PlacedFolder {
    pub(super) folder: DocumentFolder,
    pub(super) conversations_inside: usize,
    pub(super) codex_sessions_inside: usize,
    pub(super) decision: SplitDecision,
    /// Not the rule, and printed apart from it: how many files inside the folder conversations
    /// of each side touched. It is what could decide the folder if the rule listened to it.
    pub(super) touched_from: BTreeMap<Option<ContextSide>, usize>,
}

pub(super) fn place_folders(
    folders: &[DocumentFolder],
    conversations: &[PlacedConversation],
    codex: &[Option<NormalizedPath>],
) -> Vec<PlacedFolder> {
    folders
        .iter()
        .map(|folder| {
            let inside: Vec<&PlacedConversation> = conversations
                .iter()
                .filter(|conversation| {
                    conversation
                        .record
                        .working_directories
                        .iter()
                        .any(|directory| directory.is_within(&folder.path))
                })
                .collect();
            let mut touched_from = BTreeMap::new();
            for conversation in conversations {
                let touched = conversation
                    .record
                    .touched
                    .iter()
                    .filter(|path| path.is_within(&folder.path))
                    .count();
                if touched > 0 {
                    *touched_from
                        .entry(conversation.decision.side())
                        .or_default() += touched;
                }
            }
            PlacedFolder {
                folder: folder.clone(),
                conversations_inside: inside.len(),
                codex_sessions_inside: codex
                    .iter()
                    .flatten()
                    .filter(|directory| directory.is_within(&folder.path))
                    .count(),
                decision: place_folder_without_git(
                    inside.iter().map(|conversation| conversation.decision),
                ),
                touched_from,
            }
        })
        .collect()
}

/// Where one Codex session's working directory lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum CodexPlace {
    /// Its first line names no working directory.
    Unnamed,
    Temporary,
    /// Inside a repository, with the answer that repository gets.
    Project(SplitDecision),
    /// A directory holding repositories without being one.
    Mixed,
    /// A document root or a folder without git.
    Document,
    Elsewhere,
}

pub(super) fn place_codex_sessions(
    sessions: &[Option<NormalizedPath>],
    repositories: &Repositories,
    index: &FolderIndex,
    folders: &[DocumentFolder],
) -> BTreeMap<CodexPlace, usize> {
    let mut places = BTreeMap::new();
    for session in sessions {
        *places
            .entry(codex_place(session.as_ref(), repositories, index, folders))
            .or_default() += 1;
    }
    places
}

fn codex_place(
    directory: Option<&NormalizedPath>,
    repositories: &Repositories,
    index: &FolderIndex,
    folders: &[DocumentFolder],
) -> CodexPlace {
    let Some(directory) = directory else {
        return CodexPlace::Unnamed;
    };
    if is_temporary_place(directory) {
        return CodexPlace::Temporary;
    }
    if let Some(repository) = repositories.holding(directory) {
        return CodexPlace::Project(repository.effective);
    }
    if folders
        .iter()
        .any(|folder| directory.is_within(&folder.path))
    {
        return CodexPlace::Document;
    }
    match index.kind_of(&transcript_folder_name(directory.as_str())) {
        FolderKind::Mixed(_) => CodexPlace::Mixed,
        FolderKind::Document(_) => CodexPlace::Document,
        FolderKind::Project(_)
        | FolderKind::Temporary
        | FolderKind::Ambiguous(_)
        | FolderKind::Unknown => CodexPlace::Elsewhere,
    }
}
