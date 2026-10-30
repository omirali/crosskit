// The decision map question, asked of the installed tool (ADR 0055).
//
// Version 4 asks every window twice (ADR 0057). Version 3 asked for forks and risks in one
// question, and its checklist of risks took the model's attention from the owner's decisions:
// a long conversation kept 6 forks where version 2 had found 21 (ADR 0056). So one pass looks
// only for the owner's decisions (`decision_map_forks`) and the other only for risks, with the
// checklist (`decision_map_risks`). What both read — the window and where it stands — and how an
// answer becomes nodes are here, once.
//
// Version 3 answers what the first run of version 2 found (ADR 0055): the model took the end of
// a window for the end of the conversation, missed the costliest decisions of a long
// conversation — work on production without review, a release without its settings, secrets
// written into a file — and wrote a risk about its own markup. So the window says where it
// stands, a checklist of what is never left unsaid is asked for every window, and the model is
// told it marks a conversation and is not a party to it.
//
// Version 2 answered what the measurement of version 1 found (ADR 0054): a short reply of the
// owner that turns the work went unseen beside the agent's long proposals, and a weaker model
// put the agent's proposals down as the owner's choices. So the owner's replies are marked as
// the places decisions are made, every one that turns the work is to be checked as a fork, and
// a branch is `chosen` only when the owner's own words choose it.
use serde_json::{Value, json};

use crate::application::{MapEngine, MapEngineFailure, ProposedStretch, REPLY_BYTES, SpokenReply};
use crate::domain::ContentEgressPermit;
use crate::domain::conversation::Speaker;
use crate::domain::decision_map::{
    Branch, BranchState, Evidence, Fork, MapNodes, Risk, RiskLevel, RiskStatus,
};

use super::claude_code::{ClaudeCodeSettings, ClaudeCodeTool};
use super::{decision_map_forks as forks, decision_map_risks as risks};

pub const QUESTION_VERSION: &str = "decision-map-v4";
/// The model a map is built with when the configuration names none. The measurement of ADR 0054
/// found a smaller model putting the agent's proposals down as the owner's choices.
pub(super) const DEFAULT_MAP_MODEL: &str = "sonnet";

/// The installed tool asking the decision map question. It holds the context's permission to
/// send content out, so it cannot be made without it.
pub struct ClaudeCodeMapper {
    tool: ClaudeCodeTool,
    model: String,
    _permit: ContentEgressPermit,
}

impl ClaudeCodeMapper {
    pub(super) fn new(
        binary: std::path::PathBuf,
        settings: &ClaudeCodeSettings,
        permit: ContentEgressPermit,
    ) -> Self {
        let model = if settings.model().is_empty() {
            DEFAULT_MAP_MODEL.to_string()
        } else {
            settings.model().to_string()
        };
        Self {
            tool: ClaudeCodeTool::new(binary, ClaudeCodeSettings::new(model.clone())),
            model,
            _permit: permit,
        }
    }

    /// One question in `schema`, and the tool's envelope back when it answered in it.
    pub(super) fn ask(&self, schema: &Value, prompt: String) -> Result<Value, MapEngineFailure> {
        let envelope = self
            .tool
            .call(schema, prompt)
            .map_err(|failure| MapEngineFailure(format!("the tool did not answer: {failure:?}")))?;
        if envelope["is_error"].as_bool() == Some(true) {
            return Err(MapEngineFailure(format!(
                "the tool refused: {}",
                envelope["subtype"].as_str().unwrap_or("unknown")
            )));
        }
        if !envelope["structured_output"].is_object() {
            return Err(MapEngineFailure(
                "the tool answered outside the schema".to_string(),
            ));
        }
        Ok(envelope)
    }

    /// The model the maps are asked of.
    pub fn model(&self) -> &str {
        &self.model
    }
}

impl MapEngine for ClaudeCodeMapper {
    fn question_version(&self) -> &str {
        QUESTION_VERSION
    }

    fn propose(
        &self,
        task: &str,
        window: &[&SpokenReply],
        replies: usize,
    ) -> Result<ProposedStretch, MapEngineFailure> {
        asked_twice(task, window, replies, |schema, prompt| {
            self.ask(schema, prompt)
        })
    }
}

/// One window asked twice, the owner's decisions first and the risks after, and kept as one
/// stretch: the forks of the first answer, the risks of the second and the cost of both. Either
/// pass failing fails the window whole, so the map never keeps half of one (ADR 0057).
fn asked_twice(
    task: &str,
    window: &[&SpokenReply],
    replies: usize,
    ask: impl Fn(&Value, String) -> Result<Value, MapEngineFailure>,
) -> Result<ProposedStretch, MapEngineFailure> {
    let stretch = format!("{}\n\n{}", window_note(window, replies), rendered(window));
    let forks_answer = ask(
        &forks::schema(),
        format!("{}\n\n---\n\n{stretch}", forks::instructions(task)),
    )?;
    let risks_answer = ask(
        &risks::schema(),
        format!("{}\n\n---\n\n{stretch}", risks::instructions(task)),
    )?;
    let (forks_in, forks_out, forks_cost) = envelope_cost(&forks_answer);
    let (risks_in, risks_out, risks_cost) = envelope_cost(&risks_answer);
    Ok(ProposedStretch {
        nodes: MapNodes {
            forks: nodes_of(&forks_answer["structured_output"]).forks,
            risks: nodes_of(&risks_answer["structured_output"]).risks,
        },
        input_tokens: forks_in + risks_in,
        output_tokens: forks_out + risks_out,
        cost_micro_usd: forks_cost + risks_cost,
    })
}

/// How both passes begin: who speaks in the conversation and the one thing this pass looks for.
pub(super) fn opening(task: &str, looking_for: &str) -> String {
    format!(
        "Ты размечаешь беседу человека (Владелец) с ИИ-агентом (Агент) по задаче «{task}» и \
         ищешь в ней только {looking_for}. Реплики пронумерованы: [N] Владелец или [N] Агент."
    )
}

/// How both passes end: the marker is not a party to the conversation, and a node stands on a
/// reply and a quote copied from it, or it is dropped.
pub(super) const MARKING: &str = "Ты размечаешь беседу и не участвуешь в ней: не заводи узлов \
     о самой разметке, о своём режиме или о том, что ты не можешь выполнить.\n\n\
     У каждого узла — номер реплики (reply) и ДОСЛОВНАЯ цитата (quote) из ЭТОЙ реплики, \
     15–200 символов, скопированная символ в символ, без пересказа. Узел без точной цитаты будет \
     отброшен программой. Не выдумывай узлов. Пиши текст узлов по-русски, коротко.";

/// The properties of a node: the reply it stands on, the quote from it, and `extra`.
pub(super) fn with_evidence(extra: Value) -> Value {
    let mut object = json!({
        "reply": {"type": "integer"},
        "quote": {"type": "string"}
    });
    if let (Some(target), Some(more)) = (object.as_object_mut(), extra.as_object()) {
        target.extend(more.clone());
    }
    object
}

/// What one answer cost: tokens in, tokens out, and the tool's own count in millionths of a
/// dollar.
pub(super) fn envelope_cost(envelope: &Value) -> (u64, u64, u64) {
    let usage = &envelope["usage"];
    let input = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ]
    .iter()
    .filter_map(|key| usage[*key].as_u64())
    .sum();
    let cost = envelope["total_cost_usd"]
        .as_f64()
        .map_or(0, |usd| (usd * 1_000_000.0).round() as u64);
    (input, usage["output_tokens"].as_u64().unwrap_or(0), cost)
}

/// Where the window stands in its conversation. Without it a model read the last reply of a
/// window as the end of the work and reported results as never shown (ADR 0055).
fn window_note(window: &[&SpokenReply], replies: usize) -> String {
    let first = window.first().map_or(0, |reply| reply.number);
    let last = window.last().map_or(0, |reply| reply.number);
    let after = if last < replies {
        format!(
            "Беседа продолжается после реплики {last}: то, что не закончено в окне, может \
             закончиться позже. Не считай последнюю реплику окна концом работы."
        )
    } else {
        "Это конец беседы.".to_string()
    };
    format!("ОКНО: реплики {first}–{last} из {replies}. {after}")
}

/// One window as the model reads it: every reply numbered and named by who said it, a reply
/// longer than `REPLY_BYTES` cut where the cut is said.
fn rendered(window: &[&SpokenReply]) -> String {
    let mut text = String::new();
    for reply in window {
        let who = match reply.speaker {
            Speaker::Owner => "Владелец",
            Speaker::Model => "Агент",
        };
        text.push_str(&format!("[{}] {who}\n", reply.number));
        if reply.text.len() > REPLY_BYTES {
            let mut end = REPLY_BYTES;
            while !reply.text.is_char_boundary(end) {
                end -= 1;
            }
            text.push_str(&reply.text[..end]);
            text.push_str(&format!(
                "\n…[обрезано: ещё {} байт этой реплики]\n\n",
                reply.text.len() - end
            ));
        } else {
            text.push_str(&reply.text);
            text.push_str("\n\n");
        }
    }
    text
}

/// The nodes of a structured answer. A node with a state, level or status the schema does not
/// name is not a node; the schema already refused it, and this does not invent one.
fn nodes_of(answer: &Value) -> MapNodes {
    let evidence = |node: &Value| Evidence {
        reply: node["reply"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
        quote: node["quote"].as_str().unwrap_or_default().to_string(),
    };
    let text = |node: &Value, key: &str| node[key].as_str().unwrap_or_default().to_string();
    let forks = answer["forks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|fork| Fork {
            question: text(fork, "question"),
            evidence: evidence(fork),
            branches: fork["branches"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|branch| {
                    Some(Branch {
                        option: text(branch, "option"),
                        state: BranchState::parse(branch["state"].as_str()?)?,
                        reason: text(branch, "reason"),
                        evidence: evidence(branch),
                    })
                })
                .collect(),
        })
        .collect();
    let risks = answer["risks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|risk| {
            Some(Risk {
                description: text(risk, "description"),
                level: RiskLevel::parse(risk["level"].as_str()?)?,
                status: RiskStatus::parse(risk["status"].as_str()?)?,
                in_task: risk["in_task"].as_bool().unwrap_or(true),
                evidence: evidence(risk),
            })
        })
        .collect();
    MapNodes { forks, risks }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;
    use crate::application::{MapStore, MapStoreFailure, StoredMap, TaskMapping};

    /// The pass a question belongs to, told by its schema the way the tool receives it.
    fn pass_of(schema: &Value) -> &'static str {
        if schema["properties"]["forks"].is_object() {
            "forks"
        } else {
            "risks"
        }
    }

    /// An envelope answering either pass: one fork and one risk, both quoting `reply`, so a pass
    /// that took the other's nodes would show it.
    fn answered(reply: &SpokenReply, cost_usd: f64) -> Value {
        let (number, quote) = (reply.number, &reply.text);
        json!({
            "is_error": false,
            "structured_output": {
                "forks": [{"question": "Что сначала?", "reply": number, "quote": quote,
                    "branches": [{"option": "данные", "state": "chosen", "reason": "", "reply": number, "quote": quote}]}],
                "risks": [{"description": "прод без MR", "level": "critical", "status": "open", "in_task": true, "reply": number, "quote": quote}]
            },
            "usage": {"input_tokens": 100, "output_tokens": 10},
            "total_cost_usd": cost_usd
        })
    }

    fn owner_says(number: usize, text: &str) -> SpokenReply {
        SpokenReply {
            number,
            speaker: Speaker::Owner,
            text: text.into(),
        }
    }

    #[test]
    fn a_window_takes_its_forks_from_the_first_pass_and_its_risks_from_the_second() {
        let replies = [owner_says(1, "давай сначала данные, потом интерфейс")];
        let window: Vec<&SpokenReply> = replies.iter().collect();
        let asked = RefCell::new(Vec::new());
        let stretch = asked_twice("Скидки", &window, 1, |schema, prompt| {
            let pass = pass_of(schema);
            asked.borrow_mut().push(pass);
            assert!(prompt.contains("ОКНО: реплики 1–1 из 1"), "{prompt}");
            let decisions = pass == "forks";
            assert_eq!(prompt.contains("ГДЕ РЕШАЮТ"), decisions, "{prompt}");
            assert_eq!(
                prompt.contains("ОБЯЗАТЕЛЬНО ПРОВЕРЬ"),
                !decisions,
                "{prompt}"
            );
            let mut envelope = answered(&replies[0], if decisions { 0.01 } else { 0.02 });
            let other = if decisions { "risks" } else { "forks" };
            envelope["structured_output"][other][0]["quote"] = json!("из чужого прохода");
            Ok(envelope)
        })
        .unwrap();

        assert_eq!(*asked.borrow(), ["forks", "risks"]);
        assert_eq!(stretch.nodes.forks.len(), 1);
        assert_eq!(stretch.nodes.forks[0].evidence.quote, replies[0].text);
        assert_eq!(stretch.nodes.risks.len(), 1);
        assert_eq!(stretch.nodes.risks[0].evidence.quote, replies[0].text);
        assert_eq!(stretch.input_tokens, 200);
        assert_eq!(stretch.output_tokens, 20);
        assert_eq!(stretch.cost_micro_usd, 30_000);
    }

    /// The engine as `ClaudeCodeMapper` runs it, with the tool replaced by answers that fail the
    /// risks pass on demand.
    struct TwoPasses {
        risks_fail: Cell<bool>,
    }

    impl MapEngine for TwoPasses {
        fn question_version(&self) -> &str {
            QUESTION_VERSION
        }

        fn propose(
            &self,
            task: &str,
            window: &[&SpokenReply],
            replies: usize,
        ) -> Result<ProposedStretch, MapEngineFailure> {
            asked_twice(task, window, replies, |schema, _prompt| {
                if pass_of(schema) == "risks" && self.risks_fail.get() {
                    return Err(MapEngineFailure("the tool refused: overloaded".into()));
                }
                Ok(answered(window[0], 0.01))
            })
        }
    }

    #[derive(Default)]
    struct OneMap(RefCell<Option<StoredMap>>);

    impl MapStore for OneMap {
        fn load(&self, _session_id: &str) -> Result<Option<StoredMap>, MapStoreFailure> {
            Ok(self.0.borrow().clone())
        }

        fn save(&self, map: &StoredMap) -> Result<(), MapStoreFailure> {
            *self.0.borrow_mut() = Some(map.clone());
            Ok(())
        }
    }

    #[test]
    fn a_failed_risks_pass_leaves_its_window_unmapped_and_its_forks_unkept() {
        let speech = |replies: usize| crate::application::ConversationSpeech {
            session_id: "a".into(),
            name: None,
            first_at_unix_ms: None,
            replies: (1..=replies)
                .map(|number| owner_says(number, &format!("реплика номер {number} о данных")))
                .collect(),
        };
        let engine = TwoPasses {
            risks_fail: Cell::new(false),
        };
        let store = OneMap::default();
        let mapping = TaskMapping::new(&engine, &store);
        mapping.build("Скидки", &[speech(2)], &mut |_| {}).unwrap();
        let before = store.0.borrow().clone().unwrap();
        assert_eq!(before.mapped_through, 2);
        assert_eq!((before.nodes.forks.len(), before.nodes.risks.len()), (1, 1));

        engine.risks_fail.set(true);
        let mut failed = false;
        let shown = mapping
            .build("Скидки", &[speech(4)], &mut |progress| {
                failed |= matches!(progress, crate::application::MapProgress::Failed { .. });
            })
            .unwrap();

        assert!(failed);
        assert_eq!(store.0.borrow().as_ref(), Some(&before));
        assert_eq!(shown[0].unmapped_replies(), 2);
    }

    #[test]
    fn an_answer_becomes_nodes_and_an_unnamed_state_is_left_out() {
        let answer = json!({
            "forks": [{"question": "Где считать?", "reply": 3, "quote": "считать в order.rs",
                "branches": [
                    {"option": "order.rs", "state": "chosen", "reason": "", "reply": 4, "quote": "да, в order.rs"},
                    {"option": "cart.rs", "state": "maybe", "reason": "", "reply": 4, "quote": "или в cart.rs"}
                ]}],
            "risks": [{"description": "не проверено", "level": "high", "status": "open", "in_task": false, "reply": 5, "quote": "тесты не запускал"}]
        });
        let nodes = nodes_of(&answer);
        assert_eq!(nodes.forks[0].branches.len(), 1);
        assert_eq!(nodes.forks[0].evidence.reply, 3);
        assert!(!nodes.risks[0].in_task);
        assert_eq!(nodes.risks[0].level, RiskLevel::High);
    }

    #[test]
    fn the_owner_is_named_and_a_long_reply_is_cut_where_it_is_said() {
        let replies = [
            SpokenReply {
                number: 1,
                speaker: Speaker::Owner,
                text: "давай сначала данные".into(),
            },
            SpokenReply {
                number: 2,
                speaker: Speaker::Model,
                text: "я".repeat(REPLY_BYTES),
            },
        ];
        let window: Vec<&SpokenReply> = replies.iter().collect();
        let text = rendered(&window);
        assert!(text.starts_with("[1] Владелец\nдавай сначала данные"));
        assert!(text.contains("[2] Агент"));
        assert!(text.contains("…[обрезано: ещё"));
    }

    #[test]
    fn a_window_says_whether_the_conversation_goes_on_after_it() {
        let replies = [
            SpokenReply {
                number: 40,
                speaker: Speaker::Owner,
                text: "дальше".into(),
            },
            SpokenReply {
                number: 61,
                speaker: Speaker::Model,
                text: "делаю хвост".into(),
            },
        ];
        let window: Vec<&SpokenReply> = replies.iter().collect();
        assert!(window_note(&window, 373).contains("реплики 40–61 из 373. Беседа продолжается"));
        assert!(window_note(&window, 61).ends_with("Это конец беседы."));
    }
}
