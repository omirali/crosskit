// Use case: the neighbourhood of one conversation or one file, as a bounded view.
//
// The walk crosses between conversations and files, a step at a time, and every crossing is
// a pair "conversation + file" folded exactly as `touches` folds it (ADR 0041): the same
// session stored at several revisions is one conversation, and the strict answer draws only
// what was written, edited or named, counting the rest.
//
// Three rules are the view's own and are kept here:
//
//   * **a touch's path becomes a file inside this context only.** A location of this graph at
//     that path is the file; no location is a node "outside" that shows the path and leads
//     nowhere. Another context is another database, and nothing here reaches it (ADR 0008);
//   * **a reply is drawn only when it touched a file.** The reply is the latest one said at or
//     before the pair's first touch. The graph stores no edge between a reply and a touch, so
//     this is a reading by time, and a touch with no reply before it is drawn from the
//     conversation itself;
//   * **the policy hides what it hides everywhere.** A file with no path this context admits is
//     counted and never named — the rule `search` keeps.
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::domain::{
    EdgeKind, FileStanding, GraphEdge, GraphFocus, GraphNode, GraphQuery, GraphView,
    LocationStatus, NodeDetail, NormalizedPath, OwnerNaming, Provenance, TouchMark, ViewBudget,
    from_configuration, project_of_folder,
};

use super::file_touches::{TouchedPair, fold};
use super::graph_ports::{GraphReader, StoredConversation, StoredFile, StoredReply, StoredTopic};
use super::repository::RepositoryError;

#[derive(Debug)]
pub enum GraphError {
    /// The focus names nothing this context's graph holds, or nothing its policy admits.
    FocusNotFound,
    Graph(RepositoryError),
}

impl From<RepositoryError> for GraphError {
    fn from(error: RepositoryError) -> Self {
        Self::Graph(error)
    }
}

impl fmt::Display for GraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FocusNotFound => formatter.write_str(
                "nothing in this context answers to that focus; take it from a search row or from a node of a view",
            ),
            Self::Graph(error) => write!(formatter, "{error}"),
        }
    }
}

pub struct GraphExplorer<'a, Reader> {
    reader: &'a Reader,
    /// The directories of this context's roots: what a "project" is (section 15 has no
    /// project node; a root of the context is the nearest real thing).
    roots: &'a [String],
    visible: &'a dyn Fn(&NormalizedPath) -> bool,
}

impl<'a, Reader> GraphExplorer<'a, Reader>
where
    Reader: GraphReader,
{
    pub fn new(
        reader: &'a Reader,
        roots: &'a [String],
        visible: &'a dyn Fn(&NormalizedPath) -> bool,
    ) -> Self {
        Self {
            reader,
            roots,
            visible,
        }
    }

    pub fn explore(&self, query: &GraphQuery) -> Result<GraphView, GraphError> {
        Walk {
            explorer: self,
            query,
            conversations: HashMap::new(),
            files: HashMap::new(),
            at_path: HashMap::new(),
            drawn: HashSet::new(),
            in_passing: HashSet::new(),
            hidden: HashSet::new(),
            deferred: Vec::new(),
        }
        .run()
    }
}

/// Where the next step starts.
enum Frontier {
    Conversations(Vec<String>),
    Files(Vec<String>),
}

/// A chain a nearer step could not fit under its share, kept for the room the last step
/// leaves, with the node it would place.
struct Deferred {
    chain: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    places: Frontier,
    depth: u8,
}

/// What a touch's path turned out to be in this context.
enum Target {
    File(String),
    Hidden(String),
    Outside(String),
}

impl Target {
    /// The node this target is drawn as — or would be, for a file the policy hides.
    fn node_id(&self) -> String {
        match self {
            Self::File(file_id) | Self::Hidden(file_id) => file_node_id(file_id),
            Self::Outside(path) => outside_node_id(path),
        }
    }
}

struct Walk<'w, 'a, Reader> {
    explorer: &'w GraphExplorer<'a, Reader>,
    query: &'w GraphQuery,
    /// The row each session is drawn from, by session.
    conversations: HashMap<String, StoredConversation>,
    files: HashMap<String, StoredFile>,
    /// The file of this context at a path, or none: read once per path.
    at_path: HashMap<String, Option<String>>,
    /// Pairs already offered, as `(session, node of the file)`.
    drawn: HashSet<(String, String)>,
    in_passing: HashSet<(String, String)>,
    hidden: HashSet<String>,
    deferred: Vec<Deferred>,
}

impl<Reader> Walk<'_, '_, Reader>
where
    Reader: GraphReader,
{
    fn run(mut self) -> Result<GraphView, GraphError> {
        let (focus, mut frontier) = self.focus()?;
        let mut budget = ViewBudget::new(focus, self.query.depth(), self.query.limit());
        match &frontier {
            Frontier::Conversations(sessions) => {
                self.attach_conversations(&mut budget, sessions, 0)
            }
            Frontier::Files(files) => self.attach_files(&mut budget, files, 0),
        }
        for depth in 1..=self.query.depth() {
            budget.open_for_step(depth);
            frontier = match frontier {
                Frontier::Conversations(sessions) => {
                    Frontier::Files(self.step_to_files(&mut budget, &sessions, depth)?)
                }
                Frontier::Files(files) => Frontier::Conversations(self.step_to_conversations(
                    &mut budget,
                    &files,
                    depth,
                )?),
            };
        }
        self.fill_with_deferred(&mut budget);
        budget.left_in_passing(self.in_passing.len());
        budget.hidden_by_policy(self.hidden.len());
        Ok(budget.finish())
    }

    /// What a nearer step left out under its share, offered again in the room the last step
    /// left, in the order it was first offered. A node placed here is not walked from: the
    /// steps are over, and what it would have reached is not counted as found.
    fn fill_with_deferred(&mut self, budget: &mut ViewBudget) {
        budget.open_for_step(self.query.depth());
        for deferred in std::mem::take(&mut self.deferred) {
            let fresh: Vec<String> = match &deferred.places {
                Frontier::Files(files) => files
                    .iter()
                    .filter(|file_id| !budget.contains(&file_node_id(file_id)))
                    .cloned()
                    .collect(),
                Frontier::Conversations(sessions) => sessions
                    .iter()
                    .filter(|session| !budget.contains(&conversation_node_id(session)))
                    .cloned()
                    .collect(),
            };
            if !budget.offer(deferred.chain, deferred.edges) {
                continue;
            }
            match deferred.places {
                Frontier::Files(_) => self.attach_files(budget, &fresh, deferred.depth),
                Frontier::Conversations(_) => {
                    self.attach_conversations(budget, &fresh, deferred.depth)
                }
            }
        }
    }

    fn reader(&self) -> &Reader {
        self.explorer.reader
    }

    fn focus(&mut self) -> Result<(GraphNode, Frontier), GraphError> {
        match self.query.focus() {
            GraphFocus::Conversation(session) => {
                self.load_conversations(std::slice::from_ref(session))?;
                let node = self
                    .conversations
                    .get(session)
                    .filter(|conversation| self.transcript_is_visible(conversation))
                    .map(|conversation| conversation_node(conversation, 0))
                    .ok_or(GraphError::FocusNotFound)?;
                Ok((node, Frontier::Conversations(vec![session.clone()])))
            }
            GraphFocus::File(file_id) => {
                self.load_files(std::slice::from_ref(file_id))?;
                self.file_focus(file_id)
            }
            GraphFocus::Path(path) => {
                let file_id = self
                    .file_at(path.as_str())?
                    .ok_or(GraphError::FocusNotFound)?;
                self.file_focus(&file_id)
            }
        }
    }

    fn file_focus(&self, file_id: &str) -> Result<(GraphNode, Frontier), GraphError> {
        let node = self
            .files
            .get(file_id)
            .and_then(|file| self.file_node(file, 0))
            .ok_or(GraphError::FocusNotFound)?;
        Ok((node, Frontier::Files(vec![file_id.to_string()])))
    }

    /// From conversations to the files they touched. Returns the files this step placed.
    fn step_to_files(
        &mut self,
        budget: &mut ViewBudget,
        sessions: &[String],
        depth: u8,
    ) -> Result<Vec<String>, GraphError> {
        let pairs = fold(self.reader().touches_of_sessions(sessions)?);
        let paths: Vec<String> = pairs.iter().map(|pair| self.absolute_path(pair)).collect();
        self.resolve_paths(&paths)?;

        // Two spellings of one path are one file: their pairs are folded before anything is
        // decided about them, so a mark carried by either is not lost to the other.
        let mut targeted: Vec<(TouchedPair, Target)> = Vec::new();
        for pair in pairs {
            // A path found nowhere is shown as the tool wrote it, not as it was guessed.
            let target = match self.target_of(&self.absolute_path(&pair)) {
                Target::Outside(_) => Target::Outside(pair.path().to_string()),
                found => found,
            };
            let node = target.node_id();
            match targeted.iter_mut().find(|(held, target)| {
                held.session_id() == pair.session_id() && target.node_id() == node
            }) {
                Some((held, _)) => held.absorb(pair),
                None => targeted.push((pair, target)),
            }
        }
        let mut offered: Vec<(TouchedPair, Target)> = Vec::new();
        for (pair, target) in targeted {
            if let Some(key) = self.undrawn(&pair, &target) {
                self.keep_or_count(pair, target, key, &mut offered);
            }
        }
        offered.sort_by_key(|(pair, _)| {
            (
                Reverse(pair.naming() == OwnerNaming::Named),
                Reverse(pair.times()),
                pair.path().to_string(),
            )
        });
        let replies = self.replies_of(&offered)?;

        let mut placed = Vec::new();
        let mut rejected = Vec::new();
        for (pair, target) in offered {
            let Some(conversation) = self.conversations.get(pair.session_id()) else {
                continue;
            };
            let touched = match &target {
                Target::File(file_id) => match self.files.get(file_id) {
                    Some(file) => self.file_node(file, depth),
                    None => None,
                },
                Target::Outside(path) => Some(outside_node(path, depth)),
                Target::Hidden(_) => None,
            };
            let Some(touched) = touched else { continue };
            let fresh = !budget.contains(touched.id());
            let reply = replies.get(&moment_key(conversation, &pair));
            let (chain, edges) = chain(
                conversation_node(conversation, depth - 1),
                reply.map(|reply| reply_node(conversation, reply, depth)),
                touched,
                &pair,
            );
            let places = match target {
                Target::File(file_id) => vec![file_id],
                _ => Vec::new(),
            };
            match budget.offer(chain.clone(), edges.clone()) {
                true if fresh => placed.extend(places),
                true => {}
                false => rejected.push(Deferred {
                    chain,
                    edges,
                    places: Frontier::Files(places),
                    depth,
                }),
            }
        }
        self.deferred.extend(rejected);
        self.attach_files(budget, &placed, depth);
        Ok(placed)
    }

    /// From files to the conversations that touched them. Returns the sessions it placed.
    fn step_to_conversations(
        &mut self,
        budget: &mut ViewBudget,
        file_ids: &[String],
        depth: u8,
    ) -> Result<Vec<String>, GraphError> {
        // Every path the file was ever seen at that this context admits: a touch names the
        // path of its day, and a file that moved since is still the file it touched.
        let mut paths: Vec<String> = Vec::new();
        for file_id in file_ids {
            let Some(file) = self.files.get(file_id) else {
                continue;
            };
            for (path, _) in &file.locations {
                if (self.explorer.visible)(path) {
                    self.at_path
                        .insert(path.as_str().to_string(), Some(file_id.clone()));
                    paths.push(path.as_str().to_string());
                }
            }
        }
        let pairs = fold(self.reader().touches_at_paths(&paths)?);

        let mut offered: Vec<(TouchedPair, Target)> = Vec::new();
        for pair in pairs {
            let target = self.target_of(pair.path());
            if let Some(key) = self.undrawn(&pair, &target) {
                self.keep_or_count(pair, target, key, &mut offered);
            }
        }
        let sessions: Vec<String> = offered
            .iter()
            .map(|(pair, _)| pair.session_id().to_string())
            .filter(|session| !self.conversations.contains_key(session))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        self.load_conversations(&sessions)?;
        offered.retain(
            |(pair, _)| match self.conversations.get(pair.session_id()) {
                Some(conversation) => self.transcript_is_visible(conversation),
                None => false,
            },
        );
        offered.sort_by_key(|(pair, _)| {
            let last = self
                .conversations
                .get(pair.session_id())
                .and_then(|conversation| conversation.last_message_at_unix_ms);
            (
                Reverse(pair.naming() == OwnerNaming::Named),
                Reverse(pair.times()),
                Reverse(last),
                pair.session_id().to_string(),
            )
        });
        let replies = self.replies_of(&offered)?;

        let mut placed = Vec::new();
        let mut rejected = Vec::new();
        for (pair, target) in offered {
            let (Some(conversation), Target::File(file_id)) =
                (self.conversations.get(pair.session_id()), &target)
            else {
                continue;
            };
            let Some(file) = self
                .files
                .get(file_id)
                .and_then(|file| self.file_node(file, depth - 1))
            else {
                continue;
            };
            let node = conversation_node(conversation, depth);
            let fresh = !budget.contains(node.id());
            let reply = replies.get(&moment_key(conversation, &pair));
            let (chain, edges) = chain(
                node,
                reply.map(|reply| reply_node(conversation, reply, depth)),
                file,
                &pair,
            );
            let places = vec![pair.session_id().to_string()];
            match budget.offer(chain.clone(), edges.clone()) {
                true if fresh => placed.extend(places),
                true => {}
                false => rejected.push(Deferred {
                    chain,
                    edges,
                    places: Frontier::Conversations(places),
                    depth,
                }),
            }
        }
        self.deferred.extend(rejected);
        self.attach_conversations(budget, &placed, depth);
        Ok(placed)
    }

    /// The key of a pair not drawn yet, or nothing when an earlier step drew it.
    fn undrawn(&self, pair: &TouchedPair, target: &Target) -> Option<(String, String)> {
        let key = (pair.session_id().to_string(), target.node_id());
        (!self.drawn.contains(&key)).then_some(key)
    }

    /// The strict answer of ADR 0041: a pair read and never named is counted, not drawn. A
    /// hidden file is counted by the policy and not by the pair.
    fn keep_or_count(
        &mut self,
        pair: TouchedPair,
        target: Target,
        key: (String, String),
        offered: &mut Vec<(TouchedPair, Target)>,
    ) {
        if let Target::Hidden(file_id) = &target {
            self.hidden.insert(file_id.clone());
            return;
        }
        if !pair.is_deliberate() && !self.query.include_in_passing() {
            self.in_passing.insert(key);
            return;
        }
        self.drawn.insert(key);
        offered.push((pair, target));
    }

    /// The path a touch names, as this context would hold it. A tool may be handed a path
    /// relative to the directory the session ran in, and the transcript does not keep that
    /// directory; the conversation's project root is the nearest one known, so a relative path
    /// is read against it. Measured on the owner's tree: one file drawn twice, once found and
    /// once "outside", because one tool wrote `docs/…` and another the whole path.
    fn absolute_path(&self, pair: &TouchedPair) -> String {
        let written = pair.path();
        if written.starts_with('/') {
            return written.to_string();
        }
        let Some(root) = self
            .conversations
            .get(pair.session_id())
            .and_then(|conversation| self.project_root_of(conversation))
        else {
            return written.to_string();
        };
        NormalizedPath::from_absolute(&format!("{root}/{written}"))
            .map(|path| path.as_str().to_string())
            .unwrap_or_else(|_| written.to_string())
    }

    fn project_root_of(&self, conversation: &StoredConversation) -> Option<String> {
        let transcript = self.files.get(&conversation.transcript_file_id)?;
        let path = self.shown_path(transcript)?;
        project_of(self.explorer.roots, path.as_str()).map(str::to_string)
    }

    fn target_of(&self, path: &str) -> Target {
        let Ok(normalized) = NormalizedPath::from_absolute(path) else {
            return Target::Outside(path.to_string());
        };
        match self.at_path.get(normalized.as_str()).cloned().flatten() {
            None => Target::Outside(path.to_string()),
            Some(file_id) => match self
                .files
                .get(&file_id)
                .and_then(|file| self.shown_path(file))
            {
                Some(_) => Target::File(file_id),
                None => Target::Hidden(file_id),
            },
        }
    }

    /// Reads which file of this context is at each path not read yet.
    fn resolve_paths(&mut self, paths: &[String]) -> Result<(), GraphError> {
        let unread: Vec<String> = paths
            .iter()
            .filter_map(|path| NormalizedPath::from_absolute(path).ok())
            .map(|path| path.as_str().to_string())
            .filter(|path| !self.at_path.contains_key(path))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if unread.is_empty() {
            return Ok(());
        }
        let found = self.reader().files_at_paths(&unread)?;
        let file_ids: Vec<String> = found.iter().map(|(_, file)| file.clone()).collect();
        self.load_files(&file_ids)?;
        for path in unread {
            let candidates: Vec<&String> = found
                .iter()
                .filter(|(at, _)| *at == path)
                .map(|(_, file)| file)
                .collect();
            let chosen = self.choose_file_at(&path, &candidates);
            self.at_path.insert(path, chosen);
        }
        Ok(())
    }

    fn file_at(&mut self, path: &str) -> Result<Option<String>, GraphError> {
        self.resolve_paths(&[path.to_string()])?;
        Ok(self.at_path.get(path).cloned().flatten())
    }

    /// Of the files once at a path, the one there now; when none is, the one that was there
    /// last read. Two files at one path is a path reused, and the living one is the answer.
    fn choose_file_at(&self, path: &str, candidates: &[&String]) -> Option<String> {
        let present_here = |file_id: &&&String| {
            self.files.get(**file_id).is_some_and(|file| {
                file.locations
                    .iter()
                    .any(|(at, status)| at.as_str() == path && *status == LocationStatus::Present)
            })
        };
        candidates
            .iter()
            .find(present_here)
            .or_else(|| candidates.iter().min())
            .map(|file_id| (*file_id).clone())
    }

    fn load_files(&mut self, file_ids: &[String]) -> Result<(), GraphError> {
        let unread: Vec<String> = file_ids
            .iter()
            .filter(|file_id| !self.files.contains_key(*file_id))
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if unread.is_empty() {
            return Ok(());
        }
        for file in self.reader().files(&unread)? {
            self.files.insert(file.file_id.clone(), file);
        }
        Ok(())
    }

    /// Reads the rows of these sessions and keeps one per session, with its transcript.
    fn load_conversations(&mut self, sessions: &[String]) -> Result<(), GraphError> {
        if sessions.is_empty() {
            return Ok(());
        }
        let rows = self.reader().conversations_of_sessions(sessions)?;
        let mut chosen: HashMap<String, StoredConversation> = HashMap::new();
        for row in rows {
            let better = chosen
                .get(&row.session_id)
                .is_none_or(|held| latest_reading(&row) > latest_reading(held));
            if better {
                chosen.insert(row.session_id.clone(), row);
            }
        }
        let transcripts: Vec<String> = chosen
            .values()
            .map(|conversation| conversation.transcript_file_id.clone())
            .collect();
        self.load_files(&transcripts)?;
        self.conversations.extend(chosen);
        Ok(())
    }

    fn replies_of(
        &self,
        offered: &[(TouchedPair, Target)],
    ) -> Result<HashMap<(String, i64), StoredReply>, GraphError> {
        let moments: Vec<(String, i64)> = offered
            .iter()
            .filter_map(|(pair, _)| {
                let conversation = self.conversations.get(pair.session_id())?;
                Some((conversation.id.clone(), pair.at_unix_ms()?))
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if moments.is_empty() {
            return Ok(HashMap::new());
        }
        Ok(self
            .reader()
            .replies_before(&moments)?
            .into_iter()
            .map(|reply| {
                (
                    (reply.conversation_id.clone(), reply.touched_at_unix_ms),
                    reply,
                )
            })
            .collect())
    }

    fn attach_files(&self, budget: &mut ViewBudget, file_ids: &[String], depth: u8) {
        for file in file_ids
            .iter()
            .filter_map(|file_id| self.files.get(file_id))
        {
            let Some(path) = self.shown_path(file) else {
                continue;
            };
            self.attach(
                budget,
                &file_node_id(&file.file_id),
                path.as_str(),
                file.topic.as_ref(),
                depth,
            );
        }
    }

    fn attach_conversations(&self, budget: &mut ViewBudget, sessions: &[String], depth: u8) {
        for conversation in sessions
            .iter()
            .filter_map(|session| self.conversations.get(session))
        {
            let Some(path) = self
                .files
                .get(&conversation.transcript_file_id)
                .and_then(|file| self.shown_path(file))
            else {
                continue;
            };
            self.attach(
                budget,
                &conversation_node_id(&conversation.session_id),
                path.as_str(),
                conversation.topic.as_ref(),
                depth,
            );
        }
    }

    /// A node's project and topic. Offered after the step's pairs, so they never take the
    /// place of a file; a project the view already holds costs an edge and no node.
    fn attach(
        &self,
        budget: &mut ViewBudget,
        owner: &str,
        path: &str,
        topic: Option<&StoredTopic>,
        depth: u8,
    ) {
        if !budget.contains(owner) {
            return;
        }
        if let Some(root) = project_of(self.explorer.roots, path) {
            let project = project_node(root, depth);
            let edge = GraphEdge::new(owner, project.id(), EdgeKind::InProject);
            budget.offer(vec![project], vec![edge]);
        }
        if let Some(topic) = topic {
            let node = topic_node(topic, depth);
            let edge = GraphEdge::new(owner, node.id(), EdgeKind::About);
            budget.offer(vec![node], vec![edge]);
        }
    }

    /// A conversation is a file too, and a transcript the policy hides is hidden here.
    fn transcript_is_visible(&self, conversation: &StoredConversation) -> bool {
        self.files
            .get(&conversation.transcript_file_id)
            .is_some_and(|file| self.shown_path(file).is_some())
    }

    /// The path a file is shown by: a present one before a missing one, and the first in order
    /// among equals, of the paths this context admits. None when it admits none.
    fn shown_path<'f>(&self, file: &'f StoredFile) -> Option<&'f NormalizedPath> {
        file.locations
            .iter()
            .filter(|(path, _)| (self.explorer.visible)(path))
            .min_by_key(|(path, status)| (*status != LocationStatus::Present, path.as_str()))
            .map(|(path, _)| path)
    }

    fn file_node(&self, file: &StoredFile, depth: u8) -> Option<GraphNode> {
        let path = self.shown_path(file)?;
        let standing = match file.locations.iter().any(|(path, status)| {
            *status == LocationStatus::Present && (self.explorer.visible)(path)
        }) {
            true => FileStanding::Present,
            false => FileStanding::Missing,
        };
        Some(GraphNode::new(
            file_node_id(&file.file_id),
            last_segment(path.as_str()),
            depth,
            Provenance::new("File", file.file_id.clone()),
            NodeDetail::File {
                path: path.as_str().to_string(),
                standing,
            },
        ))
    }
}

/// Which of two rows of one session is the reading to draw: the current revision of its
/// transcript first, then the one that read the most, then the id, so a tie is decided the
/// same way on every run.
fn latest_reading(row: &StoredConversation) -> (bool, u64, &str) {
    (row.current, row.message_count, row.id.as_str())
}

fn moment_key(conversation: &StoredConversation, pair: &TouchedPair) -> (String, i64) {
    (
        conversation.id.clone(),
        pair.at_unix_ms().unwrap_or(i64::MIN),
    )
}

/// The chain of one pair: the conversation, the reply the touch followed when one can be
/// placed, and the file.
fn chain(
    conversation: GraphNode,
    reply: Option<GraphNode>,
    touched: GraphNode,
    pair: &TouchedPair,
) -> (Vec<GraphNode>, Vec<GraphEdge>) {
    let mark = TouchMark::new(
        pair.verbs().collect(),
        pair.naming() == OwnerNaming::Named,
        pair.times(),
    );
    match reply {
        Some(reply) => {
            // The reply is the one said last before the touch: a reading by time, and the edge
            // says so rather than passing it for a record.
            let edges = vec![
                GraphEdge::new(conversation.id(), reply.id(), EdgeKind::HasReply),
                GraphEdge::touched(reply.id(), touched.id(), mark.placed_by_time()),
            ];
            (vec![conversation, reply, touched], edges)
        }
        None => {
            let edges = vec![GraphEdge::touched(conversation.id(), touched.id(), mark)];
            (vec![conversation, touched], edges)
        }
    }
}

/// The project `path` lies in: the deepest root of the context that holds it, on a segment
/// boundary — and when that root is a folder of conversations, the root it was named after. A
/// conversation's folder is a part of its project, not a second project beside it.
fn project_of<'r>(roots: &'r [String], path: &str) -> Option<&'r str> {
    let root = roots
        .iter()
        .map(|root| root.trim_end_matches('/'))
        .filter(|root| {
            path == *root
                || path
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|root| root.len())?;
    let others = roots
        .iter()
        .map(|other| other.trim_end_matches('/'))
        .filter(|other| *other != root);
    Some(project_of_folder(&last_segment(root), others).unwrap_or(root))
}

fn last_segment(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

fn conversation_node_id(session: &str) -> String {
    format!("conversation:{session}")
}

fn file_node_id(file_id: &str) -> String {
    format!("file:{file_id}")
}

fn outside_node_id(path: &str) -> String {
    format!("outside:{path}")
}

fn conversation_node(conversation: &StoredConversation, depth: u8) -> GraphNode {
    GraphNode::new(
        conversation_node_id(&conversation.session_id),
        conversation.name.trim(),
        depth,
        Provenance::new("Conversation", conversation.id.clone()),
        NodeDetail::Conversation {
            session_id: conversation.session_id.clone(),
            message_count: conversation.message_count,
            last_message_at_unix_ms: conversation.last_message_at_unix_ms,
        },
    )
}

fn reply_node(conversation: &StoredConversation, reply: &StoredReply, depth: u8) -> GraphNode {
    GraphNode::new(
        format!("reply:{}:{}", conversation.session_id, reply.ordinal),
        format!("#{}", reply.ordinal),
        depth,
        Provenance::new("Message", reply.message_id.clone()),
        NodeDetail::Reply {
            conversation: conversation_node_id(&conversation.session_id),
            ordinal: reply.ordinal,
            speaker: reply.speaker,
            at_unix_ms: Some(reply.at_unix_ms),
        },
    )
}

/// A path no location of this context holds: a file node in the state that says so, not a
/// kind of its own — the same file is never drawn twice, once in each state.
fn outside_node(path: &str, depth: u8) -> GraphNode {
    GraphNode::new(
        outside_node_id(path),
        last_segment(path),
        depth,
        Provenance::new("FileTouch", path),
        NodeDetail::File {
            path: path.to_string(),
            standing: FileStanding::Outside,
        },
    )
}

fn project_node(root: &str, depth: u8) -> GraphNode {
    GraphNode::new(
        format!("project:{root}"),
        last_segment(root),
        depth,
        from_configuration(format!("include {root}")),
        NodeDetail::Project {
            root: root.to_string(),
        },
    )
}

fn topic_node(topic: &StoredTopic, depth: u8) -> GraphNode {
    GraphNode::new(
        format!("topic:{}", topic.label),
        topic.label.clone(),
        depth,
        Provenance::new("InsightResult", topic.result_id.clone()),
        NodeDetail::Topic,
    )
}
