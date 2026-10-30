mod alphabet;
mod analysis_rules;
mod ask;
mod cache_key;
mod calendar;
mod chunk_script;
mod context_grouping;
mod context_proposal;
mod context_root;
/// Stage 38's split measurement: its rules are not the product's, so they compile into its
/// test build and nowhere else.
#[cfg(all(test, feature = "context-split-measurement"))]
pub(crate) mod context_split;
/// Stage 32: a conversation is evidence, so its types are production and not measurement.
pub(crate) mod conversation;
/// Stage 31's ranking rules stayed measurement — the graph they scored was not built.
#[cfg(all(test, feature = "insight-layer-measurement"))]
pub(crate) mod conversation_rank;
/// A conversation's forks, branches and risks, each proved by a quote (ADR 0055).
pub mod decision_map;
mod document_score;
mod evidence;
mod extraction_profile;
mod file_identity;
mod file_location;
/// Whether the owner named a file in their own words: the mark Stage 37 writes on a touch
/// and the rule it is computed with.
mod file_mention;
mod generated_directory;
/// The mail domain a person acts under in git — the one part of an address `setup` reads.
mod git_identity;
mod git_remote;
/// UI-4: a bounded view of the graph around one conversation or file, and how it fills.
mod graph_budget;
mod graph_view;
mod index_fingerprint;
mod index_policy;
mod insight_budget;
mod insight_cache;
mod insight_job;
mod insight_result;
mod insight_topic;
mod insight_work;
/// The knowledge layer: entities and facts, each fact proved by a quote found in its source.
pub mod knowledge;
/// Stage 39: what `setup` finds on a machine, the groups it asks about and the contexts the
/// answers make.
mod machine_layout;
mod period;
mod period_phrases;
mod project_signals;
mod proposed_context;
mod reconcile;
mod reply_chunk;
mod run_retention;
mod search_rank;
mod source_commit;
mod source_language;
mod source_path;
mod suffix_stemming;
/// The owner's decisions about tasks, laid over what stitching proposes (ADR 0053).
mod task_book;
/// Which conversations belong to one task, by facts their logs record (ADR 0051, ADR 0052).
pub(crate) mod task_stitching;
/// A whole task's map joined from its conversations' maps (ADR 0056).
pub mod task_synthesis;
#[cfg(all(test, feature = "insight-layer-measurement"))]
pub(crate) mod term_association;
#[cfg(all(test, feature = "insight-layer-measurement"))]
pub(crate) mod term_window;
mod text_normalization;
/// The folder Claude Code files a transcript under, and the places kept for throwaway work.
mod transcript_folder;
mod workspace;

pub use alphabet::{Alphabet, Script};
pub use analysis_rules::AnalysisRules;
pub use ask::{Answer, AskBudget, AskCacheKey, AskError, Question};
pub use calendar::CivilDate;
pub use chunk_script::ChunkScript;
pub use context_proposal::{
    ContextProposal, SubtreeEvidence, SubtreeVolume, TreeEvidence, WalkOutcome,
};
pub use context_root::ContextRoot;
pub use document_score::DocumentScore;
pub use evidence::{ContentChunk, EvidenceCommit, EvidenceError, FileObservation, FileVersion};
pub use extraction_profile::ExtractionProfile;
pub use file_identity::{KnownIdentity, ObservedFileIdentity};
pub use file_location::{FileKind, FileLocation, LocationStatus};
pub use file_mention::{MentionRule, OwnerNaming, path_ends_with_segment};
/// Only Stage 37's measurement needs a file's name on its own; the product asks the rule
/// instead of asking for the name.
#[cfg(test)]
pub use file_mention::{bytes_outside_quoted_blocks, file_name_of};
pub use generated_directory::generated_directory_marker;
pub use git_identity::{OwnerDomain, identity_in_git_config};
pub use git_remote::remote_owner::{Origin, RemoteOwner};
pub use git_remote::{RemoteHost, RepositoryRemote};
pub use graph_budget::{GraphView, ViewBudget, from_configuration};
pub use graph_view::{
    DEEPEST, EdgeKind, FileStanding, GraphEdge, GraphFocus, GraphNode, GraphQuery, KindCount,
    MOST_NODES, NodeDetail, NodeKind, Provenance, TouchMark,
};
pub use index_fingerprint::IndexFingerprint;
pub use index_policy::{
    IndexPolicy, PathPattern, PatternError, PolicyClass, PolicyRule, RuleOrigin,
};
pub use insight_budget::{BudgetVerdict, InsightBudget, RunSpend};
pub use insight_cache::InsightCacheKey;
pub use insight_job::{InsightJob, InsightJobState};
pub use insight_result::{
    InsightFinding, InsightProvenance, InsightResult, ModelAnswer, RevisionSummary,
};
pub use insight_topic::RevisionTopic;
pub use insight_work::{
    ClaimedInsightJob, FailureDisposition, HeldJob, InsightFailureCategory, InsightLease,
    InsightRecovery, InsightRecoveryOutcome, InsightRetryPolicy, InsightWorkReport, LargeWork,
    PARTS_WITHOUT_ASKING, QueuedInsightJob,
};
#[cfg(test)]
pub(crate) use machine_layout::test_machine;
pub use machine_layout::{
    ContextLayout, ConversationStart, DocumentFolderSurvey, GroupAnswer, GroupMembers, GroupReason,
    LaidOutContext, LaidOutRoot, LeftOutKind, LeftOutReason, MachineGroups, NoSignal,
    RemainderAnswer, RootKind, RootSource, SetupAnswers, TranscriptFolderSurvey,
    places_of_document_folders,
};
pub use period::{LocalDay, Period, PeriodOrigin, PeriodPhrase};
pub use period_phrases::{FoundPhrase, PeriodPhrases, PeriodPhrasesError};
pub use project_signals::{GitMarker, HomePlace, ProjectMarker, RepositorySignal};
pub use proposed_context::{
    ContextBasis, Evidence, NotAttributed, ProposedContext, ProposedInclude, UnattributedSubtree,
};
pub use reconcile::ReconcileRun;
pub use reply_chunk::{
    ChunkMoment, ChunkSource, MOST_BYTES_PER_REPLY_CHUNK, Unplaceable, chunk_moments,
    exact_replies_behind, replies_behind,
};
pub use run_retention::{RetentionMode, RunRetention, RunRetentionReport};
pub use search_rank::DocumentCursor;
pub use source_commit::{AnalysisRequest, SourceCommit};
pub use source_language::{SUFFIX_TABLE_VERSION, SourceLanguage};
pub use source_path::{NormalizedPath, PathError};
pub use task_book::{ConfirmedTask, TaskBook, TaskBookError};
pub use text_normalization::{
    Normalization, NormalizationError, NormalizationSet, SnowballLanguage, StemRule,
    SuffixRuleError, SuffixRules, SuffixStep,
};
pub use transcript_folder::{
    is_temporary_folder_name, is_temporary_place, project_of_folder, transcript_folder_name,
};
pub use workspace::{
    ContentEgress, ContentEgressPermit, WorkspaceError, WorkspaceMap, WorkspaceName, WorkspaceRule,
};

#[derive(Debug, Clone)]
pub struct Greeting {
    message: String,
}

impl Greeting {
    pub fn new(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }

    pub fn message(&self) -> &str {
        self.message.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AICodingToolKind {
    ClaudeCode,
    Codex,
    Cursor,
    OpenCode,
    GitHubCopilot,
    ChatGPT,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AIToolSource {
    Cli,
    AppBundle,
    CustomPath,
}

impl AICodingToolKind {
    pub fn from_command_name(name: &str) -> Self {
        let normalized = name.trim().to_ascii_lowercase();
        let compact = normalized.replace(['-', '_', ' '], "");

        match compact.as_str() {
            "claude" | "claudecode" => Self::ClaudeCode,
            "codex" => Self::Codex,
            "cursor" => Self::Cursor,
            "opencode" | "opencodeapp" | "opencodecli" => Self::OpenCode,
            "githubcopilot" | "ghcopilot" | "copilot" => Self::GitHubCopilot,
            "chatgpt" | "chatgptapp" => Self::ChatGPT,
            _ => Self::Other(normalized),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AICodingTool {
    pub id: String,
    pub name: String,
    pub kind: AICodingToolKind,
    pub path: Option<String>,
    pub version: Option<String>,
    pub source: AIToolSource,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AIToolStats {
    pub chats: Option<u64>,
    pub tokens_used: Option<u64>,
    pub limit_remaining_percent: Option<f64>,
    pub disk_bytes: u64,
}

impl AICodingTool {
    pub fn new(name: &str, path: Option<String>, version: Option<String>) -> Self {
        let kind = AICodingToolKind::from_command_name(name);
        let source = if path
            .as_ref()
            .is_some_and(|value| value.ends_with(".app") || value.contains("/Applications/"))
        {
            AIToolSource::AppBundle
        } else if path
            .as_ref()
            .is_some_and(|value| value.contains("/bin/") || value.contains("/usr/local/bin/"))
        {
            AIToolSource::Cli
        } else {
            AIToolSource::CustomPath
        };

        Self {
            id: kind.to_string(),
            name: name.to_string(),
            kind,
            path,
            version,
            source,
        }
    }
}

impl std::fmt::Display for AICodingToolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ClaudeCode => write!(f, "claude-code"),
            Self::Codex => write!(f, "codex"),
            Self::Cursor => write!(f, "cursor"),
            Self::OpenCode => write!(f, "opencode"),
            Self::GitHubCopilot => write!(f, "github-copilot"),
            Self::ChatGPT => write!(f, "chatgpt"),
            Self::Other(value) => write!(f, "{value}"),
        }
    }
}
