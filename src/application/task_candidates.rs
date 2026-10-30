// Use case: which conversations of a context belong to one task (ADR 0052).
//
// The graph holds what a conversation said and touched, and nothing about tasks. Stitching
// reads every current conversation back as the facts the domain compares — the files it wrote,
// the files and conversations its owner named as it started, the tracker tasks it carried — and
// the domain's rules group them. Nothing is written: a candidate is computed on every call, and
// keeping a confirmed task is the next step (ADR 0052, decision 4).
//
// Speech comes back as chunks and replies with no edge between them. Which reply a chunk was
// cut from is the rule `replies_behind` holds for periods too; a chunk is the owner's only when
// every reply it could have come from is the owner's.
use std::collections::BTreeMap;

use crate::domain::conversation::{Speaker, TouchVerb};
use crate::domain::task_stitching::mentions::file_key;
use crate::domain::task_stitching::session::{Platform, SessionFacts};
use crate::domain::task_stitching::stitching::{
    LinkSignal, StitchRules, Strength, candidate_tasks, links,
};
use crate::domain::{ChunkSource, exact_replies_behind, replies_behind};

use super::repository::RepositoryError;

/// One current conversation as the graph holds it, for stitching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationLog {
    pub session_id: String,
    /// The conversation's name, when anybody gave it one.
    pub name: Option<String>,
    /// Whether chunk zero is the name rather than a reply.
    pub named: bool,
    /// The folder the transcript is filed under: where the conversation was held, as far as
    /// the graph knows. The working directory itself is not stored.
    pub folder: Option<String>,
    /// Every reply by its ordinal: who said it and when.
    pub replies: Vec<(Speaker, Option<i64>)>,
    /// Every chunk of the transcript's revision: its ordinal and text.
    pub chunks: Vec<(u32, String)>,
    /// Every file the conversation touched: path, verb and when.
    pub touches: Vec<(String, TouchVerb, Option<i64>)>,
}

/// What stitching reads.
pub trait ConversationLogReader {
    /// Every conversation whose transcript revision is its file's current one.
    fn conversation_logs(&self) -> Result<Vec<ConversationLog>, RepositoryError>;
}

/// How a conversation came into a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tie {
    /// By a fact or a strong signal.
    Strong,
    /// By being held soon after another one in the same place, and by nothing else.
    Neighbour,
}

/// One conversation of a candidate task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMember {
    session_id: String,
    platform: Platform,
    name: Option<String>,
    opening: Option<String>,
    first_at_unix_ms: Option<i64>,
    last_at_unix_ms: Option<i64>,
    tie: Tie,
    signals: Vec<LinkSignal>,
    /// The folder its transcript is filed under, as the graph knows it.
    folder: Option<String>,
}

impl TaskMember {
    /// The folder its transcript is filed under: where to look for the transcript itself.
    pub fn folder(&self) -> Option<&str> {
        self.folder.as_deref()
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The tool the conversation was held in.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The owner's first words, for a conversation nobody named.
    pub fn opening(&self) -> Option<&str> {
        self.opening.as_deref()
    }

    pub fn first_at_unix_ms(&self) -> Option<i64> {
        self.first_at_unix_ms
    }

    pub fn last_at_unix_ms(&self) -> Option<i64> {
        self.last_at_unix_ms
    }

    pub fn tie(&self) -> Tie {
        self.tie
    }

    /// Every signal between this conversation and another of the candidate.
    pub fn signals(&self) -> &[LinkSignal] {
        &self.signals
    }
}

/// A group of conversations stitching puts in one task, earliest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCandidate {
    members: Vec<TaskMember>,
}

impl TaskCandidate {
    pub fn members(&self) -> &[TaskMember] {
        &self.members
    }
}

/// What stitching found in one context: its candidates, and every conversation it read, for a
/// confirmed task whose conversations no candidate holds any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stitched {
    pub candidates: Vec<TaskCandidate>,
    /// Every conversation by session, as a member tied by nothing yet.
    pub conversations: BTreeMap<String, TaskMember>,
}

/// Candidate tasks of one context.
pub struct TaskCandidates<'r, Reader: ConversationLogReader> {
    reader: &'r Reader,
}

impl<'r, Reader: ConversationLogReader> TaskCandidates<'r, Reader> {
    pub fn new(reader: &'r Reader) -> Self {
        Self { reader }
    }

    /// Every candidate of at least two conversations, largest first, and every conversation
    /// read. `home` is the directory `~/` in a path the owner wrote stands for.
    pub fn find(&self, home: &str) -> Result<Stitched, RepositoryError> {
        let sessions: Vec<SessionFacts> = one_per_session(self.reader.conversation_logs()?)
            .iter()
            .map(|log| facts_of(log, home))
            .collect();
        let rules = StitchRules::product(&sessions);
        let found = links(&sessions, &rules);
        let strong = candidate_tasks(sessions.len(), &found, Strength::Strong);
        let weak = candidate_tasks(sessions.len(), &found, Strength::Weak);
        let candidates = strong
            .iter()
            .map(|group| {
                let neighbours = neighbours_of(group, &strong, &weak);
                candidate_of(&sessions, &found, group, &neighbours)
            })
            .collect();
        let conversations = sessions
            .iter()
            .map(|session| {
                (
                    session.id.clone(),
                    member_of(session, Tie::Strong, Vec::new()),
                )
            })
            .collect();
        Ok(Stitched {
            candidates,
            conversations,
        })
    }
}

/// A transcript copied to a second place is one conversation read twice; the copy with more
/// replies is the one that saw more of it.
pub(crate) fn one_per_session(logs: Vec<ConversationLog>) -> Vec<ConversationLog> {
    let mut by_session: BTreeMap<String, ConversationLog> = BTreeMap::new();
    for log in logs {
        match by_session.get(&log.session_id) {
            Some(kept) if kept.replies.len() >= log.replies.len() => {}
            _ => {
                by_session.insert(log.session_id.clone(), log);
            }
        }
    }
    by_session.into_values().collect()
}

/// The conversations a weak group adds to `group`: the rest of the weak group holding it, when
/// that weak group holds no other strong group. A neighbour between two tasks belongs to
/// neither.
fn neighbours_of(group: &[usize], strong: &[Vec<usize>], weak: &[Vec<usize>]) -> Vec<usize> {
    let Some(wider) = weak.iter().find(|wider| wider.contains(&group[0])) else {
        return Vec::new();
    };
    let other_task_inside = strong
        .iter()
        .any(|other| other != group && other.iter().any(|member| wider.contains(member)));
    if other_task_inside {
        return Vec::new();
    }
    wider
        .iter()
        .copied()
        .filter(|member| !group.contains(member))
        .collect()
}

fn candidate_of(
    sessions: &[SessionFacts],
    found: &[crate::domain::task_stitching::stitching::Link],
    group: &[usize],
    neighbours: &[usize],
) -> TaskCandidate {
    let everyone: Vec<usize> = group.iter().chain(neighbours).copied().collect();
    let mut members: Vec<TaskMember> = everyone
        .iter()
        .map(|&index| {
            let session = &sessions[index];
            let mut signals: Vec<LinkSignal> = found
                .iter()
                .filter(|link| link.earlier == index || link.later == index)
                .filter(|link| everyone.contains(&link.earlier) && everyone.contains(&link.later))
                .flat_map(|link| link.signals.iter().copied())
                .collect();
            signals.sort_unstable();
            signals.dedup();
            let tie = if group.contains(&index) {
                Tie::Strong
            } else {
                Tie::Neighbour
            };
            member_of(session, tie, signals)
        })
        .collect();
    members.sort_by_key(|member| member.first_at_unix_ms.unwrap_or(i64::MAX));
    TaskCandidate { members }
}

fn member_of(session: &SessionFacts, tie: Tie, signals: Vec<LinkSignal>) -> TaskMember {
    TaskMember {
        session_id: session.id.clone(),
        platform: session.platform,
        name: session.title.clone(),
        opening: session.opening.clone(),
        first_at_unix_ms: session.started_ms,
        last_at_unix_ms: session.ended_ms,
        tie,
        signals,
        folder: session.place.clone(),
    }
}

/// What a stored conversation tells stitching, in the order it happened: the owner's words and
/// the files touched, so that "as it started" means the same here as in a live transcript.
fn facts_of(log: &ConversationLog, home: &str) -> SessionFacts {
    let mut session = SessionFacts::new(log.session_id.clone(), Platform::ClaudeCode);
    session.place = log.folder.clone();
    session.title = log.name.clone();

    let mut events: Vec<(i64, Event<'_>)> = owner_words(log)
        .into_iter()
        .map(|(at, text)| (at.unwrap_or(i64::MAX), Event::Said(text)))
        .chain(
            log.touches
                .iter()
                .map(|(path, verb, at)| (at.unwrap_or(i64::MAX), Event::Touched(path, *verb))),
        )
        .collect();
    events.sort_by_key(|(at, _)| *at);
    for (_, _, at) in &log.touches {
        if let Some(at) = at {
            session.saw(*at);
        }
    }
    for (_, at) in &log.replies {
        if let Some(at) = at {
            session.saw(*at);
        }
    }
    for (_, event) in events {
        match event {
            Event::Said(text) => session.take_owner_words(text, home),
            Event::Touched(path, verb) => {
                let Some(key) = file_key(path, None, home) else {
                    continue;
                };
                match verb {
                    TouchVerb::Read => session.take_reference(key),
                    TouchVerb::Wrote | TouchVerb::Edited => {
                        session.written.insert(key);
                    }
                }
            }
        }
    }
    session
}

enum Event<'a> {
    Said(&'a str),
    Touched(&'a str, TouchVerb),
}

/// Every chunk the owner said, with the moment of the earliest reply it could be, in order. The
/// reply behind a chunk is read exactly from the chunks' lengths when they add up, and as a span
/// otherwise; a chunk counts as the owner's only when every reply it could be is. A conversation
/// whose chunks and replies do not line up gives none: its speech cannot be told apart.
fn owner_words(log: &ConversationLog) -> Vec<(Option<i64>, &str)> {
    let mut chunks: Vec<&(u32, String)> = log.chunks.iter().collect();
    chunks.sort_by_key(|(ordinal, _)| *ordinal);
    let lined_up = chunks
        .iter()
        .enumerate()
        .all(|(position, (ordinal, _))| *ordinal as usize == position);
    if !lined_up {
        return Vec::new();
    }
    let lengths: Vec<usize> = chunks.iter().map(|(_, text)| text.len()).collect();
    exact_replies_behind(log.named, &lengths, log.replies.len())
        .unwrap_or_else(|| replies_behind(log.named, chunks.len(), log.replies.len()))
        .into_iter()
        .zip(chunks)
        .filter_map(|(source, (_, text))| {
            let ChunkSource::Replies(replies) = source else {
                return None;
            };
            let candidates = log.replies.get(replies)?;
            let owners = candidates
                .iter()
                .all(|(speaker, _)| *speaker == Speaker::Owner);
            owners.then(|| (candidates.first().and_then(|(_, at)| *at), text.as_str()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/alice";
    const HOUR: i64 = 60 * 60 * 1000;

    struct Logs(Vec<ConversationLog>);

    impl ConversationLogReader for Logs {
        fn conversation_logs(&self) -> Result<Vec<ConversationLog>, RepositoryError> {
            Ok(self.0.clone())
        }
    }

    /// A conversation in `payments-api` where the owner says `words` at `hour` and the model
    /// answers; `touches` follow the owner's reply.
    fn log(
        session: &str,
        name: &str,
        hour: i64,
        words: &str,
        touches: &[(&str, TouchVerb)],
    ) -> ConversationLog {
        ConversationLog {
            session_id: session.into(),
            name: Some(name.into()),
            named: true,
            folder: Some("/Users/alice/.claude/projects/-w-payments-api".into()),
            replies: vec![
                (Speaker::Owner, Some(hour * HOUR)),
                (Speaker::Model, Some(hour * HOUR + 1)),
            ],
            chunks: vec![(0, name.into()), (1, words.into()), (2, "Готово.".into())],
            touches: touches
                .iter()
                .map(|(path, verb)| ((*path).to_string(), *verb, Some(hour * HOUR + 2)))
                .collect(),
        }
    }

    #[test]
    fn a_conversation_opened_on_the_handoff_another_wrote_is_one_task_with_it() {
        let reader = Logs(vec![
            log(
                "a",
                "Двойная скидка в корзине",
                0,
                "Почему скидка считается дважды?",
                &[("/w/payments-api/notes/handoff.md", TouchVerb::Wrote)],
            ),
            log(
                "b",
                "Скидка: продолжение",
                30,
                "Прочитай notes/handoff.md и продолжай",
                &[],
            ),
            log("c", "Отчёт по продажам", 60, "Собери отчёт", &[]),
        ]);
        let tasks = TaskCandidates::new(&reader).find(HOME).unwrap().candidates;
        assert_eq!(tasks.len(), 1);
        let members: Vec<&str> = tasks[0]
            .members()
            .iter()
            .map(TaskMember::session_id)
            .collect();
        assert_eq!(members, ["a", "b"]);
        assert!(
            tasks[0].members()[1]
                .signals()
                .contains(&LinkSignal::RelayFile)
        );
    }

    #[test]
    fn the_models_words_are_not_the_owners_and_name_nothing() {
        let first = log("a", "Двойная скидка в корзине", 0, "Посмотри корзину", &[]);
        let mut second = log("b", "Отчёт по продажам", 30, "Собери отчёт", &[]);
        second.chunks[2].1 = "Как в беседе «Двойная скидка в корзине».".into();
        let reader = Logs(vec![first, second]);
        assert!(
            TaskCandidates::new(&reader)
                .find(HOME)
                .unwrap()
                .candidates
                .is_empty()
        );
    }

    #[test]
    fn a_transcript_read_twice_is_one_conversation() {
        let reader = Logs(vec![
            log("a", "Двойная скидка", 0, "Почему дважды?", &[]),
            log("a", "Двойная скидка", 0, "Почему дважды?", &[]),
        ]);
        assert_eq!(one_per_session(reader.0.clone()).len(), 1);
    }

    #[test]
    fn chunks_that_do_not_line_up_with_replies_give_no_owner_words() {
        let mut broken = log("a", "Двойная скидка", 0, "Почему дважды?", &[]);
        broken.chunks.remove(1);
        assert!(owner_words(&broken).is_empty());
    }

    #[test]
    fn a_long_pasted_reply_does_not_hide_what_the_owner_said_after_it() {
        use crate::domain::MOST_BYTES_PER_REPLY_CHUNK;
        let first = log("a", "Двойная скидка в корзине", 0, "Посмотри корзину", &[]);
        let mut second = log("b", "Отчёт по продажам", 30, "вывод запроса", &[]);
        // A span alone would say the fourth chunk is the owner's second reply or the model's
        // answer, and give up on it; the length of the second chunk says which.
        second.replies = vec![
            (Speaker::Owner, Some(30 * HOUR)),
            (Speaker::Owner, Some(30 * HOUR + 1)),
            (Speaker::Model, Some(30 * HOUR + 2)),
        ];
        second.chunks = vec![
            (0, "Отчёт по продажам".into()),
            (1, "x".repeat(MOST_BYTES_PER_REPLY_CHUNK)),
            (2, "конец вставки".into()),
            (
                3,
                "мы в беседе «Двойная скидка в корзине» решили так же".into(),
            ),
            (4, "Вижу.".into()),
        ];
        let reader = Logs(vec![first, second]);
        let tasks = TaskCandidates::new(&reader).find(HOME).unwrap().candidates;
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].members()[1].signals(),
            [LinkSignal::NamedConversation]
        );
    }
}
