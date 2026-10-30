// Model analysis against the real database: what the reader gives a handler, what the cache
// keeps, and every way a job can end when a model is involved.
//
// The provider here is a fake, on purpose. Everything this stage has to be sure of — that a
// refusal is permanent, that a timeout is retried, that an over-budget revision is never
// read, that a second identical input is not paid for — is a property of this program, and
// a test that needed the network would prove less about it, not more. The one test that
// calls a real model is separate, ignored, and not part of any gate.
use std::cell::RefCell;
use std::sync::Arc;

use lbug::Value;

use super::test_support::{
    TestDatabase, evidence_commit, evidence_commit_revision, query_int64, query_string,
};
use super::{LadybugDatabaseManager, LadybugInsightCache, LadybugRevisionInput, repository};
use crate::application::JobScope;
use crate::application::{
    COUNTED_BY, CountedInsightHandler, DESCRIBED_BY, DescribedInsightHandler, InsightCache,
    InsightJobQueue, InsightProvider, InsightRequest, InsightWorkerService,
    KnowledgeGraphRepository, KnowledgeQuestion, RevisionInputReader,
};
use crate::domain::knowledge::KnowledgeVocabulary;
use crate::domain::{
    ClaimedInsightJob, ContentEgress, ContentEgressPermit, InsightBudget, InsightCacheKey,
    InsightFailureCategory, InsightLease, InsightProvenance, InsightRetryPolicy, ModelAnswer,
    RevisionSummary, RevisionTopic,
};
use crate::infrastructure::search::{AnalyzedEntityNames, built_in_normalizations};

const KEPT: &str = "/data/docs/kept.md";
const JOB_A: &str = "insight-job:insight-outbox-v1:file:kept@hash:a";
const PIPELINE: &str = "insight-outbox-v1";
const MODEL: &str = "a-model";
const PROMPT: &str = "revision-topic-v1";
const HELD: InsightLease = InsightLease::of_milliseconds(600_000);

fn permit() -> ContentEgressPermit {
    ContentEgress::Allowed.permit().unwrap()
}

fn vocabulary() -> KnowledgeVocabulary {
    KnowledgeVocabulary::new(vec!["concept".to_string()], vec!["related-to".to_string()]).unwrap()
}

fn opened(
    test_name: &str,
) -> (
    TestDatabase,
    Arc<LadybugDatabaseManager>,
    repository::LadybugGraphRepository,
) {
    let database = TestDatabase::new(test_name);
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit("file:kept", KEPT))
        .unwrap();
    (database, manager, repository)
}

fn claimed(repository: &repository::LadybugGraphRepository) -> ClaimedInsightJob {
    repository
        .claim_insight_jobs(1, HELD, JobScope::Everything)
        .unwrap()[0]
        .clone()
}

fn result_text(manager: &LadybugDatabaseManager, field: &str) -> String {
    query_string(
        manager,
        &format!("MATCH (result:InsightResult) RETURN result.{field};"),
        vec![],
    )
}

fn result_number(manager: &LadybugDatabaseManager, field: &str) -> i64 {
    query_int64(
        manager,
        &format!("MATCH (result:InsightResult) RETURN result.{field};"),
        vec![],
    )
}

fn job_text(manager: &LadybugDatabaseManager, field: &str) -> String {
    query_string(
        manager,
        &format!("MATCH (job:InsightJob {{id: $id}}) RETURN job.{field};"),
        vec![("id", Value::String(JOB_A.to_string()))],
    )
}

fn cache_entries(manager: &LadybugDatabaseManager) -> i64 {
    query_int64(
        manager,
        "MATCH (:InsightCacheEntry) RETURN COUNT(*);",
        vec![],
    )
}

/// A provider that answers, or refuses with a chosen category, and counts what it was sent.
struct FakeProvider {
    answer: Option<InsightFailureCategory>,
    sent: RefCell<Vec<String>>,
}

impl FakeProvider {
    fn answering() -> Self {
        Self {
            answer: None,
            sent: RefCell::new(Vec::new()),
        }
    }

    fn refusing(category: InsightFailureCategory) -> Self {
        Self {
            answer: Some(category),
            sent: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.sent.borrow().len()
    }
}

impl InsightProvider for FakeProvider {
    fn describe(&self, request: InsightRequest<'_>) -> Result<ModelAnswer, InsightFailureCategory> {
        self.sent.borrow_mut().push(request.content().to_string());
        match self.answer {
            Some(category) => Err(category),
            None => Ok(ModelAnswer::new(
                RevisionTopic::parse("a kept note", 0.75).unwrap(),
                InsightProvenance::from_model(DESCRIBED_BY, MODEL, PROMPT, 120, 9),
            )),
        }
    }
}

/// Runs one worker pass with the model handler over the real graph.
#[allow(
    clippy::too_many_arguments,
    reason = "a test wiring the whole path once"
)]
fn model_pass(
    manager: &Arc<LadybugDatabaseManager>,
    repository: &repository::LadybugGraphRepository,
    provider: &FakeProvider,
    budget: InsightBudget,
    attempts: i64,
    limit: usize,
) -> crate::domain::InsightWorkReport {
    let reader = LadybugRevisionInput::new(Arc::clone(manager));
    let cache = LadybugInsightCache::new(Arc::clone(manager));
    let vocabulary = vocabulary();
    let names = AnalyzedEntityNames::new(&built_in_normalizations());
    let handler = DescribedInsightHandler::new(
        &reader,
        &cache,
        provider,
        budget,
        PIPELINE,
        MODEL,
        PROMPT,
        KnowledgeQuestion::new(&vocabulary, &names),
        permit(),
    );
    InsightWorkerService::with_lifecycle(
        repository,
        &handler,
        HELD,
        InsightRetryPolicy::of_attempts(attempts),
    )
    .execute(limit, &anywhere())
    .unwrap()
}

#[test]
fn the_reader_counts_a_revision_without_touching_its_text() {
    let (_database, manager, repository) = opened("reader-summary");
    let reader = LadybugRevisionInput::new(Arc::clone(&manager));
    let job = claimed(&repository);

    // The fixture's one chunk covers bytes 0..5 of the source and says "hello".
    assert_eq!(reader.summarize(&job).unwrap(), RevisionSummary::new(1, 5));
    let parts = reader.read_parts(&job, permit()).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].number(), 1);
    assert_eq!(parts[0].chunk_id(), "chunk:file:kept@hash:a");
    assert_eq!(parts[0].text(), "hello");
}

#[test]
fn the_reader_refuses_a_job_whose_revision_the_graph_does_not_hold() {
    let (_database, manager, _repository) = opened("reader-missing");
    let reader = LadybugRevisionInput::new(Arc::clone(&manager));

    for wrong in [
        ClaimedInsightJob::new(JOB_A, "file:kept@hash:missing", "hash:a", Vec::new()),
        ClaimedInsightJob::new(JOB_A, "file:kept@hash:a", "hash:other", Vec::new()),
    ] {
        assert_eq!(
            reader.summarize(&wrong).unwrap_err(),
            InsightFailureCategory::Input
        );
    }
}

#[test]
fn the_reader_numbers_the_chunks_of_a_revision_in_their_own_order() {
    let database = TestDatabase::new("reader-order");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit("file:kept", KEPT))
        .unwrap();
    // A second and third chunk of the same revision, written out of order.
    for (ordinal, text) in [(2, "third"), (1, "second")] {
        let connection = lbug::Connection::new(&manager.owner.database).unwrap();
        connection
            .query(&format!(
                "MATCH (version:FileVersion {{id: 'file:kept@hash:a'}}) CREATE (chunk:ContentChunk {{id: 'chunk:{ordinal}', source_revision_id: 'file:kept@hash:a', ordinal: {ordinal}, start_byte: 0, end_byte: 5, text_hash: 'h', text: '{text}', script: 'latin', extraction_method: 'plain-text', extractor_id: 'plain-text', extractor_version: '1', config_hash: 'config:1', input_content_hash: 'hash:a', created_at_unix_ms: 100}}) CREATE (version)-[:HAS_CHUNK]->(chunk);"
            ))
            .unwrap();
    }
    let reader = LadybugRevisionInput::new(Arc::clone(&manager));

    let parts = reader.read_parts(&claimed(&repository), permit()).unwrap();

    let read: Vec<(usize, &str, &str)> = parts
        .iter()
        .map(|part| (part.number(), part.chunk_id(), part.text()))
        .collect();
    assert_eq!(
        read,
        [
            (1, "chunk:file:kept@hash:a", "hello"),
            (2, "chunk:1", "second"),
            (3, "chunk:2", "third")
        ]
    );
}

#[test]
fn the_counting_handler_stores_a_finding_with_no_model_behind_it() {
    let (_database, manager, repository) = opened("counted-result");
    let reader = LadybugRevisionInput::new(Arc::clone(&manager));
    let handler = CountedInsightHandler::new(&reader);

    let report = InsightWorkerService::new(&repository, &handler)
        .execute(10, &anywhere())
        .unwrap();

    assert_eq!(report.completed(), 1);
    assert_eq!(result_text(&manager, "produced_by"), COUNTED_BY);
    assert_eq!(result_text(&manager, "topic"), "");
    assert_eq!(result_text(&manager, "model_id"), "");
    assert_eq!(result_text(&manager, "prompt_version"), "");
    assert_eq!(result_number(&manager, "input_tokens"), 0);
    assert_eq!(result_number(&manager, "output_tokens"), 0);
    assert_eq!(result_number(&manager, "chunk_count"), 1);
    assert_eq!(result_number(&manager, "text_bytes"), 5);
    assert_eq!(cache_entries(&manager), 0, "nothing was bought");
}

#[test]
fn the_model_handler_stores_the_topic_and_what_answering_cost() {
    let (_database, manager, repository) = opened("model-result");
    let provider = FakeProvider::answering();

    let report = model_pass(
        &manager,
        &repository,
        &provider,
        InsightBudget::new(1_000, 10_000),
        3,
        10,
    );

    assert_eq!(report.completed(), 1);
    assert_eq!(provider.calls(), 1);
    assert_eq!(*provider.sent.borrow(), vec!["[part 1]\nhello"]);
    assert_eq!(result_text(&manager, "produced_by"), DESCRIBED_BY);
    assert_eq!(result_text(&manager, "topic"), "a kept note");
    assert_eq!(result_text(&manager, "model_id"), MODEL);
    assert_eq!(result_text(&manager, "prompt_version"), PROMPT);
    assert_eq!(result_number(&manager, "input_tokens"), 120);
    assert_eq!(result_number(&manager, "output_tokens"), 9);
    assert_eq!(job_text(&manager, "state"), "done");
}

#[test]
fn the_same_content_is_never_paid_for_twice() {
    let (_database, manager, repository) = opened("cache-hit");
    let provider = FakeProvider::answering();
    let budget = InsightBudget::new(1_000, 10_000);

    model_pass(&manager, &repository, &provider, budget, 3, 10);
    assert_eq!(cache_entries(&manager), 1);

    // A second file with byte-identical content: a different revision, the same question.
    repository
        .commit_source(evidence_commit_revision(
            "file:twin",
            "/data/docs/twin.md",
            "hash:a",
            "observation:twin",
            200,
        ))
        .unwrap();
    let second = model_pass(&manager, &repository, &provider, budget, 3, 10);

    assert_eq!(second.completed(), 1);
    assert_eq!(
        provider.calls(),
        1,
        "the second job must be served from the cache"
    );
    assert_eq!(cache_entries(&manager), 1);
    assert_eq!(
        query_int64(&manager, "MATCH (:InsightResult) RETURN COUNT(*);", vec![]),
        2,
        "both jobs still have their own result"
    );
    assert_eq!(
        query_int64(
            &manager,
            "MATCH (result:InsightResult) WHERE result.topic = 'a kept note' RETURN COUNT(*);",
            vec![]
        ),
        2
    );
}

#[test]
fn a_temporary_provider_failure_returns_the_job_to_the_queue() {
    let (_database, manager, repository) = opened("provider-unavailable");
    let provider = FakeProvider::refusing(InsightFailureCategory::Unavailable);

    let first = model_pass(
        &manager,
        &repository,
        &provider,
        InsightBudget::new(1_000, 10_000),
        3,
        10,
    );

    assert_eq!(first.failed(), 1);
    assert_eq!(job_text(&manager, "state"), "failed");
    assert_eq!(job_text(&manager, "last_error_category"), "unavailable");

    // The next pass puts it back and tries again rather than retiring it.
    let second = model_pass(
        &manager,
        &repository,
        &provider,
        InsightBudget::new(1_000, 10_000),
        3,
        10,
    );

    assert_eq!(second.recovery().returned_to_queue(), 1);
    assert_eq!(second.recovery().retired_dead(), 0);
    assert_eq!(second.claimed(), 1);
    assert_eq!(cache_entries(&manager), 0, "a failure is never cached");
}

#[test]
fn a_permanent_provider_failure_retires_the_job_without_spending_its_attempts() {
    for (category, label) in [
        (InsightFailureCategory::Refused, "refused"),
        (InsightFailureCategory::Schema, "schema"),
    ] {
        let (_database, manager, repository) = opened(&format!("provider-{label}"));
        let provider = FakeProvider::refusing(category);
        let budget = InsightBudget::new(1_000, 10_000);

        // Three attempts are allowed, and exactly one is spent before it is retired.
        let first = model_pass(&manager, &repository, &provider, budget, 3, 10);
        let second = model_pass(&manager, &repository, &provider, budget, 3, 10);

        assert_eq!(first.failed(), 1, "{label}");
        assert_eq!(second.recovery().retired_dead(), 1, "{label}");
        assert_eq!(second.recovery().returned_to_queue(), 0, "{label}");
        assert_eq!(second.claimed(), 0, "{label}");
        assert_eq!(job_text(&manager, "state"), "dead", "{label}");
        assert_eq!(job_text(&manager, "last_error_category"), label);
        assert_eq!(
            query_int64(
                &manager,
                "MATCH (job:InsightJob {id: $id}) RETURN job.attempts;",
                vec![("id", Value::String(JOB_A.to_string()))]
            ),
            1,
            "{label}: the remaining attempts were not spent to reach the same end"
        );
        assert_eq!(
            query_int64(&manager, "MATCH (:InsightResult) RETURN COUNT(*);", vec![]),
            0,
            "{label}"
        );
    }
}

#[test]
fn a_revision_above_the_job_limit_is_retired_and_never_sent() {
    let (_database, manager, repository) = opened("budget-too-large");
    let provider = FakeProvider::answering();

    let first = model_pass(
        &manager,
        &repository,
        &provider,
        InsightBudget::new(1, 10_000),
        3,
        10,
    );
    let second = model_pass(
        &manager,
        &repository,
        &provider,
        InsightBudget::new(1, 10_000),
        3,
        10,
    );

    assert_eq!(first.failed(), 1);
    assert_eq!(provider.calls(), 0, "nothing may have been sent");
    assert_eq!(job_text(&manager, "last_error_category"), "input-too-large");
    assert_eq!(second.recovery().retired_dead(), 1);
    assert_eq!(job_text(&manager, "state"), "dead");
}

#[test]
fn a_run_that_spends_its_budget_leaves_the_rest_of_the_queue_for_the_next_one() {
    let (_database, manager, repository) = opened("budget-run");
    // A second file with different content, so the cache cannot serve it.
    repository
        .commit_source(evidence_commit_revision(
            "file:other",
            "/data/docs/other.md",
            "hash:b",
            "observation:other",
            200,
        ))
        .unwrap();
    let provider = FakeProvider::answering();

    // Room for exactly one revision's worth of bytes.
    let first = model_pass(
        &manager,
        &repository,
        &provider,
        InsightBudget::new(100, 5),
        3,
        10,
    );

    assert_eq!(first.completed(), 1);
    assert_eq!(first.failed(), 1);
    assert_eq!(provider.calls(), 1);
    assert_eq!(
        query_int64(
            &manager,
            "MATCH (job:InsightJob {last_error_category: 'budget-spent'}) RETURN COUNT(job);",
            vec![]
        ),
        1
    );

    // The next run has its own budget and finishes the job that arrived too late.
    let second = model_pass(
        &manager,
        &repository,
        &provider,
        InsightBudget::new(100, 5_000),
        3,
        10,
    );

    assert_eq!(second.recovery().returned_to_queue(), 1);
    assert_eq!(second.completed(), 1);
    assert_eq!(
        query_int64(&manager, "MATCH (:InsightResult) RETURN COUNT(*);", vec![]),
        2
    );
}

#[test]
fn a_stored_answer_that_is_no_longer_a_topic_is_not_served() {
    let (_database, manager, _repository) = opened("cache-corrupt");
    let cache = LadybugInsightCache::new(Arc::clone(&manager));
    let key = InsightCacheKey::of(
        "hash:a",
        PIPELINE,
        MODEL,
        PROMPT,
        &vocabulary().fingerprint(),
    );
    cache
        .store(
            &key,
            &ModelAnswer::new(
                RevisionTopic::parse("a kept note", 0.75).unwrap(),
                InsightProvenance::from_model(DESCRIBED_BY, MODEL, PROMPT, 1, 1),
            ),
        )
        .unwrap();
    assert!(cache.lookup(&key).unwrap().is_some());

    // Something outside this program rewrote the stored label into a whole paragraph.
    let connection = lbug::Connection::new(&manager.owner.database).unwrap();
    connection
        .query("MATCH (entry:InsightCacheEntry) SET entry.topic = 'a\nparagraph';")
        .unwrap();

    assert_eq!(
        cache.lookup(&key).unwrap_err(),
        InsightFailureCategory::Schema,
        "what is on disk still has to be a topic to be served"
    );
}

/// The whole of the boundary, at the level a context is chosen: a context that did not say
/// its content may leave produces no permit, and without one neither the model handler nor
/// a request can be constructed at all.
#[test]
fn a_denying_context_has_nothing_to_build_a_request_from() {
    assert!(ContentEgress::Denied.permit().is_none());
    assert!(ContentEgress::default().permit().is_none());
    assert!(ContentEgress::Allowed.permit().is_some());
}

#[test]
fn a_result_and_a_cache_entry_carry_no_path_and_no_file_text() {
    let (_database, manager, repository) = opened("analysis-privacy");
    let provider = FakeProvider::answering();

    model_pass(
        &manager,
        &repository,
        &provider,
        InsightBudget::new(1_000, 10_000),
        3,
        10,
    );

    for field in [
        "id",
        "job_id",
        "source_revision_id",
        "input_content_hash",
        "produced_by",
        "model_id",
        "prompt_version",
        "topic",
    ] {
        let value = result_text(&manager, field);
        assert!(
            !value.contains("/data") && !value.contains("kept.md") && !value.contains("hello"),
            "result.{field} leaks a path or the file's text: {value}"
        );
    }
    for field in ["key", "topic", "model_id", "prompt_version"] {
        let value = query_string(
            &manager,
            &format!("MATCH (entry:InsightCacheEntry) RETURN entry.{field};"),
            vec![],
        );
        assert!(
            !value.contains("/data") && !value.contains("kept.md") && !value.contains("hello"),
            "cache.{field} leaks a path or the file's text: {value}"
        );
    }
    // The job record gained nothing from the analysis at all.
    assert_eq!(job_text(&manager, "last_error_category"), "");
}

#[test]
fn an_analysis_survives_reopen_with_its_cache() {
    let database = TestDatabase::new("analysis-reopen");
    {
        let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
        let repository = manager.repository();
        repository
            .commit_source(evidence_commit("file:kept", KEPT))
            .unwrap();
        model_pass(
            &manager,
            &repository,
            &FakeProvider::answering(),
            InsightBudget::new(1_000, 10_000),
            3,
            10,
        );
    }

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    assert_eq!(result_text(&reopened, "topic"), "a kept note");
    assert_eq!(cache_entries(&reopened), 1);
    assert_eq!(
        query_int64(
            &reopened,
            "MATCH (entry:InsightCacheEntry) RETURN entry.input_tokens;",
            vec![]
        ),
        120
    );
}

/// These tests are about the lifecycle of a job, not about the policy, so every path is one
/// this context still indexes.
fn anywhere() -> impl Fn(&crate::domain::NormalizedPath) -> bool {
    |_: &crate::domain::NormalizedPath| true
}
