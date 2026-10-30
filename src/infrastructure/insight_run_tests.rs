// The sweep the watcher asks for, against the real database.
//
// The provider is a fake, for the reason stage 6 gave: everything this has to be sure of —
// that a provider failure leaves the job where stage 5 says, that a sweep takes no more
// than it was asked for, that a claim nobody holds any more comes back — is a property of
// this program, and a test that needed the network would prove less about it. The local
// handler and the whole `index watch` composition are covered from the outside, in
// `tests/cli_index_watch.rs`.
use crate::domain::NormalizedPath;
use crate::infrastructure::analysis::AnalysisEngine;
use std::cell::RefCell;
use std::sync::Arc;

use super::anthropic::ModelSettings;
use super::insight_run::{InsightRun, Question};
use super::ladybug::test_support::{TestDatabase, evidence_commit_revision, query_string};
use super::ladybug::{LadybugDatabaseManager, LadybugGraphRepository};
use super::search::built_in_normalizations;
use crate::application::JobScope;
use crate::application::{
    DESCRIBED_BY, InsightJobQueue, InsightProvider, InsightQueueService, InsightRequest,
    KnowledgeGraphRepository, QueueAnalysis,
};
use crate::domain::knowledge::KnowledgeVocabulary;
use crate::domain::{
    ContentEgress, InsightBudget, InsightFailureCategory, InsightJobState, InsightLease,
    InsightProvenance, ModelAnswer, RevisionTopic,
};

const MODEL: &str = "a-model";

fn engine() -> AnalysisEngine {
    AnalysisEngine::AnthropicApi(ModelSettings::new(MODEL.to_string(), String::new()))
}

fn budget() -> InsightBudget {
    InsightBudget::new(200_000, 20_000_000)
}

/// A provider that answers every revision the same way, or refuses with a chosen category,
/// and counts what it was asked.
struct FakeProvider {
    refusal: Option<InsightFailureCategory>,
    calls: RefCell<usize>,
}

impl FakeProvider {
    fn answering() -> Self {
        Self {
            refusal: None,
            calls: RefCell::new(0),
        }
    }

    fn refusing(category: InsightFailureCategory) -> Self {
        Self {
            refusal: Some(category),
            calls: RefCell::new(0),
        }
    }

    fn calls(&self) -> usize {
        *self.calls.borrow()
    }
}

impl InsightProvider for FakeProvider {
    fn describe(
        &self,
        _request: InsightRequest<'_>,
    ) -> Result<ModelAnswer, InsightFailureCategory> {
        *self.calls.borrow_mut() += 1;
        match self.refusal {
            Some(category) => Err(category),
            None => Ok(ModelAnswer::new(
                RevisionTopic::parse("a kept note", 0.75).unwrap(),
                InsightProvenance::from_model(DESCRIBED_BY, MODEL, "revision-topic-v1", 120, 9),
            )),
        }
    }
}

/// A run owns its provider, so the test lends one instead of giving it away and can still
/// ask afterwards how many times it was called.
impl InsightProvider for &FakeProvider {
    fn describe(&self, request: InsightRequest<'_>) -> Result<ModelAnswer, InsightFailureCategory> {
        <FakeProvider as InsightProvider>::describe(self, request)
    }
}

/// A graph holding one queued job per named file, which is what a reconcile pass leaves
/// behind for every revision it committed. Each file gets content of its own, because the
/// cache of stage 6 answers the second identical revision without asking anybody.
fn opened(
    test_name: &str,
    files: &[&str],
) -> (
    TestDatabase,
    Arc<LadybugDatabaseManager>,
    LadybugGraphRepository,
) {
    let database = TestDatabase::new(test_name);
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for file in files {
        repository
            .commit_source(evidence_commit_revision(
                &format!("file:{file}"),
                &format!("/data/docs/{file}.md"),
                &format!("hash:{file}"),
                &format!("observation:{file}"),
                100,
            ))
            .unwrap();
    }
    (database, manager, repository)
}

fn queued(repository: &LadybugGraphRepository, state: InsightJobState) -> usize {
    InsightQueueService
        .execute(repository)
        .unwrap()
        .count(state)
}

fn describing<'a>(
    manager: &Arc<LadybugDatabaseManager>,
    provider: &'a FakeProvider,
) -> InsightRun<&'a FakeProvider> {
    InsightRun::describing(
        Arc::clone(manager),
        provider,
        engine(),
        budget(),
        Question::new(
            &KnowledgeVocabulary::new(vec!["concept".to_string()], vec!["related-to".to_string()])
                .unwrap(),
            &built_in_normalizations(),
        ),
        ContentEgress::Allowed.permit().unwrap(),
    )
}

#[test]
fn a_sweep_takes_no_more_than_the_limit_and_reports_what_the_queue_still_holds() {
    let (_database, manager, repository) = opened("sweep-limit", &["first", "second", "third"]);
    let provider = FakeProvider::answering();
    let run = describing(&manager, &provider);

    let pass = run
        .sweeps(&repository, &|_: &NormalizedPath| true)
        .analyze(2)
        .unwrap();

    assert_eq!(pass.claimed(), 2);
    assert_eq!(pass.completed(), 2);
    assert_eq!(pass.failed(), 0);
    // The number the sweep reports is the number the queue itself holds, not a difference.
    assert_eq!(pass.left_queued(), 1);
    assert_eq!(queued(&repository, InsightJobState::Queued), 1);
    assert_eq!(queued(&repository, InsightJobState::Done), 2);
    // The limit bounds what left the machine, not only what was recorded.
    assert_eq!(provider.calls(), 2);

    // The next sweep finds the one that was left and nothing else.
    let rest = run
        .sweeps(&repository, &|_: &NormalizedPath| true)
        .analyze(2)
        .unwrap();

    assert_eq!(rest.claimed(), 1);
    assert_eq!(rest.left_queued(), 0);
}

#[test]
fn a_provider_failure_that_another_attempt_could_survive_leaves_the_job_back_in_the_queue() {
    let (_database, manager, repository) = opened("sweep-provider-down", &["first"]);
    let provider = FakeProvider::refusing(InsightFailureCategory::Unavailable);
    let run = describing(&manager, &provider);

    let pass = run
        .sweeps(&repository, &|_: &NormalizedPath| true)
        .analyze(5)
        .unwrap();

    // The sweep itself succeeded: a provider that could not be reached is a property of one
    // job, not of the pass that worked it.
    assert_eq!(pass.claimed(), 1);
    assert_eq!(pass.completed(), 0);
    assert_eq!(pass.failed(), 1);
    assert_eq!(queued(&repository, InsightJobState::Failed), 1);
    assert_eq!(
        query_string(
            &manager,
            "MATCH (job:InsightJob) RETURN job.last_error_category;",
            vec![],
        ),
        InsightFailureCategory::Unavailable.label()
    );

    // Stage 5 decides what happens next, and it is the recovery at the head of the next
    // sweep that does it: the job is queued again, having spent one of its attempts.
    let again = run
        .sweeps(&repository, &|_: &NormalizedPath| true)
        .analyze(5)
        .unwrap();

    assert_eq!(again.claimed(), 1);
    assert_eq!(again.failed(), 1);
    assert_eq!(queued(&repository, InsightJobState::Done), 0);
}

#[test]
fn a_claim_whose_lease_ran_out_is_returned_by_the_next_sweep() {
    let (_database, manager, repository) = opened("sweep-expired-lease", &["first"]);
    // A worker that took the job and never came back: the claim is held with a lease that
    // was over the moment it was taken, which is the state a killed process leaves.
    let held = repository
        .claim_insight_jobs(1, InsightLease::of_milliseconds(0), JobScope::Everything)
        .unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(queued(&repository, InsightJobState::Queued), 0);
    assert_eq!(queued(&repository, InsightJobState::Claimed), 1);

    let provider = FakeProvider::answering();
    let run = describing(&manager, &provider);
    let pass = run
        .sweeps(&repository, &|_: &NormalizedPath| true)
        .analyze(5)
        .unwrap();

    // Nothing was queued when the sweep began, so the only job it could claim is the one
    // the lease gave back.
    assert_eq!(pass.claimed(), 1);
    assert_eq!(pass.completed(), 1);
    assert_eq!(pass.left_queued(), 0);
    assert_eq!(queued(&repository, InsightJobState::Done), 1);
}

#[test]
fn a_sweep_of_an_empty_queue_asks_the_provider_nothing() {
    let (_database, manager, repository) = opened("sweep-empty", &[]);
    let provider = FakeProvider::answering();
    let run = describing(&manager, &provider);

    let pass = run
        .sweeps(&repository, &|_: &NormalizedPath| true)
        .analyze(5)
        .unwrap();

    assert_eq!(pass.claimed(), 0);
    assert_eq!(pass.left_queued(), 0);
    assert_eq!(provider.calls(), 0);
}
