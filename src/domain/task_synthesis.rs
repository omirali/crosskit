// The decision map of a whole task: its conversations' maps joined into one main line of key
// decisions, risks followed across conversations, and findings outside the task (ADR 0056).
//
// A joined item proves nothing by itself. It names the nodes of the conversations' maps it
// stands for, and those nodes were each kept because a quote proved them (ADR 0055). An item
// that names no node a map holds is the model's words and is dropped. What a model may say about
// a joined risk is bounded by its nodes: it is resolved or accepted only if one of them was, and
// its level is the highest of theirs — raised one step when it was raised again and again and
// nobody closed it (`docs/decision-map.md`, section 5).
use std::collections::BTreeSet;

use super::decision_map::{BranchState, MapNodes, RiskLevel, RiskStatus};

/// How many times an open risk is raised before that alone makes it one level worse.
pub const RAISED_OFTEN: usize = 3;

/// A node of one conversation's map: the conversation's place in the task, and the fork's or
/// risk's place in that map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeRef {
    Fork { conversation: usize, index: usize },
    Risk { conversation: usize, index: usize },
}

impl NodeRef {
    /// `c3.f2` for the third fork of the fourth conversation, `c0.r5` for a risk.
    pub fn code(self) -> String {
        match self {
            Self::Fork {
                conversation,
                index,
            } => format!("c{conversation}.f{index}"),
            Self::Risk {
                conversation,
                index,
            } => format!("c{conversation}.r{index}"),
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        let (conversation, node) = code.strip_prefix('c')?.split_once('.')?;
        let conversation = conversation.parse().ok()?;
        if let Some(index) = node.strip_prefix('f') {
            return Some(Self::Fork {
                conversation,
                index: index.parse().ok()?,
            });
        }
        let index = node.strip_prefix('r')?.parse().ok()?;
        Some(Self::Risk {
            conversation,
            index,
        })
    }

    pub fn conversation(self) -> usize {
        match self {
            Self::Fork { conversation, .. } | Self::Risk { conversation, .. } => conversation,
        }
    }
}

/// A key decision of the task, on its main line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineStep {
    pub title: String,
    pub state: BranchState,
    pub nodes: Vec<NodeRef>,
}

/// A risk followed across the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRisk {
    pub title: String,
    pub level: RiskLevel,
    pub status: RiskStatus,
    pub nodes: Vec<NodeRef>,
}

impl TaskRisk {
    /// In how many conversations the risk was raised.
    pub fn conversations(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.conversation())
            .collect::<BTreeSet<_>>()
            .len()
    }
}

/// The whole task's map, as proposed or as kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskSynthesis {
    pub main_line: Vec<LineStep>,
    pub risks: Vec<TaskRisk>,
    pub findings: Vec<TaskRisk>,
}

/// Keeps what the conversations' maps hold up, and bounds every risk by its nodes. `maps` are
/// the conversations' maps in the task's order, as the proposal numbered them.
pub fn grounded(proposed: TaskSynthesis, maps: &[&MapNodes]) -> TaskSynthesis {
    let exists = |node: &NodeRef| match *node {
        NodeRef::Fork {
            conversation,
            index,
        } => maps
            .get(conversation)
            .is_some_and(|map| index < map.forks.len()),
        NodeRef::Risk {
            conversation,
            index,
        } => maps
            .get(conversation)
            .is_some_and(|map| index < map.risks.len()),
    };
    let real = |nodes: Vec<NodeRef>| -> Vec<NodeRef> {
        let mut kept: Vec<NodeRef> = nodes.into_iter().filter(|node| exists(node)).collect();
        kept.sort_unstable();
        kept.dedup();
        kept
    };
    let main_line = proposed
        .main_line
        .into_iter()
        .filter_map(|mut step| {
            step.nodes = real(step.nodes);
            (!step.nodes.is_empty()).then_some(step)
        })
        .collect();
    let bound = |risks: Vec<TaskRisk>| -> Vec<TaskRisk> {
        risks
            .into_iter()
            .filter_map(|mut risk| {
                risk.nodes = real(risk.nodes);
                let held: Vec<(RiskLevel, RiskStatus)> = risk
                    .nodes
                    .iter()
                    .filter_map(|node| match *node {
                        NodeRef::Risk {
                            conversation,
                            index,
                        } => maps
                            .get(conversation)
                            .and_then(|map| map.risks.get(index))
                            .map(|held| (held.level, held.status)),
                        NodeRef::Fork { .. } => None,
                    })
                    .collect();
                if held.is_empty() {
                    return None;
                }
                risk.status = bounded_status(risk.status, &held);
                let worst = held
                    .iter()
                    .map(|(level, _)| *level)
                    .min()
                    .unwrap_or(risk.level);
                risk.level = if risk.status == RiskStatus::Open && held.len() >= RAISED_OFTEN {
                    one_worse(worst)
                } else {
                    worst
                };
                Some(risk)
            })
            .collect()
    };
    TaskSynthesis {
        main_line,
        risks: bound(proposed.risks),
        findings: bound(proposed.findings),
    }
}

/// A joined risk is closed or accepted only on the word of a node that was.
fn bounded_status(proposed: RiskStatus, held: &[(RiskLevel, RiskStatus)]) -> RiskStatus {
    if held.iter().any(|(_, status)| *status == proposed) {
        proposed
    } else {
        RiskStatus::Open
    }
}

fn one_worse(level: RiskLevel) -> RiskLevel {
    match level {
        RiskLevel::Info => RiskLevel::Medium,
        RiskLevel::Medium => RiskLevel::High,
        RiskLevel::High | RiskLevel::Critical => RiskLevel::Critical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::decision_map::{Evidence, Risk};

    fn risk(level: RiskLevel, status: RiskStatus) -> Risk {
        Risk {
            description: "не закоммичено".into(),
            level,
            status,
            in_task: true,
            evidence: Evidence {
                reply: 1,
                quote: "не закоммичено".into(),
            },
        }
    }

    fn map_of(risks: Vec<Risk>) -> MapNodes {
        MapNodes {
            forks: Vec::new(),
            risks,
        }
    }

    #[test]
    fn a_node_code_reads_back_as_itself() {
        for node in [
            NodeRef::Fork {
                conversation: 3,
                index: 12,
            },
            NodeRef::Risk {
                conversation: 0,
                index: 5,
            },
        ] {
            assert_eq!(NodeRef::parse(&node.code()), Some(node));
        }
        assert_eq!(NodeRef::parse("c1.x2"), None);
    }

    #[test]
    fn a_risk_raised_again_and_again_and_never_closed_is_a_level_worse() {
        let first = map_of(vec![risk(RiskLevel::Info, RiskStatus::Open)]);
        let second = map_of(vec![risk(RiskLevel::Info, RiskStatus::Open)]);
        let third = map_of(vec![risk(RiskLevel::Medium, RiskStatus::Open)]);
        let joined = TaskRisk {
            title: "payments-api не закоммичен".into(),
            level: RiskLevel::Info,
            status: RiskStatus::Resolved,
            nodes: (0..3)
                .map(|conversation| NodeRef::Risk {
                    conversation,
                    index: 0,
                })
                .collect(),
        };
        let kept = grounded(
            TaskSynthesis {
                risks: vec![joined],
                ..TaskSynthesis::default()
            },
            &[&first, &second, &third],
        );
        let risk = &kept.risks[0];
        assert_eq!(risk.status, RiskStatus::Open, "no node was resolved");
        assert_eq!(
            risk.level,
            RiskLevel::High,
            "medium at worst, raised three times"
        );
        assert_eq!(risk.conversations(), 3);
    }

    #[test]
    fn an_item_that_names_no_node_a_map_holds_is_dropped() {
        let only = map_of(vec![risk(RiskLevel::High, RiskStatus::Resolved)]);
        let proposed = TaskSynthesis {
            main_line: vec![LineStep {
                title: "сначала данные".into(),
                state: BranchState::Chosen,
                nodes: vec![NodeRef::Fork {
                    conversation: 0,
                    index: 4,
                }],
            }],
            risks: vec![TaskRisk {
                title: "закрыт".into(),
                level: RiskLevel::Info,
                status: RiskStatus::Resolved,
                nodes: vec![
                    NodeRef::Risk {
                        conversation: 0,
                        index: 0,
                    },
                    NodeRef::Risk {
                        conversation: 7,
                        index: 0,
                    },
                ],
            }],
            findings: Vec::new(),
        };
        let kept = grounded(proposed, &[&only]);
        assert!(kept.main_line.is_empty());
        assert_eq!(kept.risks[0].nodes.len(), 1);
        assert_eq!(kept.risks[0].status, RiskStatus::Resolved);
        assert_eq!(kept.risks[0].level, RiskLevel::High);
    }
}
