// A conversation follows the folder it is filed under, and the working directories recorded
// inside it are the check. A folder without git follows the conversations that worked in it.
//
//   folder the transcript lies in       working directories inside      answer
//   a temporary place                   —                               skip
//   a decided project                   none on another side            the project's side
//   a decided project                   one on another side             ask
//   a project that is a question        —                               the same question
//   a mixed or unknown folder           decided projects of one side    that side
//   a mixed or unknown folder           decided projects of two sides   ask
//   a mixed or unknown folder           only undecided projects         the same question
//   a mixed or unknown folder           only temporary places           skip
//   a mixed or unknown folder           nothing in any project          ask
//
// The folder comes first because it is where the file physically lies: whatever a context's
// roots are, the transcript is in that folder. The working directories come second and can
// only take an answer away, never give a different one — a transcript cannot be split between
// two contexts, so a conversation that worked on both sides is a question. A project that is
// itself a question is not a side, and a working directory in one contradicts nothing.
use std::collections::BTreeSet;

use super::side::{AskBecause, ContextSide, DecidedBy, SplitDecision};

/// What the name of the folder a transcript lies in matched, before the file is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderPlace {
    /// The encoding of a project's directory, with the project's own answer.
    Project(SplitDecision),
    /// The encoding of a directory in a temporary place.
    Temporary,
    /// The encoding of a directory that holds several projects.
    Mixed,
    /// The encoding of no directory the measurement knows.
    Unknown,
}

/// Where the working directories recorded in one transcript lie, or the working directories
/// of every conversation held in one folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkingDirectoryEvidence {
    sides: BTreeSet<ContextSide>,
    undecided_projects: usize,
    outside_projects: usize,
}

impl WorkingDirectoryEvidence {
    /// A working directory inside a project, with that project's answer.
    pub(crate) fn record_project(&mut self, decision: SplitDecision) {
        match decision.side() {
            Some(side) => {
                self.sides.insert(side);
            }
            None => self.undecided_projects += 1,
        }
    }

    pub(crate) fn record_temporary_place(&mut self) {
        self.sides.insert(ContextSide::Skip);
    }

    pub(crate) fn record_outside_projects(&mut self) {
        self.outside_projects += 1;
    }

    /// Whether any working directory lay in a project that is a question.
    pub(crate) fn visited_an_undecided_project(&self) -> bool {
        self.undecided_projects > 0
    }

    /// How many working directories lay in no project at all.
    pub(crate) fn outside_projects(&self) -> usize {
        self.outside_projects
    }

    /// The sides of people's work the directories lie on — `skip` is nobody's.
    fn sides_of_work(&self) -> Vec<ContextSide> {
        self.sides
            .iter()
            .copied()
            .filter(|side| *side != ContextSide::Skip)
            .collect()
    }
}

/// The table at the top of this file.
pub(crate) fn place_conversation(
    folder: FolderPlace,
    directories: &WorkingDirectoryEvidence,
) -> SplitDecision {
    match folder {
        FolderPlace::Temporary => {
            SplitDecision::decided(ContextSide::Skip, DecidedBy::TemporaryPlace)
        }
        FolderPlace::Project(SplitDecision::Decided { side, .. }) => {
            let contradicted = directories
                .sides_of_work()
                .iter()
                .any(|other| *other != side);
            if contradicted {
                return SplitDecision::Ask(AskBecause::WorkingDirectoriesDisagree);
            }
            SplitDecision::decided(side, DecidedBy::ProjectFolder)
        }
        FolderPlace::Project(SplitDecision::Ask(_)) => {
            SplitDecision::Ask(AskBecause::FollowsAQuestion)
        }
        FolderPlace::Mixed => by_working_directories(directories, AskBecause::MixedFolder),
        FolderPlace::Unknown => by_working_directories(directories, AskBecause::UnknownFolder),
    }
}

fn by_working_directories(
    directories: &WorkingDirectoryEvidence,
    otherwise: AskBecause,
) -> SplitDecision {
    match directories.sides_of_work().as_slice() {
        [side] => SplitDecision::decided(*side, DecidedBy::WorkingDirectory),
        [_, _, ..] => SplitDecision::Ask(AskBecause::WorkingDirectoriesDisagree),
        [] if directories.undecided_projects > 0 => {
            SplitDecision::Ask(AskBecause::FollowsAQuestion)
        }
        [] if directories.sides.contains(&ContextSide::Skip) => {
            SplitDecision::decided(ContextSide::Skip, DecidedBy::TemporaryPlace)
        }
        [] => SplitDecision::Ask(otherwise),
    }
}

/// A folder without git goes where the conversations that worked in it went, when they all
/// went one way.
pub(crate) fn place_folder_without_git(
    conversations: impl IntoIterator<Item = SplitDecision>,
) -> SplitDecision {
    let mut sides = BTreeSet::new();
    let mut any = false;
    for decision in conversations {
        any = true;
        if let Some(side) = decision.side().filter(|side| *side != ContextSide::Skip) {
            sides.insert(side);
        }
    }
    let mut sides = sides.into_iter();
    match (sides.next(), sides.next()) {
        (Some(side), None) => SplitDecision::decided(side, DecidedBy::ConversationsInside),
        (Some(_), Some(_)) => SplitDecision::Ask(AskBecause::WorkingDirectoriesDisagree),
        (None, _) if any => SplitDecision::Ask(AskBecause::FollowsAQuestion),
        (None, _) => SplitDecision::Ask(AskBecause::NoConversationsInside),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FolderPlace, WorkingDirectoryEvidence, place_conversation, place_folder_without_git,
    };
    use crate::domain::context_split::{AskBecause, ContextSide, DecidedBy, SplitDecision};

    fn work_project() -> SplitDecision {
        SplitDecision::decided(ContextSide::Work, DecidedBy::OwnerMailDomain)
    }

    fn personal_project() -> SplitDecision {
        SplitDecision::decided(ContextSide::Personal, DecidedBy::OwnerMailDomain)
    }

    fn undecided_project() -> SplitDecision {
        SplitDecision::Ask(AskBecause::ForeignGroup)
    }

    fn directories(projects: &[SplitDecision]) -> WorkingDirectoryEvidence {
        let mut evidence = WorkingDirectoryEvidence::default();
        for project in projects {
            evidence.record_project(*project);
        }
        evidence
    }

    #[test]
    fn a_conversation_goes_where_the_folder_of_its_project_goes() {
        assert_eq!(
            place_conversation(
                FolderPlace::Project(work_project()),
                &directories(&[work_project()])
            ),
            SplitDecision::decided(ContextSide::Work, DecidedBy::ProjectFolder)
        );
        assert_eq!(
            place_conversation(
                FolderPlace::Project(work_project()),
                &directories(&[work_project(), undecided_project()])
            ),
            SplitDecision::decided(ContextSide::Work, DecidedBy::ProjectFolder),
            "an undecided project is not a side, so it contradicts nothing"
        );
    }

    /// The folder decides where the file lies, and the working directories can only take that
    /// answer away: a transcript cannot be split, and a confident answer about a conversation
    /// that worked on both sides would send half of it to the wrong one.
    #[test]
    fn a_working_directory_on_the_other_side_turns_the_answer_into_a_question() {
        assert_eq!(
            place_conversation(
                FolderPlace::Project(work_project()),
                &directories(&[work_project(), personal_project()])
            ),
            SplitDecision::Ask(AskBecause::WorkingDirectoriesDisagree)
        );
    }

    #[test]
    fn a_conversation_of_an_undecided_project_is_the_same_question() {
        assert_eq!(
            place_conversation(
                FolderPlace::Project(undecided_project()),
                &directories(&[work_project()])
            ),
            SplitDecision::Ask(AskBecause::FollowsAQuestion)
        );
    }

    #[test]
    fn a_mixed_folder_is_decided_by_working_directories_or_asked_about() {
        assert_eq!(
            place_conversation(FolderPlace::Mixed, &directories(&[work_project()])),
            SplitDecision::decided(ContextSide::Work, DecidedBy::WorkingDirectory)
        );
        assert_eq!(
            place_conversation(
                FolderPlace::Mixed,
                &directories(&[work_project(), personal_project()])
            ),
            SplitDecision::Ask(AskBecause::WorkingDirectoriesDisagree)
        );
        let mut nowhere = WorkingDirectoryEvidence::default();
        nowhere.record_outside_projects();
        assert_eq!(
            place_conversation(FolderPlace::Mixed, &nowhere),
            SplitDecision::Ask(AskBecause::MixedFolder)
        );
        assert_eq!(
            place_conversation(FolderPlace::Unknown, &nowhere),
            SplitDecision::Ask(AskBecause::UnknownFolder)
        );
        assert_eq!(
            place_conversation(FolderPlace::Mixed, &directories(&[undecided_project()])),
            SplitDecision::Ask(AskBecause::FollowsAQuestion)
        );
    }

    #[test]
    fn throwaway_work_is_skipped_by_its_folder_or_its_directories() {
        assert_eq!(
            place_conversation(FolderPlace::Temporary, &directories(&[work_project()])),
            SplitDecision::decided(ContextSide::Skip, DecidedBy::TemporaryPlace)
        );
        let mut temporary = WorkingDirectoryEvidence::default();
        temporary.record_temporary_place();
        assert_eq!(
            place_conversation(FolderPlace::Unknown, &temporary),
            SplitDecision::decided(ContextSide::Skip, DecidedBy::TemporaryPlace)
        );
        temporary.record_project(work_project());
        assert_eq!(
            place_conversation(FolderPlace::Project(work_project()), &temporary),
            SplitDecision::decided(ContextSide::Work, DecidedBy::ProjectFolder),
            "a temporary directory a work session stepped into is nobody's side"
        );
    }

    #[test]
    fn a_folder_without_git_follows_its_conversations_only_when_they_agree() {
        let work = SplitDecision::decided(ContextSide::Work, DecidedBy::ProjectFolder);
        let personal = SplitDecision::decided(ContextSide::Personal, DecidedBy::ProjectFolder);
        let skipped = SplitDecision::decided(ContextSide::Skip, DecidedBy::TemporaryPlace);

        assert_eq!(
            place_folder_without_git([work, work, skipped]),
            SplitDecision::decided(ContextSide::Work, DecidedBy::ConversationsInside)
        );
        assert_eq!(
            place_folder_without_git([work, personal]),
            SplitDecision::Ask(AskBecause::WorkingDirectoriesDisagree)
        );
        assert_eq!(
            place_folder_without_git([SplitDecision::Ask(AskBecause::MixedFolder)]),
            SplitDecision::Ask(AskBecause::FollowsAQuestion)
        );
        assert_eq!(
            place_folder_without_git([]),
            SplitDecision::Ask(AskBecause::NoConversationsInside)
        );
    }
}
