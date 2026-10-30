// Use case: the whole task's decision map, joined from its conversations' maps (ADR 0056).
//
// The join reads the maps, not the replies: a few dozen kilobytes of nodes where the replies are
// megabytes, so it costs cents and runs again whenever a conversation's map grows. A join is kept
// with the fingerprint of the maps it was made from; one whose maps have moved on is shown as
// stale until the next build makes it again.
use crate::domain::decision_map::MapNodes;
use crate::domain::task_synthesis::{TaskSynthesis, grounded};

use super::task_map::{MapEngineFailure, MapStoreFailure, MappedConversation};

/// One conversation as the join sees it: its place in the task is its index in the list.
#[derive(Debug, Clone, Copy)]
pub struct MergeInput<'a> {
    pub label: &'a str,
    pub first_at_unix_ms: Option<i64>,
    pub nodes: &'a MapNodes,
}

/// What a model proposed for the task, and what it cost.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergedProposal {
    pub synthesis: TaskSynthesis,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micro_usd: u64,
}

/// Joins the conversations' maps of a task.
pub trait MapMerger {
    /// The version of the join question.
    fn merge_version(&self) -> &str;
    fn merge(
        &self,
        task: &str,
        conversations: &[MergeInput<'_>],
    ) -> Result<MergedProposal, MapEngineFailure>;
}

/// A join as kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSynthesis {
    pub task_id: String,
    pub version: String,
    /// The maps it was made from: every conversation's session, question and last reply.
    pub fingerprint: String,
    /// The sessions of the joined conversations, in the order the nodes number them.
    pub sessions: Vec<String>,
    pub synthesis: TaskSynthesis,
    pub cost_micro_usd: u64,
}

/// Where joins are kept.
pub trait SynthesisStore {
    fn load_synthesis(&self, task_id: &str) -> Result<Option<StoredSynthesis>, MapStoreFailure>;
    fn save_synthesis(&self, synthesis: &StoredSynthesis) -> Result<(), MapStoreFailure>;
}

/// What the maps of `mapped` are, in one string: a join made from them is current while it
/// matches.
pub fn fingerprint(mapped: &[MappedConversation]) -> String {
    mapped
        .iter()
        .filter_map(|conversation| {
            let map = conversation.map.as_ref()?;
            Some(format!(
                "{}:{}:{}",
                conversation.session_id, map.question_version, map.mapped_through
            ))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// A join as it stands against the maps now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownSynthesis {
    pub stored: StoredSynthesis,
    /// The maps moved on after it was made.
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesisError {
    Store(MapStoreFailure),
    Engine(MapEngineFailure),
}

pub struct TaskSynthesizing<'a, Merger: MapMerger, Store: SynthesisStore> {
    merger: &'a Merger,
    store: &'a Store,
}

impl<'a, Merger: MapMerger, Store: SynthesisStore> TaskSynthesizing<'a, Merger, Store> {
    pub fn new(merger: &'a Merger, store: &'a Store) -> Self {
        Self { merger, store }
    }

    /// The kept join of the task, and whether the maps moved on since.
    pub fn show(
        &self,
        task_id: &str,
        mapped: &[MappedConversation],
    ) -> Result<Option<ShownSynthesis>, SynthesisError> {
        let stored = self
            .store
            .load_synthesis(task_id)
            .map_err(SynthesisError::Store)?;
        Ok(stored.map(|stored| ShownSynthesis {
            stale: stored.fingerprint != fingerprint(mapped)
                || stored.version != self.merger.merge_version(),
            stored,
        }))
    }

    /// The join of the task's maps, made again when the maps moved on or none is kept. Nothing
    /// is asked when no conversation has a map.
    pub fn build(
        &self,
        task: &str,
        task_id: &str,
        mapped: &[MappedConversation],
    ) -> Result<Option<ShownSynthesis>, SynthesisError> {
        if let Some(shown) = self.show(task_id, mapped)?
            && !shown.stale
        {
            return Ok(Some(shown));
        }
        let with_maps: Vec<(&MappedConversation, &MapNodes)> = mapped
            .iter()
            .filter_map(|conversation| Some((conversation, &conversation.map.as_ref()?.nodes)))
            .collect();
        if with_maps.is_empty() {
            return Ok(None);
        }
        let inputs: Vec<MergeInput<'_>> = with_maps
            .iter()
            .map(|(conversation, nodes)| MergeInput {
                label: conversation
                    .name
                    .as_deref()
                    .unwrap_or(&conversation.session_id),
                first_at_unix_ms: conversation.first_at_unix_ms,
                nodes,
            })
            .collect();
        let proposed = self
            .merger
            .merge(task, &inputs)
            .map_err(SynthesisError::Engine)?;
        let maps: Vec<&MapNodes> = with_maps.iter().map(|(_, nodes)| *nodes).collect();
        let stored = StoredSynthesis {
            task_id: task_id.to_string(),
            version: self.merger.merge_version().to_string(),
            fingerprint: fingerprint(mapped),
            sessions: with_maps
                .iter()
                .map(|(conversation, _)| conversation.session_id.clone())
                .collect(),
            synthesis: grounded(proposed.synthesis, &maps),
            cost_micro_usd: proposed.cost_micro_usd,
        };
        self.store
            .save_synthesis(&stored)
            .map_err(SynthesisError::Store)?;
        Ok(Some(ShownSynthesis {
            stored,
            stale: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::application::task_map::StoredMap;
    use crate::domain::decision_map::{Branch, BranchState, Evidence, Fork, ProofTally};
    use crate::domain::task_synthesis::{LineStep, NodeRef};

    struct Joiner(RefCell<usize>);

    impl MapMerger for Joiner {
        fn merge_version(&self) -> &str {
            "join-test"
        }

        /// One step on the first fork of every conversation, and one that names nothing real.
        fn merge(
            &self,
            _task: &str,
            conversations: &[MergeInput<'_>],
        ) -> Result<MergedProposal, MapEngineFailure> {
            *self.0.borrow_mut() += 1;
            let mut main_line: Vec<LineStep> = (0..conversations.len())
                .map(|conversation| LineStep {
                    title: "шаг".into(),
                    state: BranchState::Chosen,
                    nodes: vec![NodeRef::Fork {
                        conversation,
                        index: 0,
                    }],
                })
                .collect();
            main_line.push(LineStep {
                title: "выдумано".into(),
                state: BranchState::Chosen,
                nodes: vec![NodeRef::Fork {
                    conversation: 9,
                    index: 0,
                }],
            });
            Ok(MergedProposal {
                synthesis: TaskSynthesis {
                    main_line,
                    ..TaskSynthesis::default()
                },
                cost_micro_usd: 5,
                ..MergedProposal::default()
            })
        }
    }

    #[derive(Default)]
    struct Kept(RefCell<Option<StoredSynthesis>>);

    impl SynthesisStore for Kept {
        fn load_synthesis(
            &self,
            _task_id: &str,
        ) -> Result<Option<StoredSynthesis>, MapStoreFailure> {
            Ok(self.0.borrow().clone())
        }

        fn save_synthesis(&self, synthesis: &StoredSynthesis) -> Result<(), MapStoreFailure> {
            *self.0.borrow_mut() = Some(synthesis.clone());
            Ok(())
        }
    }

    fn mapped(session: &str, through: Option<usize>) -> MappedConversation {
        MappedConversation {
            session_id: session.into(),
            name: Some(session.into()),
            first_at_unix_ms: Some(0),
            replies: 10,
            map: through.map(|mapped_through| StoredMap {
                session_id: session.into(),
                question_version: "decision-map-v3".into(),
                mapped_through,
                nodes: MapNodes {
                    forks: vec![Fork {
                        question: "Как?".into(),
                        evidence: Evidence {
                            reply: 1,
                            quote: "как".into(),
                        },
                        branches: vec![Branch {
                            option: "так".into(),
                            state: BranchState::Chosen,
                            reason: String::new(),
                            evidence: Evidence {
                                reply: 1,
                                quote: "так".into(),
                            },
                        }],
                    }],
                    risks: Vec::new(),
                },
                tally: ProofTally::default(),
                input_tokens: 0,
                output_tokens: 0,
                cost_micro_usd: 0,
            }),
        }
    }

    #[test]
    fn a_join_is_made_from_the_mapped_conversations_and_made_again_only_when_a_map_grows() {
        let (merger, store) = (Joiner(RefCell::new(0)), Kept::default());
        let synthesizing = TaskSynthesizing::new(&merger, &store);
        let task = [
            mapped("a", Some(10)),
            mapped("b", None),
            mapped("c", Some(4)),
        ];

        let joined = synthesizing
            .build("Скидки", "task:a", &task)
            .unwrap()
            .unwrap();
        assert_eq!(joined.stored.sessions, ["a", "c"]);
        assert_eq!(
            joined.stored.synthesis.main_line.len(),
            2,
            "the step naming nothing is dropped"
        );

        synthesizing.build("Скидки", "task:a", &task).unwrap();
        assert_eq!(*merger.0.borrow(), 1, "nothing moved, nothing asked");

        let grown = [
            mapped("a", Some(10)),
            mapped("b", None),
            mapped("c", Some(9)),
        ];
        assert!(synthesizing.show("task:a", &grown).unwrap().unwrap().stale);
        synthesizing.build("Скидки", "task:a", &grown).unwrap();
        assert_eq!(*merger.0.borrow(), 2);
    }

    #[test]
    fn nothing_is_asked_when_no_conversation_has_a_map() {
        let (merger, store) = (Joiner(RefCell::new(0)), Kept::default());
        let shown = TaskSynthesizing::new(&merger, &store)
            .build("Скидки", "task:a", &[mapped("a", None)])
            .unwrap();
        assert!(shown.is_none());
        assert_eq!(*merger.0.borrow(), 0);
    }
}
