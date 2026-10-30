// The provider over HTTPS: it asks a model what a revision is about and what it states, and
// turns the answer into a value, or into a category.
//
// The answer is constrained twice. The request carries a JSON schema, so the service itself
// is asked to return only the topic, its confidence, entities and relations of the agreed
// shape (`analysis::knowledge_answer`); and whatever comes back is then put through the
// domain constructors, which refuse an empty label, a label that is not one short line, a
// confidence outside none-to-certainty and every item over its limit. The second check is
// the one that matters: a schema the service enforces is a promise, and a promise is not a
// boundary.
//
// Nothing the service wrote is ever kept. A malformed answer, a refusal or an error is a
// category and nothing else — no message, no body, no prompt and no line of the file.
use serde_json::{Value, json};

use crate::application::{DESCRIBED_BY, InsightProvider, InsightRequest};
use crate::domain::knowledge::KnowledgeVocabulary;
use crate::domain::{InsightFailureCategory, InsightProvenance, ModelAnswer};

use super::super::analysis::knowledge_answer::{answer_schema, instructions, parse_structured};
use super::https_transport::MessageTransport;

/// The wording of the question. It is part of the cache key, so changing a word here means
/// every revision is a new question and nothing stale is served. `revision-topic-v1` asked for
/// the topic alone; `revision-knowledge-v2` tells the model a quote names both ends of its fact.
pub const PROMPT_VERSION: &str = "revision-knowledge-v2";

/// A ceiling, not a spend. Sixty facts with their quotes and descriptions are a few thousand
/// tokens, and the room above that keeps a model that reasons first from being cut off.
const MAX_TOKENS: i64 = 16_000;

pub(super) struct TopicProvider<Transport> {
    transport: Transport,
    model_id: String,
    /// How hard the model is asked to work. Empty means the field is left out of the
    /// request entirely, which is what a model that does not accept it needs.
    effort: String,
}

impl<Transport: MessageTransport> TopicProvider<Transport> {
    pub(super) fn new(transport: Transport, model_id: String, effort: String) -> Self {
        Self {
            transport,
            model_id,
            effort,
        }
    }

    fn request_body(
        &self,
        content: &str,
        vocabulary: &KnowledgeVocabulary,
    ) -> Result<String, InsightFailureCategory> {
        let mut output_config = json!({
            "format": {
                "type": "json_schema",
                "schema": answer_schema(vocabulary)
            }
        });
        if !self.effort.is_empty() {
            output_config["effort"] = Value::String(self.effort.clone());
        }
        let body = json!({
            "model": self.model_id,
            "max_tokens": MAX_TOKENS,
            "system": instructions(vocabulary),
            "output_config": output_config,
            "messages": [{"role": "user", "content": content}],
        });
        serde_json::to_string(&body).map_err(|_| InsightFailureCategory::Handler)
    }
}

impl<Transport: MessageTransport> InsightProvider for TopicProvider<Transport> {
    fn describe(&self, request: InsightRequest<'_>) -> Result<ModelAnswer, InsightFailureCategory> {
        let body = self.request_body(request.content(), request.vocabulary())?;
        let answer = self.transport.post_message(&body)?;
        parse_answer(&answer, &self.model_id)
    }
}

/// Turns one response body into a value. Every way it can fail is a category: the shape is
/// not what this program accepts (`Schema`), or the model declined to answer (`Refused`).
fn parse_answer(body: &str, configured_model: &str) -> Result<ModelAnswer, InsightFailureCategory> {
    let response: Value = serde_json::from_str(body).map_err(|_| InsightFailureCategory::Schema)?;

    // A decline is an ordinary response with a stop reason, not an error, and asking the
    // same thing again would be declined again.
    match response["stop_reason"].as_str() {
        Some("refusal") => return Err(InsightFailureCategory::Refused),
        // The answer was cut off before it was finished, so what arrived is not the answer.
        Some("max_tokens") => return Err(InsightFailureCategory::Schema),
        _ => {}
    }

    let text = response["content"]
        .as_array()
        .ok_or(InsightFailureCategory::Schema)?
        .iter()
        .find(|block| block["type"].as_str() == Some("text"))
        .and_then(|block| block["text"].as_str())
        .ok_or(InsightFailureCategory::Schema)?;

    let parsed: Value = serde_json::from_str(text).map_err(|_| InsightFailureCategory::Schema)?;
    let (topic, knowledge) = parse_structured(&parsed)?;

    // Which model actually answered, as it said itself; the configured id is the fallback
    // for a response that does not name one.
    let model_id = response["model"].as_str().unwrap_or(configured_model);
    Ok(ModelAnswer::new(
        topic,
        InsightProvenance::from_model(
            DESCRIBED_BY,
            model_id,
            PROMPT_VERSION,
            response["usage"]["input_tokens"].as_i64().unwrap_or(0),
            response["usage"]["output_tokens"].as_i64().unwrap_or(0),
        ),
    )
    .with_knowledge(knowledge))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::{Value, json};

    use super::{MAX_TOKENS, PROMPT_VERSION, TopicProvider, parse_answer};
    use crate::application::{DESCRIBED_BY, InsightProvider, InsightRequest};
    use crate::domain::knowledge::KnowledgeVocabulary;
    use crate::domain::{ContentEgress, ContentEgressPermit, InsightFailureCategory};
    use crate::infrastructure::anthropic::https_transport::MessageTransport;

    const MODEL: &str = "a-model";

    fn permit() -> ContentEgressPermit {
        ContentEgress::Allowed.permit().unwrap()
    }

    /// Answers whatever it was given and keeps every body it was asked to send, so a test
    /// can read the request this program actually builds.
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
    }

    impl MessageTransport for ScriptedTransport {
        fn post_message(&self, body: &str) -> Result<String, InsightFailureCategory> {
            self.sent.borrow_mut().push(body.to_string());
            self.answer.clone()
        }
    }

    fn vocabulary() -> KnowledgeVocabulary {
        KnowledgeVocabulary::new(vec!["concept".to_string()], vec!["related-to".to_string()])
            .unwrap()
    }

    fn answered(topic: &str, confidence: f64) -> String {
        let structured = json!({
            "topic": topic, "confidence": confidence, "entities": [], "relations": []
        });
        json!({
            "model": "the-model-that-answered",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": structured.to_string()}],
            "usage": {"input_tokens": 812, "output_tokens": 24},
        })
        .to_string()
    }

    fn sent_body(effort: &str) -> Value {
        let transport = ScriptedTransport::answering(&answered("budget", 0.9));
        let provider = TopicProvider::new(transport, MODEL.to_string(), effort.to_string());
        provider
            .describe(InsightRequest::new(
                "the document text",
                &vocabulary(),
                permit(),
            ))
            .unwrap();
        let body = provider.transport.sent.borrow()[0].clone();
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn the_request_carries_the_content_the_model_and_the_answer_schema() {
        let body = sent_body("low");

        assert_eq!(body["model"], MODEL);
        assert_eq!(body["max_tokens"], MAX_TOKENS);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "the document text");
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            body["output_config"]["format"]["schema"]["required"],
            json!(["topic", "confidence", "entities", "relations"])
        );
        assert!(
            body["system"]
                .as_str()
                .unwrap()
                .contains("one of: concept.")
        );
        assert_eq!(
            body["output_config"]["format"]["schema"]["additionalProperties"],
            json!(false)
        );
        // Parameters the current models reject outright are not sent at all.
        for rejected in ["temperature", "top_p", "top_k", "thinking"] {
            assert!(body.get(rejected).is_none(), "{rejected} must not be sent");
        }
    }

    /// A model that does not accept an effort level needs the field left out, not set to
    /// something harmless: an unknown field is a rejected request.
    #[test]
    fn an_empty_effort_leaves_the_field_out_of_the_request() {
        let body = sent_body("");

        assert!(body["output_config"].get("effort").is_none());
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    }

    #[test]
    fn a_well_formed_answer_becomes_a_topic_with_what_it_cost() {
        let answer = parse_answer(&answered("quarterly budget", 0.87), MODEL).unwrap();

        assert_eq!(answer.topic().label(), "quarterly budget");
        assert_eq!(answer.topic().confidence().value(), 0.87);
        assert_eq!(answer.provenance().produced_by(), DESCRIBED_BY);
        // What answered, as it said itself, not what was asked for.
        assert_eq!(answer.provenance().model_id(), "the-model-that-answered");
        assert_eq!(answer.provenance().prompt_version(), PROMPT_VERSION);
        assert_eq!(answer.provenance().input_tokens(), 812);
        assert_eq!(answer.provenance().output_tokens(), 24);
    }

    #[test]
    fn a_decline_is_its_own_category_and_never_retried() {
        let declined = json!({
            "model": MODEL,
            "stop_reason": "refusal",
            "content": [],
            "usage": {"input_tokens": 10, "output_tokens": 0},
        })
        .to_string();

        assert_eq!(
            parse_answer(&declined, MODEL).unwrap_err(),
            InsightFailureCategory::Refused
        );
    }

    #[test]
    fn everything_that_is_not_the_agreed_shape_is_refused_as_schema() {
        let cut_off = json!({
            "model": MODEL,
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "{\"topic\": \"bud"}],
        })
        .to_string();
        let no_text_block =
            json!({"model": MODEL, "stop_reason": "end_turn", "content": []}).to_string();
        let not_json = json!({
            "model": MODEL, "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "the topic is budgets"}],
        })
        .to_string();
        let missing_field = json!({
            "model": MODEL, "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"topic\": \"budget\"}"}],
        })
        .to_string();
        let wrong_type = json!({
            "model": MODEL, "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"topic\": 7, \"confidence\": 0.5}"}],
        })
        .to_string();

        for (name, body) in [
            ("not a response at all", "<html>gateway</html>".to_string()),
            ("cut off before it finished", cut_off),
            ("no text block", no_text_block),
            ("text that is not json", not_json),
            ("a field missing", missing_field),
            ("a field of the wrong type", wrong_type),
            // The domain refuses these even though the service's schema accepted them,
            // which is the whole reason the second check exists.
            ("an empty label", answered("   ", 0.5)),
            (
                "a label that is a paragraph",
                answered(&"a".repeat(200), 0.5),
            ),
            ("a label with a newline", answered("budget\nand more", 0.5)),
            ("a confidence out of range", answered("budget", 7.0)),
        ] {
            assert_eq!(
                parse_answer(&body, MODEL).unwrap_err(),
                InsightFailureCategory::Schema,
                "{name}"
            );
        }
    }
}
