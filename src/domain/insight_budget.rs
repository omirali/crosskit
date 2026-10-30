// What a run is allowed to spend on a model, and the decision that spends it.
//
// Two limits, because they fail differently. A single revision larger than the per-job
// limit will be that large on every retry, so it is refused for good. A run that has spent
// what it was given still has jobs that are perfectly fine, so they go back in the queue
// for the next run rather than being blamed for arriving late.
//
// Spend is measured in bytes of the chunk text that would be sent, not in tokens. Bytes
// are exact and already known before anything is read: the graph stores each chunk's byte
// range, so a job over its limit is refused without its content ever being loaded, let
// alone sent. A token count would be an estimate, or another request to obtain.
use std::fmt;

use super::insight_result::InsightProvenance;
use super::knowledge::KnowledgeTally;

/// What one run may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsightBudget {
    per_job_bytes: i64,
    per_run_bytes: i64,
}

impl InsightBudget {
    pub fn new(per_job_bytes: i64, per_run_bytes: i64) -> Self {
        Self {
            per_job_bytes: per_job_bytes.max(0),
            per_run_bytes: per_run_bytes.max(0),
        }
    }

    /// The most one job may send.
    pub fn per_job_bytes(&self) -> i64 {
        self.per_job_bytes
    }

    /// Whether `bytes` may be sent, given what this run has spent so far. The answer is
    /// reached before any provider is called, so an over-budget job costs nothing.
    pub fn admits(&self, spent: &RunSpend, bytes: i64) -> BudgetVerdict {
        if bytes > self.per_job_bytes {
            return BudgetVerdict::InputTooLarge;
        }
        if spent.bytes.saturating_add(bytes) > self.per_run_bytes {
            return BudgetVerdict::RunBudgetSpent;
        }
        BudgetVerdict::Admit
    }
}

/// What this run has sent so far, what the engine said it cost, and what of the answers was
/// kept. Only a call that actually reached the provider is spent: a cache hit sends nothing,
/// so it costs nothing, and is counted apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunSpend {
    bytes: i64,
    calls: usize,
    cache_hits: usize,
    input_tokens: i64,
    output_tokens: i64,
    cost_micro_usd: i64,
    knowledge: KnowledgeTally,
}

impl RunSpend {
    pub fn record(&mut self, bytes: i64) {
        self.bytes = self.bytes.saturating_add(bytes.max(0));
    }

    /// One call that reached the engine: the bytes it carried and what the answer said it cost.
    pub fn record_call(&mut self, bytes: i64, provenance: &InsightProvenance) {
        self.record(bytes);
        self.calls += 1;
        self.input_tokens = self.input_tokens.saturating_add(provenance.input_tokens());
        self.output_tokens = self
            .output_tokens
            .saturating_add(provenance.output_tokens());
        self.cost_micro_usd = self
            .cost_micro_usd
            .saturating_add(provenance.cost_micro_usd());
    }

    pub fn record_cache_hit(&mut self) {
        self.cache_hits += 1;
    }

    pub fn record_knowledge(&mut self, tally: &KnowledgeTally) {
        self.knowledge.add(tally);
    }

    pub fn bytes(&self) -> i64 {
        self.bytes
    }

    pub fn calls(&self) -> usize {
        self.calls
    }

    pub fn cache_hits(&self) -> usize {
        self.cache_hits
    }

    pub fn input_tokens(&self) -> i64 {
        self.input_tokens
    }

    pub fn output_tokens(&self) -> i64 {
        self.output_tokens
    }

    pub fn cost_micro_usd(&self) -> i64 {
        self.cost_micro_usd
    }

    pub fn tally(&self) -> KnowledgeTally {
        self.knowledge
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetVerdict {
    Admit,
    /// This revision is larger than one job may ever send. No retry changes that.
    InputTooLarge,
    /// The run has spent what it was given. The job itself is fine; another run will take
    /// it.
    RunBudgetSpent,
}

impl fmt::Display for BudgetVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admit => f.write_str("within budget"),
            Self::InputTooLarge => f.write_str("larger than one job may send"),
            Self::RunBudgetSpent => f.write_str("this run has spent its budget"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BudgetVerdict, InsightBudget, RunSpend};

    #[test]
    fn a_job_within_both_limits_is_admitted() {
        let budget = InsightBudget::new(100, 250);
        let spend = RunSpend::default();

        assert_eq!(budget.admits(&spend, 0), BudgetVerdict::Admit);
        assert_eq!(budget.admits(&spend, 100), BudgetVerdict::Admit);
    }

    #[test]
    fn a_revision_above_the_per_job_limit_is_refused_for_good() {
        let budget = InsightBudget::new(100, 10_000);

        assert_eq!(
            budget.admits(&RunSpend::default(), 101),
            BudgetVerdict::InputTooLarge
        );
    }

    #[test]
    fn a_run_that_has_spent_its_budget_refuses_the_next_job_without_blaming_it() {
        let budget = InsightBudget::new(100, 250);
        let mut spend = RunSpend::default();
        spend.record(100);
        spend.record(100);

        assert_eq!(budget.admits(&spend, 50), BudgetVerdict::Admit);
        assert_eq!(budget.admits(&spend, 51), BudgetVerdict::RunBudgetSpent);
        // The per-job limit is checked first: an oversize job is its own problem even when
        // the run has no room for it either.
        assert_eq!(budget.admits(&spend, 500), BudgetVerdict::InputTooLarge);
    }

    #[test]
    fn spend_never_goes_backwards_and_never_wraps() {
        let budget = InsightBudget::new(10, 1_000);
        let mut spend = RunSpend::default();
        // Sending nothing spends nothing, so the run is still untouched.
        spend.record(-5);
        assert_eq!(budget.admits(&spend, 1), BudgetVerdict::Admit);
        // Twice the largest number there is. Wrapping would have made the run look
        // untouched again; saturating leaves it above every budget a run can be given.
        spend.record(i64::MAX);
        spend.record(i64::MAX);
        assert_eq!(budget.admits(&spend, 1), BudgetVerdict::RunBudgetSpent);
    }

    #[test]
    fn a_negative_limit_means_nothing_may_be_sent() {
        let budget = InsightBudget::new(-1, -1);

        assert_eq!(
            budget.admits(&RunSpend::default(), 1),
            BudgetVerdict::InputTooLarge
        );
    }
}
