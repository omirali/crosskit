// The provider that answers a question from excerpts, and turns what came back into a value
// or into a category.
//
// What it sends is the question and the excerpts, numbered from one. Not a path, not a file
// name, not an identifier of any kind: the model is asked to cite numbers, and a number is
// the only thing it could cite that this program can look up. A model that invents a source
// invents a number outside the range it was given, and the domain refuses the whole answer.
//
// What comes back is constrained twice, as in Stage 6: the request carries a JSON schema, so
// the service itself is asked for `{answer, sources}`, and whatever arrives goes through the
// domain constructor, which refuses an empty answer, an answer the size of a document and a
// source nobody sent. The second check is the one that matters — a schema the service
// enforces is a promise, and a promise is not a boundary.
//
// Nothing the service wrote is kept beyond the answer itself. A malformed response, a
// refusal or a transport error becomes a category and nothing else.
use serde_json::{Value, json};

use crate::application::{AnswerProvider, AnswerRequest, AskFailure, Excerpt};
use crate::domain::{Answer, InsightFailureCategory};

use super::https_transport::MessageTransport;

/// The wording of the question. It is part of the cache key, so changing a word here means
/// every question is a new one and nothing stale is served.
pub const ASK_PROMPT_VERSION: &str = "ask-v1";

const SYSTEM_PROMPT: &str = "You answer a question about a person's own files, using only \
the numbered excerpts you are given. Answer in the language the question is asked in. Be \
specific and short: a few sentences, not an essay. Cite the excerpts you used by their \
numbers. If the excerpts do not contain the answer, say exactly that and cite nothing — do \
not answer from anything else you know.";

/// A ceiling, not a spend: the answer is a few hundred tokens, and the room above that is
/// what keeps a model that reasons before answering from being cut off mid-answer.
const MAX_TOKENS: i64 = 8192;

pub(super) struct AnswerProviderOverHttps<Transport> {
    transport: Transport,
    model_id: String,
    effort: String,
}

impl<Transport: MessageTransport> AnswerProviderOverHttps<Transport> {
    pub(super) fn new(transport: Transport, model_id: String, effort: String) -> Self {
        Self {
            transport,
            model_id,
            effort,
        }
    }

    fn request_body(&self, request: &AnswerRequest<'_>) -> Result<String, AskFailure> {
        let mut output_config = json!({
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "answer": {"type": "string"},
                        "sources": {"type": "array", "items": {"type": "integer"}}
                    },
                    "required": ["answer", "sources"],
                    "additionalProperties": false
                }
            }
        });
        if !self.effort.is_empty() {
            output_config["effort"] = Value::String(self.effort.clone());
        }
        let body = json!({
            "model": self.model_id,
            "max_tokens": MAX_TOKENS,
            "system": SYSTEM_PROMPT,
            "output_config": output_config,
            "messages": [{"role": "user", "content": user_message(request)}],
        });
        // Our own request, not the model's answer: nothing was sent, so nothing came back
        // in a shape to complain about. The topic provider calls the same failure `Handler`
        // for the same reason.
        serde_json::to_string(&body).map_err(|_| AskFailure::Unavailable)
    }
}

/// The question and the excerpts, each under its number and nothing else.
fn user_message(request: &AnswerRequest<'_>) -> String {
    let mut message = String::with_capacity(4096);
    message.push_str("Question: ");
    message.push_str(request.question().as_str());
    if let Some(period) = request.period_line() {
        message.push('\n');
        message.push_str(&period);
    }
    message.push_str("\n\nExcerpts:\n");
    for excerpt in request.excerpts() {
        message.push_str(&excerpt_block(excerpt));
    }
    message
}

/// One excerpt as the model sees it: its number, what was concluded about it when anything
/// was, then its text. Its path, its file and the chunk it came from stay on this machine —
/// the number is the only name it is given, and therefore the only thing it can cite.
///
/// The topic is a line of its own. Spliced into the text it would be indistinguishable from
/// a sentence the file contains, and a model told to answer only from the excerpts would
/// then be answering from us.
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

impl<Transport: MessageTransport> AnswerProvider for AnswerProviderOverHttps<Transport> {
    fn answer(&self, request: AnswerRequest<'_>) -> Result<Answer, AskFailure> {
        let body = self.request_body(&request)?;
        let response = self
            .transport
            .post_message(&body)
            .map_err(transport_failure)?;
        parse_answer(&response, request.excerpts().len())
    }
}

/// The transport speaks in the categories Stage 6 gave it; this is where they become this
/// command's own. Nothing carries a message across.
///
/// All eight are named and there is no `_`. A wildcard over a closed set is a way to miss
/// the category that is not in it yet, and that is exactly how this went wrong once:
/// `Handler` — this side's own trouble — fell through `_` into `Schema` and was reported to
/// the asker as the model answering badly. `InsightFailureCategory::ALL` exists so that
/// whatever walks the categories misses none of them, and `_` is the one way to walk them
/// and miss one anyway. The compiler holds this now, so there is no test for it.
fn transport_failure(category: InsightFailureCategory) -> AskFailure {
    match category {
        InsightFailureCategory::Unavailable => AskFailure::Unavailable,
        InsightFailureCategory::Refused => AskFailure::Refused,
        // This side's own trouble, not an answer: another attempt can get past it.
        InsightFailureCategory::Handler => AskFailure::Unavailable,
        // Refused for its size before any model saw it, so there is no answer to judge.
        InsightFailureCategory::InputTooLarge => AskFailure::RequestTooLarge,
        InsightFailureCategory::Schema => AskFailure::Schema,
        // The queue's lifecycle, which means nothing on the question's road: there is no
        // job here, no claim and no run budget. Should one ever arrive it is still this
        // side's doing and still not the model answering badly.
        InsightFailureCategory::Input
        | InsightFailureCategory::LeaseExpired
        | InsightFailureCategory::BudgetSpent => AskFailure::Unavailable,
    }
}

/// One response body as a value. Every way it can fail is a category.
fn parse_answer(body: &str, sent: usize) -> Result<Answer, AskFailure> {
    let response: Value = serde_json::from_str(body).map_err(|_| AskFailure::Schema)?;

    match response["stop_reason"].as_str() {
        Some("refusal") => return Err(AskFailure::Refused),
        // Cut off before it was finished, so what arrived is not the answer.
        Some("max_tokens") => return Err(AskFailure::Schema),
        _ => {}
    }

    let text = response["content"]
        .as_array()
        .ok_or(AskFailure::Schema)?
        .iter()
        .find(|block| block["type"].as_str() == Some("text"))
        .and_then(|block| block["text"].as_str())
        .ok_or(AskFailure::Schema)?;

    let parsed: Value = serde_json::from_str(text).map_err(|_| AskFailure::Schema)?;
    let answer = parsed["answer"].as_str().ok_or(AskFailure::Schema)?;
    let sources = parsed["sources"].as_array().ok_or(AskFailure::Schema)?;
    let cited: Vec<i64> = sources
        .iter()
        .map(|value| value.as_i64().ok_or(AskFailure::Schema))
        .collect::<Result<_, _>>()?;

    Answer::parse(answer, &cited, sent).map_err(|_| AskFailure::Schema)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::{Value, json};

    use super::{
        ASK_PROMPT_VERSION, AnswerProviderOverHttps, Excerpt, parse_answer, transport_failure,
    };
    use crate::application::{AnswerProvider, AnswerRequest, AskFailure};
    use crate::domain::{
        ContentEgress, ContentEgressPermit, InsightFailureCategory, NormalizedPath, Question,
    };
    use crate::infrastructure::anthropic::https_transport::MessageTransport;

    fn permit() -> ContentEgressPermit {
        ContentEgress::Allowed.permit().unwrap()
    }

    /// The transport's categories, and which of them mean the model answered badly. Only
    /// `Schema` does. A call that never came back as a call — `Handler`, which is what an
    /// unexpected status becomes — must not be reported as the model's doing, because the
    /// sentence for `Schema` sends the asker to look at their excerpts for a fault that was
    /// never theirs and never the model's.
    #[test]
    fn a_call_this_side_could_not_make_is_not_the_model_answering_badly() {
        assert_eq!(
            transport_failure(InsightFailureCategory::Handler),
            AskFailure::Unavailable
        );
        assert_eq!(
            transport_failure(InsightFailureCategory::Unavailable),
            AskFailure::Unavailable
        );
        assert_eq!(
            transport_failure(InsightFailureCategory::Refused),
            AskFailure::Refused
        );
        assert_eq!(
            transport_failure(InsightFailureCategory::Schema),
            AskFailure::Schema
        );
    }

    struct ScriptedTransport {
        answer: Result<String, InsightFailureCategory>,
        sent: RefCell<Vec<String>>,
    }

    impl ScriptedTransport {
        fn answering(answer: &str) -> Self {
            Self {
                answer: Ok(answer.to_string()),
                sent: RefCell::new(Vec::new()),
            }
        }

        fn refusing(category: InsightFailureCategory) -> Self {
            Self {
                answer: Err(category),
                sent: RefCell::new(Vec::new()),
            }
        }
    }

    impl MessageTransport for ScriptedTransport {
        fn post_message(&self, body: &str) -> Result<String, InsightFailureCategory> {
            self.sent.borrow_mut().push(body.to_string());
            self.answer.clone()
        }
    }

    fn answered(answer: &str, sources: Vec<i64>) -> String {
        json!({
            "model": "the-model-that-answered",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": json!({"answer": answer, "sources": sources}).to_string()}],
            "usage": {"input_tokens": 900, "output_tokens": 40},
        })
        .to_string()
    }

    fn excerpts() -> Vec<Excerpt> {
        [
            ("chunk:a", "/files/one.md", "первая выдержка про сверку"),
            ("chunk:b", "/files/two.md", "вторая выдержка про индекс"),
        ]
        .into_iter()
        .enumerate()
        .map(|(position, (chunk, path, text))| {
            Excerpt::new(
                position + 1,
                chunk,
                NormalizedPath::from_absolute(path).unwrap(),
                text,
                None,
            )
        })
        .collect()
    }

    /// A request refused for its size never reached a model, so nothing said about a
    /// model's answer can be true of it.
    ///
    /// It used to arrive as `Schema`, and the asker was told their answer was empty, too
    /// long, or citing a source it was not given — three things that cannot have happened
    /// where nothing was answered. The whole road is exercised, not the mapping alone,
    /// because the mapping is only the half of this that a reader can see.
    #[test]
    fn a_request_refused_for_its_size_is_not_the_model_answering_badly() {
        let transport = ScriptedTransport::refusing(InsightFailureCategory::InputTooLarge);
        let provider =
            AnswerProviderOverHttps::new(transport, "a-model".to_string(), String::new());
        let question = Question::parse("что делает сверка?").unwrap();
        let excerpts = excerpts();

        let failure = provider.answer(AnswerRequest::new(&question, &excerpts, permit()));

        assert_eq!(failure.unwrap_err(), AskFailure::RequestTooLarge);
    }

    /// What leaves the machine: the question and the numbered excerpts. No path, no file
    /// name, no chunk id — the model cannot cite a file because it was never told of one.
    #[test]
    fn the_request_carries_the_question_and_numbered_excerpts_and_no_paths() {
        let transport = ScriptedTransport::answering(&answered("ответ", vec![1]));
        let provider =
            AnswerProviderOverHttps::new(transport, "a-model".to_string(), String::new());
        let question = Question::parse("что делает сверка?").unwrap();
        let excerpts = excerpts();

        provider
            .answer(AnswerRequest::new(&question, &excerpts, permit()))
            .unwrap();

        let body: Value = serde_json::from_str(&provider.transport.sent.borrow()[0]).unwrap();
        let message = body["messages"][0]["content"].as_str().unwrap();
        assert!(message.contains("что делает сверка?"), "{message}");
        assert!(
            message.contains("[1]") && message.contains("[2]"),
            "{message}"
        );
        assert!(message.contains("первая выдержка про сверку"), "{message}");
        assert!(
            !message.contains("/files/") && !message.contains("chunk:"),
            "a path or an identifier reached the model: {message}"
        );
        assert_eq!(body["max_tokens"], 8192);
    }

    #[test]
    fn an_answer_with_its_sources_becomes_a_value() {
        let answer = parse_answer(&answered("сверка обходит дерево", vec![2, 1]), 2).unwrap();

        assert_eq!(answer.text(), "сверка обходит дерево");
        assert_eq!(answer.used(), [1, 2]);
    }

    /// A source nobody sent is the one wrong answer that would put a file in front of a
    /// reader, and the whole answer is refused for it.
    #[test]
    fn an_answer_citing_a_source_that_was_not_sent_is_refused() {
        assert_eq!(
            parse_answer(&answered("ответ", vec![3]), 2).unwrap_err(),
            AskFailure::Schema
        );
    }

    #[test]
    fn every_shape_this_program_does_not_accept_is_a_category() {
        let refusal = json!({"stop_reason": "refusal", "content": []}).to_string();
        assert_eq!(parse_answer(&refusal, 2).unwrap_err(), AskFailure::Refused);

        let cut_off = json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "{\"answer\":\"начало"}],
        })
        .to_string();
        assert_eq!(parse_answer(&cut_off, 2).unwrap_err(), AskFailure::Schema);

        for body in [
            "not json at all".to_string(),
            json!({"content": []}).to_string(),
            json!({"content": [{"type": "text", "text": "{}"}]}).to_string(),
            json!({"content": [{"type": "text", "text": "{\"answer\":\"\",\"sources\":[]}"}]})
                .to_string(),
            json!({"content": [{"type": "text", "text": "{\"answer\":\"a\",\"sources\":\"1\"}"}]})
                .to_string(),
        ] {
            assert_eq!(
                parse_answer(&body, 2).unwrap_err(),
                AskFailure::Schema,
                "{body}"
            );
        }
    }

    /// The wording of the question is part of what a cached answer is an answer to.
    #[test]
    fn the_prompt_has_a_version() {
        assert!(!ASK_PROMPT_VERSION.is_empty());
    }
}
