// What a view of the graph around one node is: its question, its nodes and edges, and what it
// had to leave out.
//
// The graph of a context holds conversations with thousands of replies and files touched by
// dozens of conversations, and a picture of all of that is a picture of nothing. A view is
// therefore always bounded — two steps at most, a hundred nodes at most — and the bound is
// never silent: every kind of node says how many were found and how many are shown
// (architecture, section 15: `GraphQuery` and `GraphView`).
//
// A *step* is one crossing between a conversation and a file. The reply a touch followed
// stands on that crossing rather than making one of its own, so two steps from a conversation
// reach the other conversations of the files it touched — the question the view is for.
use std::fmt;

use super::conversation::{Speaker, TouchVerb};
use super::source_path::NormalizedPath;

/// The most steps a view walks. Three would reach every conversation of every file of every
/// conversation of a busy file, which is the graph rather than a neighbourhood of it.
pub const DEEPEST: u8 = 2;

/// The most nodes a view holds. The figure of the architecture, section 15.
pub const MOST_NODES: usize = 100;

/// What a view is centred on. Only a conversation and a file are places a person can stand:
/// a reply, a project or a topic is reached from one of them and is not walked from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphFocus {
    /// A conversation by its session, which is what a person resumes it by.
    Conversation(String),
    /// A file by its identity in this context's graph.
    File(String),
    /// A file by a path it has in this context, which is what a search row carries.
    Path(NormalizedPath),
}

impl GraphFocus {
    /// A focus as written: `conversation:<session>`, `file:<id>` or `path:<absolute path>`.
    pub fn parse(written: &str) -> Result<Self, GraphQueryError> {
        let unreadable = || GraphQueryError::UnreadableFocus(written.to_string());
        let (kind, value) = written.split_once(':').ok_or_else(unreadable)?;
        if value.trim().is_empty() {
            return Err(unreadable());
        }
        match kind {
            "conversation" => Ok(Self::Conversation(value.to_string())),
            "file" => Ok(Self::File(value.to_string())),
            "path" => NormalizedPath::from_absolute(value)
                .map(Self::Path)
                .map_err(|_| unreadable()),
            _ => Err(unreadable()),
        }
    }
}

/// One question about the neighbourhood of a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphQuery {
    focus: GraphFocus,
    depth: u8,
    limit: usize,
    include_in_passing: bool,
}

impl GraphQuery {
    /// A query inside the bounds, or a refusal naming the bound it crossed. A depth or a
    /// limit beyond them is refused rather than lowered: a person asking for three steps and
    /// shown two would read the second step as the end of the graph.
    pub fn new(
        focus: GraphFocus,
        depth: u8,
        limit: usize,
        include_in_passing: bool,
    ) -> Result<Self, GraphQueryError> {
        if !(1..=DEEPEST).contains(&depth) {
            return Err(GraphQueryError::DepthOutOfRange { asked: depth });
        }
        if !(1..=MOST_NODES).contains(&limit) {
            return Err(GraphQueryError::LimitOutOfRange { asked: limit });
        }
        Ok(Self {
            focus,
            depth,
            limit,
            include_in_passing,
        })
    }

    pub fn focus(&self) -> &GraphFocus {
        &self.focus
    }

    pub fn depth(&self) -> u8 {
        self.depth
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Whether the pairs the strict answer of ADR 0041 leaves out are drawn too.
    pub fn include_in_passing(&self) -> bool {
        self.include_in_passing
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphQueryError {
    UnreadableFocus(String),
    DepthOutOfRange { asked: u8 },
    LimitOutOfRange { asked: usize },
}

impl fmt::Display for GraphQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnreadableFocus(written) => write!(
                formatter,
                "{written:?} is not a focus; write conversation:<session>, file:<id> or path:<absolute path>"
            ),
            Self::DepthOutOfRange { asked } => {
                write!(formatter, "depth {asked} is not 1 or {DEEPEST}")
            }
            Self::LimitOutOfRange { asked } => {
                write!(
                    formatter,
                    "a view holds 1 to {MOST_NODES} nodes, not {asked}"
                )
            }
        }
    }
}

/// The kinds of node a view can hold, in the order a report lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKind {
    Conversation,
    Reply,
    /// A file of this context — or a path a touch named that no location of this context
    /// holds, which is the same kind of node in another state (`FileStanding::Outside`).
    File,
    /// A root of the context, from its configuration rather than from the graph.
    Project,
    Topic,
}

impl NodeKind {
    pub const ALL: [Self; 5] = [
        Self::Conversation,
        Self::Reply,
        Self::File,
        Self::Project,
        Self::Topic,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Reply => "reply",
            Self::File => "file",
            Self::Project => "project",
            Self::Topic => "topic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    /// A conversation and one of its replies.
    HasReply,
    /// A reply, or a conversation when no reply can be placed before the touch, and the file
    /// it touched.
    Touched,
    /// A file or a conversation and the root of the context it lies under.
    InProject,
    /// A file or a conversation and what the analysis concluded it is about.
    About,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HasReply => "has-reply",
            Self::Touched => "touched",
            Self::InProject => "in-project",
            Self::About => "about",
        }
    }
}

/// Which record of the graph a node was read from, by table and key. A project comes from
/// the configuration, and says so rather than borrowing a table it is not in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    record: &'static str,
    key: String,
}

impl Provenance {
    pub fn new(record: &'static str, key: impl Into<String>) -> Self {
        Self {
            record,
            key: key.into(),
        }
    }

    pub fn record(&self) -> &'static str {
        self.record
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

/// Whether a file is on disk, as the graph last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStanding {
    Present,
    Missing,
    /// No location of this context holds the path the touch named: a file of another context
    /// or of none. Shown by its path and walked no further (ADR 0008, 0037).
    Outside,
}

/// What a node says beyond its kind and label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeDetail {
    Conversation {
        session_id: String,
        message_count: u64,
        last_message_at_unix_ms: Option<i64>,
    },
    Reply {
        /// The node of the conversation this reply belongs to.
        conversation: String,
        ordinal: u64,
        speaker: Speaker,
        at_unix_ms: Option<i64>,
    },
    File {
        path: String,
        standing: FileStanding,
    },
    Project {
        root: String,
    },
    Topic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    id: String,
    kind: NodeKind,
    label: String,
    depth: u8,
    provenance: Provenance,
    detail: NodeDetail,
}

impl GraphNode {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        depth: u8,
        provenance: Provenance,
        detail: NodeDetail,
    ) -> Self {
        let kind = match &detail {
            NodeDetail::Conversation { .. } => NodeKind::Conversation,
            NodeDetail::Reply { .. } => NodeKind::Reply,
            NodeDetail::File { .. } => NodeKind::File,
            NodeDetail::Project { .. } => NodeKind::Project,
            NodeDetail::Topic => NodeKind::Topic,
        };
        Self {
            id: id.into(),
            kind,
            label: label.into(),
            depth,
            provenance,
            detail,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// How many steps from the focus. A project or a topic takes the depth of the node it
    /// was reached from.
    pub fn depth(&self) -> u8 {
        self.depth
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub fn detail(&self) -> &NodeDetail {
        &self.detail
    }

    /// Whether a view can be centred here: a conversation and a file of this context.
    pub fn is_focusable(&self) -> bool {
        match &self.detail {
            NodeDetail::Conversation { .. } => true,
            NodeDetail::File { standing, .. } => *standing != FileStanding::Outside,
            _ => false,
        }
    }
}

/// What the conversation did to the file, on the edge that says it touched it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchMark {
    verbs: Vec<TouchVerb>,
    named: bool,
    times: u32,
    by_time: bool,
}

impl TouchMark {
    pub fn new(verbs: Vec<TouchVerb>, named: bool, times: u32) -> Self {
        Self {
            verbs,
            named,
            times,
            by_time: false,
        }
    }

    /// The same mark on an edge drawn from a reply that was found by time — the latest reply
    /// said at or before the touch — and not read from any record that joins them.
    pub fn placed_by_time(mut self) -> Self {
        self.by_time = true;
        self
    }

    /// Whether the reply this touch is drawn from is a reading by time rather than a fact.
    pub fn by_time(&self) -> bool {
        self.by_time
    }

    pub fn verbs(&self) -> &[TouchVerb] {
        &self.verbs
    }

    /// Whether the owner named the file in their own words in this conversation.
    pub fn named(&self) -> bool {
        self.named
    }

    /// Zero when the rows were written before the count existed (ADR 0041).
    pub fn times(&self) -> u32 {
        self.times
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    from: String,
    to: String,
    kind: EdgeKind,
    touch: Option<TouchMark>,
}

impl GraphEdge {
    pub fn new(from: impl Into<String>, to: impl Into<String>, kind: EdgeKind) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
            touch: None,
        }
    }

    pub fn touched(from: impl Into<String>, to: impl Into<String>, mark: TouchMark) -> Self {
        Self {
            touch: Some(mark),
            ..Self::new(from, to, EdgeKind::Touched)
        }
    }

    pub fn from(&self) -> &str {
        &self.from
    }

    pub fn to(&self) -> &str {
        &self.to
    }

    pub fn kind(&self) -> EdgeKind {
        self.kind
    }

    pub fn touch(&self) -> Option<&TouchMark> {
        self.touch.as_ref()
    }
}

/// How many nodes of one kind the walk found, and how many of them the view holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KindCount {
    pub shown: usize,
    pub found: usize,
}
