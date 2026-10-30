// A decision map of one conversation: the forks where an approach was chosen, the branches at
// each with what became of them, and the risks left without an answer (`docs/decision-map.md`,
// ADR 0055).
//
// A model proposes the nodes; this keeps only the ones a quote proves. A node names the reply
// it stands on and quotes it, and it is kept when the quote occurs in that reply by the
// knowledge layer's rule — runs of whitespace forgiven, nothing else (ADR 0048). A node without
// such a quote is the model's words, and it is dropped and counted, never shown.
use std::collections::BTreeMap;

use super::knowledge::quote_occurs_in;

/// What became of a branch at a fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BranchState {
    /// The owner chose it.
    Chosen,
    /// Turned down, with the reason.
    Rejected,
    /// Put off, with the condition to come back.
    Deferred,
    /// Waited for the owner's decision and did not get it.
    Open,
    /// The owner did not answer and the agent chose by itself.
    AgentDefault,
    /// The question was answered with something else, or the subject changed.
    Abandoned,
}

impl BranchState {
    pub const ALL: [Self; 6] = [
        Self::Chosen,
        Self::Rejected,
        Self::Deferred,
        Self::Open,
        Self::AgentDefault,
        Self::Abandoned,
    ];

    /// The name the question and the stored map use.
    pub fn code(self) -> &'static str {
        match self {
            Self::Chosen => "chosen",
            Self::Rejected => "rejected",
            Self::Deferred => "deferred",
            Self::Open => "open",
            Self::AgentDefault => "agent_default",
            Self::Abandoned => "abandoned",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.code() == code)
    }
}

/// How much a risk could cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    Critical,
    High,
    Medium,
    Info,
}

impl RiskLevel {
    pub const ALL: [Self; 4] = [Self::Critical, Self::High, Self::Medium, Self::Info];

    pub fn code(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Info => "info",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.code() == code)
    }
}

/// Where a risk stood when the conversation left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskStatus {
    Open,
    /// The owner took it knowingly, with a reason.
    Accepted,
    /// Closed later in the same conversation.
    Resolved,
}

impl RiskStatus {
    pub const ALL: [Self; 3] = [Self::Open, Self::Accepted, Self::Resolved];

    pub fn code(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Accepted => "accepted",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }
}

/// Where a node stands: the reply, by its number from one, and the words of it that prove the
/// node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub reply: usize,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub option: String,
    pub state: BranchState,
    pub reason: String,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fork {
    pub question: String,
    pub evidence: Evidence,
    pub branches: Vec<Branch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Risk {
    pub description: String,
    pub level: RiskLevel,
    pub status: RiskStatus,
    /// Whether the risk belongs to the task or was found on the way (a finding outside it).
    pub in_task: bool,
    pub evidence: Evidence,
}

/// The nodes of one stretch of a conversation, as proposed or as kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapNodes {
    pub forks: Vec<Fork>,
    pub risks: Vec<Risk>,
}

/// What the proof did to one stretch's nodes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProofTally {
    pub kept: usize,
    /// Named a reply the stretch does not hold.
    pub no_such_reply: usize,
    /// Quoted words the reply does not hold.
    pub quote_not_found: usize,
}

impl ProofTally {
    pub fn add(&mut self, other: ProofTally) {
        self.kept += other.kept;
        self.no_such_reply += other.no_such_reply;
        self.quote_not_found += other.quote_not_found;
    }

    pub fn dropped(&self) -> usize {
        self.no_such_reply + self.quote_not_found
    }
}

/// Keeps the nodes of `proposed` a quote proves against `replies`, by number. A fork whose own
/// quote fails is dropped with its branches; a branch whose quote fails is dropped alone, and a
/// fork left with no branch is dropped too: a question with no option is not a fork.
pub fn proved(proposed: MapNodes, replies: &BTreeMap<usize, &str>) -> (MapNodes, ProofTally) {
    let mut tally = ProofTally::default();
    let holds = |evidence: &Evidence, tally: &mut ProofTally| match replies.get(&evidence.reply) {
        None => {
            tally.no_such_reply += 1;
            false
        }
        Some(text) if quote_occurs_in(&evidence.quote, text) => {
            tally.kept += 1;
            true
        }
        Some(_) => {
            tally.quote_not_found += 1;
            false
        }
    };
    let mut forks = Vec::new();
    for mut fork in proposed.forks {
        let fork_holds = holds(&fork.evidence, &mut tally);
        let branches: Vec<Branch> = fork
            .branches
            .into_iter()
            .filter(|branch| holds(&branch.evidence, &mut tally))
            .collect();
        if fork_holds && !branches.is_empty() {
            fork.branches = branches;
            forks.push(fork);
        }
    }
    let risks = proposed
        .risks
        .into_iter()
        .filter(|risk| holds(&risk.evidence, &mut tally))
        .collect();
    (MapNodes { forks, risks }, tally)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(reply: usize, quote: &str) -> Evidence {
        Evidence {
            reply,
            quote: quote.into(),
        }
    }

    fn branch(state: BranchState, reply: usize, quote: &str) -> Branch {
        Branch {
            option: "вариант".into(),
            state,
            reason: String::new(),
            evidence: at(reply, quote),
        }
    }

    #[test]
    fn only_what_a_quote_proves_is_kept_and_a_fork_needs_a_branch() {
        let replies = BTreeMap::from([
            (1, "Почему скидка   считается дважды?"),
            (2, "Убираю расчёт из cart.rs, оставляю в order.rs."),
        ]);
        let proposed = MapNodes {
            forks: vec![
                Fork {
                    question: "Где считать скидку?".into(),
                    evidence: at(1, "скидка считается дважды"),
                    branches: vec![
                        branch(BranchState::Chosen, 2, "оставляю в order.rs"),
                        branch(BranchState::Rejected, 2, "оставляю в cart.rs"),
                    ],
                },
                Fork {
                    question: "Как назвать?".into(),
                    evidence: at(1, "Почему"),
                    branches: vec![branch(BranchState::Open, 9, "нет такой")],
                },
            ],
            risks: vec![Risk {
                description: "двойной расчёт".into(),
                level: RiskLevel::High,
                status: RiskStatus::Open,
                in_task: true,
                evidence: at(1, "считается трижды"),
            }],
        };
        let (kept, tally) = proved(proposed, &replies);
        assert_eq!(kept.forks.len(), 1);
        assert_eq!(kept.forks[0].branches.len(), 1);
        assert!(kept.risks.is_empty());
        assert_eq!(
            tally,
            ProofTally {
                kept: 3,
                no_such_reply: 1,
                quote_not_found: 2
            }
        );
    }

    #[test]
    fn every_code_reads_back_as_itself() {
        for state in BranchState::ALL {
            assert_eq!(BranchState::parse(state.code()), Some(state));
        }
        for level in RiskLevel::ALL {
            assert_eq!(RiskLevel::parse(level.code()), Some(level));
        }
        for status in RiskStatus::ALL {
            assert_eq!(RiskStatus::parse(status.code()), Some(status));
        }
    }
}
