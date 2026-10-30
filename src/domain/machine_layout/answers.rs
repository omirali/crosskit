// What the person said about each group, and the contexts that makes.
//
// A context is a name the person typed and the roots of the groups they sent to it: each
// project's directory, its transcript folders and the folders without git its conversations
// worked in. Nothing reaches a context except through an answer about the group it lies in. The
// remainder — folders nobody's conversations worked in, and the groups left without an answer —
// goes to one context when the person named one for it before the first group; otherwise each
// folder of it is asked about after the groups, one at a time. Either way it is written as the
// list of its directories, never as a rule that would also take whatever appears beside them
// later.
use crate::domain::{NormalizedPath, WorkspaceName};

use super::grouping::MachineGroups;
use super::members::GroupMembers;

/// The answer about one group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupAnswer {
    /// To the context with this index among the names given.
    Context(usize),
    /// To no context.
    Skip,
    /// No answer: to the remainder's context when there is one, otherwise to none.
    Unanswered,
}

/// What the person said about the remainder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemainderAnswer {
    /// All of it — its folders and every group left without an answer — to this context.
    Context(usize),
    /// Folder by folder, one answer each in the order of `remainder_folders()`; a group left
    /// without an answer goes nowhere.
    OneByOne(Vec<GroupAnswer>),
}

impl RemainderAnswer {
    fn context(&self) -> Option<usize> {
        match self {
            Self::Context(context) => Some(*context),
            Self::OneByOne(_) => None,
        }
    }
}

/// Every answer of one conversation with the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupAnswers {
    contexts: Vec<WorkspaceName>,
    remainder: RemainderAnswer,
    groups: Vec<GroupAnswer>,
}

/// Answers that do not describe contexts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerError {
    NoContext,
    NameGivenTwice(WorkspaceName),
    NoSuchContext(usize),
    AnswersDoNotMatchGroups { groups: usize, answers: usize },
    AnswersDoNotMatchFolders { folders: usize, answers: usize },
}

impl SetupAnswers {
    pub fn new(
        contexts: Vec<WorkspaceName>,
        remainder: RemainderAnswer,
        groups: Vec<GroupAnswer>,
    ) -> Result<Self, AnswerError> {
        if contexts.is_empty() {
            return Err(AnswerError::NoContext);
        }
        for (index, name) in contexts.iter().enumerate() {
            if contexts[..index].contains(name) {
                return Err(AnswerError::NameGivenTwice(name.clone()));
            }
        }
        let folders: &[GroupAnswer] = match &remainder {
            RemainderAnswer::Context(_) => &[],
            RemainderAnswer::OneByOne(folders) => folders,
        };
        let chosen =
            remainder
                .context()
                .into_iter()
                .chain(
                    groups
                        .iter()
                        .chain(folders)
                        .filter_map(|answer| match answer {
                            GroupAnswer::Context(index) => Some(*index),
                            GroupAnswer::Skip | GroupAnswer::Unanswered => None,
                        }),
                );
        for index in chosen {
            if index >= contexts.len() {
                return Err(AnswerError::NoSuchContext(index));
            }
        }
        Ok(Self {
            contexts,
            remainder,
            groups,
        })
    }
}

/// What kind of directory a root of a context is, for the line that explains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootKind {
    Project {
        nested: usize,
    },
    Transcripts {
        directory: NormalizedPath,
        conversations: usize,
    },
    Document {
        files: u64,
    },
}

/// Which answer put a root in its context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    /// The answer about the group with this index.
    Group(usize),
    /// The group with this index was left without an answer, and the remainder's context
    /// took it.
    UnansweredGroup(usize),
    /// A folder of the remainder itself, sent with all of it.
    Remainder,
    /// A folder of the remainder, sent by the answer about this one folder.
    RemainderFolder,
}

/// One `include` of a context, and the answer that put it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaidOutRoot {
    path: NormalizedPath,
    kind: RootKind,
    source: RootSource,
}

impl LaidOutRoot {
    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn kind(&self) -> &RootKind {
        &self.kind
    }

    pub fn source(&self) -> RootSource {
        self.source
    }
}

/// One context to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaidOutContext {
    name: WorkspaceName,
    roots: Vec<LaidOutRoot>,
}

impl LaidOutContext {
    pub fn name(&self) -> &WorkspaceName {
        &self.name
    }

    pub fn roots(&self) -> &[LaidOutRoot] {
        &self.roots
    }

    pub fn projects(&self) -> usize {
        self.count(|kind| matches!(kind, RootKind::Project { .. }))
    }

    pub fn document_folders(&self) -> usize {
        self.count(|kind| matches!(kind, RootKind::Document { .. }))
    }

    pub fn conversations(&self) -> usize {
        self.roots
            .iter()
            .map(|root| match root.kind {
                RootKind::Transcripts { conversations, .. } => conversations,
                RootKind::Project { .. } | RootKind::Document { .. } => 0,
            })
            .sum()
    }

    fn count(&self, kind: impl Fn(&RootKind) -> bool) -> usize {
        self.roots.iter().filter(|root| kind(&root.kind)).count()
    }
}

/// The contexts the answers make, and what stays outside all of them by the person's word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextLayout {
    contexts: Vec<LaidOutContext>,
    /// Names given that no group was sent to: a context with nothing in it is not written.
    empty: Vec<WorkspaceName>,
    skipped: GroupMembers,
    /// Groups left without an answer and the remainder, when no context was named for them.
    unanswered: GroupMembers,
}

impl ContextLayout {
    /// The contexts that hold something, in the order their names were given.
    pub fn contexts(&self) -> &[LaidOutContext] {
        &self.contexts
    }

    pub fn empty(&self) -> &[WorkspaceName] {
        &self.empty
    }

    pub fn skipped(&self) -> &GroupMembers {
        &self.skipped
    }

    pub fn unanswered(&self) -> &GroupMembers {
        &self.unanswered
    }
}

impl MachineGroups {
    /// The contexts `answers` make of these groups. One answer per group, in the order of
    /// `groups()`; the answers were checked against the names when they were built, and
    /// against the groups here.
    pub fn lay_out(&self, answers: &SetupAnswers) -> Result<ContextLayout, AnswerError> {
        if answers.groups.len() != self.groups().len() {
            return Err(AnswerError::AnswersDoNotMatchGroups {
                groups: self.groups().len(),
                answers: answers.groups.len(),
            });
        }
        let mut roots: Vec<Vec<LaidOutRoot>> = vec![Vec::new(); answers.contexts.len()];
        let mut skipped = GroupMembers::default();
        let mut unanswered = GroupMembers::default();

        for (index, (group, answer)) in self.groups().iter().zip(&answers.groups).enumerate() {
            let (target, source) = match (answer, answers.remainder.context()) {
                (GroupAnswer::Context(context), _) => (*context, RootSource::Group(index)),
                (GroupAnswer::Unanswered, Some(context)) => {
                    (context, RootSource::UnansweredGroup(index))
                }
                (GroupAnswer::Skip, _) => {
                    absorb(&mut skipped, group.members());
                    continue;
                }
                (GroupAnswer::Unanswered, None) => {
                    absorb(&mut unanswered, group.members());
                    continue;
                }
            };
            roots[target].extend(roots_of(group.members(), source));
        }
        match &answers.remainder {
            RemainderAnswer::Context(context) => {
                roots[*context].extend(roots_of(self.remainder(), RootSource::Remainder))
            }
            RemainderAnswer::OneByOne(folders) => {
                let asked = self.remainder_folders();
                if folders.len() != asked.len() {
                    return Err(AnswerError::AnswersDoNotMatchFolders {
                        folders: asked.len(),
                        answers: folders.len(),
                    });
                }
                for (folder, answer) in asked.iter().zip(folders) {
                    match answer {
                        GroupAnswer::Context(context) => {
                            roots[*context].extend(roots_of(folder, RootSource::RemainderFolder))
                        }
                        GroupAnswer::Skip => absorb(&mut skipped, folder),
                        GroupAnswer::Unanswered => absorb(&mut unanswered, folder),
                    }
                }
            }
        }

        let mut contexts = Vec::new();
        let mut empty = Vec::new();
        for (name, roots) in answers.contexts.iter().zip(roots) {
            match roots.is_empty() {
                true => empty.push(name.clone()),
                false => contexts.push(LaidOutContext {
                    name: name.clone(),
                    roots,
                }),
            }
        }
        Ok(ContextLayout {
            contexts,
            empty,
            skipped,
            unanswered,
        })
    }
}

fn roots_of(members: &GroupMembers, source: RootSource) -> Vec<LaidOutRoot> {
    let projects = members.projects().iter().map(|project| LaidOutRoot {
        path: project.root().clone(),
        kind: RootKind::Project {
            nested: project.nested().len(),
        },
        source,
    });
    let transcripts = members.transcripts().iter().map(|transcript| LaidOutRoot {
        path: transcript.path().clone(),
        kind: RootKind::Transcripts {
            directory: transcript.directory().clone(),
            conversations: transcript.conversations(),
        },
        source,
    });
    let documents = members.documents().iter().map(|document| LaidOutRoot {
        path: document.path().clone(),
        kind: RootKind::Document {
            files: document.files(),
        },
        source,
    });
    projects.chain(transcripts).chain(documents).collect()
}

fn absorb(into: &mut GroupMembers, members: &GroupMembers) {
    into.projects.extend(members.projects().iter().cloned());
    into.transcripts
        .extend(members.transcripts().iter().cloned());
    into.documents.extend(members.documents().iter().cloned());
}

#[cfg(test)]
#[path = "answers_tests.rs"]
mod tests;
