// The shape of the answer: what a draft of contexts is made of, and what every piece of it
// stands on.
//
// These types carry no rule. Which directory ends up where is decided in
// `context_grouping`; here is only what a decision looks like once it has been made, so that
// a proposal and the evidence behind it cannot be separated on the way to the printer.
use super::context_proposal::SubtreeVolume;
use super::git_remote::RemoteHost;
use super::project_signals::{HomePlace, ProjectMarker, RepositorySignal};
use super::source_path::NormalizedPath;
use super::workspace::WorkspaceName;

/// Why a group of directories is one context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextBasis {
    RemoteHost(RemoteHost),
    RepositoriesWithoutARemote,
    ProjectsWithoutARepository,
    PersonalDocuments,
}

impl ContextBasis {
    /// The name this basis suggests. It is a suggestion and nothing else: the operator
    /// renames it, and every name is checked against `WorkspaceName` before it is offered.
    pub(super) fn suggested_name(&self) -> String {
        match self {
            Self::RemoteHost(host) => name_from_host(host.as_str()),
            Self::RepositoriesWithoutARemote => "without-remote".to_string(),
            Self::ProjectsWithoutARepository => "without-repository".to_string(),
            Self::PersonalDocuments => "personal".to_string(),
        }
    }

    /// The order contexts are printed in: hosts first and alphabetically, then the groups
    /// that stand on the absence of a signal.
    pub(super) fn rank(&self) -> (u8, String) {
        match self {
            Self::RemoteHost(host) => (0, host.as_str().to_string()),
            Self::RepositoriesWithoutARemote => (1, String::new()),
            Self::ProjectsWithoutARepository => (2, String::new()),
            Self::PersonalDocuments => (3, String::new()),
        }
    }
}

/// A host written as a name. What a name may hold is `WorkspaceName`'s rule; everything else
/// becomes a dash. Turning `.` into `-` here is not a sanitizer deciding for the operator:
/// the host itself is printed beside the name as the evidence it stands on.
fn name_from_host(host: &str) -> String {
    host.chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

const FALLBACK_NAME: &str = "context";

/// The first name of this shape that a context may be called and no other context has taken.
/// Two hosts can spell one name, and a draft with one name twice would not load.
pub(super) fn unique_name(preferred: &str, taken: &mut Vec<WorkspaceName>) -> WorkspaceName {
    let base = WorkspaceName::parse(preferred)
        .map(|name| name.as_str().to_string())
        .unwrap_or_else(|_| FALLBACK_NAME.to_string());
    let mut attempt: u32 = 1;
    loop {
        let candidate = if attempt == 1 {
            base.clone()
        } else {
            format!("{base}-{attempt}")
        };
        if let Ok(name) = WorkspaceName::parse(&candidate)
            && !taken.contains(&name)
        {
            taken.push(name.clone());
            return name;
        }
        attempt += 1;
    }
}

/// What one proposal stands on. A proposal without evidence is not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    Repository(RepositorySignal),
    Marker(ProjectMarker),
    HomePlace(HomePlace),
    /// Repositories that lie inside this one. They are covered by it rather than proposed on
    /// their own, because two contexts may not cover one path.
    NestedRepositories(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedInclude {
    path: NormalizedPath,
    volume: SubtreeVolume,
    evidence: Vec<Evidence>,
}

impl ProposedInclude {
    pub(super) fn new(
        path: NormalizedPath,
        volume: SubtreeVolume,
        evidence: Vec<Evidence>,
    ) -> Self {
        Self {
            path,
            volume,
            evidence,
        }
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn volume(&self) -> SubtreeVolume {
        self.volume
    }

    pub fn evidence(&self) -> &[Evidence] {
        &self.evidence
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedContext {
    name: WorkspaceName,
    basis: ContextBasis,
    includes: Vec<ProposedInclude>,
}

impl ProposedContext {
    pub(super) fn new(
        name: WorkspaceName,
        basis: ContextBasis,
        includes: Vec<ProposedInclude>,
    ) -> Self {
        Self {
            name,
            basis,
            includes,
        }
    }

    pub fn name(&self) -> &WorkspaceName {
        &self.name
    }

    pub fn basis(&self) -> &ContextBasis {
        &self.basis
    }

    pub fn includes(&self) -> &[ProposedInclude] {
        &self.includes
    }
}

/// Why a subtree is left to the operator instead of being proposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAttributed {
    /// Nothing in it named a repository or a project.
    NoSignal,
    /// Files that lie beside the projects inside it rather than in one of them.
    BesideProjects,
    /// A directory of the standard home layout that holds state of tools and the system.
    ToolStore,
    /// A directory of the standard home layout that everything arrives in.
    MixedInbox,
    /// A whole-directory proposal would cover the projects inside it, and two contexts may
    /// not cover one path, so neither is offered for it.
    OverlapsProjects,
    /// A repository whose remotes name several hosts and whose `origin` settles nothing.
    SeveralRemoteHosts(Vec<RemoteHost>),
    /// The path holds `*` or `?`, which a coverage pattern reads as a wildcard. It cannot be
    /// written as a rule that means this directory and only this one.
    PathIsNotExpressible,
    /// Files lying directly in the walked root.
    DirectlyInTheRoot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnattributedSubtree {
    path: NormalizedPath,
    volume: SubtreeVolume,
    remaining_files: u64,
    remaining_bytes: u64,
    reason: NotAttributed,
}

impl UnattributedSubtree {
    pub(super) fn new(
        path: NormalizedPath,
        volume: SubtreeVolume,
        remaining_files: u64,
        remaining_bytes: u64,
        reason: NotAttributed,
    ) -> Self {
        Self {
            path,
            volume,
            remaining_files,
            remaining_bytes,
            reason,
        }
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    /// The whole subtree, proposals inside it included.
    pub fn volume(&self) -> SubtreeVolume {
        self.volume
    }

    /// The part of it no proposal covers.
    pub fn remaining_files(&self) -> u64 {
        self.remaining_files
    }

    pub fn remaining_bytes(&self) -> u64 {
        self.remaining_bytes
    }

    pub fn covers_the_whole_subtree(&self) -> bool {
        self.remaining_files == self.volume.files()
    }

    pub fn reason(&self) -> &NotAttributed {
        &self.reason
    }
}
