// A transcript folder goes with the project it is the folder of.
//
// Claude Code names the folder after the directory a session was started in, by a rule that
// loses what it replaces, so the name is never decoded. The direction is the other one: the
// first working directory recorded inside each conversation is encoded, and it has to give
// back exactly the folder's name. Only then is that directory taken as the folder's, and the
// folder goes where the directory lies.
//
//   what the folder holds                                  answer
//   no conversation                                        left out: nothing to check by
//   conversations recording no working directory           left out
//   a first directory that encodes to another name          left out: the rule disagrees
//   first directories that differ but share the name        left out: which one is not known
//   a directory inside a project                            that project
//   a directory inside a folder without git                 that folder
//   a directory holding projects                            left out: mixed
//   a directory in a temporary place                        left out: nobody's work
//   anything else                                          left out: unknown
//
// What the walk found comes before a temporary place: a tree the person chose to walk is theirs
// wherever it lies, so a session started in one of its projects belongs to the project, and a
// session started above several of them is mixed rather than throwaway.
use crate::domain::{
    NormalizedPath, is_temporary_folder_name, is_temporary_place, transcript_folder_name,
};

use super::members::LeftOutReason;
use super::projects::Project;
use super::survey::{DocumentFolderSurvey, TranscriptFolderSurvey};

/// Where one transcript folder goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TranscriptPlace {
    /// With this project, by index.
    Project(usize),
    /// With this folder without git, by index.
    Document(usize),
}

/// The table at the top of this file: where the folder goes and the directory it was verified
/// to be the folder of, or why it goes nowhere.
pub(super) fn place_transcript_folder(
    folder: &TranscriptFolderSurvey,
    projects: &[Project],
    documents: &[DocumentFolderSurvey],
) -> Result<(TranscriptPlace, NormalizedPath), LeftOutReason> {
    if folder.conversations().is_empty() {
        return Err(match is_temporary_folder_name(folder.name()) {
            true => LeftOutReason::Temporary,
            false => LeftOutReason::NoConversations,
        });
    }
    let directory = verified_directory(folder)?;

    if let Some(index) = projects
        .iter()
        .position(|project| directory.is_within(&project.member.root))
    {
        return Ok((TranscriptPlace::Project(index), directory));
    }
    if let Some(index) = documents
        .iter()
        .position(|document| directory.is_within(document.path()))
    {
        return Ok((TranscriptPlace::Document(index), directory));
    }
    if projects
        .iter()
        .any(|project| project.member.root.is_within(&directory))
    {
        return Err(LeftOutReason::MixedFolder { directory });
    }
    if is_temporary_place(&directory) {
        return Err(LeftOutReason::Temporary);
    }
    Err(LeftOutReason::UnknownDirectory { directory })
}

/// The one directory every conversation of the folder was started in, checked by encoding it.
fn verified_directory(folder: &TranscriptFolderSurvey) -> Result<NormalizedPath, LeftOutReason> {
    let mut found: Option<&NormalizedPath> = None;
    for start in folder
        .conversations()
        .iter()
        .filter_map(|conversation| conversation.first_working_directory())
    {
        if transcript_folder_name(start.as_str()) != folder.name() {
            return Err(LeftOutReason::NameDisagrees {
                started_in: start.clone(),
            });
        }
        match found {
            None => found = Some(start),
            Some(first) if first != start => return Err(LeftOutReason::SeveralDirectories),
            Some(_) => {}
        }
    }
    found.cloned().ok_or(LeftOutReason::NoWorkingDirectory)
}
