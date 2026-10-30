// Use case: the decision map of a confirmed task — every conversation of it mapped by a model
// into forks, branches and risks, each kept only when a quote proves it (ADR 0055).
//
// A map is built once and grown: each conversation's map remembers the last reply it covers,
// and a later build asks only about the replies after it. A map built by another version of the
// question is built again, because two questions do not make one map. Nothing is asked of a
// model unless the caller asks to build; showing a map reads what was kept.
use std::collections::{BTreeMap, BTreeSet};

use crate::domain::conversation::Speaker;
use crate::domain::decision_map::{MapNodes, ProofTally, proved};
use crate::domain::{ChunkSource, exact_replies_behind};

use super::repository::RepositoryError;
use super::task_candidates::{ConversationLog, ConversationLogReader, one_per_session};

/// The most bytes of replies one question is given.
pub const WINDOW_BYTES: usize = 60_000;
/// The most bytes of one reply a question carries; the engine cuts a longer one and says so.
pub const REPLY_BYTES: usize = 20_000;

/// One reply as it was said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenReply {
    /// From one, in the order the replies were said.
    pub number: usize,
    pub speaker: Speaker,
    pub text: String,
}

/// One conversation's replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSpeech {
    pub session_id: String,
    pub name: Option<String>,
    pub first_at_unix_ms: Option<i64>,
    pub replies: Vec<SpokenReply>,
}

/// The replies of the conversations of `sessions`, put back together from the graph's chunks by
/// their lengths (`exact_replies_behind`). A conversation whose chunks do not add up that way is
/// left out rather than mapped with its speakers guessed.
pub fn speech_of(
    reader: &impl ConversationLogReader,
    sessions: &BTreeSet<String>,
) -> Result<Vec<ConversationSpeech>, RepositoryError> {
    let logs = one_per_session(reader.conversation_logs()?);
    let mut speech: Vec<ConversationSpeech> = logs
        .iter()
        .filter(|log| sessions.contains(&log.session_id))
        .filter_map(|log| {
            Some(ConversationSpeech {
                session_id: log.session_id.clone(),
                name: log.name.clone(),
                first_at_unix_ms: log.replies.iter().filter_map(|(_, at)| *at).min(),
                replies: replies_of(log)?,
            })
        })
        .collect();
    speech.sort_by_key(|conversation| conversation.first_at_unix_ms.unwrap_or(i64::MAX));
    Ok(speech)
}

fn replies_of(log: &ConversationLog) -> Option<Vec<SpokenReply>> {
    let mut chunks: Vec<&(u32, String)> = log.chunks.iter().collect();
    chunks.sort_by_key(|(ordinal, _)| *ordinal);
    let lengths: Vec<usize> = chunks.iter().map(|(_, text)| text.len()).collect();
    let sources = exact_replies_behind(log.named, &lengths, log.replies.len())?;
    let mut replies: Vec<SpokenReply> = log
        .replies
        .iter()
        .enumerate()
        .map(|(index, (speaker, _))| SpokenReply {
            number: index + 1,
            speaker: *speaker,
            text: String::new(),
        })
        .collect();
    for (source, (_, text)) in sources.into_iter().zip(chunks) {
        if let ChunkSource::Replies(range) = source {
            replies[*range.start()].text.push_str(text);
        }
    }
    Some(replies)
}

/// The replies after `after`, in windows of whole replies, each at most `WINDOW_BYTES` once a
/// long reply is counted at `REPLY_BYTES`. A reply with nothing in it is not asked about.
pub fn windows_after(replies: &[SpokenReply], after: usize) -> Vec<Vec<&SpokenReply>> {
    let mut windows: Vec<Vec<&SpokenReply>> = Vec::new();
    let mut size = 0;
    for reply in replies
        .iter()
        .filter(|reply| reply.number > after && !reply.text.trim().is_empty())
    {
        let bytes = reply.text.len().min(REPLY_BYTES);
        if windows.is_empty() || size + bytes > WINDOW_BYTES {
            windows.push(Vec::new());
            size = 0;
        }
        size += bytes;
        let last = windows.len() - 1;
        windows[last].push(reply);
    }
    windows
}

/// What the model proposed for one window, and what the question cost.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProposedStretch {
    pub nodes: MapNodes,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micro_usd: u64,
}

/// Why a window could not be mapped. The sentence is for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEngineFailure(pub String);

/// Asks a model for one window's nodes.
pub trait MapEngine {
    /// The version of the question this engine asks.
    fn question_version(&self) -> &str;
    /// Proposes the nodes of `window`, a stretch of a conversation of `replies` replies.
    fn propose(
        &self,
        task: &str,
        window: &[&SpokenReply],
        replies: usize,
    ) -> Result<ProposedStretch, MapEngineFailure>;
}

/// One conversation's map as kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMap {
    pub session_id: String,
    pub question_version: String,
    /// The last reply the map covers.
    pub mapped_through: usize,
    pub nodes: MapNodes,
    pub tally: ProofTally,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micro_usd: u64,
}

/// Why a kept map could not be read or written. The sentence is for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapStoreFailure(pub String);

/// Where every conversation's map is kept.
pub trait MapStore {
    fn load(&self, session_id: &str) -> Result<Option<StoredMap>, MapStoreFailure>;
    fn save(&self, map: &StoredMap) -> Result<(), MapStoreFailure>;
}

/// One conversation of a task's map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedConversation {
    pub session_id: String,
    pub name: Option<String>,
    pub first_at_unix_ms: Option<i64>,
    pub replies: usize,
    /// The map as kept, of this question's version; nothing when none is.
    pub map: Option<StoredMap>,
}

impl MappedConversation {
    /// Replies said after the map was built, or every reply when there is no map.
    pub fn unmapped_replies(&self) -> usize {
        let through = self.map.as_ref().map_or(0, |map| map.mapped_through);
        self.replies.saturating_sub(through)
    }
}

/// A step of a build, for a person to watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapProgress<'a> {
    Asking {
        conversation: &'a str,
        first_reply: usize,
        last_reply: usize,
    },
    Failed {
        conversation: &'a str,
        reason: &'a str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskMapError {
    Store(MapStoreFailure),
}

/// The decision map of one task.
pub struct TaskMapping<'a, Engine: MapEngine, Store: MapStore> {
    engine: &'a Engine,
    store: &'a Store,
}

impl<'a, Engine: MapEngine, Store: MapStore> TaskMapping<'a, Engine, Store> {
    pub fn new(engine: &'a Engine, store: &'a Store) -> Self {
        Self { engine, store }
    }

    /// The maps as kept, without asking anything.
    pub fn show(
        &self,
        speech: &[ConversationSpeech],
    ) -> Result<Vec<MappedConversation>, TaskMapError> {
        speech
            .iter()
            .map(|conversation| {
                Ok(mapped(
                    conversation,
                    self.current(&conversation.session_id)?,
                ))
            })
            .collect()
    }

    /// Asks about every reply no current map covers, keeps what a quote proves, and shows the
    /// maps. A window the engine fails on ends that conversation's build where it stands: the
    /// map keeps what came before it, and the next build starts there.
    pub fn build(
        &self,
        task: &str,
        speech: &[ConversationSpeech],
        progress: &mut dyn FnMut(MapProgress<'_>),
    ) -> Result<Vec<MappedConversation>, TaskMapError> {
        let mut shown = Vec::with_capacity(speech.len());
        for conversation in speech {
            let mut map = self
                .current(&conversation.session_id)?
                .unwrap_or_else(|| StoredMap {
                    session_id: conversation.session_id.clone(),
                    question_version: self.engine.question_version().to_string(),
                    mapped_through: 0,
                    nodes: MapNodes::default(),
                    tally: ProofTally::default(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cost_micro_usd: 0,
                });
            let label = conversation
                .name
                .as_deref()
                .unwrap_or(&conversation.session_id);
            for window in windows_after(&conversation.replies, map.mapped_through) {
                let (Some(first), Some(last)) = (window.first(), window.last()) else {
                    continue;
                };
                progress(MapProgress::Asking {
                    conversation: label,
                    first_reply: first.number,
                    last_reply: last.number,
                });
                let proposed = match self
                    .engine
                    .propose(task, &window, conversation.replies.len())
                {
                    Ok(proposed) => proposed,
                    Err(MapEngineFailure(reason)) => {
                        progress(MapProgress::Failed {
                            conversation: label,
                            reason: &reason,
                        });
                        break;
                    }
                };
                let texts: BTreeMap<usize, &str> = window
                    .iter()
                    .map(|reply| (reply.number, reply.text.as_str()))
                    .collect();
                let (kept, tally) = proved(proposed.nodes, &texts);
                map.nodes.forks.extend(kept.forks);
                map.nodes.risks.extend(kept.risks);
                map.tally.add(tally);
                map.input_tokens += proposed.input_tokens;
                map.output_tokens += proposed.output_tokens;
                map.cost_micro_usd += proposed.cost_micro_usd;
                map.mapped_through = last.number;
                self.store.save(&map).map_err(TaskMapError::Store)?;
            }
            shown.push(mapped(conversation, Some(map)));
        }
        Ok(shown)
    }

    /// The kept map of a conversation if this question built it.
    fn current(&self, session_id: &str) -> Result<Option<StoredMap>, TaskMapError> {
        Ok(self
            .store
            .load(session_id)
            .map_err(TaskMapError::Store)?
            .filter(|map| map.question_version == self.engine.question_version()))
    }
}

fn mapped(conversation: &ConversationSpeech, map: Option<StoredMap>) -> MappedConversation {
    MappedConversation {
        session_id: conversation.session_id.clone(),
        name: conversation.name.clone(),
        first_at_unix_ms: conversation.first_at_unix_ms,
        replies: conversation.replies.len(),
        map,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::domain::decision_map::{Branch, BranchState, Evidence, Fork};

    struct Echo {
        asked: RefCell<Vec<(usize, usize)>>,
    }

    impl MapEngine for Echo {
        fn question_version(&self) -> &str {
            "test-v1"
        }

        /// One fork per window on its first reply, quoting it whole, and one branch that
        /// quotes words the reply does not hold.
        fn propose(
            &self,
            _task: &str,
            window: &[&SpokenReply],
            _replies: usize,
        ) -> Result<ProposedStretch, MapEngineFailure> {
            let first = window[0];
            self.asked
                .borrow_mut()
                .push((first.number, window[window.len() - 1].number));
            let at = |quote: &str| Evidence {
                reply: first.number,
                quote: quote.into(),
            };
            Ok(ProposedStretch {
                nodes: MapNodes {
                    forks: vec![Fork {
                        question: "Как?".into(),
                        evidence: at(&first.text),
                        branches: vec![
                            Branch {
                                option: "так".into(),
                                state: BranchState::Chosen,
                                reason: String::new(),
                                evidence: at(&first.text),
                            },
                            Branch {
                                option: "иначе".into(),
                                state: BranchState::Rejected,
                                reason: String::new(),
                                evidence: at("слов этих нет"),
                            },
                        ],
                    }],
                    risks: Vec::new(),
                },
                cost_micro_usd: 10,
                ..ProposedStretch::default()
            })
        }
    }

    #[derive(Default)]
    struct Kept(RefCell<BTreeMap<String, StoredMap>>);

    impl MapStore for Kept {
        fn load(&self, session_id: &str) -> Result<Option<StoredMap>, MapStoreFailure> {
            Ok(self.0.borrow().get(session_id).cloned())
        }

        fn save(&self, map: &StoredMap) -> Result<(), MapStoreFailure> {
            self.0
                .borrow_mut()
                .insert(map.session_id.clone(), map.clone());
            Ok(())
        }
    }

    fn conversation(replies: usize, bytes: usize) -> ConversationSpeech {
        ConversationSpeech {
            session_id: "a".into(),
            name: Some("Скидки".into()),
            first_at_unix_ms: Some(0),
            replies: (1..=replies)
                .map(|number| SpokenReply {
                    number,
                    speaker: if number % 2 == 1 {
                        Speaker::Owner
                    } else {
                        Speaker::Model
                    },
                    text: format!("реплика {number} {}", "x".repeat(bytes)),
                })
                .collect(),
        }
    }

    #[test]
    fn a_map_is_grown_by_the_new_replies_only_and_keeps_what_a_quote_proves() {
        let engine = Echo {
            asked: RefCell::new(Vec::new()),
        };
        let store = Kept::default();
        let mapping = TaskMapping::new(&engine, &store);
        let mut seen = 0;

        let first = mapping
            .build("Скидки", &[conversation(4, 10)], &mut |_| seen += 1)
            .unwrap();
        let map = first[0].map.clone().unwrap();
        assert_eq!(map.mapped_through, 4);
        assert_eq!(map.nodes.forks.len(), 1);
        assert_eq!(map.nodes.forks[0].branches.len(), 1);
        assert_eq!(map.tally.quote_not_found, 1);

        mapping
            .build("Скидки", &[conversation(6, 10)], &mut |_| seen += 1)
            .unwrap();
        assert_eq!(*engine.asked.borrow(), [(1, 4), (5, 6)]);
        assert_eq!(seen, 2);
        assert_eq!(store.0.borrow()["a"].cost_micro_usd, 20);
    }

    #[test]
    fn long_replies_go_into_windows_counted_at_their_cut() {
        let replies = conversation(5, 30_000).replies;
        let cut: Vec<Vec<usize>> = windows_after(&replies, 0)
            .iter()
            .map(|window| window.iter().map(|reply| reply.number).collect())
            .collect();
        assert_eq!(cut, [vec![1, 2, 3], vec![4, 5]]);
        assert!(windows_after(&replies, 5).is_empty());
    }

    #[test]
    fn showing_asks_nothing_and_a_map_of_another_question_is_built_again_from_the_start() {
        let engine = Echo {
            asked: RefCell::new(Vec::new()),
        };
        let store = Kept::default();
        store
            .save(&StoredMap {
                session_id: "a".into(),
                question_version: "old".into(),
                mapped_through: 4,
                nodes: MapNodes::default(),
                tally: ProofTally::default(),
                input_tokens: 0,
                output_tokens: 0,
                cost_micro_usd: 0,
            })
            .unwrap();
        let mapping = TaskMapping::new(&engine, &store);
        let shown = mapping.show(&[conversation(4, 10)]).unwrap();
        assert!(shown[0].map.is_none());
        assert_eq!(shown[0].unmapped_replies(), 4);
        assert!(engine.asked.borrow().is_empty());

        mapping
            .build("Скидки", &[conversation(4, 10)], &mut |_| {})
            .unwrap();
        assert_eq!(*engine.asked.borrow(), [(1, 4)]);
        assert_eq!(store.0.borrow()["a"].question_version, "test-v1");
    }
}
