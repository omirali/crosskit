// The owner's word on a risk as the machine answers spell it (ADR 0061): in the board of
// `tasks --json` with the place it stands at, and on a risk of `tasks map --json` without it.
// One spelling for both, because a reader of one is a reader of the other.
use serde::{Deserialize, Serialize};

use crate::domain::risk_decision::{RiskAnchor, RiskDecision, RiskVerdict};

/// A decision with the place of the risk it is about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RiskDecisionLine {
    pub(super) session: String,
    pub(super) reply: usize,
    #[serde(flatten)]
    pub(super) decided: Decided,
}

/// What was decided, when, and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Decided {
    /// `accepted` or `resolved`.
    pub(super) verdict: String,
    pub(super) reason: String,
    pub(super) at_unix_ms: i64,
}

impl From<&RiskDecision> for RiskDecisionLine {
    fn from(decision: &RiskDecision) -> Self {
        Self {
            session: decision.anchor.session.clone(),
            reply: decision.anchor.reply,
            decided: Decided::from(decision),
        }
    }
}

impl From<&RiskDecision> for Decided {
    fn from(decision: &RiskDecision) -> Self {
        Self {
            verdict: decision.verdict.code().to_string(),
            reason: decision.reason.clone(),
            at_unix_ms: decision.at_unix_ms,
        }
    }
}

impl RiskDecisionLine {
    /// The decision back, or nothing for a verdict this build does not know.
    pub(super) fn decision(&self) -> Option<RiskDecision> {
        Some(RiskDecision {
            anchor: RiskAnchor {
                session: self.session.clone(),
                reply: self.reply,
            },
            verdict: RiskVerdict::parse(&self.decided.verdict)?,
            reason: self.decided.reason.clone(),
            at_unix_ms: self.decided.at_unix_ms,
        })
    }
}
