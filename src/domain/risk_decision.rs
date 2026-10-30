// The owner's word on a risk of a task's map: accepted with a reason, or resolved
// (`docs/decision-map.md`, section 5; ADR 0061).
//
// A model marks a risk open, accepted or resolved by what the conversation says. The owner's
// decision is stronger: it outlives every rebuild of the map, and it is what turns a red mark
// grey. It is kept by where the risk stands — the conversation and the reply its quote is in —
// because that is what a rebuilt map keeps when it finds the same risk again, while the risk's
// wording is the model's and changes.

/// Where a risk node stands: the conversation and the reply its quote is in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RiskAnchor {
    pub session: String,
    pub reply: usize,
}

/// What the owner said about a risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskVerdict {
    /// Taken knowingly: the work goes on with it.
    Accepted,
    /// Closed: by a commit, a test or a decision the reason names.
    Resolved,
}

impl RiskVerdict {
    pub const ALL: [Self; 2] = [Self::Accepted, Self::Resolved];

    pub fn code(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Resolved => "resolved",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|verdict| verdict.code() == code)
    }
}

/// The owner's decision on the risk at `anchor`, with the reason and the moment it was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskDecision {
    pub anchor: RiskAnchor,
    pub verdict: RiskVerdict,
    pub reason: String,
    pub at_unix_ms: i64,
}

/// The decision on a risk that stands at any of `anchors`; the latest one when several of its
/// nodes were decided on. A joined risk stands at every node it was joined from, so a decision
/// on any of them is a decision on it.
pub fn decision_for<'a>(
    decisions: &'a [RiskDecision],
    anchors: &[RiskAnchor],
) -> Option<&'a RiskDecision> {
    decisions
        .iter()
        .filter(|decision| anchors.contains(&decision.anchor))
        .max_by_key(|decision| decision.at_unix_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(session: &str, reply: usize) -> RiskAnchor {
        RiskAnchor {
            session: session.into(),
            reply,
        }
    }

    fn decided(anchor: RiskAnchor, verdict: RiskVerdict, at_unix_ms: i64) -> RiskDecision {
        RiskDecision {
            anchor,
            verdict,
            reason: "по снапшоту".into(),
            at_unix_ms,
        }
    }

    #[test]
    fn a_decision_on_any_node_of_a_joined_risk_is_its_decision_and_the_latest_wins() {
        let decisions = [
            decided(at("a", 3), RiskVerdict::Accepted, 10),
            decided(at("b", 7), RiskVerdict::Resolved, 20),
        ];
        assert_eq!(
            decision_for(&decisions, &[at("a", 3), at("b", 7)]).map(|d| d.verdict),
            Some(RiskVerdict::Resolved)
        );
        assert_eq!(
            decision_for(&decisions, &[at("a", 3)]).map(|d| d.verdict),
            Some(RiskVerdict::Accepted)
        );
        assert!(decision_for(&decisions, &[at("a", 4)]).is_none());
    }
}
