// `crosskit [--context <name>] graph --focus <id> [--depth 1|2] [--limit N]
// [--include-in-passing] [--json]`.
//
// The neighbourhood of one conversation or one file: its replies that touched files, those
// files, the roots of the context they lie under, their topics — and, a step further, the other
// conversations of the same files. The text form is a tree a person can check without the
// panel; `--json` is the same view as one object of the machine schema.
use std::collections::HashSet;

use crate::application::GraphExplorer;
use crate::domain::{
    DEEPEST, EdgeKind, FileStanding, GraphEdge, GraphFocus, GraphNode, GraphQuery, GraphView,
    MOST_NODES, NodeDetail, NodeKind, WorkspaceName,
};
use crate::infrastructure::utc_stamp;

use super::help;
use super::machine_answer::{self, refusal_line};
use super::machine_graph::{speaker_label, view_line};
use super::reading_ground::{OwnRun, ReadingGround};
use super::refusal::Refusal;
use super::search::one_line;
use super::search_index::visible_paths;
use super::service_client;
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] graph --focus <conversation:<session>|file:<id>|path:<path>> [--depth 1|2] [--limit <N>] [--include-in-passing] [--json]";

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    if arguments.iter().any(|one| one == machine_answer::FLAG) {
        if let Some(line) = service_client::answered("graph", arguments, context) {
            println!("{line}");
            return Ok(());
        }
        let (line, outcome) = machine_line(arguments, context, &OwnRun::load());
        println!("{line}");
        return outcome.map_err(String::from);
    }
    let mut selected: Option<WorkspaceName> = None;
    let (selected, view) = answer(arguments, context, &mut selected, &OwnRun::load())?;
    print_tree(&selected, &view);
    Ok(())
}

/// The machine answer as one line, on whatever ground it is read from, and whether it was a
/// refusal.
pub(super) fn machine_line(
    arguments: &[String],
    context: Option<&str>,
    ground: &dyn ReadingGround,
) -> (String, Result<(), Refusal>) {
    let mut selected: Option<WorkspaceName> = None;
    match answer(arguments, context, &mut selected, ground) {
        Ok((selected, view)) => (view_line(&selected, &view), Ok(())),
        Err(refusal) => (refusal_line(&refusal, selected.as_ref()), Err(refusal)),
    }
}

fn answer(
    arguments: &[String],
    context: Option<&str>,
    selected: &mut Option<WorkspaceName>,
    ground: &dyn ReadingGround,
) -> Result<(WorkspaceName, GraphView), Refusal> {
    let query = parse(arguments).map_err(Refusal::usage)?;
    let loaded = ground.policy()?;
    let chosen = workspace::choose(loaded, context)?;
    *selected = Some(chosen.clone());
    let manager = ground.graph(&chosen)?;
    let repository = manager.repository();
    let roots: Vec<String> = loaded
        .workspaces()
        .roots_of(&chosen)
        .iter()
        .filter_map(|root| root.directory().map(str::to_string))
        .collect();
    let visible = visible_paths(loaded, &chosen);
    let view = GraphExplorer::new(&repository, &roots, &visible)
        .explore(&query)
        .map_err(Refusal::of_graph_view)?;
    Ok((chosen.clone(), view))
}

fn print_tree(context: &WorkspaceName, view: &GraphView) {
    println!("Context: {context}");
    println!("Depth {}, at most {} nodes", view.depth(), view.limit());
    let mut printed_edges: HashSet<usize> = HashSet::new();
    let mut visited: HashSet<&str> = HashSet::new();
    if let Some(focus) = view.node(view.focus()) {
        println!("{}", node_line(view, focus));
        visited.insert(focus.id());
        print_children(view, focus.id(), 1, &mut visited, &mut printed_edges);
    }
    println!();
    println!("{}", counts_line(view));
    if view.in_passing() > 0 {
        println!(
            "Left out, read and never named: {} (add --include-in-passing)",
            view.in_passing()
        );
    }
    if view.hidden_by_policy() > 0 {
        println!(
            "Files at paths this context's policy no longer admits, not shown: {}",
            view.hidden_by_policy()
        );
    }
}

/// The view as a tree from its focus: every node once, under the first node that reached it,
/// and every other edge as a line back to a node printed elsewhere. A project and a topic are
/// leaves, printed after the replies: walking on through a project would hang every file of
/// the view under it, which says where files lie and not who touched them.
fn print_children<'v>(
    view: &'v GraphView,
    parent: &str,
    level: usize,
    visited: &mut HashSet<&'v str>,
    printed_edges: &mut HashSet<usize>,
) {
    let mut edges: Vec<(usize, &GraphEdge)> = view.edges().iter().enumerate().collect();
    edges.sort_by_key(|(index, edge)| (is_attachment(edge), *index));
    for (index, edge) in edges {
        let other = match (edge.from() == parent, edge.to() == parent) {
            (true, _) => edge.to(),
            (_, true) => edge.from(),
            _ => continue,
        };
        let Some(node) = view.node(other) else {
            continue;
        };
        if !printed_edges.insert(index) {
            continue;
        }
        let indent = "  ".repeat(level);
        if !visited.insert(node.id()) {
            println!("{indent}{}also {}", edge_prefix(edge), short_label(node));
            continue;
        }
        println!("{indent}{}{}", edge_prefix(edge), node_line(view, node));
        if !is_attachment(edge) {
            print_children(view, node.id(), level + 1, visited, printed_edges);
        }
    }
}

fn is_attachment(edge: &GraphEdge) -> bool {
    matches!(edge.kind(), EdgeKind::InProject | EdgeKind::About)
}

fn edge_prefix(edge: &GraphEdge) -> String {
    let Some(mark) = edge.touch() else {
        return String::new();
    };
    let verbs: Vec<&str> = mark.verbs().iter().map(|verb| verb.as_str()).collect();
    let mut about = vec![verbs.join("+")];
    if mark.by_time() {
        about.push("reply placed by time".to_string());
    }
    if mark.times() > 1 {
        about.push(format!("{} times", mark.times()));
    }
    if mark.named() {
        about.push("you named this file".to_string());
    }
    format!("[{}] ", about.join(", "))
}

fn node_line(view: &GraphView, node: &GraphNode) -> String {
    match node.detail() {
        NodeDetail::Conversation {
            session_id,
            message_count,
            ..
        } => format!(
            "{} — session {session_id}, {message_count} replies, {} not drawn (focus: {})",
            short_label(node),
            view.replies_not_shown(node.id()),
            node.id()
        ),
        NodeDetail::Reply {
            speaker,
            at_unix_ms,
            ..
        } => format!(
            "reply {} by {}{}",
            node.label(),
            speaker_label(*speaker),
            at_unix_ms
                .map(|at| format!(", {}", utc_stamp(at)))
                .unwrap_or_default()
        ),
        NodeDetail::File {
            path,
            standing: FileStanding::Outside,
        } => format!("file [outside this context] {path}"),
        NodeDetail::File { path, standing } => format!(
            "file {} {path} (focus: {})",
            match standing {
                FileStanding::Missing => "[missing]",
                _ => "[present]",
            },
            node.id()
        ),
        NodeDetail::Project { root } => format!("project {root}"),
        NodeDetail::Topic => format!("topic: {}", one_line(node.label())),
    }
}

/// How a node is named where it is met a second time.
fn short_label(node: &GraphNode) -> String {
    match node.kind() {
        NodeKind::Conversation => match one_line(node.label()).as_str() {
            "" => "conversation (unnamed)".to_string(),
            name => format!("conversation «{name}»"),
        },
        NodeKind::Reply => format!("reply {}", node.label()),
        NodeKind::File => format!("file {}", node.label()),
        NodeKind::Project => format!("project {}", node.label()),
        NodeKind::Topic => format!("topic: {}", one_line(node.label())),
    }
}

/// "Nodes: 10 of 49, cut at the limit — conversations 2 of 2, …": every kind found, never
/// only the ones that fit.
fn counts_line(view: &GraphView) -> String {
    let kinds: Vec<String> = view
        .counts()
        .iter()
        .filter(|(_, count)| count.found > 0)
        .map(|(kind, count)| format!("{} {} of {}", plural(*kind), count.shown, count.found))
        .collect();
    format!(
        "Nodes: {} of {}{} — {}",
        view.shown(),
        view.found(),
        match view.truncated() {
            true => ", cut at the limit",
            false => "",
        },
        kinds.join(", ")
    )
}

fn plural(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Conversation => "conversations",
        NodeKind::Reply => "replies",
        NodeKind::File => "files",
        NodeKind::Project => "projects",
        NodeKind::Topic => "topics",
    }
}

/// The arguments as a query. The default bound is the whole of what a view may hold: the view
/// of the mockup, two steps and a hundred nodes.
fn parse(arguments: &[String]) -> Result<GraphQuery, String> {
    let mut focus: Option<GraphFocus> = None;
    let mut depth = DEEPEST;
    let mut limit = MOST_NODES;
    let mut include_in_passing = false;
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        let mut value = |name: &str| {
            remaining
                .next()
                .cloned()
                .ok_or_else(|| format!("{name} requires a value. {USAGE}"))
        };
        match argument.as_str() {
            "--focus" => {
                focus = Some(
                    GraphFocus::parse(&value("--focus")?)
                        .map_err(|error| format!("{error}. {USAGE}"))?,
                );
            }
            "--depth" => {
                depth = value("--depth")?
                    .parse()
                    .map_err(|_| format!("--depth takes 1 or {DEEPEST}. {USAGE}"))?;
            }
            "--limit" => {
                limit = value("--limit")?
                    .parse()
                    .map_err(|_| format!("--limit takes a number of nodes. {USAGE}"))?;
            }
            "--include-in-passing" => include_in_passing = true,
            flag if flag == machine_answer::FLAG => {}
            other => return Err(format!("unknown argument {other:?}. {USAGE}")),
        }
    }
    let Some(focus) = focus else {
        return Err(help::misuse(USAGE));
    };
    GraphQuery::new(focus, depth, limit, include_in_passing)
        .map_err(|error| format!("{error}. {USAGE}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Result<GraphQuery, String> {
        parse(
            &arguments
                .iter()
                .map(|argument| (*argument).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn the_default_view_is_two_steps_and_a_hundred_nodes() {
        let query = parsed(&["--focus", "conversation:s1"]).unwrap();
        assert_eq!(query.depth(), 2);
        assert_eq!(query.limit(), 100);
        assert_eq!(query.focus(), &GraphFocus::Conversation("s1".to_string()));
        assert!(!query.include_in_passing());
    }

    #[test]
    fn a_search_row_path_is_a_focus() {
        let query = parsed(&["--focus", "path:/work/a.rs", "--depth", "1", "--json"]).unwrap();
        assert_eq!(query.depth(), 1);
        assert!(matches!(query.focus(), GraphFocus::Path(_)));
    }

    /// The bounds are refused, not lowered: three steps shown as two would read as the end.
    #[test]
    fn a_view_beyond_its_bounds_is_refused() {
        assert!(parsed(&["--focus", "file:f", "--depth", "3"]).is_err());
        assert!(parsed(&["--focus", "file:f", "--depth", "0"]).is_err());
        assert!(parsed(&["--focus", "file:f", "--limit", "101"]).is_err());
        assert!(parsed(&["--focus", "file:f", "--limit", "0"]).is_err());
    }

    #[test]
    fn a_focus_is_needed_and_has_to_be_one_of_three_forms() {
        assert!(parsed(&[]).is_err());
        assert!(parsed(&["--focus"]).is_err());
        assert!(parsed(&["--focus", "project:/work"]).is_err());
        assert!(parsed(&["--focus", "path:relative/a.rs"]).is_err());
        assert!(parsed(&["--focus", "conversation:"]).is_err());
    }
}
