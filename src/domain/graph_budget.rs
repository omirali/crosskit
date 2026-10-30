// How a view fills up to its bound, and how it says what did not fit.
//
// The walk offers the view chains rather than nodes: a conversation, the reply that touched a
// file and the file, together. A chain is taken whole or not at all, so every node a view
// holds is joined to its focus by nodes it also holds — a file never floats beside the reply
// that was cut. The walk offers the nearest and the most worked on first; what is offered
// after the view is full is still counted, kind by kind, which is what makes "shown 10 of 49"
// a fact rather than a guess.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::graph_view::{
    EdgeKind, GraphEdge, GraphNode, KindCount, NodeDetail, NodeKind, Provenance,
};

/// A view being filled.
#[derive(Debug)]
pub struct ViewBudget {
    focus: String,
    depth: u8,
    limit: usize,
    /// How far the step being walked may fill the view: the limit, or less while a farther
    /// step still has to fit.
    ceiling: usize,
    nodes: Vec<GraphNode>,
    positions: HashMap<String, usize>,
    edges: Vec<GraphEdge>,
    edge_keys: HashSet<(String, String, EdgeKind)>,
    found: BTreeMap<NodeKind, BTreeSet<String>>,
    in_passing: usize,
    hidden_by_policy: usize,
}

impl ViewBudget {
    /// A view of at most `limit` nodes, holding its focus. The focus always fits: a limit is
    /// at least one, and a view without its focus is not a view of anything.
    pub fn new(focus: GraphNode, depth: u8, limit: usize) -> Self {
        let mut budget = Self {
            focus: focus.id().to_string(),
            depth,
            limit: limit.max(1),
            ceiling: limit.max(1),
            nodes: Vec::new(),
            positions: HashMap::new(),
            edges: Vec::new(),
            edge_keys: HashSet::new(),
            found: BTreeMap::new(),
            in_passing: 0,
            hidden_by_policy: 0,
        };
        budget.place(focus);
        budget
    }

    pub fn contains(&self, id: &str) -> bool {
        self.positions.contains_key(id)
    }

    /// Lets the step about to be walked fill the view up to its share of the limit
    /// (`share_of_step`), never below what it already holds.
    pub fn open_for_step(&mut self, step: u8) {
        self.ceiling = share_of_step(self.limit, self.depth, step).max(self.nodes.len());
    }

    /// Offers a chain of nodes and the edges between them. Taken whole when its new nodes fit,
    /// counted as found either way. Returns whether it was taken.
    pub fn offer(&mut self, chain: Vec<GraphNode>, edges: Vec<GraphEdge>) -> bool {
        let mut fresh: Vec<GraphNode> = Vec::new();
        for node in chain {
            self.found
                .entry(node.kind())
                .or_default()
                .insert(node.id().to_string());
            let seen = self.contains(node.id()) || fresh.iter().any(|one| one.id() == node.id());
            if !seen {
                fresh.push(node);
            }
        }
        if self.nodes.len() + fresh.len() > self.ceiling {
            return false;
        }
        for node in fresh {
            self.place(node);
        }
        for edge in edges {
            self.join(edge);
        }
        true
    }

    /// Pairs the strict answer left out: read once, never named (ADR 0041).
    pub fn left_in_passing(&mut self, pairs: usize) {
        self.in_passing += pairs;
    }

    /// Files this context's policy no longer admits. Counted, never named — the rule `search`
    /// keeps for the same files.
    pub fn hidden_by_policy(&mut self, files: usize) {
        self.hidden_by_policy += files;
    }

    pub fn finish(self) -> GraphView {
        let counts = NodeKind::ALL
            .iter()
            .map(|kind| {
                let shown = self
                    .nodes
                    .iter()
                    .filter(|node| node.kind() == *kind)
                    .count();
                let found = self.found.get(kind).map_or(0, BTreeSet::len).max(shown);
                (*kind, KindCount { shown, found })
            })
            .collect();
        GraphView {
            focus: self.focus,
            depth: self.depth,
            limit: self.limit,
            nodes: self.nodes,
            edges: self.edges,
            counts,
            in_passing: self.in_passing,
            hidden_by_policy: self.hidden_by_policy,
        }
    }

    fn place(&mut self, node: GraphNode) {
        self.found
            .entry(node.kind())
            .or_default()
            .insert(node.id().to_string());
        self.positions
            .insert(node.id().to_string(), self.nodes.len());
        self.nodes.push(node);
    }

    /// An edge is drawn only between nodes the view holds, and once.
    fn join(&mut self, edge: GraphEdge) {
        if !self.contains(edge.from()) || !self.contains(edge.to()) {
            return;
        }
        let key = (edge.from().to_string(), edge.to().to_string(), edge.kind());
        if self.edge_keys.insert(key) {
            self.edges.push(edge);
        }
    }
}

/// How many nodes a view may hold once `step` of `depth` is walked.
///
/// A step of a two-step view may fill two thirds and leaves the rest to the step after it. Without
/// the share a conversation that touched 138 files fills a hundred nodes with its own files,
/// and the other conversations of those files — the second step, and the reason to ask for it
/// — are never drawn: measured on the owner's tree, one conversation of six.
pub fn share_of_step(limit: usize, depth: u8, step: u8) -> usize {
    if step >= depth {
        return limit;
    }
    limit * (usize::from(step) + 1) / (usize::from(depth) + 1)
}

/// The neighbourhood of one node, bounded, with what the bound left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphView {
    focus: String,
    depth: u8,
    limit: usize,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    counts: Vec<(NodeKind, KindCount)>,
    in_passing: usize,
    hidden_by_policy: usize,
}

impl GraphView {
    /// The id of the node the view is centred on; always the first of `nodes`.
    pub fn focus(&self) -> &str {
        &self.focus
    }

    pub fn depth(&self) -> u8 {
        self.depth
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// In the order they were taken: the focus first, then nearest and most worked on.
    pub fn nodes(&self) -> &[GraphNode] {
        &self.nodes
    }

    pub fn edges(&self) -> &[GraphEdge] {
        &self.edges
    }

    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|node| node.id() == id)
    }

    /// Every kind, in the order of `NodeKind::ALL`, with how many were found and shown.
    pub fn counts(&self) -> &[(NodeKind, KindCount)] {
        &self.counts
    }

    /// Whether anything the walk found is not in the view.
    pub fn truncated(&self) -> bool {
        self.counts
            .iter()
            .any(|(_, count)| count.shown < count.found)
    }

    pub fn shown(&self) -> usize {
        self.nodes.len()
    }

    pub fn found(&self) -> usize {
        self.counts.iter().map(|(_, count)| count.found).sum()
    }

    pub fn in_passing(&self) -> usize {
        self.in_passing
    }

    pub fn hidden_by_policy(&self) -> usize {
        self.hidden_by_policy
    }

    /// How many replies of a conversation the view does not draw: every reply that touched
    /// nothing, and every one the bound cut. Zero for a node that is not a conversation.
    pub fn replies_not_shown(&self, conversation: &str) -> u64 {
        let Some(NodeDetail::Conversation { message_count, .. }) =
            self.node(conversation).map(GraphNode::detail)
        else {
            return 0;
        };
        let drawn = self
            .nodes
            .iter()
            .filter(|node| {
                matches!(node.detail(), NodeDetail::Reply { conversation: of, .. } if of == conversation)
            })
            .count() as u64;
        message_count.saturating_sub(drawn)
    }

    /// How many nodes came from each kind of record, in the order of first appearance.
    pub fn provenance_summary(&self) -> Vec<(&'static str, usize)> {
        let mut summary: Vec<(&'static str, usize)> = Vec::new();
        for record in self.nodes.iter().map(|node| node.provenance().record()) {
            match summary.iter_mut().find(|(seen, _)| *seen == record) {
                Some((_, count)) => *count += 1,
                None => summary.push((record, 1)),
            }
        }
        summary
    }
}

/// The provenance of a node read from the configuration rather than from the graph.
pub fn from_configuration(key: impl Into<String>) -> Provenance {
    Provenance::new("configuration", key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation::Speaker;
    use crate::domain::graph_view::{FileStanding, Provenance};

    fn conversation(id: &str, messages: u64) -> GraphNode {
        GraphNode::new(
            id,
            id,
            0,
            Provenance::new("Conversation", id),
            NodeDetail::Conversation {
                session_id: id.to_string(),
                message_count: messages,
                last_message_at_unix_ms: None,
            },
        )
    }

    fn reply(conversation: &str, ordinal: u64) -> GraphNode {
        GraphNode::new(
            format!("reply:{conversation}:{ordinal}"),
            format!("#{ordinal}"),
            1,
            Provenance::new("Message", "m"),
            NodeDetail::Reply {
                conversation: conversation.to_string(),
                ordinal,
                speaker: Speaker::Model,
                at_unix_ms: None,
            },
        )
    }

    fn file(id: &str) -> GraphNode {
        GraphNode::new(
            id,
            id,
            1,
            Provenance::new("File", id),
            NodeDetail::File {
                path: format!("/work/{id}"),
                standing: FileStanding::Present,
            },
        )
    }

    /// A chain of reply and file for each of `files` files, offered to a view of `limit`.
    fn filled(limit: usize, files: usize) -> GraphView {
        let mut budget = ViewBudget::new(conversation("c", 2_000), 2, limit);
        for at in 0..files {
            let reply = reply("c", at as u64);
            let file = file(&format!("f{at}"));
            let edges = vec![
                GraphEdge::new("c", reply.id(), EdgeKind::HasReply),
                GraphEdge::new(reply.id(), file.id(), EdgeKind::Touched),
            ];
            budget.offer(vec![reply, file], edges);
        }
        budget.finish()
    }

    #[test]
    fn a_view_never_holds_more_nodes_than_its_limit() {
        let view = filled(10, 40);
        assert!(view.shown() <= 10, "{}", view.shown());
        assert_eq!(view.nodes()[0].id(), "c", "фокус всегда первый");
    }

    /// The bound is said kind by kind: what was found, and what of it is shown.
    #[test]
    fn what_did_not_fit_is_counted_by_kind() {
        let view = filled(10, 40);
        assert!(view.truncated());
        let count = |kind| {
            view.counts()
                .iter()
                .find(|(of, _)| *of == kind)
                .map(|(_, count)| *count)
                .unwrap()
        };
        assert_eq!(count(NodeKind::File).found, 40);
        assert_eq!(count(NodeKind::Reply).found, 40);
        assert_eq!(count(NodeKind::File).shown, 4, "по паре узлов на цепочку");
        assert_eq!(
            count(NodeKind::Conversation),
            KindCount { shown: 1, found: 1 }
        );
        assert_eq!(view.found(), 81);
    }

    /// A chain is taken whole: no file is drawn without the reply that touched it.
    #[test]
    fn every_edge_joins_two_nodes_the_view_holds() {
        let view = filled(9, 40);
        for edge in view.edges() {
            assert!(view.node(edge.from()).is_some() && view.node(edge.to()).is_some());
        }
        let files = view
            .nodes()
            .iter()
            .filter(|node| node.kind() == NodeKind::File)
            .count();
        let touched = view
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::Touched)
            .count();
        assert_eq!(files, touched, "у каждого файла — своя реплика");
    }

    /// The nearer step leaves room for the farther one; the last step may fill the view.
    #[test]
    fn a_nearer_step_leaves_a_third_of_the_view_to_the_next() {
        assert_eq!(share_of_step(100, 2, 1), 66);
        assert_eq!(share_of_step(100, 2, 2), 100);
        assert_eq!(share_of_step(100, 1, 1), 100);

        let mut budget = ViewBudget::new(conversation("c", 2_000), 2, 10);
        budget.open_for_step(1);
        for at in 0..40 {
            let reply = reply("c", at);
            let file = file(&format!("f{at}"));
            budget.offer(vec![reply, file], vec![]);
        }
        let view = budget.finish();
        assert_eq!(
            view.shown(),
            5,
            "фокус и две цепочки: 6 из 10 — доля первого шага"
        );
    }

    #[test]
    fn a_view_that_holds_everything_is_not_truncated() {
        let view = filled(100, 3);
        assert!(!view.truncated());
        assert_eq!(view.shown(), view.found());
    }

    /// A conversation of 2 000 replies draws the replies that touched something and counts
    /// the rest; it does not draw 2 000 nodes.
    #[test]
    fn the_replies_a_view_does_not_draw_are_counted_on_their_conversation() {
        let view = filled(100, 3);
        assert_eq!(view.replies_not_shown("c"), 1_997);
        assert_eq!(view.replies_not_shown("f0"), 0);
    }

    #[test]
    fn a_node_met_twice_is_one_node_and_an_edge_drawn_twice_is_one_edge() {
        let mut budget = ViewBudget::new(conversation("c", 5), 1, 100);
        for _ in 0..2 {
            let reply = reply("c", 1);
            let edges = vec![GraphEdge::new("c", reply.id(), EdgeKind::HasReply)];
            budget.offer(vec![reply], edges);
        }
        let view = budget.finish();
        assert_eq!(view.shown(), 2);
        assert_eq!(view.edges().len(), 1);
        assert_eq!(
            view.provenance_summary(),
            vec![("Conversation", 1), ("Message", 1)]
        );
    }
}
