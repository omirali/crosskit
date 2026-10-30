// Use case: where a confirmed task stands, read from the maps already kept and the owner's
// decisions, without asking a model or reading a reply from the graph (ADR 0061).
//
// This is what a board of tasks shows on every card — how far the main line got, whether its
// head waits for the owner, how many risks are open and how many of them are critical — and
// what the rule «a task with an open critical risk is not marked done» counts. The owner's word
// on a risk is stronger than the model's: a risk the owner accepted or resolved is not open,
// whatever the map says.
use crate::domain::ConfirmedTask;
use crate::domain::decision_map::{BranchState, RiskLevel, RiskStatus};
use crate::domain::risk_decision::{RiskAnchor, decision_for};
use crate::domain::task_synthesis::NodeRef;

use super::task_map::{MapStore, MapStoreFailure, StoredMap};
use super::task_synthesis::SynthesisStore;

/// Where one task stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskProgress {
    /// Its conversations.
    pub conversations: usize,
    /// Of them, the ones mapped by the current question.
    pub mapped: usize,
    /// Steps of the joined main line; nothing is joined while this is zero.
    pub steps: usize,
    /// What became of the last step: `Open` means the task waits for the owner.
    pub head: Option<BranchState>,
    pub open_risks: usize,
    pub open_critical: usize,
    /// Risks the owner accepted or resolved.
    pub decided_risks: usize,
}

/// Reads where a task stands.
pub trait TaskProgressReader {
    fn progress(&self, task: &ConfirmedTask) -> Result<TaskProgress, MapStoreFailure>;
}

/// The progress of tasks from the store that keeps their maps and joins.
pub struct KeptProgress<'a, Store: MapStore + SynthesisStore> {
    store: &'a Store,
    question_version: &'a str,
}

impl<'a, Store: MapStore + SynthesisStore> KeptProgress<'a, Store> {
    /// `question_version` is the map question in use: a map built by another counts as none.
    pub fn new(store: &'a Store, question_version: &'a str) -> Self {
        Self {
            store,
            question_version,
        }
    }
}

impl<Store: MapStore + SynthesisStore> TaskProgressReader for KeptProgress<'_, Store> {
    fn progress(&self, task: &ConfirmedTask) -> Result<TaskProgress, MapStoreFailure> {
        let mut maps: Vec<StoredMap> = Vec::new();
        for session in task.members() {
            if let Some(map) = self.store.load(session)?
                && map.question_version == self.question_version
            {
                maps.push(map);
            }
        }
        let reply_of = |session: &str, index: usize| -> Option<usize> {
            let map = maps.iter().find(|map| map.session_id == session)?;
            Some(map.nodes.risks.get(index)?.evidence.reply)
        };
        let mut progress = TaskProgress {
            conversations: task.members().len(),
            mapped: maps.len(),
            ..TaskProgress::default()
        };
        // Every risk with the places its quotes stand at: the join's when there is one, the
        // conversations' own otherwise.
        let mut risks: Vec<(RiskLevel, RiskStatus, Vec<RiskAnchor>)> = Vec::new();
        if let Some(stored) = self.store.load_synthesis(task.id())? {
            progress.steps = stored.synthesis.main_line.len();
            progress.head = stored.synthesis.main_line.last().map(|step| step.state);
            for risk in &stored.synthesis.risks {
                let anchors = risk
                    .nodes
                    .iter()
                    .filter_map(|node| match *node {
                        NodeRef::Risk {
                            conversation,
                            index,
                        } => {
                            let session = stored.sessions.get(conversation)?;
                            Some(RiskAnchor {
                                session: session.clone(),
                                reply: reply_of(session, index)?,
                            })
                        }
                        NodeRef::Fork { .. } => None,
                    })
                    .collect();
                risks.push((risk.level, risk.status, anchors));
            }
        } else {
            for map in &maps {
                for risk in map.nodes.risks.iter().filter(|risk| risk.in_task) {
                    risks.push((
                        risk.level,
                        risk.status,
                        vec![RiskAnchor {
                            session: map.session_id.clone(),
                            reply: risk.evidence.reply,
                        }],
                    ));
                }
            }
        }
        for (level, status, anchors) in risks {
            if decision_for(task.risk_decisions(), &anchors).is_some() {
                progress.decided_risks += 1;
            } else if status == RiskStatus::Open {
                progress.open_risks += 1;
                if level == RiskLevel::Critical {
                    progress.open_critical += 1;
                }
            }
        }
        Ok(progress)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::application::task_synthesis::StoredSynthesis;
    use crate::domain::TaskBook;
    use crate::domain::decision_map::{Evidence, MapNodes, ProofTally, Risk};
    use crate::domain::risk_decision::{RiskDecision, RiskVerdict};
    use crate::domain::task_synthesis::{LineStep, TaskRisk, TaskSynthesis};

    #[derive(Default)]
    struct Kept {
        maps: RefCell<BTreeMap<String, StoredMap>>,
        join: RefCell<Option<StoredSynthesis>>,
    }

    impl MapStore for Kept {
        fn load(&self, session_id: &str) -> Result<Option<StoredMap>, MapStoreFailure> {
            Ok(self.maps.borrow().get(session_id).cloned())
        }

        fn save(&self, map: &StoredMap) -> Result<(), MapStoreFailure> {
            self.maps
                .borrow_mut()
                .insert(map.session_id.clone(), map.clone());
            Ok(())
        }
    }

    impl SynthesisStore for Kept {
        fn load_synthesis(
            &self,
            _task_id: &str,
        ) -> Result<Option<StoredSynthesis>, MapStoreFailure> {
            Ok(self.join.borrow().clone())
        }

        fn save_synthesis(&self, synthesis: &StoredSynthesis) -> Result<(), MapStoreFailure> {
            *self.join.borrow_mut() = Some(synthesis.clone());
            Ok(())
        }
    }

    fn risk(level: RiskLevel, reply: usize) -> Risk {
        Risk {
            description: "риск".into(),
            level,
            status: RiskStatus::Open,
            in_task: true,
            evidence: Evidence {
                reply,
                quote: "цитата".into(),
            },
        }
    }

    /// One conversation `a` mapped by `v1` with a critical risk at reply 3 and a high one at 9,
    /// and a second conversation `b` mapped by an older question.
    fn kept() -> Kept {
        let store = Kept::default();
        let map = |session: &str, version: &str, risks: Vec<Risk>| StoredMap {
            session_id: session.into(),
            question_version: version.into(),
            mapped_through: 10,
            nodes: MapNodes {
                forks: Vec::new(),
                risks,
            },
            tally: ProofTally::default(),
            input_tokens: 0,
            output_tokens: 0,
            cost_micro_usd: 0,
        };
        store
            .save(&map(
                "a",
                "v1",
                vec![risk(RiskLevel::Critical, 3), risk(RiskLevel::High, 9)],
            ))
            .unwrap();
        store
            .save(&map("b", "v0", vec![risk(RiskLevel::Critical, 1)]))
            .unwrap();
        store
    }

    fn task(decisions: Vec<RiskDecision>) -> ConfirmedTask {
        let mut book = TaskBook::default();
        book.confirm("Скидки", BTreeSet::from(["a".to_string(), "b".to_string()]))
            .unwrap();
        for decision in decisions {
            book.decide_risk("Скидки", decision).unwrap();
        }
        book.tasks()[0].clone()
    }

    fn accepted_at(reply: usize) -> RiskDecision {
        RiskDecision {
            anchor: RiskAnchor {
                session: "a".into(),
                reply,
            },
            verdict: RiskVerdict::Accepted,
            reason: "по снапшоту".into(),
            at_unix_ms: 1,
        }
    }

    #[test]
    fn without_a_join_the_conversations_risks_count_and_a_map_of_another_question_does_not() {
        let store = kept();
        let progress = KeptProgress::new(&store, "v1")
            .progress(&task(Vec::new()))
            .unwrap();
        assert_eq!(
            progress,
            TaskProgress {
                conversations: 2,
                mapped: 1,
                steps: 0,
                head: None,
                open_risks: 2,
                open_critical: 1,
                decided_risks: 0,
            }
        );
    }

    /// The join's risk stands where its nodes' quotes stand, so the owner's word on that reply
    /// takes it off the open ones; the head of the main line says whether the task waits.
    #[test]
    fn a_joined_risk_the_owner_accepted_is_not_open_and_the_head_is_read() {
        let store = kept();
        store
            .save_synthesis(&StoredSynthesis {
                task_id: "task:a".into(),
                version: "join-v1".into(),
                fingerprint: String::new(),
                sessions: vec!["a".into()],
                synthesis: TaskSynthesis {
                    main_line: vec![LineStep {
                        title: "Как показывать?".into(),
                        state: BranchState::Open,
                        nodes: Vec::new(),
                    }],
                    risks: vec![TaskRisk {
                        title: "Возврат".into(),
                        level: RiskLevel::Critical,
                        status: RiskStatus::Open,
                        nodes: vec![NodeRef::Risk {
                            conversation: 0,
                            index: 0,
                        }],
                    }],
                    findings: Vec::new(),
                },
                cost_micro_usd: 0,
            })
            .unwrap();
        let reader = KeptProgress::new(&store, "v1");

        let before = reader.progress(&task(Vec::new())).unwrap();
        assert_eq!((before.steps, before.head), (1, Some(BranchState::Open)));
        assert_eq!((before.open_risks, before.open_critical), (1, 1));

        let after = reader.progress(&task(vec![accepted_at(3)])).unwrap();
        assert_eq!(
            (after.open_risks, after.open_critical, after.decided_risks),
            (0, 0, 1)
        );
    }
}
