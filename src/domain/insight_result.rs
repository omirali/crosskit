// What a finished job leaves behind. One job can only ever name one result, because the
// result's id is derived from the job's; the schema refuses the second by primary key, so
// a repeated run cannot produce a duplicate even if something asks it to.
//
// A finding has three parts, and the split is deliberate. `input` describes the revision
// the work was drawn from and every handler can state it. `topic` is the insight itself,
// and only a model can produce one, so the handler that runs without a model leaves it
// empty rather than inventing a weaker version of it. `provenance` says who produced the
// finding and what it cost, which is what makes a stored insight auditable later.
//
// No chunk text and no path is ever part of a result: the only thing here derived from
// what the file says is the topic label, which its own type keeps to one short line.
use super::insight_topic::RevisionTopic;
use super::insight_work::ClaimedInsightJob;
use super::knowledge::{KnowledgeProposal, RevisionKnowledge};

/// What the revision amounted to: how many chunks it has and how many bytes of source text
/// those chunks cover. Both are already in the graph, so stating them needs no model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RevisionSummary {
    chunk_count: i64,
    text_bytes: i64,
}

impl RevisionSummary {
    pub fn new(chunk_count: i64, text_bytes: i64) -> Self {
        Self {
            chunk_count,
            text_bytes,
        }
    }

    pub fn chunk_count(&self) -> i64 {
        self.chunk_count
    }

    pub fn text_bytes(&self) -> i64 {
        self.text_bytes
    }
}

/// Who produced a finding and what it cost. A handler that used no model leaves the model
/// and prompt empty and both token counts at zero, which is the honest record of a finding
/// nobody was billed for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InsightProvenance {
    produced_by: String,
    model_id: String,
    prompt_version: String,
    input_tokens: i64,
    output_tokens: i64,
    /// What the call cost, in millionths of a dollar, as the engine reported it; zero when it
    /// reported nothing. A subscription tool reports what the call would have cost at list
    /// price, which is a measure of the work and not a bill.
    cost_micro_usd: i64,
}

impl InsightProvenance {
    /// A handler that asked no model.
    pub fn local(produced_by: impl Into<String>) -> Self {
        Self {
            produced_by: produced_by.into(),
            ..Self::default()
        }
    }

    pub fn from_model(
        produced_by: impl Into<String>,
        model_id: impl Into<String>,
        prompt_version: impl Into<String>,
        input_tokens: i64,
        output_tokens: i64,
    ) -> Self {
        Self {
            produced_by: produced_by.into(),
            model_id: model_id.into(),
            prompt_version: prompt_version.into(),
            input_tokens,
            output_tokens,
            cost_micro_usd: 0,
        }
    }

    /// The same provenance with what the engine said the call cost.
    pub fn costing(mut self, cost_micro_usd: i64) -> Self {
        self.cost_micro_usd = cost_micro_usd.max(0);
        self
    }

    pub fn produced_by(&self) -> &str {
        &self.produced_by
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn prompt_version(&self) -> &str {
        &self.prompt_version
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
}

/// What a model answered, before it is joined to the input it was drawn from. A cache
/// stores exactly this, because it is the part that depends only on the input and the
/// model, never on which job happened to ask.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelAnswer {
    topic: RevisionTopic,
    /// What the model proposed about the revision's entities and facts, cut to the limits but
    /// not yet checked against any source: that check belongs to each revision, and a cached
    /// answer may serve several.
    knowledge: KnowledgeProposal,
    provenance: InsightProvenance,
}

impl ModelAnswer {
    pub fn new(topic: RevisionTopic, provenance: InsightProvenance) -> Self {
        Self {
            topic,
            knowledge: KnowledgeProposal::default(),
            provenance,
        }
    }

    pub fn with_knowledge(mut self, knowledge: KnowledgeProposal) -> Self {
        self.knowledge = knowledge;
        self
    }

    pub fn topic(&self) -> &RevisionTopic {
        &self.topic
    }

    pub fn knowledge(&self) -> &KnowledgeProposal {
        &self.knowledge
    }

    pub fn provenance(&self) -> &InsightProvenance {
        &self.provenance
    }

    /// The finding this answer makes about `input`, with what of its knowledge the revision
    /// proved.
    pub fn about(self, input: RevisionSummary, knowledge: RevisionKnowledge) -> InsightFinding {
        InsightFinding {
            knowledge: Some(knowledge),
            ..InsightFinding::described(input, self.topic, self.provenance)
        }
    }
}

/// What one handler produced for one revision.
#[derive(Debug, Clone, PartialEq)]
pub struct InsightFinding {
    input: RevisionSummary,
    topic: Option<RevisionTopic>,
    /// The entities and facts the revision proved. Nothing for a finding that asked no model.
    knowledge: Option<RevisionKnowledge>,
    provenance: InsightProvenance,
    /// The answer was bought by another job with the same content. Its provenance still says
    /// what it cost when it was bought; this job paid nothing, and a sum of costs must know.
    answered_from_cache: bool,
}

impl InsightFinding {
    /// A finding with no insight in it: the input was counted and nothing was concluded.
    pub fn counted(input: RevisionSummary, provenance: InsightProvenance) -> Self {
        Self {
            input,
            topic: None,
            knowledge: None,
            provenance,
            answered_from_cache: false,
        }
    }

    /// A finding that carries what a model concluded about the revision.
    pub fn described(
        input: RevisionSummary,
        topic: RevisionTopic,
        provenance: InsightProvenance,
    ) -> Self {
        Self {
            input,
            topic: Some(topic),
            knowledge: None,
            provenance,
            answered_from_cache: false,
        }
    }

    pub fn input(&self) -> RevisionSummary {
        self.input
    }

    pub fn topic(&self) -> Option<&RevisionTopic> {
        self.topic.as_ref()
    }

    pub fn knowledge(&self) -> Option<&RevisionKnowledge> {
        self.knowledge.as_ref()
    }

    /// The same finding, answered from the cache rather than bought.
    pub fn served_from_cache(mut self) -> Self {
        self.answered_from_cache = true;
        self
    }

    pub fn answered_from_cache(&self) -> bool {
        self.answered_from_cache
    }

    pub fn provenance(&self) -> &InsightProvenance {
        &self.provenance
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsightResult {
    id: String,
    job_id: String,
    source_revision_id: String,
    input_content_hash: String,
    finding: InsightFinding,
    produced_at_unix_ms: i64,
}

impl InsightResult {
    /// The result of `job`. Every field but the finding and the moment comes from the claim
    /// the worker holds, so a result can never name a revision its job does not.
    pub fn of(job: &ClaimedInsightJob, finding: InsightFinding, produced_at_unix_ms: i64) -> Self {
        Self {
            id: format!("insight-result:{}", job.id()),
            job_id: job.id().to_string(),
            source_revision_id: job.source_revision_id().to_string(),
            input_content_hash: job.input_content_hash().to_string(),
            finding,
            produced_at_unix_ms,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn source_revision_id(&self) -> &str {
        &self.source_revision_id
    }

    pub fn input_content_hash(&self) -> &str {
        &self.input_content_hash
    }

    pub fn finding(&self) -> &InsightFinding {
        &self.finding
    }

    pub fn produced_at_unix_ms(&self) -> i64 {
        self.produced_at_unix_ms
    }
}

#[cfg(test)]
mod tests {
    use super::{InsightFinding, InsightProvenance, InsightResult, RevisionSummary};
    use crate::domain::{ClaimedInsightJob, RevisionTopic};

    fn job() -> ClaimedInsightJob {
        ClaimedInsightJob::new(
            "insight-job:insight-outbox-v1:file:7@hash:a",
            "file:7@hash:a",
            "hash:a",
            Vec::new(),
        )
    }

    fn counted() -> InsightFinding {
        InsightFinding::counted(
            RevisionSummary::new(2, 11),
            InsightProvenance::local("revision-summary"),
        )
    }

    #[test]
    fn a_result_is_named_by_its_job_alone() {
        let first = InsightResult::of(&job(), counted(), 500);
        // A second attempt of the same job, with a different finding and moment, still
        // names the same record: the identity the schema refuses to duplicate.
        let retried = InsightResult::of(
            &job(),
            InsightFinding::described(
                RevisionSummary::new(9, 99),
                RevisionTopic::parse("budget", 0.5).unwrap(),
                InsightProvenance::from_model("model", "a-model", "p1", 10, 3),
            ),
            900,
        );

        assert_eq!(
            first.id(),
            "insight-result:insight-job:insight-outbox-v1:file:7@hash:a"
        );
        assert_eq!(retried.id(), first.id());
        assert_eq!(
            first.job_id(),
            "insight-job:insight-outbox-v1:file:7@hash:a"
        );
        assert_eq!(first.source_revision_id(), "file:7@hash:a");
        assert_eq!(first.input_content_hash(), "hash:a");
        assert_eq!(first.finding().input(), RevisionSummary::new(2, 11));
        assert_eq!(first.produced_at_unix_ms(), 500);
    }

    #[test]
    fn a_handler_without_a_model_concludes_nothing_and_bills_nothing() {
        let finding = counted();

        assert!(finding.topic().is_none());
        assert_eq!(finding.provenance().produced_by(), "revision-summary");
        assert_eq!(finding.provenance().model_id(), "");
        assert_eq!(finding.provenance().prompt_version(), "");
        assert_eq!(finding.provenance().input_tokens(), 0);
        assert_eq!(finding.provenance().output_tokens(), 0);
    }

    #[test]
    fn a_finding_from_a_model_records_which_model_and_what_it_cost() {
        let finding = InsightFinding::described(
            RevisionSummary::new(3, 4_096),
            RevisionTopic::parse("quarterly budget", 0.9).unwrap(),
            InsightProvenance::from_model("model", "some-model-id", "revision-topic-v1", 812, 24),
        );

        assert_eq!(finding.topic().unwrap().label(), "quarterly budget");
        assert_eq!(finding.topic().unwrap().confidence().value(), 0.9);
        assert_eq!(finding.provenance().model_id(), "some-model-id");
        assert_eq!(finding.provenance().prompt_version(), "revision-topic-v1");
        assert_eq!(finding.provenance().input_tokens(), 812);
        assert_eq!(finding.provenance().output_tokens(), 24);
        assert_eq!(finding.input().text_bytes(), 4_096);
    }
}
