// The `[index]` policy file: TOML with `include`, `metadata_only`, `exclude` and
// `max_content_size_mb`, layered on top of the built-in exclude list.
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::domain::knowledge::{KnowledgeVocabulary, VocabularyError};
use crate::domain::{
    AnalysisRules, AskBudget, IndexPolicy, InsightBudget, NormalizationSet, NormalizedPath,
    PathPattern, PatternError, PolicyClass, PolicyRule, RuleOrigin, WorkspaceError, WorkspaceMap,
    WorkspaceName, WorkspaceRule,
};

use super::super::analysis::{self, AnalysisEngine, ClaudeCodeSettings};
use super::super::anthropic::ModelSettings;
use super::super::search::{
    DEFAULT_LANGUAGES, LanguagePackError, built_in_normalizations, load_normalizations,
};

use super::super::app_directories::{self, DirectoryError};
use super::knowledge_section::{KnowledgeSection, default_vocabulary, parse_knowledge};

const CONFIG_FILE_NAME: &str = "config.toml";
/// The largest source whose content is read, and at the same time the ceiling a policy
/// file may not raise. Chosen by measurement in Stage 2T: one source of this size peaks at
/// 322–334 MiB, which leaves the whole scan more than 150 MiB below the 509 MiB bar, while
/// 32 MiB reaches 387 MiB and leaves too little. A larger text file is still indexed, by
/// its metadata: its content would be a dump or a dataset rather than a document, and tens
/// of thousands of chunks of it would be noise in search even if they fit in memory.
const DEFAULT_MAX_CONTENT_SIZE_MB: u64 = 16;

/// Always applied, before any configured rule. Keeps repositories, dependency caches,
/// build output, secrets and OS caches out of the index even without a policy file.
///
/// A list of names is the weaker half of the rule and always will be: it can only know the
/// tools that existed when it was written. The stronger half is
/// [`crate::domain::generated_directory_marker`], which reads a marker the tool itself
/// leaves and so does not age. A name belongs here only when no marker covers it and the
/// name was measured on a real tree — the counts below are from the owner's machine,
/// 200 813 files that these rules already admitted.
const BUILT_IN_EXCLUDES: &[&str] = &[
    "**/.git/**",
    "**/node_modules/**",
    "**/target/**",
    "**/.env",
    "**/*.key",
    "**/Library/Caches/**",
    // pnpm's content-addressable store. It is beside `node_modules`, not inside it, so the
    // rule above never reached it: 86 701 files, 1.06 GB — the largest single source of
    // noise on the tree measured.
    "**/.pnpm-store/**",
    // Third-party code committed on purpose, which is why `.gitignore` does not cover it:
    // 35 918 files, 0.36 GB.
    "**/vendor/**",
    // Compiled Python beside its source, so a marker in the environment root does not reach
    // it: 12 453 files, of which 2 309 stand outside any marked directory.
    "**/__pycache__/**",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedIndexPolicy {
    policy: IndexPolicy,
    workspaces: WorkspaceMap,
    engine: AnalysisEngine,
    analysis_rules: AnalysisRules,
    insight_budget: InsightBudget,
    normalizations: NormalizationSet,
    ask_budget: AskBudget,
    watch_analysis_limit: Option<usize>,
    vocabulary: KnowledgeVocabulary,
    path: PathBuf,
    found: bool,
}

impl LoadedIndexPolicy {
    pub fn policy(&self) -> &IndexPolicy {
        &self.policy
    }

    /// Which context owns which paths. A file with no `[workspace.…]` section declares no
    /// context, and then nothing is indexable until one is written.
    pub fn workspaces(&self) -> &WorkspaceMap {
        &self.workspaces
    }

    /// Which engine a context that allows it is answered by. One choice, read by both
    /// `index jobs run` and `ask`.
    pub fn engine(&self) -> &AnalysisEngine {
        &self.engine
    }

    /// Which revisions are analysed at all: the ones a document rule covers, and the ones
    /// the owner approved. A revision no rule covers gets no job and waits on the review
    /// list.
    pub fn analysis_rules(&self) -> &AnalysisRules {
        &self.analysis_rules
    }

    /// What one run of the queue may send, which belongs to the work rather than to the
    /// engine and is therefore the same whichever engine answers.
    pub fn insight_budget(&self) -> InsightBudget {
        self.insight_budget
    }

    /// Which languages this corpus is written in, as the configuration declares them. The
    /// search index is written and read with exactly these, in this order.
    pub fn normalizations(&self) -> &NormalizationSet {
        &self.normalizations
    }

    /// How much of this context may leave in one question: how many excerpts and how many
    /// bytes of them together.
    pub fn ask_budget(&self) -> AskBudget {
        self.ask_budget
    }

    /// How many insight jobs one `index watch` pass may work, or nothing when analysis was
    /// not turned on. Nothing is the default, and it is what makes a watcher left running
    /// overnight cost nothing beyond the disk it walks.
    pub fn watch_analysis_limit(&self) -> Option<usize> {
        self.watch_analysis_limit
    }

    /// The entity and relation types the knowledge layer extracts (ADR 0048).
    pub fn vocabulary(&self) -> &KnowledgeVocabulary {
        &self.vocabulary
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `false` when the file does not exist and only the built-in defaults apply.
    pub fn found(&self) -> bool {
        self.found
    }
}

pub fn load_index_policy() -> Result<LoadedIndexPolicy, ConfigError> {
    let path = config_path()?;
    let home = env::var_os("HOME").map(PathBuf::from);
    let data_directory = own_data_directory();
    match fs::read_to_string(&path) {
        Ok(text) => {
            let parts = parse_index_policy(&text, home.as_deref(), data_directory.as_deref())?;
            Ok(LoadedIndexPolicy {
                policy: parts.policy,
                workspaces: parts.workspaces,
                engine: parts.engine,
                analysis_rules: parts.analysis_rules,
                insight_budget: parts.insight_budget,
                normalizations: parts.normalizations,
                ask_budget: parts.ask_budget,
                watch_analysis_limit: parts.watch_analysis_limit,
                vocabulary: parts.vocabulary,
                path,
                found: true,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LoadedIndexPolicy {
            policy: built_in_policy(data_directory.as_deref())?,
            workspaces: empty_workspaces(),
            engine: default_engine_settings(),
            analysis_rules: default_analysis_rules(),
            insight_budget: default_insight_budget(),
            normalizations: default_normalizations(),
            ask_budget: default_ask_budget(),
            watch_analysis_limit: None,
            vocabulary: default_vocabulary(),
            path,
            found: false,
        }),
        Err(error) => Err(ConfigError::Read {
            path,
            error: error.to_string(),
        }),
    }
}

/// Where Crosskit keeps its own files, resolved the way the scan will see it.
///
/// Canonical, because the scan resolves its root and a macOS temp or home path resolves
/// through a symlink; a rule written from the unresolved spelling would cover nothing. When
/// it cannot be located at all, no command can open a database either.
fn own_data_directory() -> Option<PathBuf> {
    let directory = app_directories::data_directory().ok()?;
    Some(resolved_even_if_absent(&directory))
}

/// The canonical spelling of a path that may not exist yet: the deepest ancestor that does
/// exist is resolved and the rest is appended to it.
///
/// The first run is exactly the case that matters here. The policy is read before anything
/// creates the data directory, so on that run the directory is not there to resolve — and
/// if the rule were written in the unresolved spelling, the very first scan would index the
/// database it was about to create.
fn resolved_even_if_absent(path: &Path) -> PathBuf {
    if let Ok(resolved) = fs::canonicalize(path) {
        return resolved;
    }
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut current = path;
    while let (Some(parent), Some(name)) = (current.parent(), current.file_name()) {
        tail.push(name);
        if let Ok(mut resolved) = fs::canonicalize(parent) {
            resolved.extend(tail.iter().rev());
            return resolved;
        }
        current = parent;
    }
    path.to_path_buf()
}

/// `CROSSKIT_CONFIG` names the file itself; otherwise the file is `config.toml` inside
/// the data directory next to the graph database.
/// Where the policy file is read from, whether or not it exists or loads.
pub fn config_file() -> Result<PathBuf, ConfigError> {
    config_path()
}

fn config_path() -> Result<PathBuf, ConfigError> {
    config_path_from(
        env::var_os("CROSSKIT_CONFIG").map(PathBuf::from),
        app_directories::data_directory(),
    )
}

fn config_path_from(
    explicit_file: Option<PathBuf>,
    data_directory: Result<PathBuf, DirectoryError>,
) -> Result<PathBuf, ConfigError> {
    if let Some(path) = explicit_file {
        return Ok(path);
    }
    data_directory
        .map(|directory| directory.join(CONFIG_FILE_NAME))
        .map_err(ConfigError::Directory)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[serde(default)]
    index: IndexSection,
    /// `[workspace.<name>]`, read in the order TOML declares them. A map with a defined
    /// order, so two loads of one file produce the same rules and the same message.
    #[serde(default)]
    workspace: BTreeMap<String, WorkspaceSection>,
    #[serde(default)]
    insight: InsightSection,
    #[serde(default)]
    analysis: AnalysisSection,
    #[serde(default)]
    search: SearchSection,
    #[serde(default)]
    ask: AskSection,
    #[serde(default)]
    watch: WatchSection,
    #[serde(default)]
    knowledge: KnowledgeSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceSection {
    #[serde(default)]
    include: Vec<String>,
    /// Whether this context's file content may leave the machine. Absent means no: a
    /// context that says nothing has not said yes.
    #[serde(default)]
    send_content_outside: bool,
}

/// `[insight]`: what one run may send. Which engine answers, and how that engine is
/// configured, is `[analysis]` — the same choice `ask` reads, so the two roads cannot
/// disagree about who is answering.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InsightSection {
    #[serde(default = "default_max_input_bytes")]
    max_input_bytes: i64,
    #[serde(default = "default_max_run_bytes")]
    max_run_bytes: i64,
}

impl Default for InsightSection {
    fn default() -> Self {
        Self {
            max_input_bytes: default_max_input_bytes(),
            max_run_bytes: default_max_run_bytes(),
        }
    }
}

/// `[analysis]`: which engine answers, and its own settings under its own name.
///
/// The engine is named once and read by both roads, `index jobs run` and `ask`. Its
/// settings live under it, so a field that belongs to one engine cannot be written for
/// another and quietly do nothing: `deny_unknown_fields` on each subsection turns that into
/// a refusal to load, and so does a subsection for an engine that was not chosen.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnalysisSection {
    #[serde(default = "default_engine")]
    engine: String,
    /// What is analysed without asking. Documents, by default.
    #[serde(default = "default_automatic_analysis")]
    automatic: Vec<String>,
    /// What the owner looked at and said yes to. Empty until they do, and written by
    /// `index review approve` rather than by hand — though by hand works too.
    #[serde(default)]
    approved: Vec<String>,
    #[serde(rename = "claude-code")]
    claude_code: Option<ClaudeCodeEngineSection>,
    #[serde(rename = "anthropic-api")]
    anthropic_api: Option<AnthropicApiEngineSection>,
}

/// A file with no `[analysis]` section says the same thing as one that names the default.
impl Default for AnalysisSection {
    fn default() -> Self {
        Self {
            engine: default_engine(),
            automatic: default_automatic_analysis(),
            approved: Vec::new(),
            claude_code: None,
            anthropic_api: None,
        }
    }
}

/// The installed subscription tool. It has no key and no endpoint; the model is the tool's
/// own unless an operator names one.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeCodeEngineSection {
    #[serde(default)]
    model: String,
}

/// The vendor's API. `effort` belongs here and only here: it is a field of that API's
/// request, and the tool has nothing to do with it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnthropicApiEngineSection {
    #[serde(default = "default_model")]
    model: String,
    #[serde(default = "default_effort")]
    effort: String,
}

impl Default for AnthropicApiEngineSection {
    fn default() -> Self {
        Self {
            model: default_model(),
            effort: default_effort(),
        }
    }
}

/// The subscription tool, because the tokens of a subscription are the ones its owner has
/// already paid for. The API engine is a choice, not the road.
fn default_engine() -> String {
    analysis::CLAUDE_CODE.to_string()
}

/// What analyses itself: documents, and only documents.
///
/// Argued from the owner's own tree rather than from a sense of what documents are. Of
/// 25 952 revisions the index admits there, `.md` is 1 662 and `.txt` is 41 — together
/// 6.6 %, about five hours of a subscription against seventy-seven for everything. The rest
/// is source code, and what a person wants said about their code is a decision they should
/// make rather than find already paid for.
///
/// `.rst`, `.org` and `.rtf` were looked for and found once each, which is not a number a
/// standing rule is written on; `.markdown`, `.adoc`, `.asciidoc` and `.tex` were not on the
/// tree at all. An operator with such a tree adds one line.
fn default_automatic_analysis() -> Vec<String> {
    vec!["**/*.md".to_string(), "**/*.txt".to_string()]
}

/// The masks that decide which revisions are analysed at all.
fn parse_analysis_rules(section: &AnalysisSection) -> Result<AnalysisRules, ConfigError> {
    Ok(AnalysisRules::new(
        parse_analysis_masks(&section.automatic, "analysis automatic")?,
        parse_analysis_masks(&section.approved, "analysis approved")?,
    ))
}

fn parse_analysis_masks(
    patterns: &[String],
    key: &'static str,
) -> Result<Vec<PathPattern>, ConfigError> {
    patterns
        .iter()
        .map(|pattern| {
            PathPattern::parse(pattern).map_err(|error| ConfigError::InvalidPattern {
                key,
                pattern: pattern.clone(),
                error,
            })
        })
        .collect()
}

/// The engine the file chose, with its settings, or a refusal naming what this program
/// knows.
fn parse_analysis(section: AnalysisSection) -> Result<AnalysisEngine, ConfigError> {
    let engine = match section.engine.as_str() {
        analysis::CLAUDE_CODE => {
            refuse_settings_of_the_other_engine(
                section.anthropic_api.is_some(),
                analysis::ANTHROPIC_API,
                analysis::CLAUDE_CODE,
            )?;
            AnalysisEngine::ClaudeCode(ClaudeCodeSettings::new(
                section.claude_code.unwrap_or_default().model,
            ))
        }
        analysis::ANTHROPIC_API => {
            refuse_settings_of_the_other_engine(
                section.claude_code.is_some(),
                analysis::CLAUDE_CODE,
                analysis::ANTHROPIC_API,
            )?;
            let settings = section.anthropic_api.unwrap_or_default();
            AnalysisEngine::AnthropicApi(ModelSettings::new(settings.model, settings.effort))
        }
        unknown => {
            return Err(ConfigError::UnknownEngine {
                named: unknown.to_string(),
            });
        }
    };
    Ok(engine)
}

/// A section that configures an engine nobody chose is refused rather than ignored. The
/// alternative is a file that says one thing and a program that does another, which is the
/// exact shape of a setting nobody notices is dead.
fn refuse_settings_of_the_other_engine(
    present: bool,
    named: &'static str,
    chosen: &'static str,
) -> Result<(), ConfigError> {
    match present {
        true => Err(ConfigError::EngineSettingsUnused { named, chosen }),
        false => Ok(()),
    }
}

/// The default is chosen for the shape of the work, not for the ceiling of the lineup: one
/// short topic label per revision, under a two-field schema, and a queue on a real tree is
/// tens of thousands of jobs. The dated id rather than the `claude-haiku-4-5` alias, so a
/// later model cannot arrive here silently; an operator who wants a larger model writes one
/// line. What this costs against a mid-tier model, and what is still unmeasured about its
/// answers, is in ADR 0010.
fn default_model() -> String {
    "claude-haiku-4-5-20251001".to_string()
}

/// Empty, which leaves the field out of the request entirely: the default model does not
/// accept an effort level, and sending one to a model that rejects it fails the call. A
/// model that does accept one takes it from the configuration.
fn default_effort() -> String {
    String::new()
}

/// One document's worth of text. A source above this is refused without being read.
fn default_max_input_bytes() -> i64 {
    200_000
}

/// What one `index jobs run` may send in total. Reached, the remaining jobs go back in the
/// queue for the next run rather than being counted as failures of their own.
fn default_max_run_bytes() -> i64 {
    20_000_000
}

/// `[watch]`: what `index watch` does besides watching.
///
/// Analysis is off unless it is both turned on and given a bound. Two keys rather than one
/// because they answer two questions an operator should answer on purpose: whether a
/// watcher may spend money at all, and how much of the queue one pass may take. A file that
/// says neither leaves the watcher doing exactly what it did before this existed.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchSection {
    #[serde(default)]
    analyze: bool,
    analyze_limit: Option<usize>,
}

/// How many jobs a watcher pass may work, from what the file said.
///
/// Turning analysis on without a bound is refused rather than given a default. A default
/// here would be this program deciding how much of somebody's money one pass is worth, and
/// the operator would find out from the bill.
fn parse_watch_analysis(section: &WatchSection) -> Result<Option<usize>, ConfigError> {
    if !section.analyze {
        return Ok(None);
    }
    match section.analyze_limit {
        Some(limit) if limit > 0 => Ok(Some(limit)),
        Some(_) => Err(ConfigError::Watch(
            "analyze_limit must be at least 1; a watcher allowed to work no job at all is analysis turned off, which is `analyze = false`",
        )),
        None => Err(ConfigError::Watch(
            "analyze = true also needs analyze_limit = <N>, the largest number of insight jobs one watcher pass may work",
        )),
    }
}

/// `[search]`: which languages the corpus is written in. The list is declared rather than
/// detected, and a name this build has no normalization for fails the load: a corpus
/// indexed by rules nobody asked for would answer wrongly with nothing to show for it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchSection {
    #[serde(default = "default_language_names")]
    languages: Vec<String>,
}

/// `[ask]`: how much of a context may leave in one question.
///
/// Two numbers, because they bound different things: how many excerpts a question carries
/// and how many bytes of them together. Both are the operator's to set, because both decide
/// what leaves the machine — the permit says whether content may leave at all, and this says
/// how much of it a single question takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AskSection {
    #[serde(default = "default_max_sources")]
    max_sources: usize,
    #[serde(default = "default_max_question_bytes")]
    max_bytes: usize,
}

impl Default for AskSection {
    fn default() -> Self {
        Self {
            max_sources: default_max_sources(),
            max_bytes: default_max_question_bytes(),
        }
    }
}

/// Enough excerpts for a question about a tree to reach more than one file, few enough that
/// each of them is a paragraph rather than a line.
fn default_max_sources() -> usize {
    8
}

/// About forty kilobytes: ten chunks of the size this program cuts, which is a few pages of
/// prose and well inside what a model reads in one message.
fn default_max_question_bytes() -> usize {
    40_000
}

impl Default for SearchSection {
    fn default() -> Self {
        Self {
            languages: default_language_names(),
        }
    }
}

fn default_language_names() -> Vec<String> {
    DEFAULT_LANGUAGES
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexSection {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    metadata_only: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    max_content_size_mb: Option<u64>,
}

/// Everything one policy file says.
#[derive(Debug)]
struct PolicyParts {
    policy: IndexPolicy,
    workspaces: WorkspaceMap,
    engine: AnalysisEngine,
    analysis_rules: AnalysisRules,
    insight_budget: InsightBudget,
    normalizations: NormalizationSet,
    ask_budget: AskBudget,
    watch_analysis_limit: Option<usize>,
    vocabulary: KnowledgeVocabulary,
}

/// Built-in excludes come first, then configured `exclude`, `metadata_only` and
/// `include`. Declaration order only breaks ties between equally specific rules.
fn parse_index_policy(
    text: &str,
    home: Option<&Path>,
    data_directory: Option<&Path>,
) -> Result<PolicyParts, ConfigError> {
    let file: PolicyFile =
        toml::from_str(text).map_err(|error| ConfigError::Parse(error.to_string()))?;
    let mut rules = built_in_rules(data_directory)?;
    append_rules(
        &mut rules,
        PolicyClass::Exclude,
        "exclude",
        &file.index.exclude,
        home,
    )?;
    append_rules(
        &mut rules,
        PolicyClass::Metadata,
        "metadata_only",
        &file.index.metadata_only,
        home,
    )?;
    append_rules(
        &mut rules,
        PolicyClass::Include,
        "include",
        &file.index.include,
        home,
    )?;

    // A policy may read less of a file than the compiled maximum, never more: the maximum
    // is what the memory of a pass was measured against.
    let max_content_size_mb = file
        .index
        .max_content_size_mb
        .unwrap_or(DEFAULT_MAX_CONTENT_SIZE_MB)
        .min(DEFAULT_MAX_CONTENT_SIZE_MB);
    Ok(PolicyParts {
        policy: IndexPolicy::new(rules, content_limit_bytes(max_content_size_mb)?),
        workspaces: parse_workspaces(&file.workspace, home)?,
        analysis_rules: parse_analysis_rules(&file.analysis)?,
        engine: parse_analysis(file.analysis)?,
        insight_budget: InsightBudget::new(
            file.insight.max_input_bytes,
            file.insight.max_run_bytes,
        ),
        normalizations: load_normalizations(&file.search.languages)
            .map_err(ConfigError::Language)?,
        ask_budget: AskBudget::new(file.ask.max_sources, file.ask.max_bytes),
        watch_analysis_limit: parse_watch_analysis(&file.watch)?,
        vocabulary: parse_knowledge(&file.knowledge).map_err(ConfigError::Knowledge)?,
    })
}

/// Contexts declared as `[workspace.<name>]` with the paths they cover. The map refuses two
/// contexts that could cover one path, so the boundary is checked once, here, and never
/// again at query time.
fn parse_workspaces(
    sections: &BTreeMap<String, WorkspaceSection>,
    home: Option<&Path>,
) -> Result<WorkspaceMap, ConfigError> {
    let mut rules = Vec::new();
    let mut allow_egress = Vec::new();
    for (name, section) in sections {
        let workspace = WorkspaceName::parse(name).map_err(ConfigError::Workspace)?;
        if section.send_content_outside {
            allow_egress.push(workspace.clone());
        }
        for pattern in &section.include {
            let expanded =
                expand_home(pattern, home).map_err(|reason| ConfigError::HomeExpansion {
                    key: "workspace include",
                    pattern: pattern.clone(),
                    reason,
                })?;
            let parsed =
                PathPattern::parse(&expanded).map_err(|error| ConfigError::InvalidPattern {
                    key: "workspace include",
                    pattern: pattern.clone(),
                    error,
                })?;
            rules.push(WorkspaceRule::new(workspace.clone(), parsed));
        }
    }
    WorkspaceMap::with_egress(rules, allow_egress).map_err(ConfigError::Workspace)
}

/// What a file with no `[analysis]` section, or no file at all, means: the tool already
/// installed, with the model the subscription decided.
fn default_engine_settings() -> AnalysisEngine {
    AnalysisEngine::ClaudeCode(ClaudeCodeSettings::new(String::new()))
}

/// What a file with no `[analysis]` section, or no file at all, analyses: documents.
fn default_analysis_rules() -> AnalysisRules {
    AnalysisRules::new(
        default_automatic_analysis()
            .iter()
            .map(|pattern| PathPattern::parse(pattern).expect("a compiled-in mask must parse"))
            .collect(),
        Vec::new(),
    )
}

/// What a file with no `[insight]` section, or no file at all, means.
fn default_insight_budget() -> InsightBudget {
    InsightBudget::new(default_max_input_bytes(), default_max_run_bytes())
}

/// What a file with no `[ask]` section, or no file at all, means.
fn default_ask_budget() -> AskBudget {
    AskBudget::new(default_max_sources(), default_max_question_bytes())
}

/// What a file with no `[search]` section, or no file at all, means.
fn default_normalizations() -> NormalizationSet {
    built_in_normalizations()
}

/// A file that declares no context, or no file at all: nothing is covered, so nothing is
/// indexable until a context is written.
fn empty_workspaces() -> WorkspaceMap {
    WorkspaceMap::new(Vec::new()).expect("no rules cannot overlap")
}

fn built_in_policy(data_directory: Option<&Path>) -> Result<IndexPolicy, ConfigError> {
    Ok(IndexPolicy::new(
        built_in_rules(data_directory)?,
        content_limit_bytes(DEFAULT_MAX_CONTENT_SIZE_MB)?,
    ))
}

fn built_in_rules(data_directory: Option<&Path>) -> Result<Vec<PolicyRule>, ConfigError> {
    let mut rules: Vec<PolicyRule> = BUILT_IN_EXCLUDES
        .iter()
        .map(|pattern| {
            PathPattern::parse(pattern)
                .map(|parsed| PolicyRule::new(PolicyClass::Exclude, parsed, RuleOrigin::BuiltIn))
                .map_err(|error| ConfigError::InvalidPattern {
                    key: "built-in exclude",
                    pattern: pattern.to_string(),
                    error,
                })
        })
        .collect::<Result<_, _>>()?;
    rules.extend(own_data_directory_rule(data_directory));
    Ok(rules)
}

/// Crosskit's own directory is never indexed.
///
/// The graph database and the search index of every context live there. Indexing them
/// would put the program's own files in its own graph, and — the watch-probe lesson — every
/// write to the index would look like a change on disk, which starts a pass, which writes
/// the index again. The rule is literal rather than a glob, so a data directory whose name
/// happens to contain `*` or `?` excludes itself and nothing else.
fn own_data_directory_rule(data_directory: Option<&Path>) -> Option<PolicyRule> {
    let path = NormalizedPath::from_absolute(data_directory?.to_str()?).ok()?;
    Some(PolicyRule::new(
        PolicyClass::Exclude,
        PathPattern::literal(&path),
        RuleOrigin::BuiltIn,
    ))
}

fn append_rules(
    rules: &mut Vec<PolicyRule>,
    class: PolicyClass,
    key: &'static str,
    patterns: &[String],
    home: Option<&Path>,
) -> Result<(), ConfigError> {
    for pattern in patterns {
        let expanded = expand_home(pattern, home).map_err(|reason| ConfigError::HomeExpansion {
            key,
            pattern: pattern.clone(),
            reason,
        })?;
        let parsed =
            PathPattern::parse(&expanded).map_err(|error| ConfigError::InvalidPattern {
                key,
                pattern: pattern.clone(),
                error,
            })?;
        rules.push(PolicyRule::new(class, parsed, RuleOrigin::Configuration));
    }
    Ok(())
}

/// Only `~` and `~/...` are expanded; `~user` forms are rejected rather than guessed.
fn expand_home(pattern: &str, home: Option<&Path>) -> Result<String, &'static str> {
    if pattern != "~" && !pattern.starts_with("~/") {
        if pattern.starts_with('~') {
            return Err("only `~` and `~/` are supported");
        }
        return Ok(pattern.to_string());
    }
    let home = home.ok_or("HOME is not set")?;
    let home = home.to_string_lossy();
    Ok(format!("{}{}", home.trim_end_matches('/'), &pattern[1..]))
}

fn content_limit_bytes(megabytes: u64) -> Result<u64, ConfigError> {
    megabytes
        .checked_mul(1024 * 1024)
        .ok_or(ConfigError::ContentLimitTooLarge(megabytes))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Directory(DirectoryError),
    Read {
        path: PathBuf,
        error: String,
    },
    Parse(String),
    InvalidPattern {
        key: &'static str,
        pattern: String,
        error: PatternError,
    },
    HomeExpansion {
        key: &'static str,
        pattern: String,
        reason: &'static str,
    },
    ContentLimitTooLarge(u64),
    /// `[analysis] engine` named something this program has no engine for.
    UnknownEngine {
        named: String,
    },
    /// The file configures an engine it did not choose. Refused rather than ignored.
    EngineSettingsUnused {
        named: &'static str,
        chosen: &'static str,
    },
    Workspace(WorkspaceError),
    Language(LanguagePackError),
    Watch(&'static str),
    Knowledge(VocabularyError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory(error) => write!(f, "locate policy file: {error}"),
            Self::Read { path, error } => {
                write!(f, "read policy file {}: {error}", path.display())
            }
            Self::Parse(error) => write!(f, "parse policy file: {error}"),
            Self::UnknownEngine { named } => write!(
                f,
                "[analysis] engine is {named:?}, which is not an engine this program has; \
                 the engines are {}",
                analysis::ENGINE_NAMES.join(", ")
            ),
            Self::EngineSettingsUnused { named, chosen } => write!(
                f,
                "[analysis.{named}] configures an engine this file did not choose: \
                 [analysis] engine is {chosen:?}, so nothing under [analysis.{named}] would \
                 be read"
            ),
            Self::InvalidPattern {
                key,
                pattern,
                error,
            } => write!(f, "policy {key} pattern {pattern:?}: {error}"),
            Self::HomeExpansion {
                key,
                pattern,
                reason,
            } => write!(f, "policy {key} pattern {pattern:?}: {reason}"),
            Self::ContentLimitTooLarge(megabytes) => {
                write!(
                    f,
                    "max_content_size_mb {megabytes} exceeds the supported range"
                )
            }
            Self::Workspace(error) => write!(f, "policy contexts: {error}"),
            Self::Language(error) => write!(f, "search languages: {error}"),
            Self::Watch(reason) => write!(f, "policy [watch]: {reason}"),
            Self::Knowledge(error) => write!(f, "policy {error}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ConfigError, DEFAULT_MAX_CONTENT_SIZE_MB, config_path_from, parse_index_policy};
    use crate::domain::{NormalizedPath, PolicyClass, RuleOrigin};
    use crate::infrastructure::app_directories::DirectoryError;

    const DOCUMENTED_EXAMPLE: &str = r#"
[index]
include = [
    "~/Documents",
    "~/Desktop",
    "~/Downloads",
    "~/Projects"
]

metadata_only = [
    "~/Pictures",
    "~/Movies"
]

exclude = [
    "**/.git/**",
    "**/node_modules/**",
    "**/target/**",
    "**/.env",
    "**/*.key",
    "**/Library/Caches/**"
]

max_content_size_mb = 8
"#;

    /// Analysis inside the watcher costs money, so it takes two deliberate keys and neither
    /// of them has a helpful default.
    #[test]
    fn a_watcher_analyses_nothing_unless_the_file_turns_it_on_and_bounds_it() {
        let analysis_of = |text: &str| {
            parse_index_policy(text, None, None).map(|parts| parts.watch_analysis_limit)
        };

        assert_eq!(analysis_of("").unwrap(), None);
        assert_eq!(analysis_of("[watch]\n").unwrap(), None);
        // A bound on its own is inert: nothing was turned on.
        assert_eq!(analysis_of("[watch]\nanalyze_limit = 5\n").unwrap(), None);
        assert_eq!(
            analysis_of("[watch]\nanalyze = true\nanalyze_limit = 5\n").unwrap(),
            Some(5)
        );

        // Turned on without a bound, or with one that works nothing, is refused at load:
        // a default here would be this program deciding what a pass of somebody's money is
        // worth, and they would find out from the bill.
        for refused in [
            "[watch]\nanalyze = true\n",
            "[watch]\nanalyze = true\nanalyze_limit = 0\n",
        ] {
            let error = analysis_of(refused).unwrap_err();
            assert!(
                matches!(error, ConfigError::Watch(_))
                    && error.to_string().contains("analyze_limit"),
                "{refused:?} produced {error}"
            );
        }
        assert!(
            analysis_of("[watch]\nanalyse = true\n").is_err(),
            "an unknown key is refused"
        );
    }

    fn home() -> Option<&'static Path> {
        Some(Path::new("/Users/alice/"))
    }

    /// Only the policy half of a load; the context half has tests of its own.
    fn policy_of(text: &str, home: Option<&Path>) -> crate::domain::IndexPolicy {
        parse_index_policy(text, home, None).unwrap().policy
    }

    fn decide(policy: &crate::domain::IndexPolicy, path: &str) -> PolicyClass {
        policy
            .decide(&NormalizedPath::from_absolute(path).unwrap())
            .class()
    }

    #[test]
    fn parses_the_documented_example_and_expands_home() {
        let policy = policy_of(DOCUMENTED_EXAMPLE, home());

        assert_eq!(policy.max_content_bytes(), 8 * 1024 * 1024);
        assert_eq!(
            decide(&policy, "/Users/alice/Documents/plan.md"),
            PolicyClass::Include
        );
        assert_eq!(
            decide(&policy, "/Users/alice/Pictures/cat.jpg"),
            PolicyClass::Metadata
        );
        assert_eq!(
            decide(&policy, "/Users/alice/Projects/app/.env"),
            PolicyClass::Exclude
        );
        assert_eq!(
            decide(&policy, "/Users/bob/Documents/plan.md"),
            PolicyClass::Exclude
        );
        let built_in = policy
            .decide(&NormalizedPath::from_absolute("/Users/alice/Documents/.git/config").unwrap());
        assert_eq!(built_in.class(), PolicyClass::Exclude);
        assert_eq!(built_in.rule().unwrap().origin(), RuleOrigin::BuiltIn);
    }

    /// Contexts are read as written, and an unknown key inside one is refused like every
    /// other unknown key in this file.
    #[test]
    fn context_sections_declare_coverage_and_refuse_what_they_do_not_know() {
        let workspaces = parse_index_policy(
            "[workspace.work]\ninclude = [\"/tree/work\"]\n\n[workspace.personal]\ninclude = [\"~/Personal\"]\n",
            home(),
            None,
        )
        .unwrap()
        .workspaces;

        assert_eq!(
            workspaces
                .workspace_of(&NormalizedPath::from_absolute("/tree/work/a.md").unwrap())
                .map(|name| name.as_str().to_string()),
            Some("work".to_string())
        );
        assert_eq!(
            workspaces
                .workspace_of(&NormalizedPath::from_absolute("/Users/alice/Personal/b.md").unwrap())
                .map(|name| name.as_str().to_string()),
            Some("personal".to_string()),
            "a context pattern expands ~ like every other pattern"
        );
        assert_eq!(
            workspaces.workspace_of(&NormalizedPath::from_absolute("/tree/other").unwrap()),
            None
        );

        let unknown = parse_index_policy(
            "[workspace.work]\ninclude_all = [\"/tree\"]\n",
            home(),
            None,
        );
        assert!(matches!(unknown, Err(ConfigError::Parse(_))));
    }

    /// The overlap of two contexts is a load error, and it arrives as one message a person
    /// can act on rather than as a surprise at query time.
    #[test]
    fn two_contexts_covering_one_path_fail_the_load() {
        let error = parse_index_policy(
            "[workspace.work]\ninclude = [\"/tree\"]\n\n[workspace.personal]\ninclude = [\"/tree/personal\"]\n",
            home(),
            None,
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("work") && message.contains("personal"),
            "{message}"
        );
        assert!(message.contains("/tree/personal"), "{message}");
    }

    #[test]
    fn built_in_excludes_apply_without_a_policy_file() {
        let policy = policy_of("", None);

        assert_eq!(
            policy.max_content_bytes(),
            DEFAULT_MAX_CONTENT_SIZE_MB * 1024 * 1024
        );
        assert_eq!(
            decide(&policy, "/repo/node_modules/pkg/index.js"),
            PolicyClass::Exclude
        );
        assert_eq!(decide(&policy, "/repo/README.md"), PolicyClass::Exclude);
    }

    #[test]
    fn unknown_keys_and_invalid_patterns_are_rejected() {
        let typo = parse_index_policy("[index]\nmetadata-only = [\"~/Pictures\"]\n", home(), None);
        assert!(matches!(typo, Err(ConfigError::Parse(_))));

        let relative = parse_index_policy("[index]\nexclude = [\"node_modules\"]\n", home(), None);
        assert!(matches!(
            relative,
            Err(ConfigError::InvalidPattern { key: "exclude", .. })
        ));

        let user_home =
            parse_index_policy("[index]\ninclude = [\"~bob/Documents\"]\n", home(), None);
        assert!(matches!(
            user_home,
            Err(ConfigError::HomeExpansion { key: "include", .. })
        ));

        let no_home = parse_index_policy("[index]\ninclude = [\"~/Documents\"]\n", None, None);
        assert!(matches!(no_home, Err(ConfigError::HomeExpansion { .. })));
    }

    #[test]
    fn explicit_config_path_wins_over_the_data_directory() {
        assert_eq!(
            config_path_from(
                Some(PathBuf::from("/etc/crosskit.toml")),
                Ok(PathBuf::from("/data"))
            )
            .unwrap(),
            PathBuf::from("/etc/crosskit.toml")
        );
        assert_eq!(
            config_path_from(None, Ok(PathBuf::from("/data"))).unwrap(),
            PathBuf::from("/data/config.toml")
        );
        assert_eq!(
            config_path_from(None, Err(DirectoryError::HomeNotSet)),
            Err(ConfigError::Directory(DirectoryError::HomeNotSet))
        );
    }
}

#[cfg(test)]
mod content_limit_tests {
    use super::{DEFAULT_MAX_CONTENT_SIZE_MB, parse_index_policy};

    /// A policy may read less of a file than the measurement covered, never more: the
    /// compiled maximum is what the memory of a pass was measured against, so a file that
    /// asks for more is held to it rather than refused.
    #[test]
    fn a_policy_may_lower_the_content_limit_but_not_raise_it() {
        let lowered = parse_index_policy("[index]\nmax_content_size_mb = 4\n", None, None)
            .unwrap()
            .policy;
        assert_eq!(lowered.max_content_bytes(), 4 * 1024 * 1024);

        let raised = parse_index_policy("[index]\nmax_content_size_mb = 512\n", None, None)
            .unwrap()
            .policy;
        assert_eq!(
            raised.max_content_bytes(),
            DEFAULT_MAX_CONTENT_SIZE_MB * 1024 * 1024,
            "a policy must not raise the limit above the compiled maximum"
        );
    }
}

#[cfg(test)]
mod engine_default_tests {
    use super::{
        AnalysisEngine, ClaudeCodeSettings, ModelSettings, analysis, default_model,
        parse_index_policy,
    };

    fn engine_of(text: &str) -> AnalysisEngine {
        parse_index_policy(text, None, None).unwrap().engine
    }

    fn refusal(text: &str) -> String {
        parse_index_policy(text, None, None)
            .expect_err("this file must not load")
            .to_string()
    }

    /// The engine a file says nothing about. It is the tool already installed, because the
    /// tokens of a subscription are the ones its owner has already paid for; the model is
    /// left to the subscription, which has already decided one.
    #[test]
    fn a_file_without_an_analysis_section_uses_the_installed_tool() {
        assert_eq!(
            engine_of("[index]\n"),
            AnalysisEngine::ClaudeCode(ClaudeCodeSettings::new(String::new()))
        );
    }

    /// The other engine, chosen by name, with the settings that belong to it.
    #[test]
    fn the_api_engine_is_chosen_by_name_and_carries_its_own_settings() {
        assert_eq!(
            engine_of(
                "[analysis]\nengine = \"anthropic-api\"\n\n[analysis.anthropic-api]\nmodel = \"claude-opus-5\"\neffort = \"low\"\n"
            ),
            AnalysisEngine::AnthropicApi(ModelSettings::new(
                "claude-opus-5".to_string(),
                "low".to_string()
            ))
        );
    }

    /// The API engine's default model is a dated snapshot, not a moving alias: an alias
    /// would let a different model, with a different price and different answers, arrive
    /// without anyone changing a line of this program.
    #[test]
    fn the_default_api_model_is_pinned_to_a_dated_snapshot() {
        let id = default_model();
        let (_, date) = id
            .rsplit_once('-')
            .expect("a dated model id ends in -YYYYMMDD");
        assert!(
            date.len() == 8 && date.chars().all(|character| character.is_ascii_digit()),
            "{id:?} is not pinned to a dated snapshot"
        );
    }

    /// An engine this program has no implementation for is a refusal to load, naming what
    /// it does have. Falling back to the default would be this program choosing where
    /// somebody's file content goes.
    #[test]
    fn an_unknown_engine_refuses_to_load_and_names_the_engines_there_are() {
        let message = refusal("[analysis]\nengine = \"ollama\"\n");
        assert!(message.contains("ollama"), "{message}");
        for name in analysis::ENGINE_NAMES {
            assert!(message.contains(name), "{message}");
        }
    }

    /// A setting that belongs to an engine the file did not choose does nothing. Doing
    /// nothing quietly is the shape of a field nobody notices is dead, so it is refused.
    #[test]
    fn settings_of_an_engine_that_was_not_chosen_refuse_to_load() {
        let message = refusal(
            "[analysis]\nengine = \"claude-code\"\n\n[analysis.anthropic-api]\nmodel = \"claude-opus-5\"\n",
        );
        assert!(message.contains("anthropic-api"), "{message}");
        assert!(message.contains("claude-code"), "{message}");
    }

    /// And a setting of the chosen engine that that engine has no use for. `effort` is a
    /// field of one vendor's API request; the installed tool has nothing to do with it.
    #[test]
    fn a_setting_the_chosen_engine_has_no_use_for_refuses_to_load() {
        let message = refusal(
            "[analysis]\nengine = \"claude-code\"\n\n[analysis.claude-code]\neffort = \"high\"\n",
        );
        assert!(message.contains("effort"), "{message}");
    }

    /// The fields that moved. A file written for the shape before this stage named the
    /// model under `[insight]`; it now says nothing, and saying nothing quietly is what
    /// this program does not do.
    #[test]
    fn the_settings_that_moved_out_of_the_insight_section_are_refused_where_they_were() {
        let message = refusal("[insight]\nmodel = \"claude-opus-5\"\n");
        assert!(message.contains("model"), "{message}");
    }
}

#[cfg(test)]
mod data_directory_tests {
    use std::fs;

    use super::{own_data_directory_rule, resolved_even_if_absent};
    use crate::domain::{NormalizedPath, PolicyClass};

    /// The one case the scan cannot forgive: a data directory that does not exist yet, under
    /// a path that resolves through a symlink. macOS temp directories are exactly that, and
    /// so the rule is written in the spelling the walker will produce rather than the one
    /// the environment handed over.
    #[test]
    fn a_directory_that_does_not_exist_yet_is_still_resolved() {
        let root = std::env::temp_dir().join(format!(
            "crosskit-data-directory-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("real")).unwrap();
        let link = root.join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("real"), &link).unwrap();

        let absent = link.join("Crosskit");
        let resolved = resolved_even_if_absent(&absent);
        assert!(
            !absent.exists(),
            "the test is about a directory that is not there"
        );
        assert_eq!(
            resolved,
            fs::canonicalize(root.join("real"))
                .unwrap()
                .join("Crosskit"),
            "the rule must be written in the spelling a resolved walk produces"
        );

        // And that spelling is what the rule then excludes, together with everything in it.
        let rule = own_data_directory_rule(Some(&resolved)).unwrap();
        assert_eq!(rule.class(), PolicyClass::Exclude);
        for inside in ["", "/graph-work.lbug", "/search-work/meta.json"] {
            let path =
                NormalizedPath::from_absolute(&format!("{}{inside}", resolved.display())).unwrap();
            assert!(rule.pattern().matches(&path), "{path} must be excluded");
        }
        let outside =
            NormalizedPath::from_absolute(&format!("{}-other/notes.md", resolved.display()))
                .unwrap();
        assert!(
            !rule.pattern().matches(&outside),
            "{outside} is another path"
        );

        fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod search_language_tests {
    use super::{ConfigError, parse_index_policy};
    use crate::domain::NormalizationSet;
    use crate::infrastructure::search::LanguagePackError;

    fn normalizations_of(text: &str) -> Result<NormalizationSet, ConfigError> {
        parse_index_policy(text, None, None).map(|parts| parts.normalizations)
    }

    fn names(set: &NormalizationSet) -> Vec<&str> {
        set.active()
            .iter()
            .map(|normalization| normalization.id())
            .collect()
    }

    /// A file that says nothing about languages gets the two built-in ones, which is what
    /// the corpus this program was measured on is written in.
    #[test]
    fn a_file_without_a_search_section_declares_russian_and_english() {
        assert_eq!(
            names(&normalizations_of("[index]\n").unwrap()),
            ["ru", "en"]
        );
    }

    /// The corpus declares its languages; nothing here guesses them from the text.
    #[test]
    fn the_declared_languages_are_the_ones_used() {
        assert_eq!(
            names(&normalizations_of("[search]\nlanguages = [\"en\"]\n").unwrap()),
            ["en"]
        );
        assert_eq!(
            names(&normalizations_of("[search]\nlanguages = []\n").unwrap()),
            Vec::<&str>::new()
        );
    }

    /// A language this build cannot normalize fails the load. Skipping it quietly would
    /// index that language by no rule at all while the configuration said otherwise.
    #[test]
    fn a_language_this_build_does_not_have_fails_the_load() {
        let error = normalizations_of("[search]\nlanguages = [\"ru\", \"xx\"]\n").unwrap_err();
        assert_eq!(
            error,
            ConfigError::Language(LanguagePackError::Unknown("xx".to_string()))
        );
        let message = error.to_string();
        assert!(
            message.contains("xx") && message.contains("ru, en, kk"),
            "{message}"
        );
    }

    #[test]
    fn an_unknown_key_in_the_search_section_is_refused_like_every_other() {
        assert!(matches!(
            normalizations_of("[search]\nlanguage = [\"ru\"]\n"),
            Err(ConfigError::Parse(_))
        ));
    }
}
