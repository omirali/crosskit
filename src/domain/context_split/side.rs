// The answer the split gives about one thing on the machine: which context it goes to and what
// decided it, or that nothing did and the owner has to be asked.
//
// Two closed sets and no string. A reason a report prints is a variant somebody can count, and
// a question the rule could not answer is kept apart from an answer it gave: the whole stage is
// the difference between the two.

/// Where one thing goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ContextSide {
    Work,
    Personal,
    /// Nobody's work: a test run's temporary directory, a tool's scratch space. A side of its
    /// own rather than an absence, because a context that took it would index what no person
    /// wrote.
    Skip,
}

impl ContextSide {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Personal => "personal",
            Self::Skip => "skip",
        }
    }

    /// The side a label written by a person names, exactly as the owner's file spells it.
    pub(crate) fn of_label(label: &str) -> Option<Self> {
        match label {
            "work" => Some(Self::Work),
            "personal" => Some(Self::Personal),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

/// What decided a thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DecidedBy {
    /// The group or the host of `origin` carries the domain of the owner's own mail.
    OwnerMailDomain,
    /// A repository inside another follows the one that holds it: two contexts may not cover
    /// one path (ADR 0008), so the inner one has no answer of its own.
    EnclosingRepository,
    /// The directory lies where the system keeps throwaway work.
    TemporaryPlace,
    /// A conversation filed under the transcript folder of a decided project.
    ProjectFolder,
    /// A conversation whose folder names no project, and whose working directories lie in
    /// decided projects of one side.
    WorkingDirectory,
    /// A folder without git, all of whose conversations were decided to one side.
    ConversationsInside,
}

/// Why nothing decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum AskBecause {
    /// The repository's own `[user]` names a mail domain other than the owner's: it was set
    /// up apart from the rest of the machine, and the machine's rule does not speak for it.
    OwnIdentity,
    /// No `origin`, or an `origin` that points at a path on this machine.
    NoRemote,
    /// `origin` names a host and a repository directly under it, with no group between.
    NoGroup,
    /// The machine records no mail domain of the owner, so no group can be matched to one.
    NoOwnerDomain,
    /// The machine records several mail domains of the owner, and which of them is the
    /// employer's is not written anywhere on it.
    SeveralOwnerDomains,
    /// `origin` names a group that does not carry the owner's mail domain — on any host,
    /// including the host of a group that does.
    ForeignGroup,
    /// It follows something that is itself a question — a project, or the conversations
    /// that worked in a folder: the same question, not a new one.
    FollowsAQuestion,
    /// A transcript folder that encodes a directory holding several projects.
    MixedFolder,
    /// A transcript folder that encodes no directory the measurement knows.
    UnknownFolder,
    /// The working directories inside a conversation lie in projects of different sides, or
    /// contradict the side of the folder it is filed under.
    WorkingDirectoriesDisagree,
    /// A folder without git in which no conversation worked.
    NoConversationsInside,
}

/// The rule's answer about one thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum SplitDecision {
    Decided { side: ContextSide, by: DecidedBy },
    Ask(AskBecause),
}

impl SplitDecision {
    pub(crate) fn decided(side: ContextSide, by: DecidedBy) -> Self {
        Self::Decided { side, by }
    }

    /// The side, when the rule gave one.
    pub(crate) fn side(&self) -> Option<ContextSide> {
        match self {
            Self::Decided { side, .. } => Some(*side),
            Self::Ask(_) => None,
        }
    }
}
