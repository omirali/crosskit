// The machine answer of `crosskit graph --json`: a bounded view, as one object of the schema
// `machine_answer` declares and numbers.
//
// The same rules hold as for a page of `search`: one object on stdout, a failure included; a
// field holds only what the core knows, and what it does not know is absent rather than empty.
// The view is `GraphView` of the architecture, section 15 — nodes, edges, whether it was cut
// and where its nodes were read from — with the cut said kind by kind.
use serde::Serialize;

use crate::domain::conversation::Speaker;
use crate::domain::{
    FileStanding, GraphEdge, GraphNode, GraphView, KindCount, NodeDetail, NodeKind, WorkspaceName,
};

use super::machine_answer::{SCHEMA, object_line};
use super::search::one_line;

#[derive(Serialize)]
struct View<'a> {
    schema: u32,
    context: &'a str,
    /// The id of the node the view is centred on; the first of `nodes`.
    focus: &'a str,
    depth: u8,
    limit: usize,
    nodes: Vec<Node<'a>>,
    edges: Vec<Edge<'a>>,
    /// Whether anything the walk found is not in `nodes`.
    truncated: bool,
    counts: Counts,
    not_shown: NotShown,
    /// How many nodes were read from each kind of record, in order of first appearance.
    provenance_summary: Vec<Provenance<'a>>,
}

#[derive(Serialize)]
struct Node<'a> {
    id: &'a str,
    kind: &'static str,
    /// A conversation nobody named has an empty label; its session is in `conversation`.
    label: String,
    depth: u8,
    /// Whether the view can be centred here, with this `id` as `--focus`.
    focusable: bool,
    provenance: Provenance<'a>,
    conversation: Option<ConversationFacts<'a>>,
    reply: Option<ReplyFacts<'a>>,
    file: Option<FileFacts<'a>>,
    project: Option<RootOnly<'a>>,
}

#[derive(Serialize)]
struct ConversationFacts<'a> {
    session_id: &'a str,
    message_count: u64,
    /// Replies the view does not draw: those that touched nothing, and those the bound cut.
    replies_not_shown: u64,
    last_message_at_unix_ms: Option<i64>,
}

#[derive(Serialize)]
struct ReplyFacts<'a> {
    /// The node of the conversation this reply belongs to.
    conversation: &'a str,
    ordinal: u64,
    speaker: &'static str,
    at_unix_ms: Option<i64>,
}

#[derive(Serialize)]
struct FileFacts<'a> {
    /// The path the view knows the file by; for a file outside the context, the path as the
    /// touch wrote it.
    path: &'a str,
    /// `present`, `missing`, or `outside` — no location of this context holds the path, and
    /// the node cannot be focused.
    state: &'static str,
}

#[derive(Serialize)]
struct RootOnly<'a> {
    root: &'a str,
}

#[derive(Serialize)]
struct Provenance<'a> {
    record: &'static str,
    /// The key the record is found by; a count in `provenance_summary`.
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<usize>,
}

#[derive(Serialize)]
struct Edge<'a> {
    from: &'a str,
    to: &'a str,
    kind: &'static str,
    /// Present exactly when `kind` is `touched`.
    touch: Option<Touch>,
}

#[derive(Serialize)]
struct Touch {
    verbs: Vec<&'static str>,
    /// The owner named the file in their own words in that conversation.
    named: bool,
    /// Zero when the rows were written before the count existed.
    times: u32,
    /// The reply this edge is drawn from was found by time — the latest one said at or before
    /// the touch — and is not joined to it by any record.
    by_time: bool,
}

/// Shown and found, for every kind of node, whether or not the view holds one.
#[derive(Serialize)]
struct Counts {
    conversation: Count,
    reply: Count,
    file: Count,
    project: Count,
    topic: Count,
}

#[derive(Serialize)]
struct NotShown {
    /// Pairs read once and never named, left out by the strict answer (ADR 0041).
    in_passing: usize,
    /// Files only at paths this context's policy no longer admits. Counted, never named.
    by_policy: usize,
}

#[derive(Serialize)]
struct Count {
    shown: usize,
    found: usize,
}

/// The view as the line `graph --json` prints.
pub(super) fn view_line(context: &WorkspaceName, view: &GraphView) -> String {
    let count = |kind: NodeKind| {
        let count: KindCount = view
            .counts()
            .iter()
            .find(|(of, _)| *of == kind)
            .map(|(_, count)| *count)
            .unwrap_or_default();
        Count {
            shown: count.shown,
            found: count.found,
        }
    };
    object_line(&View {
        schema: SCHEMA,
        context: context.as_str(),
        focus: view.focus(),
        depth: view.depth(),
        limit: view.limit(),
        nodes: view.nodes().iter().map(|one| node(view, one)).collect(),
        edges: view.edges().iter().map(edge).collect(),
        truncated: view.truncated(),
        counts: Counts {
            conversation: count(NodeKind::Conversation),
            reply: count(NodeKind::Reply),
            file: count(NodeKind::File),
            project: count(NodeKind::Project),
            topic: count(NodeKind::Topic),
        },
        not_shown: NotShown {
            in_passing: view.in_passing(),
            by_policy: view.hidden_by_policy(),
        },
        provenance_summary: view
            .provenance_summary()
            .into_iter()
            .map(|(record, nodes)| Provenance {
                record,
                key: None,
                nodes: Some(nodes),
            })
            .collect(),
    })
}

fn node<'a>(view: &GraphView, node: &'a GraphNode) -> Node<'a> {
    let mut written = Node {
        id: node.id(),
        kind: node.kind().as_str(),
        // A label comes out of somebody's transcript or file name, and stays one line.
        label: one_line(node.label()),
        depth: node.depth(),
        focusable: node.is_focusable(),
        provenance: Provenance {
            record: node.provenance().record(),
            key: Some(node.provenance().key()),
            nodes: None,
        },
        conversation: None,
        reply: None,
        file: None,
        project: None,
    };
    match node.detail() {
        NodeDetail::Conversation {
            session_id,
            message_count,
            last_message_at_unix_ms,
        } => {
            written.conversation = Some(ConversationFacts {
                session_id,
                message_count: *message_count,
                replies_not_shown: view.replies_not_shown(node.id()),
                last_message_at_unix_ms: *last_message_at_unix_ms,
            });
        }
        NodeDetail::Reply {
            conversation,
            ordinal,
            speaker,
            at_unix_ms,
        } => {
            written.reply = Some(ReplyFacts {
                conversation,
                ordinal: *ordinal,
                speaker: speaker_label(*speaker),
                at_unix_ms: *at_unix_ms,
            });
        }
        NodeDetail::File { path, standing } => {
            written.file = Some(FileFacts {
                path,
                state: match standing {
                    FileStanding::Present => "present",
                    FileStanding::Missing => "missing",
                    FileStanding::Outside => "outside",
                },
            });
        }
        NodeDetail::Project { root } => written.project = Some(RootOnly { root }),
        NodeDetail::Topic => {}
    }
    written
}

fn edge(edge: &GraphEdge) -> Edge<'_> {
    Edge {
        from: edge.from(),
        to: edge.to(),
        kind: edge.kind().as_str(),
        touch: edge.touch().map(|mark| Touch {
            verbs: mark.verbs().iter().map(|verb| verb.as_str()).collect(),
            named: mark.named(),
            times: mark.times(),
            by_time: mark.by_time(),
        }),
    }
}

/// The words the graph stores for the two parties, as the schema spells them.
pub(super) fn speaker_label(speaker: Speaker) -> &'static str {
    match speaker {
        Speaker::Owner => "owner",
        Speaker::Model => "model",
    }
}
