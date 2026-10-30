// The rules of the graph view, against a graph held in memory.
//
// One small context, written here: two conversations that share a file, a file only read in
// passing, a file the policy hides, a path no location of this context holds, and a session
// stored at two revisions. Every rule of `graph_explorer` is one of these cases.
use crate::domain::conversation::{Speaker, TouchVerb};
use crate::domain::{
    FileStanding, GraphFocus, GraphQuery, GraphView, LocationStatus, NodeDetail, NodeKind,
    NormalizedPath, OwnerNaming,
};

use super::file_touches::StoredTouch;
use super::graph_explorer::{GraphError, GraphExplorer};
use super::graph_ports::{GraphReader, StoredConversation, StoredFile, StoredReply, StoredTopic};
use super::repository::RepositoryError;

/// `(session, path, verb, naming, moment)`.
type Touch = (&'static str, &'static str, TouchVerb, OwnerNaming, i64);

struct Memory {
    conversations: Vec<StoredConversation>,
    touches: Vec<Touch>,
    files: Vec<StoredFile>,
    /// `(conversation row, ordinal, speaker, moment)`.
    replies: Vec<(&'static str, u64, Speaker, i64)>,
}

impl Memory {
    fn stored(&self, keep: impl Fn(&Touch) -> bool) -> Vec<StoredTouch> {
        self.touches
            .iter()
            .filter(|touch| keep(touch))
            .map(|(session, path, verb, naming, at)| {
                StoredTouch::new(*session, "", *path, *verb, *naming, 1, Some(*at))
            })
            .collect()
    }
}

impl GraphReader for Memory {
    fn conversations_of_sessions(
        &self,
        sessions: &[String],
    ) -> Result<Vec<StoredConversation>, RepositoryError> {
        Ok(self
            .conversations
            .iter()
            .filter(|row| sessions.contains(&row.session_id))
            .cloned()
            .collect())
    }

    fn touches_of_sessions(
        &self,
        sessions: &[String],
    ) -> Result<Vec<StoredTouch>, RepositoryError> {
        Ok(self.stored(|touch| sessions.iter().any(|session| session == touch.0)))
    }

    fn touches_at_paths(&self, paths: &[String]) -> Result<Vec<StoredTouch>, RepositoryError> {
        Ok(self.stored(|touch| paths.iter().any(|path| path == touch.1)))
    }

    fn files_at_paths(&self, paths: &[String]) -> Result<Vec<(String, String)>, RepositoryError> {
        Ok(self
            .files
            .iter()
            .flat_map(|file| {
                file.locations
                    .iter()
                    .filter(|(at, _)| paths.iter().any(|path| path == at.as_str()))
                    .map(|(at, _)| (at.as_str().to_string(), file.file_id.clone()))
            })
            .collect())
    }

    fn files(&self, file_ids: &[String]) -> Result<Vec<StoredFile>, RepositoryError> {
        Ok(self
            .files
            .iter()
            .filter(|file| file_ids.contains(&file.file_id))
            .cloned()
            .collect())
    }

    fn replies_before(
        &self,
        moments: &[(String, i64)],
    ) -> Result<Vec<StoredReply>, RepositoryError> {
        Ok(moments
            .iter()
            .filter_map(|(conversation, at)| {
                let (_, ordinal, speaker, said) = self
                    .replies
                    .iter()
                    .filter(|reply| reply.0 == conversation && reply.3 <= *at)
                    .max_by_key(|reply| reply.1)?;
                Some(StoredReply {
                    conversation_id: conversation.clone(),
                    touched_at_unix_ms: *at,
                    message_id: format!("{conversation}:m{ordinal}"),
                    ordinal: *ordinal,
                    speaker: *speaker,
                    at_unix_ms: *said,
                })
            })
            .collect())
    }
}

const SETTLEMENT: &str = "/work/ledger/settlement.rs";
const TOTALS: &str = "/work/ledger/totals.rs";
const HIDDEN: &str = "/work/ledger/keys.confidential";
const ELSEWHERE: &str = "/other/registry.rs";

fn file(id: &str, path: &str, topic: Option<&str>) -> StoredFile {
    StoredFile {
        file_id: id.to_string(),
        locations: vec![(
            NormalizedPath::from_absolute(path).unwrap(),
            LocationStatus::Present,
        )],
        topic: topic.map(|label| StoredTopic {
            result_id: format!("result:{id}"),
            label: label.to_string(),
        }),
    }
}

fn conversation(
    id: &str,
    session: &str,
    messages: u64,
    transcript: &str,
    current: bool,
) -> StoredConversation {
    StoredConversation {
        id: id.to_string(),
        session_id: session.to_string(),
        name: format!("беседа {session}"),
        message_count: messages,
        last_message_at_unix_ms: Some(1_000),
        transcript_file_id: transcript.to_string(),
        current,
        topic: None,
    }
}

fn memory() -> Memory {
    Memory {
        conversations: vec![
            conversation("c1", "s1", 2_000, "t1", true),
            // The same session read at an earlier revision: never the row that is drawn.
            conversation("c1-old", "s1", 10, "t1", false),
            conversation("c2", "s2", 4, "t2", true),
        ],
        touches: vec![
            (
                "s1",
                SETTLEMENT,
                TouchVerb::Edited,
                OwnerNaming::NotNamed,
                100,
            ),
            ("s1", TOTALS, TouchVerb::Read, OwnerNaming::NotNamed, 200),
            (
                "s1",
                ELSEWHERE,
                TouchVerb::Edited,
                OwnerNaming::NotNamed,
                300,
            ),
            ("s1", HIDDEN, TouchVerb::Edited, OwnerNaming::NotNamed, 400),
            (
                "s2",
                SETTLEMENT,
                TouchVerb::Wrote,
                OwnerNaming::NotNamed,
                50,
            ),
        ],
        files: vec![
            file("f1", SETTLEMENT, Some("расчёты")),
            file("f2", TOTALS, None),
            file("f3", HIDDEN, None),
            file("t1", "/work/chats/a.jsonl", None),
            file("t2", "/work/chats/b.jsonl", None),
        ],
        replies: vec![
            ("c1", 0, Speaker::Owner, 10),
            ("c1", 5, Speaker::Model, 90),
            ("c1", 7, Speaker::Model, 250),
            ("c2", 0, Speaker::Owner, 40),
        ],
    }
}

fn admitted(path: &NormalizedPath) -> bool {
    !path.as_str().ends_with(".confidential")
}

fn explored(memory: &Memory, focus: &str, depth: u8, limit: usize, widen: bool) -> GraphView {
    try_explore(memory, focus, depth, limit, widen).expect("вид")
}

fn try_explore(
    memory: &Memory,
    focus: &str,
    depth: u8,
    limit: usize,
    widen: bool,
) -> Result<GraphView, GraphError> {
    explore_under(&["/work"], memory, focus, depth, limit, widen)
}

fn explore_under(
    roots: &[&str],
    memory: &Memory,
    focus: &str,
    depth: u8,
    limit: usize,
    widen: bool,
) -> Result<GraphView, GraphError> {
    let roots: Vec<String> = roots.iter().map(|root| (*root).to_string()).collect();
    let query = GraphQuery::new(GraphFocus::parse(focus).unwrap(), depth, limit, widen).unwrap();
    GraphExplorer::new(memory, &roots, &admitted).explore(&query)
}

/// The ids of one kind, sorted: the order nodes are taken in is the bound's business, and
/// these tests are about which nodes there are.
fn ids(view: &GraphView, kind: NodeKind) -> Vec<&str> {
    let mut ids: Vec<&str> = view
        .nodes()
        .iter()
        .filter(|node| node.kind() == kind)
        .map(|node| node.id())
        .collect();
    ids.sort_unstable();
    ids
}

fn touched(view: &GraphView, from: &str, to: &str) -> bool {
    view.edges()
        .iter()
        .any(|edge| edge.from() == from && edge.to() == to && edge.touch().is_some())
}

/// One step from a conversation: the reply each touch followed, the file inside the context,
/// the path outside it, the project and the topic — and nothing that was only read in passing.
#[test]
fn one_step_from_a_conversation_is_its_replies_files_and_project() {
    let view = explored(&memory(), "conversation:s1", 1, 100, false);
    assert_eq!(view.focus(), "conversation:s1");
    assert_eq!(
        ids(&view, NodeKind::File),
        vec!["file:f1", "outside:/other/registry.rs"]
    );
    assert_eq!(
        ids(&view, NodeKind::Reply),
        vec!["reply:s1:5", "reply:s1:7"]
    );
    assert!(
        touched(&view, "reply:s1:5", "file:f1"),
        "{:?}",
        view.edges()
    );
    assert_eq!(ids(&view, NodeKind::Project), vec!["project:/work"]);
    assert_eq!(ids(&view, NodeKind::Topic), vec!["topic:расчёты"]);
    assert!(ids(&view, NodeKind::Conversation) == vec!["conversation:s1"]);
}

/// A touch's path is a file of this context only when a location holds it; otherwise it is a
/// node of its own that shows the path and leads nowhere.
#[test]
fn a_path_no_location_holds_is_outside_the_context_and_not_focusable() {
    let view = explored(&memory(), "conversation:s1", 2, 100, false);
    let outside = view
        .node(&format!("outside:{ELSEWHERE}"))
        .expect("узел вне контекста");
    assert!(!outside.is_focusable());
    assert_eq!(
        outside.kind(),
        NodeKind::File,
        "вне контекста — состояние файла, не вид"
    );
    assert!(matches!(
        outside.detail(),
        NodeDetail::File { path, standing: FileStanding::Outside } if path == ELSEWHERE
    ));
    assert!(
        !view
            .edges()
            .iter()
            .any(|edge| edge.from() == outside.id() && edge.to().starts_with("project:")),
        "вне контекста проекта нет"
    );
}

/// The strict default of ADR 0041: read once and never named is counted, not drawn.
#[test]
fn a_file_only_read_in_passing_is_counted_and_widening_draws_it() {
    let strict = explored(&memory(), "conversation:s1", 1, 100, false);
    assert!(strict.node("file:f2").is_none());
    assert_eq!(strict.in_passing(), 1);

    let widened = explored(&memory(), "conversation:s1", 1, 100, true);
    assert!(widened.node("file:f2").is_some());
    assert_eq!(widened.in_passing(), 0);
}

#[test]
fn a_file_the_policy_hides_is_counted_and_never_named() {
    let view = explored(&memory(), "conversation:s1", 2, 100, false);
    assert_eq!(view.hidden_by_policy(), 1);
    assert!(view.node("file:f3").is_none());
    assert!(
        view.nodes()
            .iter()
            .all(|node| !node.id().contains("confidential"))
    );
}

/// Two steps from a conversation reach the other conversations of the files it touched.
#[test]
fn two_steps_from_a_conversation_reach_another_conversation_of_the_same_file() {
    let one = explored(&memory(), "conversation:s1", 1, 100, false);
    assert!(one.node("conversation:s2").is_none());

    let two = explored(&memory(), "conversation:s1", 2, 100, false);
    assert!(two.node("conversation:s2").is_some());
    assert!(touched(&two, "reply:s2:0", "file:f1"));
}

/// A session stored at two revisions is one conversation, read from its current row.
#[test]
fn a_conversation_is_drawn_from_its_current_revision() {
    let view = explored(&memory(), "conversation:s1", 1, 100, false);
    let focus = view.node("conversation:s1").unwrap();
    assert_eq!(focus.provenance().record(), "Conversation");
    assert_eq!(focus.provenance().key(), "c1");
    assert_eq!(view.replies_not_shown("conversation:s1"), 1_998);
}

/// A search row's path is a focus, and a file's step reaches the conversations of it.
#[test]
fn a_path_focus_is_the_file_and_one_step_is_its_conversations() {
    let view = explored(&memory(), &format!("path:{SETTLEMENT}"), 1, 100, false);
    assert_eq!(view.focus(), "file:f1");
    assert_eq!(
        ids(&view, NodeKind::Conversation),
        vec!["conversation:s1", "conversation:s2"]
    );
    assert!(view.node("file:f2").is_none(), "второй шаг не сделан");
}

#[test]
fn no_topic_node_appears_where_nothing_was_concluded() {
    let view = explored(&memory(), "file:f2", 1, 100, true);
    assert!(ids(&view, NodeKind::Topic).is_empty(), "{:?}", view.nodes());
}

/// The bound holds, and what it cut is said by kind.
#[test]
fn a_view_cut_at_its_limit_says_what_it_left_out() {
    let view = explored(&memory(), "conversation:s1", 2, 3, true);
    assert!(view.shown() <= 3);
    assert!(view.truncated());
    assert!(view.found() > view.shown());
    for edge in view.edges() {
        assert!(view.node(edge.from()).is_some() && view.node(edge.to()).is_some());
    }
}

#[test]
fn a_focus_the_context_does_not_hold_or_admit_is_not_found() {
    for focus in ["conversation:nobody", "file:f9", &format!("path:{HIDDEN}")] {
        assert!(
            matches!(
                try_explore(&memory(), focus, 1, 100, false),
                Err(GraphError::FocusNotFound)
            ),
            "{focus}"
        );
    }
}

/// A reply found by time is said to be found by time, on the edge it is drawn from; a touch
/// drawn from the conversation itself has no reply to be unsure about.
#[test]
fn an_edge_from_a_reply_placed_by_time_says_so() {
    let view = explored(&memory(), "conversation:s1", 1, 100, false);
    let from_reply = view
        .edges()
        .iter()
        .find(|edge| edge.from() == "reply:s1:5" && edge.to() == "file:f1")
        .and_then(|edge| edge.touch())
        .expect("ребро от реплики");
    assert!(from_reply.by_time());
}

const PROJECT: &str = "/home/a/payments-api";
const CHATS: &str = "/home/a/.claude/projects/-home-a-payments-api";

/// A project, its folder of conversations, and two conversations that name one file — one by
/// the whole path, the other relative to where the session ran.
fn a_project_with_its_conversations() -> Memory {
    let contract = format!("{PROJECT}/src/contract.rs");
    Memory {
        conversations: vec![
            conversation("c1", "s1", 10, "t1", true),
            conversation("c2", "s2", 10, "t2", true),
        ],
        touches: vec![
            (
                "s1",
                "src/contract.rs",
                TouchVerb::Edited,
                OwnerNaming::Named,
                100,
            ),
            (
                "s1",
                "/home/a/payments-api/src/contract.rs",
                TouchVerb::Wrote,
                OwnerNaming::NotNamed,
                200,
            ),
            (
                "s2",
                "/home/a/payments-api/src/contract.rs",
                TouchVerb::Wrote,
                OwnerNaming::NotNamed,
                100,
            ),
        ],
        files: vec![
            file("f1", &contract, None),
            file("t1", &format!("{CHATS}/a.jsonl"), None),
            file("t2", &format!("{CHATS}/b.jsonl"), None),
        ],
        replies: vec![],
    }
}

#[test]
fn a_relative_path_is_read_against_the_conversations_project_and_is_one_file() {
    let memory = a_project_with_its_conversations();
    let view = explore_under(&[PROJECT, CHATS], &memory, "conversation:s1", 2, 100, false).unwrap();
    assert_eq!(
        ids(&view, NodeKind::File),
        vec!["file:f1"],
        "{:?}",
        view.nodes()
    );
    assert!(
        view.node("conversation:s2").is_some(),
        "второй шаг через тот же файл"
    );
}

/// The folder Claude Code keeps a project's conversations in is that project, not a second one
/// named by its encoded folder.
#[test]
fn a_folder_of_conversations_is_part_of_its_project() {
    let memory = a_project_with_its_conversations();
    let view = explore_under(&[PROJECT, CHATS], &memory, "conversation:s1", 2, 100, false).unwrap();
    assert_eq!(
        ids(&view, NodeKind::Project),
        vec!["project:/home/a/payments-api"]
    );
    assert_eq!(
        view.node("project:/home/a/payments-api").unwrap().label(),
        "payments-api"
    );
}

/// One conversation that wrote a file by its whole path and edited it by a relative one touched
/// one file, and the edge carries both: the mark of either spelling is not lost to the other.
#[test]
fn two_spellings_of_one_path_are_folded_into_one_edge() {
    let memory = a_project_with_its_conversations();
    let view = explore_under(&[PROJECT, CHATS], &memory, "conversation:s1", 1, 100, false).unwrap();
    let touched: Vec<_> = view
        .edges()
        .iter()
        .filter(|edge| edge.from() == "conversation:s1" && edge.to() == "file:f1")
        .filter_map(|edge| edge.touch())
        .collect();
    assert_eq!(touched.len(), 1, "{touched:?}");
    assert_eq!(touched[0].verbs(), &[TouchVerb::Wrote, TouchVerb::Edited]);
    assert!(touched[0].named());
    assert_eq!(touched[0].times(), 2);
}
