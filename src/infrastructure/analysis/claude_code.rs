// The installed subscription tool as a provider for both ports.
//
// The tool is a program on this machine, not an endpoint: there is no key here, no URL and
// no vendor protocol. What it shares with the HTTPS provider is everything that matters —
// the question is constrained by a schema the tool itself enforces, and whatever comes back
// is put through the same domain constructors, which are the boundary. A schema a tool
// promises to honour is a promise; the constructor is the check.
//
// Nothing the tool wrote is ever kept. A malformed answer, a refusal, a rate limit or a
// timeout is a category and nothing else.
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use crate::application::{
    AnswerProvider, AnswerRequest, AskFailure, DESCRIBED_BY, InsightProvider, InsightRequest,
};
use crate::domain::{Answer, InsightFailureCategory, InsightProvenance, ModelAnswer};

use super::knowledge_answer::{answer_schema, instructions, parse_structured};
use super::tool_call::{ToolCallFailure, run};

/// The wording of the question, and part of every cache key: changing a word here makes
/// every revision a new question rather than serving a stale answer to a different one.
/// `revision-topic-tool-v1` asked for the topic alone; `revision-knowledge-tool-v1` added
/// entities and facts; `v2` tells the model a quote names both ends of its fact.
pub const PROMPT_VERSION: &str = "revision-knowledge-tool-v2";

/// The wording `ask` uses, for the same reason.
pub const ASK_PROMPT_VERSION: &str = "ask-answer-tool-v1";

/// What one call is given before it is stopped. A subscription tool starts a runtime, reads
/// its settings and then asks a model, so this is not the latency of an HTTP request; it is
/// chosen against the measured whole, with room for a slow answer and none for a hang.
const CALL_TIMEOUT: Duration = Duration::from_secs(180);

/// How the tool is named to the model it runs. Empty leaves the tool's own default, which
/// is what a subscription already decided for its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeSettings {
    model: String,
}

impl ClaudeCodeSettings {
    pub fn new(model: String) -> Self {
        Self { model }
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

/// The tool, found on this machine.
pub struct ClaudeCodeTool {
    binary: PathBuf,
    settings: ClaudeCodeSettings,
}

impl ClaudeCodeTool {
    pub(super) fn new(binary: PathBuf, settings: ClaudeCodeSettings) -> Self {
        Self { binary, settings }
    }

    /// One call, with the flags that make it a tool this program can be responsible for.
    ///
    /// Every flag here is a boundary, not a preference:
    ///
    /// - `--print` with input on standard input: no terminal, and a document too large for
    ///   an argument list still arrives whole.
    /// - `--output-format json` and `--json-schema`: the tool is asked to answer in a
    ///   shape, and says whether it managed to.
    /// - `--no-session-persistence`: the tool writes a transcript of everything it was
    ///   given to `~/.claude/projects/<cwd>/<session>.jsonl` unless told not to. Stage 22
    ///   measured this with a marked string and found it there; with this flag the file is
    ///   not written and the directory is not created. It is the one flag on this list a
    ///   reader must not remove.
    /// - `--strict-mcp-config` with no config, and `--setting-sources` empty: this
    ///   program's call must not pick up whatever servers, hooks or instructions happen to
    ///   be configured on the machine for somebody's editing sessions.
    /// - `--disallowed-tools`: a labeller has no business reading or writing files. The
    ///   content it is given arrives on standard input and nowhere else.
    pub(super) fn call(&self, schema: &Value, prompt: String) -> Result<Value, ToolCallFailure> {
        let mut command = Command::new(&self.binary);
        command
            .arg("--print")
            .arg("--output-format")
            .arg("json")
            .arg("--json-schema")
            .arg(schema.to_string())
            .arg("--no-session-persistence")
            .arg("--strict-mcp-config")
            .arg("--setting-sources")
            .arg("")
            .arg("--disallowed-tools")
            .arg("Bash Edit Write Read Glob Grep WebFetch WebSearch Task")
            // The tool refuses to start inside another of its own sessions, which is how a
            // developer running this program from one would meet a failure nobody else can
            // reproduce.
            .env_remove("CLAUDECODE");
        if !self.settings.model.is_empty() {
            command.arg("--model").arg(&self.settings.model);
        }
        let output = run(command, prompt, CALL_TIMEOUT)?;
        let answer: Value =
            serde_json::from_str(&output.stdout).map_err(|_| ToolCallFailure::NotAnAnswer)?;
        if !output.status_success && answer["subtype"].as_str().is_none() {
            return Err(ToolCallFailure::NotAnAnswer);
        }
        Ok(answer)
    }
}

impl InsightProvider for ClaudeCodeTool {
    fn describe(&self, request: InsightRequest<'_>) -> Result<ModelAnswer, InsightFailureCategory> {
        let vocabulary = request.vocabulary();
        let prompt = format!(
            "{}\n\n---\n\n{}",
            instructions(vocabulary),
            request.content()
        );
        let answer = self
            .call(&answer_schema(vocabulary), prompt)
            .map_err(insight_category)?;
        parse_answer(&answer, self.settings.model())
    }
}

/// Turns one finished call into a value, or into a category.
fn parse_answer(
    answer: &Value,
    configured_model: &str,
) -> Result<ModelAnswer, InsightFailureCategory> {
    if let Some(category) = refusal_category(answer) {
        return Err(category);
    }
    let (topic, knowledge) = parse_structured(&answer["structured_output"])?;
    Ok(ModelAnswer::new(
        topic,
        InsightProvenance::from_model(
            DESCRIBED_BY,
            answered_by(answer, configured_model),
            PROMPT_VERSION,
            input_token_count(answer),
            token_count(answer, "output_tokens"),
        )
        .costing(reported_cost_micro_usd(answer)),
    )
    .with_knowledge(knowledge))
}

/// What the tool says went wrong, as a category, or nothing when it answered.
///
/// The tool reports its own trouble in a result envelope rather than by failing to run, so
/// this is where a rate limit, a refusal and an overload are told apart. Only the shape of
/// the report is read; its words are not kept.
fn refusal_category(answer: &Value) -> Option<InsightFailureCategory> {
    if answer["is_error"].as_bool() != Some(true) {
        return None;
    }
    match answer["subtype"].as_str() {
        // The subscription's allowance for this window is spent. Temporary by definition:
        // the window moves.
        Some("error_max_turns") | Some("error_during_execution") => {
            Some(InsightFailureCategory::Unavailable)
        }
        Some("refusal") => Some(InsightFailureCategory::Refused),
        _ => Some(InsightFailureCategory::Unavailable),
    }
}

fn answered_by(answer: &Value, configured_model: &str) -> String {
    answer["modelUsage"]
        .as_object()
        .and_then(|usage| usage.keys().next())
        .map(String::to_string)
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| model_or_default(configured_model))
}

fn model_or_default(configured_model: &str) -> String {
    match configured_model.is_empty() {
        true => "subscription-default".to_string(),
        false => configured_model.to_string(),
    }
}

fn token_count(answer: &Value, field: &str) -> i64 {
    answer["usage"][field].as_i64().unwrap_or_default()
}

/// Every token the model read: the tool reports the part it served from its own prompt cache
/// apart from the rest, and all of it was read.
fn input_token_count(answer: &Value) -> i64 {
    [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ]
    .into_iter()
    .map(|field| token_count(answer, field))
    .sum()
}

/// What the tool says the call cost, in millionths of a dollar. A subscription is not billed
/// per call; the figure is the list price of the same work, which is what a forecast needs.
fn reported_cost_micro_usd(answer: &Value) -> i64 {
    answer["total_cost_usd"]
        .as_f64()
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .map_or(0, |cost| (cost * 1_000_000.0).round() as i64)
}

/// What a call that produced nothing usable costs the job that asked for it.
///
/// The line runs between not having heard the answer and having heard one that does not
/// fit. Only the second is final: a job retired for the first would be retired for a
/// hiccup on this side of the pipe, and `dead` is forever — no sweep, no pass and no
/// republication of the revision brings it back.
fn insight_category(failure: ToolCallFailure) -> InsightFailureCategory {
    match failure {
        // Trying again can help: the tool may be busy, the machine loaded, the window moved.
        ToolCallFailure::NotStarted | ToolCallFailure::TimedOut => {
            InsightFailureCategory::Unavailable
        }
        // This side could not collect what the tool wrote, so nothing is known about the
        // answer. The handler ran and produced no result, which is what `Handler` says.
        ToolCallFailure::AnswerUnread => InsightFailureCategory::Handler,
        ToolCallFailure::NotAnAnswer => InsightFailureCategory::Schema,
    }
}

impl AnswerProvider for ClaudeCodeTool {
    fn answer(&self, request: AnswerRequest<'_>) -> Result<Answer, AskFailure> {
        let sent = request.excerpts().len();
        let prompt = super::ask_prompt(&request);
        let answer = self
            .call(&super::answer_schema(), prompt)
            .map_err(ask_failure)?;
        super::parse_ask_answer(&answer, sent)
    }
}

/// The same line on the question's road. There is no queue here and nothing to bury, but
/// the sentence the asker reads still has to be true: an answer this side failed to collect
/// is not the model answering badly, and telling them to look at their excerpt budget would
/// send them after the wrong thing.
fn ask_failure(failure: ToolCallFailure) -> AskFailure {
    match failure {
        ToolCallFailure::NotStarted | ToolCallFailure::TimedOut | ToolCallFailure::AnswerUnread => {
            AskFailure::Unavailable
        }
        ToolCallFailure::NotAnAnswer => AskFailure::Schema,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::{Value, json};

    use super::{
        ClaudeCodeSettings, ClaudeCodeTool, PROMPT_VERSION, ToolCallFailure, ask_failure,
        insight_category, parse_answer,
    };
    use crate::application::AskFailure;
    use crate::domain::{FailureDisposition, InsightFailureCategory};

    fn any_schema() -> Value {
        json!({"type": "object"})
    }

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A script on disk standing in for the installed tool. It ignores the flags the real
    /// call passes, which is what lets one line of shell answer as the tool would.
    fn tool_that(body: &str) -> (PathBuf, ClaudeCodeTool) {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-claude-code-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join("claude");
        fs::write(&binary, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let tool = ClaudeCodeTool::new(binary, ClaudeCodeSettings::new(String::new()));
        (root, tool)
    }

    /// Every way one call can fail, the category it costs a job, and whether that category
    /// lets the job be tried again. The table is the decision: the first three are this
    /// side's trouble and the job survives them, the fourth is the tool's answer and only
    /// it is final. `dead` cannot be undone by a sweep, a pass or a republished revision,
    /// so a wrong `Permanent` here is a document nobody ever labels.
    #[test]
    fn what_each_way_a_call_can_fail_costs_a_job() {
        let expected = [
            (
                ToolCallFailure::NotStarted,
                InsightFailureCategory::Unavailable,
                FailureDisposition::Retry,
            ),
            (
                ToolCallFailure::TimedOut,
                InsightFailureCategory::Unavailable,
                FailureDisposition::Retry,
            ),
            (
                ToolCallFailure::AnswerUnread,
                InsightFailureCategory::Handler,
                FailureDisposition::Retry,
            ),
            (
                ToolCallFailure::NotAnAnswer,
                InsightFailureCategory::Schema,
                FailureDisposition::Permanent,
            ),
        ];

        for (failure, category, disposition) in expected {
            assert_eq!(insight_category(failure), category, "{failure:?}");
            assert_eq!(
                insight_category(failure).disposition(),
                disposition,
                "{failure:?} decides whether the job is ever tried again"
            );
        }
    }

    /// The same line on the question's road. Nothing is buried here, but the sentence the
    /// asker reads has to name what actually went wrong: `Schema` sends them to look at
    /// their excerpt budget for a pipe this side failed to read.
    #[test]
    fn what_each_way_a_call_can_fail_costs_a_question() {
        let expected = [
            (ToolCallFailure::NotStarted, AskFailure::Unavailable),
            (ToolCallFailure::TimedOut, AskFailure::Unavailable),
            (ToolCallFailure::AnswerUnread, AskFailure::Unavailable),
            (ToolCallFailure::NotAnAnswer, AskFailure::Schema),
        ];

        for (failure, expected) in expected {
            assert_eq!(ask_failure(failure), expected, "{failure:?}");
        }
    }

    /// Output read whole that is not the tool's envelope at all: heard, and not an answer.
    #[test]
    fn output_that_is_read_whole_and_is_not_json_is_not_an_answer() {
        let (root, tool) = tool_that("cat >/dev/null; printf '%s' 'I am not JSON'");

        let failure = tool.call(&any_schema(), "prompt".to_string());

        assert_eq!(failure.err(), Some(ToolCallFailure::NotAnAnswer));
        fs::remove_dir_all(root).unwrap();
    }

    /// What the tool reports beside the answer is what the call cost: every token it read,
    /// its own cache included, and its list price in millionths of a dollar.
    #[test]
    fn an_answer_records_every_token_read_and_the_price_the_tool_reported() {
        let envelope = json!({
            "type": "result",
            "is_error": false,
            "total_cost_usd": 0.0421,
            "usage": {
                "input_tokens": 12,
                "cache_creation_input_tokens": 3_000,
                "cache_read_input_tokens": 9_000,
                "output_tokens": 640
            },
            "modelUsage": {"a-model": {}},
            "structured_output": {
                "topic": "реестр", "confidence": 0.7, "entities": [], "relations": []
            }
        });

        let answer = parse_answer(&envelope, "").unwrap();

        assert_eq!(answer.provenance().input_tokens(), 12_012);
        assert_eq!(answer.provenance().output_tokens(), 640);
        assert_eq!(answer.provenance().cost_micro_usd(), 42_100);
        assert_eq!(answer.provenance().model_id(), "a-model");
        assert_eq!(answer.provenance().prompt_version(), PROMPT_VERSION);
        assert!(answer.knowledge().facts().is_empty());
    }

    /// A tool that failed and did not say why in the shape it promises is the same case:
    /// this program has the whole of what it wrote and cannot read an answer out of it.
    #[test]
    fn a_failure_the_tool_does_not_name_is_not_an_answer() {
        let (root, tool) = tool_that("cat >/dev/null; printf '%s' '{\"type\":\"result\"}'; exit 1");

        let failure = tool.call(&any_schema(), "prompt".to_string());

        assert_eq!(failure.err(), Some(ToolCallFailure::NotAnAnswer));
        fs::remove_dir_all(root).unwrap();
    }
}
