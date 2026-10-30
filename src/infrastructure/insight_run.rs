// How one context's insight jobs get worked, decided once and then run as often as the
// caller needs: once for `index jobs run`, once after every completed pass for
// `index watch`.
//
// Which handler may exist here is decided by the context before anything is claimed. A
// denying context has no permit, so the branch that would build an outgoing request has no
// value to be built from: the refusal is an absence, not a check.
//
// The handler is built inside every sweep rather than once for the whole run. That is what
// keeps the byte budget of stage 6 a property of one sweep, the same way it is a property
// of one `index jobs run`. A watcher holding a single budget for days would stop analysing
// in the middle of the night and then spend the remaining attempts of every job failing it
// as over budget — a cap that retires work instead of postponing it.
use std::sync::Arc;

use crate::application::{
    AnalysisPass, CountedInsightHandler, DescribedInsightHandler, InsightProvider,
    InsightQueueService, InsightWorkerService, JobScope, KnowledgeQuestion, QueueAnalysis,
    RecheckReport, RepositoryError, recheck_knowledge,
};
use crate::domain::knowledge::KnowledgeVocabulary;
use crate::domain::{
    ClaimedInsightJob, ContentEgress, ContentEgressPermit, HeldJob, InsightBudget, InsightJobState,
    InsightWorkReport, LargeWork, NormalizationSet, NormalizedPath, PARTS_WITHOUT_ASKING, RunSpend,
};

use super::analysis::{self, AnalysisEngine, EngineUnavailable};
use super::ladybug::{
    LadybugDatabaseManager, LadybugGraphRepository, LadybugInsightCache, LadybugRevisionInput,
};
use super::search::AnalyzedEntityNames;

/// The pipeline the jobs in this outbox belong to. It is part of what makes two analyses
/// the same question, so it travels into the cache key with the model and the prompt.
const PIPELINE_VERSION: &str = "insight-outbox-v1";

/// What this context's queue is worked with.
pub struct InsightRun<Provider> {
    manager: Arc<LadybugDatabaseManager>,
    analysis: Analysis<Provider>,
}

enum Analysis<Provider> {
    /// The context keeps its content to itself: a deterministic handler, no key, no
    /// network, and nothing concluded.
    Counting,
    /// The context allowed its content to leave and the chosen engine is usable.
    Describing {
        provider: Provider,
        engine: AnalysisEngine,
        budget: InsightBudget,
        question: Question,
        permit: ContentEgressPermit,
    },
}

/// What a describing run asks about entities and facts: the closed lists, and the analyzer a
/// name is reduced to its key by.
pub struct Question {
    vocabulary: KnowledgeVocabulary,
    names: AnalyzedEntityNames,
}

impl Question {
    pub fn new(vocabulary: &KnowledgeVocabulary, normalizations: &NormalizationSet) -> Self {
        Self {
            vocabulary: vocabulary.clone(),
            names: AnalyzedEntityNames::new(normalizations),
        }
    }
}

/// What one sweep did: the jobs it moved and, when a model was asked, what that cost and what
/// of the answers was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepReport {
    work: InsightWorkReport,
    spend: Option<RunSpend>,
    held: Vec<HeldJob>,
}

impl SweepReport {
    pub fn work(&self) -> &InsightWorkReport {
        &self.work
    }

    /// Nothing for a context that asked no model.
    pub fn spend(&self) -> Option<&RunSpend> {
        self.spend.as_ref()
    }

    /// The jobs left queued because they are larger than the run may send unasked.
    pub fn held(&self) -> &[HeldJob] {
        &self.held
    }
}

/// What this context can work its queue with, or nothing when it may send content outside
/// and no key is configured.
///
/// Nothing is neither a failure nor a fallback to the local handler: no job is claimed at
/// all. Claiming would count an attempt against a call that was never going to happen, and
/// a job that quietly fell back to counting could never be analysed later.
pub fn decide(
    manager: &Arc<LadybugDatabaseManager>,
    egress: ContentEgress,
    engine: &AnalysisEngine,
    budget: InsightBudget,
    question: Question,
) -> Result<InsightRun<Box<dyn InsightProvider>>, EngineUnavailable> {
    let Some(permit) = egress.permit() else {
        return Ok(InsightRun {
            manager: Arc::clone(manager),
            analysis: Analysis::Counting,
        });
    };
    Ok(InsightRun {
        manager: Arc::clone(manager),
        analysis: Analysis::Describing {
            provider: analysis::insight_provider(engine)?,
            engine: engine.clone(),
            budget,
            question,
            permit,
        },
    })
}

impl<Provider: InsightProvider> InsightRun<Provider> {
    /// What this run does, for the one line a command prints before it takes a job.
    pub fn label(&self) -> String {
        match &self.analysis {
            Analysis::Counting => "local (this context does not send content outside)".to_string(),
            Analysis::Describing { engine, .. } => format!(
                "engine {} (model {})",
                engine.name(),
                analysis::model_label(engine)
            ),
        }
    }

    /// One bounded sweep of the queue: at most `limit` jobs, oldest first.
    /// `is_visible` says which paths this context still indexes. It is asked once per
    /// claimed job, not once per queued job: the queue outlives the policy that filled it,
    /// and a revision the policy stopped asking about must not be paid for.
    ///
    /// Before anything is claimed, the knowledge of every revision a newer one replaced is
    /// retired, in both forms of the run: a context that stopped sending content still must not
    /// show facts its files no longer say.
    pub fn sweep(
        &self,
        repository: &LadybugGraphRepository,
        limit: usize,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<SweepReport, RepositoryError> {
        self.sweep_within(
            repository,
            limit,
            is_visible,
            JobScope::Everything,
            LargeWork::HoldOver(PARTS_WITHOUT_ASKING),
        )
    }

    /// The same sweep, taking jobs only from `scope` — the work one directory selected.
    pub fn sweep_within(
        &self,
        repository: &LadybugGraphRepository,
        limit: usize,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
        scope: JobScope<'_>,
        large: LargeWork,
    ) -> Result<SweepReport, RepositoryError> {
        repository.retire_stale_knowledge()?;
        // Only a run that asks a model holds anything back: counting costs nothing.
        let held = match (&self.analysis, large) {
            (Analysis::Describing { .. }, LargeWork::HoldOver(most)) => {
                held_in(repository.queued_jobs_over_parts(most)?, scope)
            }
            _ => Vec::new(),
        };
        let held_ids: Vec<String> = held.iter().map(|job| job.id().to_string()).collect();
        let narrowed: Vec<String>;
        let scope = match scope {
            _ if held_ids.is_empty() => scope,
            JobScope::Among(ids) => {
                narrowed = ids
                    .iter()
                    .filter(|id| !held_ids.contains(id))
                    .cloned()
                    .collect();
                JobScope::Among(&narrowed)
            }
            _ => JobScope::Except(&held_ids),
        };
        let described = self.with_described(|handler| {
            let work = InsightWorkerService::new(repository, handler)
                .execute_within(limit, is_visible, scope)?;
            Ok(SweepReport {
                work,
                spend: Some(handler.spent()),
                held: held.clone(),
            })
        });
        if let Some(swept) = described {
            return swept;
        }
        let reader = LadybugRevisionInput::new(Arc::clone(&self.manager));
        let handler = CountedInsightHandler::new(&reader);
        Ok(SweepReport {
            work: InsightWorkerService::new(repository, &handler)
                .execute_within(limit, is_visible, scope)?,
            spend: None,
            held,
        })
    }

    /// Applies the rules of this build to every analysed current revision from its cached
    /// answer, without asking the model; nothing for a context that asks no model, which has no
    /// answers to recheck.
    pub fn recheck(
        &self,
        repository: &LadybugGraphRepository,
    ) -> Result<Option<(RecheckReport, RunSpend)>, RepositoryError> {
        self.with_described(|handler| {
            let report =
                recheck_knowledge(repository, &|job: &ClaimedInsightJob| handler.recheck(job))?;
            Ok((report, handler.spent()))
        })
        .transpose()
    }

    /// Runs `work` with the model handler of this run, or answers nothing when the context
    /// asks no model. One place builds the handler, so a sweep and a recheck cannot ask
    /// different questions.
    fn with_described<T>(
        &self,
        work: impl FnOnce(
            &DescribedInsightHandler<'_, LadybugRevisionInput, LadybugInsightCache, Provider>,
        ) -> T,
    ) -> Option<T> {
        let Analysis::Describing {
            provider,
            engine,
            budget,
            question,
            permit,
        } = &self.analysis
        else {
            return None;
        };
        let reader = LadybugRevisionInput::new(Arc::clone(&self.manager));
        let cache = LadybugInsightCache::new(Arc::clone(&self.manager));
        let handler = DescribedInsightHandler::new(
            &reader,
            &cache,
            provider,
            *budget,
            PIPELINE_VERSION,
            analysis::model_label(engine),
            analysis::insight_prompt_version(engine),
            KnowledgeQuestion::new(&question.vocabulary, &question.names),
            *permit,
        );
        Some(work(&handler))
    }

    /// The same run as the watcher reaches it: one sweep per completed pass, reported with
    /// what the queue holds afterwards.
    pub fn sweeps<'a>(
        &'a self,
        repository: &'a LadybugGraphRepository,
        is_visible: &'a dyn Fn(&NormalizedPath) -> bool,
    ) -> InsightSweeps<'a, Provider> {
        InsightSweeps {
            run: self,
            repository,
            is_visible,
        }
    }
}

/// The large jobs that fall inside `scope`: of a selection, only the selected ones.
fn held_in(large: Vec<HeldJob>, scope: JobScope<'_>) -> Vec<HeldJob> {
    match scope {
        JobScope::Among(ids) => large
            .into_iter()
            .filter(|job| ids.iter().any(|id| id == job.id()))
            .collect(),
        _ => large,
    }
}

/// The watcher's port into this run.
pub struct InsightSweeps<'a, Provider> {
    run: &'a InsightRun<Provider>,
    repository: &'a LadybugGraphRepository,
    is_visible: &'a dyn Fn(&NormalizedPath) -> bool,
}

impl<Provider: InsightProvider> QueueAnalysis for InsightSweeps<'_, Provider> {
    fn analyze(&self, limit: usize) -> Result<AnalysisPass, RepositoryError> {
        let swept = self.run.sweep(self.repository, limit, self.is_visible)?;
        // Asked of the queue rather than derived from the sweep: the pass this sweep
        // follows queued jobs of its own, so anything subtracted here would not be the
        // number `index jobs` prints a moment later.
        let left_queued = InsightQueueService
            .execute(self.repository)?
            .count(InsightJobState::Queued);
        Ok(AnalysisPass::new(*swept.work(), left_queued)
            .with_spend(swept.spend().copied())
            .with_held(swept.held().len()))
    }
}

#[cfg(test)]
impl<Provider: InsightProvider> InsightRun<Provider> {
    /// The run of a context that allowed its content to leave, for a provider a test
    /// supplies. Production builds this through `decide`, which is the only place a key is
    /// read; a test that needed a key would be a test that needed the network.
    pub(super) fn describing(
        manager: Arc<LadybugDatabaseManager>,
        provider: Provider,
        engine: AnalysisEngine,
        budget: InsightBudget,
        question: Question,
        permit: ContentEgressPermit,
    ) -> Self {
        Self {
            manager,
            analysis: Analysis::Describing {
                provider,
                engine,
                budget,
                question,
                permit,
            },
        }
    }
}
