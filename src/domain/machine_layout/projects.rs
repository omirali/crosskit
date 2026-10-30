// The projects of a walked tree, and what ties each of them to others.
//
// A project is a directory the walk found a repository or a project marker in, with no other
// such directory above it — the same candidates `contexts propose` starts from. A repository
// inside a project is not a project of its own: it goes with the one it lies in, and the one
// it lies in says so. What ties a project to others is one thing only, the group of its
// `origin`; a host is not enough, because one host was measured holding three organisations
// (Stage 38).
use crate::domain::context_proposal::SubtreeEvidence;
use crate::domain::project_signals::{GitMarker, HomePlace};
use crate::domain::{NormalizedPath, Origin, RemoteOwner};

use super::members::{LeftOut, LeftOutKind, LeftOutReason, NoSignal, ProjectMember};

/// One project, with what ties it to others.
#[derive(Debug, Clone)]
pub(super) struct Project {
    pub(super) member: ProjectMember,
    pub(super) tie: ProjectTie,
}

#[derive(Debug, Clone)]
pub(super) enum ProjectTie {
    Owned(RemoteOwner),
    Alone(NoSignal),
}

/// Every project of the tree, and the candidates that are no project of anybody's: the ones
/// inside a directory a tool keeps for itself, counted per directory, and the ones a coverage
/// rule cannot spell.
pub(super) fn projects_of(
    subtrees: &[SubtreeEvidence],
    root: &NormalizedPath,
    home: Option<&NormalizedPath>,
) -> (Vec<Project>, Vec<LeftOut>) {
    let mut projects: Vec<Project> = Vec::new();
    let mut left_out: Vec<LeftOut> = Vec::new();
    let mut passed_over: Vec<NormalizedPath> = Vec::new();

    for subtree in subtrees
        .iter()
        .filter(|subtree| subtree.path() != root && subtree.is_a_candidate())
    {
        let path = subtree.path();
        if passed_over.iter().any(|skipped| path.is_within(skipped)) {
            continue;
        }
        if let Some(covering) = projects
            .iter_mut()
            .find(|project| path.is_within(&project.member.root))
        {
            if subtree.repository().is_some() {
                covering.member.nested.push(path.clone());
            }
            continue;
        }
        if let Some(place) = home.and_then(|home| tools_own_place(path, home)) {
            count_in_tools_place(&mut left_out, place);
            passed_over.push(path.clone());
            continue;
        }
        if path.as_str().contains(['*', '?']) {
            left_out.push(LeftOut {
                path: path.clone(),
                kind: LeftOutKind::Project,
                conversations: 0,
                reason: LeftOutReason::PathIsNotExpressible,
            });
            passed_over.push(path.clone());
            continue;
        }
        projects.push(Project {
            member: ProjectMember {
                root: path.clone(),
                nested: Vec::new(),
            },
            tie: tie_of(subtree),
        });
    }
    (projects, left_out)
}

fn tie_of(subtree: &SubtreeEvidence) -> ProjectTie {
    let Some(repository) = subtree.repository() else {
        return ProjectTie::Alone(NoSignal::NotARepository);
    };
    if repository.marker() == GitMarker::NotADirectory {
        return ProjectTie::Alone(NoSignal::GitEntryIsAPointer);
    }
    match repository.origin() {
        None => ProjectTie::Alone(NoSignal::ConfigNotRead),
        Some(Origin::Owned(owner)) => ProjectTie::Owned(owner.clone()),
        Some(Origin::Absent) => ProjectTie::Alone(NoSignal::NoOrigin),
        Some(Origin::NotAHost) => ProjectTie::Alone(NoSignal::OriginIsAPath),
        Some(Origin::Ungrouped(host)) => {
            ProjectTie::Alone(NoSignal::OriginWithoutGroup(host.clone()))
        }
    }
}

/// The directory directly in the home that a tool keeps for itself, when `path` lies in one: a
/// name starting with a dot, which is how a tool hides its state, or a place the home layout
/// fixes as a tool's store. The extensions of an editor are projects by every marker and
/// nobody's work.
fn tools_own_place(path: &NormalizedPath, home: &NormalizedPath) -> Option<NormalizedPath> {
    let mut child = path.clone();
    loop {
        let parent = child.parent()?;
        if parent == *home {
            break;
        }
        child = parent;
    }
    let name = child.file_name();
    let tools =
        name.starts_with('.') || HomePlace::of_home_child(name) == Some(HomePlace::ToolStore);
    tools.then_some(child)
}

fn count_in_tools_place(left_out: &mut Vec<LeftOut>, place: NormalizedPath) {
    for entry in left_out.iter_mut() {
        if let (true, LeftOutReason::ToolsOwnPlace { projects }) =
            (entry.path == place, &mut entry.reason)
        {
            *projects += 1;
            return;
        }
    }
    left_out.push(LeftOut {
        path: place,
        kind: LeftOutKind::Project,
        conversations: 0,
        reason: LeftOutReason::ToolsOwnPlace { projects: 1 },
    });
}
