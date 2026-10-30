// The groups `setup` asks about, one question each.
//
//   what                                 goes                                   because
//   projects with one `origin` group     together, one group                    one group of `origin`
//   a project with nothing tying it      a group of its own                     no signal
//   a repository inside a project        with the project                       it lies inside it
//   a transcript folder                  with the project it is the folder of   the conversations of it
//   a folder without git                 with the group whose conversations     they worked in it
//                                        worked in it
//   a folder without git nobody          the remainder, shown before the        no conversation worked in it
//   worked in                            first question
//   everything else found                nowhere, with the reason               see `LeftOutReason`
//
// A conversation and a folder without git never get a question of their own: they go where the
// project they belong to goes, and a folder without a project to follow is in the remainder,
// which the person answers about as a whole. A group of `origin` named after the domain of the
// person's own mail is marked as looking like their organisation, and that is a mark: it is
// asked about like every other group.
use crate::domain::context_proposal::TreeEvidence;
use crate::domain::{NormalizedPath, OwnerDomain, RemoteOwner};

use super::members::{
    DocumentMember, GroupMembers, GroupReason, LeftOut, LeftOutKind, LeftOutReason, SetupGroup,
    TranscriptMember,
};
use super::projects::{Project, ProjectTie, projects_of};
use super::survey::{DocumentFolderSurvey, TranscriptFolderSurvey};
use super::transcripts::{TranscriptPlace, place_transcript_folder};

/// The directories of a home whose children are folders a person keeps documents in. Three and
/// not the whole standard layout: `Pictures`, `Music` and `Movies` hold the libraries of an
/// application, not folders somebody named.
const DOCUMENT_PLACES: [&str; 3] = ["Documents", "Desktop", "Downloads"];

/// Everything `setup` found, grouped for its questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineGroups {
    groups: Vec<SetupGroup>,
    remainder: GroupMembers,
    left_out: Vec<LeftOut>,
}

impl MachineGroups {
    /// The groups of one walked tree. `transcripts` are the folders under `~/.claude/projects`
    /// and `documents` the folders without git listed in `places_of_document_folders`.
    pub fn assemble(
        evidence: &TreeEvidence,
        home: Option<&NormalizedPath>,
        transcripts: &[TranscriptFolderSurvey],
        documents: &[DocumentFolderSurvey],
        owner: &OwnerDomain,
    ) -> Self {
        let (projects, mut left_out) = projects_of(&evidence.subtrees, &evidence.root, home);
        let documents = documents_beside(documents, &projects, &mut left_out);
        let (mut groups, group_of_project) = group_projects(&projects, owner);

        let mut on_documents: Vec<Vec<TranscriptMember>> = vec![Vec::new(); documents.len()];
        let mut worked_in: Vec<Vec<usize>> = vec![Vec::new(); documents.len()];
        for folder in transcripts {
            let conversations = folder.conversations().len();
            match place_transcript_folder(folder, &projects, &documents) {
                Ok((place, directory)) => {
                    let member = TranscriptMember {
                        path: folder.path().clone(),
                        directory,
                        conversations,
                    };
                    match place {
                        TranscriptPlace::Project(project) => {
                            let group = group_of_project[project];
                            record_work(folder, &documents, group, &mut worked_in);
                            groups[group].members.transcripts.push(member);
                        }
                        TranscriptPlace::Document(document) => {
                            on_documents[document].push(member);
                        }
                    }
                }
                Err(reason) => left_out.push(LeftOut {
                    path: folder.path().clone(),
                    kind: LeftOutKind::TranscriptFolder,
                    conversations,
                    reason,
                }),
            }
        }

        let mut remainder = GroupMembers::default();
        for ((document, transcripts), groups_that_worked) in
            documents.iter().zip(on_documents).zip(worked_in)
        {
            let member = DocumentMember {
                path: document.path().clone(),
                files: document.files(),
            };
            let target = match groups_that_worked.as_slice() {
                [] => &mut remainder,
                [group] => &mut groups[*group].members,
                [_, _, ..] => {
                    left_out.push(LeftOut {
                        path: member.path,
                        kind: LeftOutKind::DocumentFolder,
                        conversations: 0,
                        reason: LeftOutReason::SeveralGroupsWorkedHere,
                    });
                    for transcript in transcripts {
                        left_out.push(LeftOut {
                            conversations: transcript.conversations,
                            path: transcript.path,
                            kind: LeftOutKind::TranscriptFolder,
                            reason: LeftOutReason::SeveralGroupsWorkedHere,
                        });
                    }
                    continue;
                }
            };
            target.documents.push(member);
            target.transcripts.extend(transcripts);
        }

        for group in &mut groups {
            sort_members(&mut group.members);
        }
        sort_members(&mut remainder);
        left_out.sort_by(|first, second| first.path.as_str().cmp(second.path.as_str()));
        Self {
            groups,
            remainder,
            left_out,
        }
    }

    pub fn groups(&self) -> &[SetupGroup] {
        &self.groups
    }

    /// Folders without git that no group's conversations worked in, with their own transcript
    /// folders: what the question before the first group is about.
    pub fn remainder(&self) -> &GroupMembers {
        &self.remainder
    }

    /// The remainder folder by folder, each with the transcript folders whose conversations
    /// were started inside it: what is asked about one at a time when the remainder as a whole
    /// went to no context.
    pub fn remainder_folders(&self) -> Vec<GroupMembers> {
        self.remainder
            .documents
            .iter()
            .map(|document| GroupMembers {
                projects: Vec::new(),
                transcripts: self
                    .remainder
                    .transcripts
                    .iter()
                    .filter(|transcript| transcript.directory.is_within(&document.path))
                    .cloned()
                    .collect(),
                documents: vec![document.clone()],
            })
            .collect()
    }

    pub fn left_out(&self) -> &[LeftOut] {
        &self.left_out
    }

    /// Whether there is anything at all to put in a context.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.remainder.is_empty()
    }
}

/// The directories whose children the census lists as folders without git: the three document
/// places of a walked home, and every directory that holds a project directly — the home
/// itself excepted, and nothing inside a project or a tool's own place.
pub fn places_of_document_folders(
    evidence: &TreeEvidence,
    home: Option<&NormalizedPath>,
) -> Vec<NormalizedPath> {
    let (projects, _) = projects_of(&evidence.subtrees, &evidence.root, home);
    let mut places: Vec<NormalizedPath> = Vec::new();
    if home == Some(&evidence.root) {
        places.extend(
            evidence
                .subtrees
                .iter()
                .filter(|subtree| subtree.path().parent().as_ref() == Some(&evidence.root))
                .filter(|subtree| DOCUMENT_PLACES.contains(&subtree.path().file_name()))
                .filter(|subtree| !subtree.is_a_candidate())
                .map(|subtree| subtree.path().clone()),
        );
    }
    for project in &projects {
        let Some(parent) = project.member.root.parent() else {
            continue;
        };
        let inside_a_project = projects
            .iter()
            .any(|other| parent.is_within(&other.member.root));
        if Some(&parent) == home || !parent.is_within(&evidence.root) || inside_a_project {
            continue;
        }
        if !places.contains(&parent) {
            places.push(parent);
        }
    }
    places.sort_by(|first, second| first.as_str().cmp(second.as_str()));
    places
}

/// The folders that can be a root of a context: not a project, not holding one, and spelled
/// so that a coverage rule means this folder alone. The others are left out with the reason.
fn documents_beside(
    documents: &[DocumentFolderSurvey],
    projects: &[Project],
    left_out: &mut Vec<LeftOut>,
) -> Vec<DocumentFolderSurvey> {
    let mut kept = Vec::new();
    for document in documents {
        let path = document.path();
        if projects
            .iter()
            .any(|project| path.is_within(&project.member.root))
        {
            // A project itself, or a folder inside one: it is already its project's.
            continue;
        }
        if projects
            .iter()
            .any(|project| project.member.root.is_within(path))
        {
            left_out.push(left_out_folder(path, LeftOutReason::HoldsProjects));
            continue;
        }
        if path.as_str().contains(['*', '?']) {
            left_out.push(left_out_folder(path, LeftOutReason::PathIsNotExpressible));
            continue;
        }
        kept.push(document.clone());
    }
    kept
}

fn left_out_folder(path: &NormalizedPath, reason: LeftOutReason) -> LeftOut {
    LeftOut {
        path: path.clone(),
        kind: LeftOutKind::DocumentFolder,
        conversations: 0,
        reason,
    }
}

/// One group per group of `origin`, then one per project nothing ties to another; and for
/// every project, the index of its group. Projects are taken owned first and by owner, so the
/// projects of one owner arrive one after another and join the group the first of them opened.
fn group_projects(projects: &[Project], owner: &OwnerDomain) -> (Vec<SetupGroup>, Vec<usize>) {
    let mut order: Vec<usize> = (0..projects.len()).collect();
    order.sort_by(|&first, &second| sort_key(&projects[first]).cmp(&sort_key(&projects[second])));

    let mut groups: Vec<SetupGroup> = Vec::new();
    let mut group_of_project = vec![0; projects.len()];
    for index in order {
        let project = &projects[index];
        let reason = match &project.tie {
            ProjectTie::Owned(remote) => GroupReason::OriginGroup(remote.clone()),
            ProjectTie::Alone(why) => GroupReason::NoSignal(why.clone()),
        };
        let joins_the_last = matches!(&project.tie, ProjectTie::Owned(_))
            && groups.last().is_some_and(|last| last.reason == reason);
        if !joins_the_last {
            let organization = match &project.tie {
                ProjectTie::Owned(remote) => {
                    owner.one().is_some_and(|domain| remote.lies_under(domain))
                }
                ProjectTie::Alone(_) => false,
            };
            groups.push(SetupGroup {
                reason,
                organization,
                members: GroupMembers::default(),
            });
        }
        let group = groups.len() - 1;
        groups[group].members.projects.push(project.member.clone());
        group_of_project[index] = group;
    }
    (groups, group_of_project)
}

/// Owned projects first, by host and group; then the others, by path.
fn sort_key(project: &Project) -> (u8, Option<&RemoteOwner>, &str) {
    match &project.tie {
        ProjectTie::Owned(remote) => (0, Some(remote), project.member.root.as_str()),
        ProjectTie::Alone(_) => (1, None, project.member.root.as_str()),
    }
}

/// Which folders without git the conversations of one transcript folder worked in, on behalf
/// of the group the folder went to.
fn record_work(
    folder: &TranscriptFolderSurvey,
    documents: &[DocumentFolderSurvey],
    group: usize,
    worked_in: &mut [Vec<usize>],
) {
    for (index, document) in documents.iter().enumerate() {
        let worked = folder.conversations().iter().any(|conversation| {
            conversation
                .working_directories()
                .iter()
                .any(|directory| directory.is_within(document.path()))
        });
        if worked && !worked_in[index].contains(&group) {
            worked_in[index].push(group);
        }
    }
}

fn sort_members(members: &mut GroupMembers) {
    members
        .projects
        .sort_by(|first, second| first.root.as_str().cmp(second.root.as_str()));
    members
        .transcripts
        .sort_by(|first, second| first.path.as_str().cmp(second.path.as_str()));
    members
        .documents
        .sort_by(|first, second| first.path.as_str().cmp(second.path.as_str()));
}

#[cfg(test)]
#[path = "grouping_tests.rs"]
mod tests;
