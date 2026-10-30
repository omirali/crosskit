// Which engine answers, chosen once and used by both roads.
//
// Two engines exist. The default is the subscription tool already installed on the machine:
// its tokens are the ones the owner is already paying for, and it is how they work. The
// other is the HTTPS provider of stage 6, still measured and still tested, chosen by name.
//
// The choice is made here and nowhere else, so `index jobs run` and `ask` cannot disagree
// about it, and a third engine by key is added by extending one enum — the ports above this
// module stay exactly as they are.
mod claude_code;
mod decision_map_forks;
mod decision_map_question;
mod decision_map_risks;
/// The question both engines ask of a revision, and how its answer is read (ADR 0048).
pub(super) mod knowledge_answer;
mod task_synthesis_question;
mod tool_call;

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::application::{AnswerProvider, AnswerRequest, AskFailure, Excerpt, InsightProvider};
use crate::domain::Answer;

use super::anthropic::{self, ModelSettings};
pub use claude_code::ClaudeCodeSettings;
use claude_code::ClaudeCodeTool;
pub use decision_map_question::{ClaudeCodeMapper, QUESTION_VERSION as MAP_QUESTION_VERSION};
pub use task_synthesis_question::MERGE_VERSION as MAP_MERGE_VERSION;

/// What the installation chose, with the settings of that choice and nothing of the other's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisEngine {
    /// The installed subscription tool. The default.
    ClaudeCode(ClaudeCodeSettings),
    /// The vendor's API, reached over HTTPS with a key from the environment.
    AnthropicApi(ModelSettings),
}

impl AnalysisEngine {
    /// The name this engine is written under in the configuration.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ClaudeCode(_) => CLAUDE_CODE,
            Self::AnthropicApi(_) => ANTHROPIC_API,
        }
    }
}

pub const CLAUDE_CODE: &str = "claude-code";
pub const ANTHROPIC_API: &str = "anthropic-api";

/// Every engine name this program knows, for the message that refuses an unknown one.
pub const ENGINE_NAMES: [&str; 2] = [CLAUDE_CODE, ANTHROPIC_API];

/// Why an engine cannot be used right now. Said once, before any job is claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineUnavailable {
    /// The tool is not on this machine's PATH.
    NotInstalled,
    /// No key is configured for the API engine.
    NoApiKey,
}

impl EngineUnavailable {
    /// One sentence, with what to do about it. It names no path and no key.
    pub fn reason(self, consequence: &str) -> String {
        match self {
            Self::NotInstalled => format!(
                "the `{CLAUDE_CODE}` engine needs the `claude` command on PATH, and it is not \
                 there; install it, or choose another engine in `[analysis] engine`. \
                 {consequence}"
            ),
            // The variable is named and nothing about its value is: not the key, not its
            // length, not whether one was found.
            Self::NoApiKey => format!(
                "this context may send content to a model and `[analysis] engine` is \
                 `{ANTHROPIC_API}`, but {} is not set. {consequence}",
                anthropic::API_KEY_VARIABLE
            ),
        }
    }
}

/// The insight provider of the chosen engine, or why there is none.
///
/// Nothing is neither a failure nor a fallback: no job is claimed at all. Claiming would
/// count an attempt against a call that was never going to happen.
pub fn insight_provider(
    engine: &AnalysisEngine,
) -> Result<Box<dyn InsightProvider>, EngineUnavailable> {
    match engine {
        AnalysisEngine::ClaudeCode(settings) => Ok(Box::new(ClaudeCodeTool::new(
            installed_tool()?,
            settings.clone(),
        ))),
        AnalysisEngine::AnthropicApi(settings) => {
            anthropic::configured_provider(settings).ok_or(EngineUnavailable::NoApiKey)
        }
    }
}

/// The answer provider of the same chosen engine. One installation, one engine: a question
/// and a label must not be answered by two different things.
pub fn answer_provider(
    engine: &AnalysisEngine,
) -> Result<Box<dyn AnswerProvider>, EngineUnavailable> {
    match engine {
        AnalysisEngine::ClaudeCode(settings) => Ok(Box::new(ClaudeCodeTool::new(
            installed_tool()?,
            settings.clone(),
        ))),
        AnalysisEngine::AnthropicApi(settings) => {
            anthropic::configured_answer_provider(settings).ok_or(EngineUnavailable::NoApiKey)
        }
    }
}

/// Which prompt version the chosen engine answers under. It travels into the cache key, so
/// an answer produced by one engine is never served for a question asked of the other.
pub fn insight_prompt_version(engine: &AnalysisEngine) -> &'static str {
    match engine {
        AnalysisEngine::ClaudeCode(_) => claude_code::PROMPT_VERSION,
        AnalysisEngine::AnthropicApi(_) => anthropic::PROMPT_VERSION,
    }
}

/// What the cache key records as the model that answered. For the tool it is the engine's
/// own name unless an operator pinned a model: a subscription decides its model, and the
/// key must not claim to know one it was never told.
pub fn model_label(engine: &AnalysisEngine) -> &str {
    match engine {
        AnalysisEngine::ClaudeCode(settings) => match settings.model().is_empty() {
            true => CLAUDE_CODE,
            false => settings.model(),
        },
        AnalysisEngine::AnthropicApi(settings) => settings.model_id(),
    }
}

pub fn ask_prompt_version(engine: &AnalysisEngine) -> &'static str {
    match engine {
        AnalysisEngine::ClaudeCode(_) => claude_code::ASK_PROMPT_VERSION,
        AnalysisEngine::AnthropicApi(_) => anthropic::ASK_PROMPT_VERSION,
    }
}

/// One call of the installed tool with a question and schema of the caller's own, and the
/// tool's envelope back whole: the same flags and the same process handling as every call of
/// the product. Only the decision map measurement asks this way, and only with the context's
/// permission to send content out, which it must hold to call at all.
#[cfg(all(test, feature = "decision-map-measurement"))]
pub(in crate::infrastructure) fn ask_installed_tool(
    model: &str,
    schema: &Value,
    prompt: String,
    _permit: &crate::domain::ContentEgressPermit,
) -> Result<Value, String> {
    let binary = installed_tool().map_err(|_| "the `claude` tool is not on PATH".to_string())?;
    ClaudeCodeTool::new(binary, ClaudeCodeSettings::new(model.to_string()))
        .call(schema, prompt)
        .map_err(|failure| format!("{failure:?}"))
}

/// The engine a decision map is built with, holding the context's permission to send content
/// out. Only the installed tool asks this question today; the API engine does not.
pub fn map_engine(
    engine: &AnalysisEngine,
    permit: crate::domain::ContentEgressPermit,
) -> Result<ClaudeCodeMapper, String> {
    let AnalysisEngine::ClaudeCode(settings) = engine else {
        return Err(format!(
            "a decision map is built through the `{CLAUDE_CODE}` engine; this context's analysis \
             is set to `{ANTHROPIC_API}`"
        ));
    };
    let binary = installed_tool().map_err(|_| "the `claude` tool is not on PATH".to_string())?;
    Ok(ClaudeCodeMapper::new(binary, settings, permit))
}

/// The name of the tool's binary. Looked up on PATH rather than configured: a path to an
/// executable in a configuration file is a way to make this program run something else.
const TOOL_BINARY: &str = "claude";

/// Where the tool is, or that it is not installed.
fn installed_tool() -> Result<PathBuf, EngineUnavailable> {
    let path = std::env::var_os("PATH").ok_or(EngineUnavailable::NotInstalled)?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(TOOL_BINARY))
        .find(|candidate| is_executable(candidate))
        .ok_or(EngineUnavailable::NotInstalled)
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

// --- what the two engines answer the same question with ---------------------

/// The schema `ask` is answered under, on either engine.
fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "answer": {"type": "string"},
            "sources": {"type": "array", "items": {"type": "integer"}}
        },
        "required": ["answer", "sources"],
        "additionalProperties": false
    })
}

const ASK_SYSTEM_PROMPT: &str = "You answer a question about a person's own files, using \
only the numbered excerpts you are given. Answer in the language the question is asked in. \
Be specific and short: a few sentences, not an essay. Cite the excerpts you used by their \
numbers. If the excerpts do not contain the answer, say exactly that and cite nothing — do \
not answer from anything else you know.";

/// The question and the excerpts, each under its number and nothing else. A number is the
/// only name an excerpt is given, and therefore the only thing a model could cite.
fn ask_prompt(request: &AnswerRequest<'_>) -> String {
    let mut prompt = String::with_capacity(4096);
    prompt.push_str(ASK_SYSTEM_PROMPT);
    prompt.push_str("\n\nQuestion: ");
    prompt.push_str(request.question().as_str());
    if let Some(period) = request.period_line() {
        prompt.push('\n');
        prompt.push_str(&period);
    }
    prompt.push_str("\n\nExcerpts:\n");
    for excerpt in request.excerpts() {
        prompt.push_str(&excerpt_block(excerpt));
    }
    prompt
}

/// The topic is a labelled line of its own above the text, never part of it. A label glued
/// into the excerpt would be indistinguishable from a sentence the file contains, and a
/// model told to answer only from the excerpts would then be answering from us.
fn excerpt_block(excerpt: &Excerpt) -> String {
    match excerpt.topic() {
        Some(topic) => format!(
            "\n[{}]\nabout: {topic}\ntext:\n{}\n",
            excerpt.ordinal(),
            excerpt.text()
        ),
        None => format!("\n[{}]\n{}\n", excerpt.ordinal(), excerpt.text()),
    }
}

/// The tool's finished call as an answer, through the same domain constructor the HTTPS
/// provider uses: a schema the tool enforces is a promise, and the constructor is the check.
fn parse_ask_answer(answer: &Value, sent: usize) -> Result<Answer, AskFailure> {
    if answer["is_error"].as_bool() == Some(true) {
        return match answer["subtype"].as_str() {
            Some("refusal") => Err(AskFailure::Refused),
            _ => Err(AskFailure::Unavailable),
        };
    }
    let structured = &answer["structured_output"];
    let text = structured["answer"].as_str().ok_or(AskFailure::Schema)?;
    let sources = structured["sources"]
        .as_array()
        .ok_or(AskFailure::Schema)?
        .iter()
        .map(|value| value.as_i64().ok_or(AskFailure::Schema))
        .collect::<Result<Vec<_>, _>>()?;
    Answer::parse(text, &sources, sent).map_err(|_| AskFailure::Schema)
}
