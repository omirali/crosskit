// Use case: answer a question about one context from that context's own text.
//
// The loop this closes is short and every step of it is bounded. The index says which
// chunks look relevant; the graph says which of them this run may show and what they say;
// the budget says how much of that may leave the machine; the model answers; and what it
// answered is checked before anybody reads it.
//
// Two things here are not ordinary plumbing.
//
// The permit. A context that did not allow its content to leave cannot produce one, and the
// request type holds one by value — so in a denying context the object that would carry a
// question outside does not exist. There is no flag to forget and no branch to get wrong;
// the command refuses earlier, with the reason, because it cannot build this at all.
//
// The sources. The model is handed excerpts numbered from one and nothing else: no path, no
// file name, no identifier. It cites numbers, and the numbers are looked up in this
// program's own list of what it sent. A model cannot name a file it was not given, because
// it was never told the name of one it was.
use crate::domain::{
    Answer, AskBudget, AskCacheKey, AskError, ContentEgressPermit, NormalizedPath, Period, Question,
};

use super::ask_ports::{AnswerCache, AnswerProvider, AnswerRequest, AskFailure, Excerpt};
use super::period_scope::PeriodScope;
use super::search::{SearchRequest, SearchService, SearchSource};

/// A question about a period: what to look for once the words that named the period are
/// taken out, and what the period admits.
///
/// The words are given rather than derived here because the phrase that named the period is
/// language data, and this layer does not read the language packs. A question without a period
/// is asked with no `PeriodQuestion` at all, and then looks for exactly what it always did.
#[derive(Debug, Clone, Copy)]
pub struct PeriodQuestion<'a> {
    words: &'a str,
    scope: &'a PeriodScope,
}

impl<'a> PeriodQuestion<'a> {
    pub fn new(words: &'a str, scope: &'a PeriodScope) -> Self {
        Self { words, scope }
    }
}
use super::search_ports::{SearchGraphReader, SearchIndexReader};

/// What one question cost and what it answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskReport {
    answer: Answer,
    sources: Vec<Excerpt>,
    bytes_sent: usize,
    budget: AskBudget,
    from_cache: bool,
}

impl AskReport {
    pub fn answer(&self) -> &Answer {
        &self.answer
    }

    /// Every excerpt the question carried, in the order they were numbered.
    pub fn sources(&self) -> &[Excerpt] {
        &self.sources
    }

    /// The ones the answer cited, in this program's own words about where they came from.
    pub fn cited(&self) -> Vec<&Excerpt> {
        self.answer
            .used()
            .iter()
            .filter_map(|ordinal| self.sources.get(ordinal - 1))
            .collect()
    }

    pub fn bytes_sent(&self) -> usize {
        self.bytes_sent
    }

    /// What the operator allowed one question to take, so a report can say how much of it
    /// this question used.
    pub fn budget(&self) -> AskBudget {
        self.budget
    }

    /// Whether this answer came back from the cache rather than from the model.
    pub fn answered_from_cache(&self) -> bool {
        self.from_cache
    }
}

pub struct AskService<'a, Index, Graph, Provider, Cache> {
    index: &'a Index,
    graph: &'a Graph,
    provider: &'a Provider,
    cache: &'a Cache,
    budget: AskBudget,
    model_id: &'a str,
    prompt_version: &'a str,
    permit: ContentEgressPermit,
}

impl<'a, Index, Graph, Provider, Cache> AskService<'a, Index, Graph, Provider, Cache>
where
    Index: SearchIndexReader,
    Graph: SearchGraphReader,
    Provider: AnswerProvider,
    Cache: AnswerCache,
{
    /// `permit` is the context's permission, taken by value rather than checked.
    #[allow(
        clippy::too_many_arguments,
        reason = "each argument names a separate decision"
    )]
    pub fn new(
        index: &'a Index,
        graph: &'a Graph,
        provider: &'a Provider,
        cache: &'a Cache,
        budget: AskBudget,
        model_id: &'a str,
        prompt_version: &'a str,
        permit: ContentEgressPermit,
    ) -> Self {
        Self {
            index,
            graph,
            provider,
            cache,
            budget,
            model_id,
            prompt_version,
            permit,
        }
    }

    /// `period` is the period the question is about, when it is about one. Nothing outside
    /// it is sent; when nothing inside it matches, nothing is sent at all and the failure says
    /// so, rather than the question being answered from another week.
    pub fn execute(
        &self,
        question: &Question,
        period: Option<PeriodQuestion<'_>>,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<AskReport, AskFailure> {
        let excerpts = self.gather(question, period, is_visible)?;
        if excerpts.is_empty() {
            return Err(match period {
                Some(period) => AskFailure::NothingInPeriod {
                    files_with_events: period.scope.files_with_events(),
                },
                None => AskFailure::NothingToAnswerFrom,
            });
        }
        let bytes_sent = excerpts.iter().map(|excerpt| excerpt.text().len()).sum();
        let about = period.map(|period| period.scope.period());
        let not_shown = period.and_then(|period| period.scope.gaps().sentence());

        // The same question over the same excerpts, asked of the same model with the same
        // wording, was answered before. Nothing is sent and nothing is spent.
        let key = self.cache_key(question, &excerpts, about);
        if let Some(answer) = self.cache.lookup(&key)? {
            return Ok(AskReport {
                answer,
                sources: excerpts,
                bytes_sent,
                budget: self.budget,
                from_cache: true,
            });
        }

        let answer = self
            .provider
            .answer(AnswerRequest::new(question, &excerpts, self.permit).about(about, not_shown))?;
        self.cache.store(&key, &answer)?;
        Ok(AskReport {
            answer,
            sources: excerpts,
            bytes_sent,
            budget: self.budget,
            from_cache: false,
        })
    }

    /// The excerpts one question carries: the best matches this run may show, in rank order,
    /// cut to the budget.
    ///
    /// The budget is spent in the order the index ranked them, so what is dropped is what
    /// matched least. An excerpt that does not fit whole is cut rather than dropped — part
    /// of the paragraph that matched best answers better than the whole of the next one.
    fn gather(
        &self,
        question: &Question,
        period: Option<PeriodQuestion<'_>>,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<Vec<Excerpt>, AskFailure> {
        // The words of the question, not the question: a query parser reads punctuation as
        // instructions, and a person asking about `ContentEgressPermit:` is not writing one.
        // About a period, the words without the phrase that named it — Stage 29's frame.
        let words = match period {
            Some(period) => period.words.to_string(),
            None => question.search_words(),
        };
        let request = SearchRequest::new(words, self.budget.max_sources(), None, false)
            .map_err(|_| AskFailure::Shape(AskError::EmptyQuestion))?;
        let found: Vec<SearchSource> = SearchService::new(self.index, self.graph)
            .sources(&request, period.map(|period| period.scope), is_visible)
            .map_err(AskFailure::Search)?;

        let mut excerpts = Vec::with_capacity(found.len());
        let mut bytes = 0usize;
        for source in found {
            if !self
                .budget
                .admits(excerpts.len(), bytes, source.text().len())
            {
                break;
            }
            let room = self.budget.remaining_bytes(bytes).max(1);
            let text = cut_to(source.text(), room);
            bytes += text.len();
            excerpts.push(
                Excerpt::new(
                    excerpts.len() + 1,
                    source.chunk_id(),
                    source.path().clone(),
                    text,
                    source.topic().map(str::to_string),
                )
                .admitted_by(source.event()),
            );
        }
        Ok(excerpts)
    }

    fn cache_key(
        &self,
        question: &Question,
        excerpts: &[Excerpt],
        period: Option<&Period>,
    ) -> AskCacheKey {
        let sources: Vec<(&str, usize)> = excerpts
            .iter()
            .map(|excerpt| (excerpt.chunk_id(), excerpt.text().len()))
            .collect();
        AskCacheKey::of(
            question,
            self.model_id,
            self.prompt_version,
            &sources,
            period,
        )
    }
}

/// `text`, cut to at most `bytes` without splitting a character.
fn cut_to(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_string();
    }
    let mut end = bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{AnswerCache, AnswerProvider, AnswerRequest, AskFailure, AskService};
    use crate::application::PeriodScope;
    use crate::application::RepositoryError;
    use crate::application::search_ports::{
        DocumentMatch, MatchFragment, MatchedDocuments, ResolvedChunk, SearchDocument,
        SearchGraphReader, SearchIndexError, SearchIndexReader, SearchIndexState,
    };
    use crate::domain::{
        Answer, AskBudget, AskCacheKey, ContentEgress, ContentEgressPermit, DocumentCursor,
        IndexFingerprint, LocationStatus, NormalizationSet, NormalizedPath, Question,
    };

    fn permit() -> ContentEgressPermit {
        ContentEgress::Allowed.permit().unwrap()
    }

    fn fingerprint() -> IndexFingerprint {
        IndexFingerprint::of(
            &["text"],
            1,
            "test/1",
            &NormalizationSet::new(Vec::new()).unwrap(),
        )
    }

    /// An index whose rows are scripted, in the order they are written. One row per file,
    /// and a chunk id standing for the file it belongs to, which is what a question carries.
    struct FakeIndex {
        matched: Vec<(String, f32)>,
    }

    impl SearchIndexReader for FakeIndex {
        fn matches(
            &self,
            _query: &str,
            limit: usize,
            after: Option<&DocumentCursor>,
            within: Option<&PeriodScope>,
        ) -> Result<MatchedDocuments, SearchIndexError> {
            let start = after
                .map(|cursor| {
                    self.matched
                        .iter()
                        .position(|(id, _)| id == cursor.file_id())
                        .map_or(0, |position| position + 1)
                })
                .unwrap_or(0);
            Ok(MatchedDocuments::new(
                self.matched
                    .iter()
                    .skip(start)
                    .filter(|(id, _)| within.is_none_or(|scope| scope.admits(id, id)))
                    .take(limit)
                    .map(|(id, score)| DocumentMatch::new(id, 1, DocumentCursor::new(*score, id)))
                    .collect(),
            ))
        }

        fn fragments(
            &self,
            _query: &str,
            texts: &[String],
        ) -> Result<Vec<MatchFragment>, SearchIndexError> {
            Ok(texts
                .iter()
                .map(|text| MatchFragment::new(text.clone(), Vec::new()))
                .collect())
        }

        fn state(&self) -> Result<SearchIndexState, SearchIndexError> {
            Ok(SearchIndexState::new(
                self.matched.len() as u64,
                Some(fingerprint()),
                fingerprint(),
            ))
        }
    }

    /// A graph that knows the text and one present location of each chunk.
    struct FakeGraph {
        chunks: HashMap<String, (String, String)>,
    }

    impl FakeGraph {
        fn holding(chunks: &[(&str, &str, &str)]) -> Self {
            Self {
                chunks: chunks
                    .iter()
                    .map(|(id, path, text)| {
                        (
                            (*id).to_string(),
                            ((*path).to_string(), (*text).to_string()),
                        )
                    })
                    .collect(),
            }
        }
    }

    impl SearchGraphReader for FakeGraph {
        fn current_documents(
            &self,
            _after_chunk_id: &str,
            _limit: usize,
        ) -> Result<Vec<SearchDocument>, RepositoryError> {
            unreachable!("asking does not rebuild")
        }

        fn current_versions(
            &self,
            _file_ids: &[String],
        ) -> Result<HashMap<String, String>, RepositoryError> {
            unreachable!("asking does not publish")
        }

        fn current_chunk_count(&self) -> Result<u64, RepositoryError> {
            unreachable!("asking does not count")
        }

        fn resolve(&self, chunk_ids: &[String]) -> Result<Vec<ResolvedChunk>, RepositoryError> {
            Ok(chunk_ids
                .iter()
                .filter_map(|id| {
                    let (path, text) = self.chunks.get(id)?;
                    Some(ResolvedChunk::new(
                        id,
                        text,
                        vec![(
                            NormalizedPath::from_absolute(path).unwrap(),
                            LocationStatus::Present,
                        )],
                        None,
                    ))
                })
                .collect())
        }
    }

    /// One question as the model received it: the words, and the numbered excerpts.
    type Asked = (String, Vec<(usize, String)>);

    /// A model that answers from a script and records exactly what it was given.
    struct FakeProvider {
        answer: RefCell<Result<(String, Vec<i64>), AskFailure>>,
        seen: RefCell<Vec<Asked>>,
    }

    impl FakeProvider {
        fn answering(text: &str, sources: Vec<i64>) -> Self {
            Self {
                answer: RefCell::new(Ok((text.to_string(), sources))),
                seen: RefCell::new(Vec::new()),
            }
        }

        fn failing(failure: AskFailure) -> Self {
            Self {
                answer: RefCell::new(Err(failure)),
                seen: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.seen.borrow().len()
        }
    }

    impl AnswerProvider for FakeProvider {
        fn answer(&self, request: AnswerRequest<'_>) -> Result<Answer, AskFailure> {
            self.seen.borrow_mut().push((
                request.question().as_str().to_string(),
                request
                    .excerpts()
                    .iter()
                    .map(|excerpt| (excerpt.ordinal(), excerpt.text().to_string()))
                    .collect(),
            ));
            let scripted = self.answer.borrow().clone();
            let (text, sources) = scripted?;
            Answer::parse(&text, &sources, request.excerpts().len()).map_err(|_| AskFailure::Schema)
        }
    }

    #[derive(Default)]
    struct FakeCache {
        entries: RefCell<HashMap<String, Answer>>,
        stores: RefCell<usize>,
    }

    impl AnswerCache for FakeCache {
        fn lookup(&self, key: &AskCacheKey) -> Result<Option<Answer>, AskFailure> {
            Ok(self.entries.borrow().get(key.as_str()).cloned())
        }

        fn store(&self, key: &AskCacheKey, answer: &Answer) -> Result<(), AskFailure> {
            *self.stores.borrow_mut() += 1;
            self.entries
                .borrow_mut()
                .insert(key.as_str().to_string(), answer.clone());
            Ok(())
        }
    }

    struct Fixture {
        index: FakeIndex,
        graph: FakeGraph,
        cache: FakeCache,
    }

    fn fixture(chunks: &[(&str, &str, &str)]) -> Fixture {
        Fixture {
            index: FakeIndex {
                matched: chunks
                    .iter()
                    .enumerate()
                    .map(|(position, (id, _, _))| {
                        ((*id).to_string(), 1.0 - position as f32 / 100.0)
                    })
                    .collect(),
            },
            graph: FakeGraph::holding(chunks),
            cache: FakeCache::default(),
        }
    }

    fn ask<'a>(
        fixture: &'a Fixture,
        provider: &'a FakeProvider,
        budget: AskBudget,
        question: &str,
    ) -> Result<super::AskReport, AskFailure> {
        AskService::new(
            &fixture.index,
            &fixture.graph,
            provider,
            &fixture.cache,
            budget,
            "a-model",
            "ask-v1",
            permit(),
        )
        .execute(&Question::parse(question).unwrap(), None, &|_| true)
    }

    const CHUNKS: &[(&str, &str, &str)] = &[
        ("chunk:a", "/files/one.md", "первая выдержка про сверку"),
        ("chunk:b", "/files/two.md", "вторая выдержка про индекс"),
        ("chunk:c", "/files/three.md", "третья выдержка про очередь"),
    ];

    /// The ordinary case: what matched is sent, numbered from one, and the answer names its
    /// sources by this program's own list.
    #[test]
    fn the_answer_names_sources_from_the_list_this_program_sent() {
        let fixture = fixture(CHUNKS);
        let provider = FakeProvider::answering("сверка обходит дерево", vec![2]);

        let report = ask(
            &fixture,
            &provider,
            AskBudget::new(8, 40_000),
            "что делает сверка",
        )
        .unwrap();

        assert_eq!(report.answer().text(), "сверка обходит дерево");
        assert_eq!(report.sources().len(), 3);
        let cited: Vec<String> = report
            .cited()
            .iter()
            .map(|excerpt| excerpt.path().as_str().to_string())
            .collect();
        assert_eq!(
            cited,
            ["/files/two.md"],
            "the second excerpt is the second path"
        );
        assert!(!report.answered_from_cache());
    }

    /// The budget decides how much leaves: the count first, and then the bytes. What is
    /// dropped is what matched least.
    #[test]
    fn the_budget_bounds_how_many_excerpts_and_how_many_bytes_leave() {
        let counted = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![]);

        let report = ask(&counted, &provider, AskBudget::new(2, 40_000), "вопрос").unwrap();
        assert_eq!(report.sources().len(), 2);
        assert_eq!(
            report.sources()[0].path().as_str(),
            "/files/one.md",
            "in the order the index ranked them"
        );

        let bounded = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![]);
        let tight = ask(&bounded, &provider, AskBudget::new(8, 60), "вопрос").unwrap();
        assert!(tight.bytes_sent() <= 60, "{}", tight.bytes_sent());
        assert!(!tight.sources().is_empty(), "one excerpt always fits");
    }

    /// A question whose best source is one long paragraph is answered from part of it rather
    /// than from nothing.
    #[test]
    fn the_first_excerpt_is_cut_to_the_budget_rather_than_dropped() {
        let long = "а".repeat(10_000);
        let chunks = [("chunk:a", "/files/one.md", long.as_str())];
        let fixture = fixture(&chunks);
        let provider = FakeProvider::answering("ответ", vec![1]);

        let report = ask(&fixture, &provider, AskBudget::new(8, 100), "вопрос").unwrap();

        assert_eq!(report.sources().len(), 1);
        assert!(report.bytes_sent() <= 100, "{}", report.bytes_sent());
        assert!(!report.sources()[0].text().is_empty());
    }

    /// Nothing matched: nothing is sent, nothing is spent, and the caller is told which of
    /// the two things happened.
    #[test]
    fn a_question_nothing_matches_sends_nothing() {
        let fixture = fixture(&[]);
        let provider = FakeProvider::answering("ответ", vec![]);

        let failure = ask(&fixture, &provider, AskBudget::new(8, 40_000), "вопрос").unwrap_err();

        assert_eq!(failure, AskFailure::NothingToAnswerFrom);
        assert_eq!(provider.calls(), 0);
    }

    /// The same question over the same sources is not paid for twice.
    #[test]
    fn the_same_question_over_the_same_sources_is_answered_from_the_cache() {
        let fixture = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![1]);
        let budget = AskBudget::new(8, 40_000);

        let first = ask(&fixture, &provider, budget, "что делает сверка").unwrap();
        let second = ask(&fixture, &provider, budget, "что делает сверка").unwrap();

        assert_eq!(provider.calls(), 1, "the model was asked once");
        assert_eq!(first.answer(), second.answer());
        assert!(!first.answered_from_cache() && second.answered_from_cache());
        assert_eq!(*fixture.cache.stores.borrow(), 1);
    }

    /// Another question is another key, and the model is asked again.
    #[test]
    fn another_question_is_not_answered_from_the_cache() {
        let fixture = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![1]);
        let budget = AskBudget::new(8, 40_000);

        ask(&fixture, &provider, budget, "первый вопрос").unwrap();
        ask(&fixture, &provider, budget, "второй вопрос").unwrap();

        assert_eq!(provider.calls(), 2);
    }

    /// A provider that fails fails the question. Nothing is cached, and the category is what
    /// crosses the boundary — never what the provider wrote.
    #[test]
    fn a_failing_provider_is_a_category_and_nothing_is_cached() {
        for failure in [
            AskFailure::Unavailable,
            AskFailure::Refused,
            AskFailure::Schema,
        ] {
            let asked = fixture(CHUNKS);
            let provider = FakeProvider::failing(failure.clone());

            let met = ask(&asked, &provider, AskBudget::new(8, 40_000), "вопрос").unwrap_err();

            assert_eq!(met, failure);
            assert_eq!(*asked.cache.stores.borrow(), 0);
        }
    }

    /// What the model is given: the question and the excerpts, numbered from one. The paths
    /// stay here.
    #[test]
    fn the_model_is_given_the_question_and_numbered_excerpts() {
        let fixture = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![]);

        ask(
            &fixture,
            &provider,
            AskBudget::new(8, 40_000),
            "что делает сверка",
        )
        .unwrap();

        let seen = provider.seen.borrow();
        let (question, excerpts) = &seen[0];
        assert_eq!(question, "что делает сверка");
        assert_eq!(
            excerpts,
            &vec![
                (1, "первая выдержка про сверку".to_string()),
                (2, "вторая выдержка про индекс".to_string()),
                (3, "третья выдержка про очередь".to_string()),
            ]
        );
    }

    /// A path this run may not show is not sent either: the same rule that keeps it out of a
    /// page keeps it out of a question.
    #[test]
    fn a_path_this_run_may_not_show_is_never_sent() {
        let fixture = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![]);

        let report = AskService::new(
            &fixture.index,
            &fixture.graph,
            &provider,
            &fixture.cache,
            AskBudget::new(8, 40_000),
            "a-model",
            "ask-v1",
            permit(),
        )
        .execute(
            &Question::parse("вопрос").unwrap(),
            None,
            &|path: &NormalizedPath| !path.as_str().ends_with("two.md"),
        )
        .unwrap();

        let paths: Vec<&str> = report
            .sources()
            .iter()
            .map(|excerpt| excerpt.path().as_str())
            .collect();
        assert_eq!(paths, ["/files/one.md", "/files/three.md"]);
        let sent: Vec<String> = provider.seen.borrow()[0]
            .1
            .iter()
            .map(|(_, text)| text.clone())
            .collect();
        assert!(
            !sent.iter().any(|text| text.contains("индекс")),
            "the hidden file's text left the machine: {sent:?}"
        );
    }

    fn last_week() -> crate::domain::Period {
        crate::domain::Period::of_phrase(
            crate::domain::PeriodPhrase::LastWeek,
            crate::domain::LocalDay::new(
                crate::domain::CivilDate::new(2026, 10, 1).unwrap(),
                5 * 3600,
            ),
        )
    }

    fn ask_within<'a>(
        fixture: &'a Fixture,
        provider: &'a FakeProvider,
        question: &str,
        words: &str,
        scope: &PeriodScope,
    ) -> Result<super::AskReport, AskFailure> {
        AskService::new(
            &fixture.index,
            &fixture.graph,
            provider,
            &fixture.cache,
            AskBudget::new(8, 40_000),
            "a-model",
            "ask-v1",
            permit(),
        )
        .execute(
            &Question::parse(question).unwrap(),
            Some(super::PeriodQuestion::new(words, scope)),
            &|_| true,
        )
    }

    /// Only what the period admits is sent, and each excerpt carries the event that put it
    /// there.
    #[test]
    fn a_question_about_a_period_sends_only_what_the_period_admits() {
        let fixture = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![1]);
        let scope = PeriodScope::admitting_chunks(last_week(), &[("chunk:b", 7)]);

        let report = ask_within(
            &fixture,
            &provider,
            "что было на прошлой неделе",
            "что было",
            &scope,
        )
        .unwrap();

        let sent: Vec<&str> = report
            .sources()
            .iter()
            .map(|excerpt| excerpt.path().as_str())
            .collect();
        assert_eq!(sent, ["/files/two.md"]);
        assert_eq!(
            report.sources()[0].event(),
            Some(crate::application::PeriodEvent::Reply { at_unix_ms: 7 })
        );
    }

    /// A period with nothing in it that matches sends nothing, and says it was the period.
    #[test]
    fn a_period_nothing_inside_matches_sends_nothing() {
        let fixture = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![]);
        let scope = PeriodScope::admitting_chunks(last_week(), &[("chunk:elsewhere", 7)]);

        let failure = ask_within(
            &fixture,
            &provider,
            "что было на прошлой неделе",
            "что было",
            &scope,
        )
        .unwrap_err();

        assert_eq!(
            failure,
            AskFailure::NothingInPeriod {
                files_with_events: 1
            }
        );
        assert_eq!(provider.calls(), 0);
    }

    /// The same question over the same sources, once without a period and once with one, is
    /// two pieces of work: the model was told different things.
    #[test]
    fn a_period_is_part_of_what_the_cache_remembers() {
        let fixture = fixture(CHUNKS);
        let provider = FakeProvider::answering("ответ", vec![1]);
        let everything = PeriodScope::admitting_chunks(
            last_week(),
            &[("chunk:a", 1), ("chunk:b", 1), ("chunk:c", 1)],
        );

        ask(&fixture, &provider, AskBudget::new(8, 40_000), "вопрос").unwrap();
        let within = ask_within(&fixture, &provider, "вопрос", "вопрос", &everything).unwrap();

        assert_eq!(provider.calls(), 2);
        assert!(!within.answered_from_cache());
    }
}
