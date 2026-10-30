// What a job's work actually is, in the two forms this stage has: counting what the graph
// already holds, and asking a model what the revision is about and what it states.
//
// Both are `InsightHandler`s, so the lifecycle of stage 5 does not know which one ran. The
// difference between them is not a branch inside one handler but two types, and only one
// of them can be built at all in a context that refuses to let content leave: the model
// handler takes a `ContentEgressPermit` by value, and a denying context cannot produce one.
// That is why the boundary here is not a check somebody could forget — in a denying
// context the object that would build an outgoing request does not exist.
//
// The order the model handler works in matters. It asks how big the input is before
// reading any of it, so an over-budget revision is refused without its content ever being
// loaded; it looks in the cache before spending anything; and it reads the text to send last,
// at the one point where sending it has already been permitted, budgeted and found to be new.
// A cache hit reads the parts too, and sends nothing: the facts of a cached answer are proved
// against the chunks of the revision that is asking, not of the one that paid.
//
// One call answers two questions: the topic of stage 6, and the entities and facts of the
// knowledge layer (ADR 0048). Asking twice would pay the start of the tool twice — 39 % of a
// call, measured at stage 22 — for content that was already read. The facts are checked here,
// against the parts that were sent, and never on the model's word: a fact whose quote is not in
// its part is refused and counted before anything is written.
use std::cell::RefCell;

use crate::domain::ModelAnswer;
use crate::domain::knowledge::{EntityKey, KnowledgeVocabulary, SourcePart, verify};
use crate::domain::{
    BudgetVerdict, ClaimedInsightJob, ContentEgressPermit, InsightBudget, InsightCacheKey,
    InsightFailureCategory, InsightFinding, InsightProvenance, RevisionSummary, RunSpend,
};

use super::insight_worker::InsightHandler;

/// What the counting handler records as its author.
pub const COUNTED_BY: &str = "revision-summary";

/// What the model handler records as its author. The model that answered is recorded
/// separately, by the provider, because this name is about which handler ran.
pub const DESCRIBED_BY: &str = "revision-topic";

/// The revision a job names, read from the graph. `summarize` never touches chunk text;
/// `read_parts` does, and takes the permit, so no code path can extract a file's content
/// from the graph on behalf of a context that did not allow it to leave.
pub trait RevisionInputReader {
    fn summarize(&self, job: &ClaimedInsightJob)
    -> Result<RevisionSummary, InsightFailureCategory>;

    /// The chunks of the revision in their order, numbered from one: the numbers the request
    /// shows the model and the ones a fact names its part by.
    fn read_parts(
        &self,
        job: &ClaimedInsightJob,
        permit: ContentEgressPermit,
    ) -> Result<Vec<SourcePart>, InsightFailureCategory>;
}

/// A model, named by nothing a vendor would recognize. The request carries the permit, so
/// this port cannot even be called on behalf of a context that refuses egress.
pub trait InsightProvider {
    fn describe(&self, request: InsightRequest<'_>) -> Result<ModelAnswer, InsightFailureCategory>;
}

/// One installation chooses one engine out of several, so the thing that is chosen has to
/// have one type. Boxing it costs one allocation per run, not one per job.
impl InsightProvider for Box<dyn InsightProvider> {
    fn describe(&self, request: InsightRequest<'_>) -> Result<ModelAnswer, InsightFailureCategory> {
        (**self).describe(request)
    }
}

/// What already came back for this exact question. A miss costs nothing; a hit means the
/// same content, pipeline, model and prompt were answered before and are not paid for
/// again.
pub trait InsightCache {
    fn lookup(&self, key: &InsightCacheKey) -> Result<Option<ModelAnswer>, InsightFailureCategory>;

    fn store(
        &self,
        key: &InsightCacheKey,
        answer: &ModelAnswer,
    ) -> Result<(), InsightFailureCategory>;
}

/// How a name becomes the key of its entity. A port, because the reduction is the search
/// index's analyzer: one rule for what two words are the same word, wherever it is asked.
pub trait EntityNames {
    fn key_of(&self, name: &str) -> Option<EntityKey>;
}

/// What is sent. Holding the permit by value is what makes this type impossible to build
/// for a denying context.
pub struct InsightRequest<'a> {
    content: &'a str,
    vocabulary: &'a KnowledgeVocabulary,
    /// Never read, and that is the point: it exists so this type cannot be constructed
    /// without the proof, and a provider that holds one has been handed the permission
    /// along with the content.
    _permit: ContentEgressPermit,
}

impl<'a> InsightRequest<'a> {
    pub fn new(
        content: &'a str,
        vocabulary: &'a KnowledgeVocabulary,
        permit: ContentEgressPermit,
    ) -> Self {
        Self {
            content,
            vocabulary,
            _permit: permit,
        }
    }

    /// The revision's parts, each under its number.
    pub fn content(&self) -> &str {
        self.content
    }

    /// The types the answer must choose from.
    pub fn vocabulary(&self) -> &KnowledgeVocabulary {
        self.vocabulary
    }
}

/// The parts as the model reads them: each under a numbered line of its own. The number is
/// the only name a part is given, and therefore the only thing a fact can cite.
fn numbered_parts(parts: &[SourcePart]) -> String {
    let mut content = String::with_capacity(parts.iter().map(|part| part.text().len() + 16).sum());
    for part in parts {
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(&format!("[part {}]\n", part.number()));
        content.push_str(part.text());
    }
    content
}

/// The handler that needs no model, no key and no network. It is what runs in a context
/// that refuses to let content leave, and it is deliberately unable to conclude anything:
/// it reports what the revision amounted to and stops there.
pub struct CountedInsightHandler<'a, Reader> {
    reader: &'a Reader,
}

impl<'a, Reader: RevisionInputReader> CountedInsightHandler<'a, Reader> {
    pub fn new(reader: &'a Reader) -> Self {
        Self { reader }
    }
}

impl<Reader: RevisionInputReader> InsightHandler for CountedInsightHandler<'_, Reader> {
    fn handle(&self, job: &ClaimedInsightJob) -> Result<InsightFinding, InsightFailureCategory> {
        Ok(InsightFinding::counted(
            self.reader.summarize(job)?,
            InsightProvenance::local(COUNTED_BY),
        ))
    }
}

/// What the model handler is asked and how it reads names: the closed lists, and the key.
pub struct KnowledgeQuestion<'a> {
    vocabulary: &'a KnowledgeVocabulary,
    names: &'a dyn EntityNames,
}

impl<'a> KnowledgeQuestion<'a> {
    pub fn new(vocabulary: &'a KnowledgeVocabulary, names: &'a dyn EntityNames) -> Self {
        Self { vocabulary, names }
    }
}

/// The handler that asks a model. It exists only where a context allowed its content to
/// leave, and it spends only what this run was given.
pub struct DescribedInsightHandler<'a, Reader, Cache, Provider> {
    reader: &'a Reader,
    cache: &'a Cache,
    provider: &'a Provider,
    budget: InsightBudget,
    pipeline_version: &'a str,
    model_id: &'a str,
    prompt_version: &'a str,
    question: KnowledgeQuestion<'a>,
    permit: ContentEgressPermit,
    /// What this run has already sent and what it got back. One pass is one worker on one
    /// thread, so a cell is the whole of the sharing this needs.
    spent: RefCell<RunSpend>,
}

impl<'a, Reader, Cache, Provider> DescribedInsightHandler<'a, Reader, Cache, Provider>
where
    Reader: RevisionInputReader,
    Cache: InsightCache,
    Provider: InsightProvider,
{
    /// `permit` is the context's permission. It is taken by value rather than checked,
    /// which is what makes a denying context unable to construct this handler at all.
    #[allow(
        clippy::too_many_arguments,
        reason = "each argument names a separate decision"
    )]
    pub fn new(
        reader: &'a Reader,
        cache: &'a Cache,
        provider: &'a Provider,
        budget: InsightBudget,
        pipeline_version: &'a str,
        model_id: &'a str,
        prompt_version: &'a str,
        question: KnowledgeQuestion<'a>,
        permit: ContentEgressPermit,
    ) -> Self {
        Self {
            reader,
            cache,
            provider,
            budget,
            pipeline_version,
            model_id,
            prompt_version,
            question,
            permit,
            spent: RefCell::new(RunSpend::default()),
        }
    }

    /// What this handler has sent, heard and kept so far.
    pub fn spent(&self) -> RunSpend {
        *self.spent.borrow()
    }

    fn cache_key(&self, job: &ClaimedInsightJob) -> InsightCacheKey {
        InsightCacheKey::of(
            job.input_content_hash(),
            self.pipeline_version,
            self.model_id,
            self.prompt_version,
            &self.question.vocabulary.fingerprint(),
        )
    }

    /// The answer checked against the parts it was drawn from.
    fn proved(
        &self,
        answer: ModelAnswer,
        input: RevisionSummary,
        parts: &[SourcePart],
    ) -> InsightFinding {
        let names = self.question.names;
        let knowledge = verify(
            answer.knowledge(),
            parts,
            self.question.vocabulary,
            &|name: &str| names.key_of(name),
        );
        self.spent.borrow_mut().record_knowledge(&knowledge.tally());
        answer.about(input, knowledge)
    }
}

impl<Reader, Cache, Provider> DescribedInsightHandler<'_, Reader, Cache, Provider>
where
    Reader: RevisionInputReader,
    Cache: InsightCache,
    Provider: InsightProvider,
{
    /// The finding the cached answer for `job` makes under the rules of this build, or nothing
    /// when no answer is cached. The model is not asked and nothing is sent: a rule about what
    /// proves a fact is applied again to what was already bought.
    pub fn recheck(
        &self,
        job: &ClaimedInsightJob,
    ) -> Result<Option<InsightFinding>, InsightFailureCategory> {
        let input = self.reader.summarize(job)?;
        let Some(answer) = self.cache.lookup(&self.cache_key(job))? else {
            return Ok(None);
        };
        let parts = self.reader.read_parts(job, self.permit)?;
        self.spent.borrow_mut().record_cache_hit();
        Ok(Some(self.proved(answer, input, &parts).served_from_cache()))
    }
}

impl<Reader, Cache, Provider> InsightHandler
    for DescribedInsightHandler<'_, Reader, Cache, Provider>
where
    Reader: RevisionInputReader,
    Cache: InsightCache,
    Provider: InsightProvider,
{
    fn handle(&self, job: &ClaimedInsightJob) -> Result<InsightFinding, InsightFailureCategory> {
        let input = self.reader.summarize(job)?;

        // Asked and answered before, for this exact content, pipeline, model, prompt and
        // vocabulary. Nothing is sent and nothing is spent; the parts are read, because the
        // facts are proved against this revision's own chunks and not against whichever
        // revision paid for the answer.
        let key = self.cache_key(job);
        if let Some(answer) = self.cache.lookup(&key)? {
            let parts = self.reader.read_parts(job, self.permit)?;
            self.spent.borrow_mut().record_cache_hit();
            return Ok(self.proved(answer, input, &parts).served_from_cache());
        }

        // The size is known from the graph, so the refusal happens before the content is
        // loaded, let alone sent.
        match self.budget.admits(&self.spent.borrow(), input.text_bytes()) {
            BudgetVerdict::Admit => {}
            BudgetVerdict::InputTooLarge => return Err(InsightFailureCategory::InputTooLarge),
            BudgetVerdict::RunBudgetSpent => return Err(InsightFailureCategory::BudgetSpent),
        }

        let parts = self.reader.read_parts(job, self.permit)?;
        let content = numbered_parts(&parts);
        let answer = self.provider.describe(InsightRequest::new(
            &content,
            self.question.vocabulary,
            self.permit,
        ))?;
        // Recorded after the call: what was actually sent is what was spent.
        self.spent
            .borrow_mut()
            .record_call(input.text_bytes(), answer.provenance());
        self.cache.store(&key, &answer)?;
        Ok(self.proved(answer, input, &parts))
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::NormalizedPath;
    use std::cell::RefCell;

    use super::{
        COUNTED_BY, CountedInsightHandler, DESCRIBED_BY, DescribedInsightHandler, EntityNames,
        InsightCache, InsightProvider, InsightRequest, KnowledgeQuestion, RevisionInputReader,
        numbered_parts,
    };
    use crate::application::InsightHandler;
    use crate::domain::knowledge::{
        EntityDraft, EntityKey, FactDraft, KnowledgeProposal, KnowledgeVocabulary, SourcePart,
    };
    use crate::domain::{
        ClaimedInsightJob, ContentEgress, ContentEgressPermit, InsightBudget, InsightCacheKey,
        InsightFailureCategory, InsightProvenance, ModelAnswer, RevisionSummary, RevisionTopic,
    };

    const PIPELINE: &str = "insight-outbox-v1";
    const MODEL: &str = "a-model";
    const PROMPT: &str = "revision-topic-v1";
    const QUOTE: &str = "каждый пакет принадлежит одному контрагенту";

    fn permit() -> ContentEgressPermit {
        ContentEgress::Allowed
            .permit()
            .expect("an allowing context has a permit")
    }

    fn vocabulary() -> KnowledgeVocabulary {
        KnowledgeVocabulary::new(vec!["concept".to_string()], vec!["belongs-to".to_string()])
            .unwrap()
    }

    /// Lowered words: enough to show that a fact's ends are found by key.
    struct LoweredNames;

    impl EntityNames for LoweredNames {
        fn key_of(&self, name: &str) -> Option<EntityKey> {
            EntityKey::from_words(name.split_whitespace().map(str::to_lowercase))
        }
    }

    fn job(hash: &str) -> ClaimedInsightJob {
        ClaimedInsightJob::new(
            format!("job:{hash}"),
            format!("file:1@{hash}"),
            hash,
            vec![NormalizedPath::from_absolute("/scan/note.md").unwrap()],
        )
    }

    struct StubReader {
        summary: RevisionSummary,
        text: String,
        reads: RefCell<usize>,
    }

    impl StubReader {
        fn holding(text: &str) -> Self {
            Self {
                summary: RevisionSummary::new(1, text.len() as i64),
                text: text.to_string(),
                reads: RefCell::new(0),
            }
        }
    }

    impl RevisionInputReader for StubReader {
        fn summarize(
            &self,
            _job: &ClaimedInsightJob,
        ) -> Result<RevisionSummary, InsightFailureCategory> {
            Ok(self.summary)
        }

        fn read_parts(
            &self,
            job: &ClaimedInsightJob,
            _permit: ContentEgressPermit,
        ) -> Result<Vec<SourcePart>, InsightFailureCategory> {
            *self.reads.borrow_mut() += 1;
            Ok(vec![SourcePart::new(
                1,
                format!("chunk:{}", job.source_revision_id()),
                self.text.clone(),
            )])
        }
    }

    /// Counts what was sent and what it was asked, so a test can say "nothing left" rather
    /// than "the result looked local".
    #[derive(Default)]
    struct RecordingProvider {
        sent: RefCell<Vec<String>>,
        refuse: Option<InsightFailureCategory>,
        facts: Vec<FactDraft>,
    }

    impl InsightProvider for RecordingProvider {
        fn describe(
            &self,
            request: InsightRequest<'_>,
        ) -> Result<ModelAnswer, InsightFailureCategory> {
            self.sent.borrow_mut().push(request.content().to_string());
            if let Some(category) = self.refuse {
                return Err(category);
            }
            let entities = ["Пакет", "Контрагент"]
                .into_iter()
                .map(|name| EntityDraft {
                    name: name.to_string(),
                    entity_type: "concept".to_string(),
                    description: String::new(),
                })
                .collect();
            Ok(ModelAnswer::new(
                RevisionTopic::parse("budget planning", 0.8).unwrap(),
                InsightProvenance::from_model(DESCRIBED_BY, MODEL, PROMPT, 100, 7).costing(1_500),
            )
            .with_knowledge(KnowledgeProposal::from_drafts(entities, self.facts.clone())))
        }
    }

    fn fact(quote: &str) -> FactDraft {
        FactDraft {
            subject: "пакет".to_string(),
            predicate: "belongs-to".to_string(),
            object: "контрагент".to_string(),
            description: String::new(),
            weight: 6,
            part: 1,
            quote: quote.to_string(),
        }
    }

    #[derive(Default)]
    struct MemoryCache {
        entries: RefCell<Vec<(InsightCacheKey, ModelAnswer)>>,
        lookups: RefCell<usize>,
    }

    impl InsightCache for MemoryCache {
        fn lookup(
            &self,
            key: &InsightCacheKey,
        ) -> Result<Option<ModelAnswer>, InsightFailureCategory> {
            *self.lookups.borrow_mut() += 1;
            Ok(self
                .entries
                .borrow()
                .iter()
                .find(|(stored, _)| stored == key)
                .map(|(_, answer)| answer.clone()))
        }

        fn store(
            &self,
            key: &InsightCacheKey,
            answer: &ModelAnswer,
        ) -> Result<(), InsightFailureCategory> {
            self.entries
                .borrow_mut()
                .push((key.clone(), answer.clone()));
            Ok(())
        }
    }

    struct Fixture {
        vocabulary: KnowledgeVocabulary,
        names: LoweredNames,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                vocabulary: vocabulary(),
                names: LoweredNames,
            }
        }

        fn described<'a>(
            &'a self,
            reader: &'a StubReader,
            cache: &'a MemoryCache,
            provider: &'a RecordingProvider,
            budget: InsightBudget,
        ) -> DescribedInsightHandler<'a, StubReader, MemoryCache, RecordingProvider> {
            DescribedInsightHandler::new(
                reader,
                cache,
                provider,
                budget,
                PIPELINE,
                MODEL,
                PROMPT,
                KnowledgeQuestion::new(&self.vocabulary, &self.names),
                permit(),
            )
        }
    }

    #[test]
    fn the_counting_handler_reports_the_input_and_concludes_nothing() {
        let reader = StubReader::holding("hello");

        let finding = CountedInsightHandler::new(&reader)
            .handle(&job("hash:a"))
            .unwrap();

        assert_eq!(finding.input(), RevisionSummary::new(1, 5));
        assert!(finding.topic().is_none());
        assert!(finding.knowledge().is_none());
        assert_eq!(finding.provenance().produced_by(), COUNTED_BY);
        assert_eq!(finding.provenance().model_id(), "");
        assert_eq!(
            *reader.reads.borrow(),
            0,
            "the counting handler must never read chunk text"
        );
    }

    #[test]
    fn the_model_handler_sends_numbered_parts_and_records_what_answered() {
        let reader = StubReader::holding("quarterly budget notes");
        let cache = MemoryCache::default();
        let provider = RecordingProvider::default();
        let fixture = Fixture::new();

        let handler = fixture.described(
            &reader,
            &cache,
            &provider,
            InsightBudget::new(1_000, 10_000),
        );
        let finding = handler.handle(&job("hash:a")).unwrap();

        assert_eq!(
            *provider.sent.borrow(),
            vec!["[part 1]\nquarterly budget notes"]
        );
        assert_eq!(finding.topic().unwrap().label(), "budget planning");
        assert_eq!(finding.provenance().produced_by(), DESCRIBED_BY);
        assert_eq!(finding.provenance().model_id(), MODEL);
        assert_eq!(finding.input(), RevisionSummary::new(1, 22));
        let spent = handler.spent();
        assert_eq!(spent.calls(), 1);
        assert_eq!(spent.bytes(), 22);
        assert_eq!(spent.cost_micro_usd(), 1_500);
    }

    #[test]
    fn a_fact_is_kept_only_when_its_quote_is_in_the_part_it_names() {
        let reader = StubReader::holding(&format!("Правило: {QUOTE}."));
        let cache = MemoryCache::default();
        let provider = RecordingProvider {
            facts: vec![fact(QUOTE), fact("пакет иногда принадлежит двум")],
            ..RecordingProvider::default()
        };
        let fixture = Fixture::new();

        let handler = fixture.described(
            &reader,
            &cache,
            &provider,
            InsightBudget::new(1_000, 10_000),
        );
        let finding = handler.handle(&job("hash:a")).unwrap();
        let knowledge = finding.knowledge().unwrap();

        assert_eq!(knowledge.facts().len(), 1);
        assert_eq!(knowledge.facts()[0].chunk_id(), "chunk:file:1@hash:a");
        assert_eq!(knowledge.facts()[0].quote(), QUOTE);
        assert_eq!(knowledge.tally().facts_rejected_by_quote, 1);
        assert_eq!(handler.spent().tally().facts_rejected_by_quote, 1);
    }

    /// A cached answer is checked again, against the revision that is asking: the chunk a
    /// fact points at must be this revision's, not the one that paid.
    #[test]
    fn a_second_job_with_the_same_content_is_not_paid_for_twice_and_is_proved_again() {
        let reader = StubReader::holding(QUOTE);
        let cache = MemoryCache::default();
        let provider = RecordingProvider {
            facts: vec![fact(QUOTE)],
            ..RecordingProvider::default()
        };
        let fixture = Fixture::new();
        let handler = fixture.described(
            &reader,
            &cache,
            &provider,
            InsightBudget::new(1_000, 10_000),
        );

        let first = handler.handle(&job("hash:a")).unwrap();
        let second = handler
            .handle(&ClaimedInsightJob::new(
                "job:other",
                "file:2@hash:a",
                "hash:a",
                vec![],
            ))
            .unwrap();

        assert_eq!(
            provider.sent.borrow().len(),
            1,
            "the provider must be asked once"
        );
        assert_eq!(first.topic(), second.topic());
        assert_eq!(second.provenance().model_id(), MODEL);
        assert_eq!(
            second.knowledge().unwrap().facts()[0].chunk_id(),
            "chunk:file:2@hash:a"
        );
        assert_eq!(handler.spent().calls(), 1);
        assert_eq!(handler.spent().cache_hits(), 1);
        assert!(!first.answered_from_cache());
        assert!(second.answered_from_cache(), "the second job paid nothing");
    }

    #[test]
    fn a_revision_larger_than_one_job_may_send_is_refused_without_being_read() {
        let reader = StubReader::holding("a rather long document");
        let cache = MemoryCache::default();
        let provider = RecordingProvider::default();
        let fixture = Fixture::new();

        let refusal = fixture
            .described(&reader, &cache, &provider, InsightBudget::new(5, 10_000))
            .handle(&job("hash:a"))
            .unwrap_err();

        assert_eq!(refusal, InsightFailureCategory::InputTooLarge);
        assert!(provider.sent.borrow().is_empty());
        assert_eq!(*reader.reads.borrow(), 0, "nothing may have been read");
    }

    #[test]
    fn a_run_that_has_spent_its_budget_stops_asking_without_blaming_the_job() {
        let reader = StubReader::holding("0123456789");
        let cache = MemoryCache::default();
        let provider = RecordingProvider::default();
        let fixture = Fixture::new();
        let handler = fixture.described(&reader, &cache, &provider, InsightBudget::new(10, 10));

        handler.handle(&job("hash:a")).unwrap();
        let refusal = handler.handle(&job("hash:b")).unwrap_err();

        assert_eq!(refusal, InsightFailureCategory::BudgetSpent);
        assert_eq!(provider.sent.borrow().len(), 1);
        assert_eq!(*reader.reads.borrow(), 1);
    }

    #[test]
    fn a_provider_refusal_reaches_the_lifecycle_as_its_category() {
        let reader = StubReader::holding("notes");
        let cache = MemoryCache::default();
        let provider = RecordingProvider {
            refuse: Some(InsightFailureCategory::Schema),
            ..RecordingProvider::default()
        };
        let fixture = Fixture::new();

        let refusal = fixture
            .described(&reader, &cache, &provider, InsightBudget::new(100, 100))
            .handle(&job("hash:a"))
            .unwrap_err();

        assert_eq!(refusal, InsightFailureCategory::Schema);
        assert!(
            cache.entries.borrow().is_empty(),
            "a refusal must not be cached as though it were an answer"
        );
    }

    #[test]
    fn parts_are_sent_each_under_its_own_number() {
        let parts = [
            SourcePart::new(1, "c0", "первая"),
            SourcePart::new(2, "c1", "вторая"),
        ];

        assert_eq!(
            numbered_parts(&parts),
            "[part 1]\nпервая\n\n[part 2]\nвторая"
        );
    }

    /// A denying context has no permit, so the model handler has no constructor argument
    /// to be built from. This is the whole of the boundary: not a check, an absence.
    #[test]
    fn a_denying_context_cannot_produce_the_permit_the_model_handler_needs() {
        assert!(ContentEgress::Denied.permit().is_none());
        assert!(ContentEgress::default().permit().is_none());
    }
}
