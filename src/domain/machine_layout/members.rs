// The shape of the answer: what one group is made of and why it is one, and what no group
// holds and why not.
//
// These types carry no rule. Which thing lands in which group is decided in `grouping`; here is
// only what a decision looks like, so that a group and the reason it is one travel together to
// the question the person is asked.
use crate::domain::{NormalizedPath, RemoteHost, RemoteOwner};

/// Why a project stands alone: nothing on the disk ties it to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoSignal {
    /// A project marker and no repository.
    NotARepository,
    /// `.git` is a pointer file, whose repository is not followed.
    GitEntryIsAPointer,
    /// The `.git/config` could not be read as text.
    ConfigNotRead,
    /// The repository has no remote called `origin`.
    NoOrigin,
    /// `origin` is a path on this machine.
    OriginIsAPath,
    /// `origin` names a host and a repository directly under it, with no group between.
    OriginWithoutGroup(RemoteHost),
}

/// Why the things of one group are asked about together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupReason {
    /// Every project in it has an `origin` under this host and this group.
    OriginGroup(RemoteOwner),
    /// One project that nothing ties to another.
    NoSignal(NoSignal),
}

/// One project of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMember {
    pub(super) root: NormalizedPath,
    /// Repositories lying inside it. They go with it: two contexts may not cover one path
    /// (ADR 0008), so an inner repository has no question of its own.
    pub(super) nested: Vec<NormalizedPath>,
}

impl ProjectMember {
    pub fn root(&self) -> &NormalizedPath {
        &self.root
    }

    pub fn nested(&self) -> &[NormalizedPath] {
        &self.nested
    }
}

/// One transcript folder of a group, and the directory it was verified to be the folder of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMember {
    pub(super) path: NormalizedPath,
    pub(super) directory: NormalizedPath,
    pub(super) conversations: usize,
}

impl TranscriptMember {
    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    /// Where its conversations were started: the directory whose encoding is its name.
    pub fn directory(&self) -> &NormalizedPath {
        &self.directory
    }

    pub fn conversations(&self) -> usize {
        self.conversations
    }
}

/// One folder without git of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentMember {
    pub(super) path: NormalizedPath,
    pub(super) files: u64,
}

impl DocumentMember {
    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn files(&self) -> u64 {
        self.files
    }
}

/// Everything one group — or the remainder — holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupMembers {
    pub(super) projects: Vec<ProjectMember>,
    pub(super) transcripts: Vec<TranscriptMember>,
    pub(super) documents: Vec<DocumentMember>,
}

impl GroupMembers {
    pub fn projects(&self) -> &[ProjectMember] {
        &self.projects
    }

    pub fn transcripts(&self) -> &[TranscriptMember] {
        &self.transcripts
    }

    pub fn documents(&self) -> &[DocumentMember] {
        &self.documents
    }

    pub fn conversations(&self) -> usize {
        self.transcripts
            .iter()
            .map(TranscriptMember::conversations)
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.projects.is_empty() && self.transcripts.is_empty() && self.documents.is_empty()
    }
}

/// One question: things that belong together, and why they do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupGroup {
    pub(super) reason: GroupReason,
    pub(super) organization: bool,
    pub(super) members: GroupMembers,
}

impl SetupGroup {
    pub fn reason(&self) -> &GroupReason {
        &self.reason
    }

    /// The group of its `origin` is named after the domain of the person's mail, or its host
    /// lies under that domain. A mark for the person to read, never a decision: the group is
    /// asked about like any other.
    pub fn looks_like_your_organization(&self) -> bool {
        self.organization
    }

    pub fn members(&self) -> &GroupMembers {
        &self.members
    }
}

/// Why something found is in no group and will be in no context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftOutReason {
    /// A transcript folder of a session started in a temporary place: nobody's work.
    Temporary,
    /// A transcript folder with no conversation lying in it, so nothing to check its name by.
    NoConversations,
    /// Its conversations record no working directory.
    NoWorkingDirectory,
    /// A conversation in it was started in a directory whose encoding is another folder's
    /// name. The tool's naming rule and this one no longer agree, and nothing is placed on a
    /// rule that was just seen to disagree.
    NameDisagrees { started_in: NormalizedPath },
    /// Its conversations were started in several directories that encode to one name.
    SeveralDirectories,
    /// A transcript folder of a directory that holds projects without being one — `~/Projects`
    /// or the home itself. Its conversations could be about any of them.
    MixedFolder { directory: NormalizedPath },
    /// A transcript folder of a directory that is none of what was found.
    UnknownDirectory { directory: NormalizedPath },
    /// A folder without git holding a project: a context over it would cover the project too.
    HoldsProjects,
    /// A folder without git that conversations of several groups worked in.
    SeveralGroupsWorkedHere,
    /// The path holds `*` or `?`, which a coverage rule reads as a wildcard.
    PathIsNotExpressible,
    /// Projects inside a directory a tool keeps for itself — `~/.vscode`, `~/Library`.
    ToolsOwnPlace { projects: usize },
}

/// What kind of thing was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftOutKind {
    Project,
    TranscriptFolder,
    DocumentFolder,
}

/// One thing in no group, with why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftOut {
    pub(super) path: NormalizedPath,
    pub(super) kind: LeftOutKind,
    pub(super) conversations: usize,
    pub(super) reason: LeftOutReason,
}

impl LeftOut {
    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn kind(&self) -> LeftOutKind {
        self.kind
    }

    /// Conversations lying in it; zero for anything that is not a transcript folder.
    pub fn conversations(&self) -> usize {
        self.conversations
    }

    pub fn reason(&self) -> &LeftOutReason {
        &self.reason
    }
}
