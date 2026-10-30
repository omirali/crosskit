mod analysis_review;
mod ask;
mod ask_ports;
mod context_pass;
mod context_proposal;
mod context_status;
/// Stage 32: what the lines of a transcript add up to.
pub(crate) mod conversation_assembly;
mod doctor;
mod file_identity;
/// Stage 37: where a file was worked on, and what a conversation worked on.
mod file_touches;
/// UI-4: the bounded view of the graph around one conversation or file.
mod graph_explorer;
#[cfg(test)]
mod graph_explorer_tests;
mod graph_ports;
mod graph_recovery;
mod initial_scan;
mod insight_analysis;
mod insight_queue;
mod insight_withdrawal;
mod insight_worker;
/// The knowledge layer as the map and the commands read it (ADR 0048).
mod knowledge_map;
/// The rules of this build applied to stored knowledge, without the model.
mod knowledge_recheck;
mod period_scope;
mod policy_explain;
pub(crate) mod policy_scan;
mod reconcile;
mod repository;
mod root_coverage;
mod run_retention;
mod search;
mod search_index;
mod search_indexing;
mod search_ports;
mod source_batch;
mod source_walker;
mod subtree_volumes;
mod task_board;
mod task_candidates;
mod task_map;
mod task_synthesis;
mod watch;
mod watch_ports;
mod watch_report;
mod watch_scope;
#[cfg(test)]
mod watch_tests;
/// The work one directory selects for a run (`index jobs run --under | --touching`).
mod work_scope;

pub use analysis_review::{
    AnalysisReview, AnalysisReviewService, InventoriedRevision, ReviewGroup, RevisionInventory,
};
pub use ask::{AskReport, AskService, PeriodQuestion};
pub use ask_ports::{AnswerCache, AnswerProvider, AnswerRequest, AskFailure, Excerpt};
pub use context_pass::{
    ContextCompletion, ContextIncomplete, ContextPassReport, PassFailure, RootOutcome,
};
pub use context_proposal::ContextProposalService;
pub use context_status::{
    ContextStatusReport, ContextStatusService, GraphInventory, GraphInventoryReader,
};
pub use doctor::DoctorService;
pub use file_identity::FileIdentityHistory;
pub use file_touches::{
    Direction, StoredTouch, TouchGroup, TouchQuery, TouchReader, TouchReport, TouchService,
    TouchedPair,
};
pub use graph_explorer::{GraphError, GraphExplorer};
pub use graph_ports::{GraphReader, StoredConversation, StoredFile, StoredReply, StoredTopic};
pub use graph_recovery::{GraphMaintenance, GraphRecoveryService, RecoveryReport};
pub use initial_scan::{IndexError, InitialScanService};
/// Named only where a test checks which handler produced a stored finding; production code
/// never spells it, because the handler writes it itself.
#[cfg(test)]
pub use insight_analysis::COUNTED_BY;
pub use insight_analysis::{
    CountedInsightHandler, DESCRIBED_BY, DescribedInsightHandler, EntityNames, InsightCache,
    InsightProvider, InsightRequest, KnowledgeQuestion, RevisionInputReader,
};
pub use insight_queue::{InsightQueueReader, InsightQueueService, InsightQueueSummary};
pub use insight_withdrawal::{
    InsightQueueWithdrawal, InsightWithdrawalReport, InsightWithdrawalService,
};
/// Named only where a test implements a handler of its own; production code passes one to
/// the worker without naming the trait.
#[cfg(test)]
pub use insight_worker::InsightHandler;
pub use insight_worker::{InsightJobQueue, InsightWorkerService, JobScope};
pub use knowledge_map::{
    CostRate, KnowledgeMapService, KnowledgeReader, KnowledgeTotals, PaidCall, RevisionSource,
    SourceConversation, entities_named,
};
pub use knowledge_recheck::{KnowledgeRewrite, RecheckReport, Requeued, recheck_knowledge};
pub use period_scope::{
    ConversationTimeline, PeriodEvent, PeriodEventReader, PeriodScope, PeriodTouch,
};
pub use policy_explain::{PolicyExplainService, PolicyExplanation};
pub use policy_scan::ScanReport;
pub use reconcile::{IncompleteReason, ReconcileCompletion, ReconcileReport, ReconcileService};
pub use repository::{
    CommitResult, GraphCheckpoint, KnowledgeGraphRepository, PassAttemptLog, ReconcileRepository,
    RepositoryError, RetirementReport,
};
pub use root_coverage::{FailedPass, RootCoverage, RootCoverageReader};
pub use run_retention::{ReconcileRunMaintenance, RunRetentionService};
pub use search::{FileState, SearchPage, SearchRequest, SearchResult, SearchService};
pub use search_index::{SearchIndexReport, SearchIndexService};
pub use search_indexing::SearchIndexUpdate;
pub use search_ports::{
    DocumentMatch, MatchFragment, MatchedConversation, MatchedDocuments, ResolvedChunk,
    SearchDocument, SearchError, SearchGraphReader, SearchIndexError, SearchIndexReader,
    SearchIndexState, SearchIndexWriter,
};
#[cfg(feature = "batch-limit-benchmark")]
pub use source_batch::SourceBatchLimits;
pub use source_batch::SourceBatchWriter;
pub use source_walker::{
    Admission, ContentSkipReason, EvidenceRead, SourceEntry, SourceVisitor, SourceWalker,
};
pub use task_board::{
    Board, TaskBoard, TaskBoardError, TaskBookStore, TaskBookStoreError, TaskDecision,
};
/// The candidates alone, without the owner's decisions: what the end-to-end test of stitching
/// asks for.
#[cfg(test)]
pub use task_candidates::TaskCandidates;
pub use task_candidates::{ConversationLog, ConversationLogReader, TaskMember, Tie};
pub use task_map::{
    ConversationSpeech, MapEngine, MapEngineFailure, MapProgress, MapStore, MapStoreFailure,
    MappedConversation, ProposedStretch, REPLY_BYTES, SpokenReply, StoredMap, TaskMapError,
    TaskMapping, speech_of,
};
pub use task_synthesis::{
    MapMerger, MergeInput, MergedProposal, ShownSynthesis, StoredSynthesis, SynthesisError,
    SynthesisStore, TaskSynthesizing,
};
pub use watch::{WatchSchedule, WatchService};
pub use watch_ports::{
    EventDelivery, QueueAnalysis, ReconcilePass, RootWatch, SubscribeFailure, UnconfirmedReason,
    WatchClock, WatchError, WatchEventSource, WatchObserver, WatchSignal, WatchSubscription,
};
pub use watch_report::{
    AnalysisPass, AnalysisTotals, PassOutcome, PassRecord, PassTrigger, WatchMode, WatchReport,
};
pub use watch_scope::{ChangedPaths, PassScope, ScopeProbe, directories_to_walk};
pub use work_scope::{RevisionSize, SelectedWork, WorkScope, WorkScopeReader, WorkScopeService};

use crate::domain::{AICodingTool, AICodingToolKind, Greeting};

pub struct GreetingService;

impl GreetingService {
    pub fn execute(&self) -> Greeting {
        Greeting::new("Crosskit is ready.")
    }
}

pub struct OnboardingState {
    marker_exists: bool,
}

impl OnboardingState {
    pub fn from_marker_exists(marker_exists: bool) -> Self {
        Self { marker_exists }
    }

    pub fn should_show_onboarding(&self) -> bool {
        !self.marker_exists
    }
}

#[allow(dead_code)]
pub struct ToolDiscoveryService;

impl ToolDiscoveryService {
    #[allow(dead_code)]
    pub fn normalize_tool_name(name: &str) -> AICodingToolKind {
        AICodingToolKind::from_command_name(name)
    }

    #[allow(dead_code)]
    pub fn create_tool(name: &str, path: Option<String>, version: Option<String>) -> AICodingTool {
        AICodingTool::new(name, path, version)
    }
}

#[cfg(test)]
mod tests {
    use super::{OnboardingState, ToolDiscoveryService};
    use crate::domain::AICodingToolKind;

    #[test]
    fn missing_marker_requires_onboarding() {
        let state = OnboardingState::from_marker_exists(false);
        assert!(state.should_show_onboarding());
    }

    #[test]
    fn existing_marker_skips_onboarding() {
        let state = OnboardingState::from_marker_exists(true);
        assert!(!state.should_show_onboarding());
    }

    #[test]
    fn tool_names_are_recognized() {
        assert_eq!(
            ToolDiscoveryService::normalize_tool_name("claude"),
            AICodingToolKind::ClaudeCode
        );
        assert_eq!(
            ToolDiscoveryService::normalize_tool_name("claude-code"),
            AICodingToolKind::ClaudeCode
        );
        assert_eq!(
            ToolDiscoveryService::normalize_tool_name("codex"),
            AICodingToolKind::Codex
        );
        assert_eq!(
            ToolDiscoveryService::normalize_tool_name("opencode"),
            AICodingToolKind::OpenCode
        );
        assert_eq!(
            ToolDiscoveryService::normalize_tool_name("open-code"),
            AICodingToolKind::OpenCode
        );
        assert_eq!(
            ToolDiscoveryService::normalize_tool_name("gh-copilot"),
            AICodingToolKind::GitHubCopilot
        );
    }
}
