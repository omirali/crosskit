// What the model is asked: the rules of a decision map from `docs/decision-map.md`, a schema
// it must answer in, and a window of numbered replies.
//
// A long conversation is put in windows of whole replies, each asked on its own: one call has a
// time limit and a size the product already holds its requests to. A pasted dump inside a reply
// is cut to its start, with the cut said in place — a quote from the cut part cannot be proved,
// and that is counted, not hidden.
use serde_json::{Value, json};

use super::speech::Reply;
use crate::domain::conversation::Speaker;

/// The most bytes of replies one call is given.
pub(super) const WINDOW_BYTES: usize = 60_000;
/// The most bytes of one reply that reach the model.
const REPLY_BYTES: usize = 6_000;
/// The version of the question: a map from another version is not compared with this one.
pub(super) const QUESTION_VERSION: &str = "decision-map-v1";

pub(super) fn instructions(task: &str) -> String {
    format!(
        "Ты размечаешь беседу человека с ИИ-агентом как карту решений по задаче «{task}».\n\
         Реплики пронумерованы: [N] Владелец или [N] Агент.\n\n\
         РАЗВИЛКА — точка, где выбирали подход: «как сделать X?». Только если выбор меняет \
         направление кода или данных или его дорого откатить. Мелкие выборы не развилки.\n\
         У развилки ветки — варианты. Состояние ветки:\n\
         - chosen — выбрана человеком;\n\
         - rejected — отклонена (обязательно причина);\n\
         - deferred — отложена (условие возврата);\n\
         - open — ждала решения человека и не получила его;\n\
         - agent_default — человек не ответил, агент выбрал сам;\n\
         - abandoned — на вопрос ответили другим или сменили тему.\n\n\
         РИСК — предупреждение, вопрос или непроверенное действие, на которое в беседе не \
         ответили или не закрыли. Уровень:\n\
         - critical — деньги, потеря или порча данных, правки на проде, безопасность;\n\
         - high — непроверенное поведение: проверка не запускалась или упала, команда отклонена;\n\
         - medium — вопрос агента без ответа, допущение;\n\
         - info — TODO, мелочь.\n\
         Статус риска: open, accepted (человек осознанно принял, с причиной), resolved (закрыт \
         позже в этой же беседе). in_task = false, если риск не про задачу «{task}» \
         (находка вне задачи).\n\n\
         У каждого узла — номер реплики (reply) и ДОСЛОВНАЯ цитата (quote) из этой реплики, \
         15–200 символов, скопированная символ в символ. Узел без точной цитаты будет отброшен. \
         Лучше пропустить узел, чем выдумать: лишний узел хуже пропущенного. Пиши вопрос, вариант, \
         причину и описание по-русски, коротко."
    )
}

pub(super) fn schema() -> Value {
    let quoted = |extra: Value| {
        let mut object = json!({
            "reply": {"type": "integer"},
            "quote": {"type": "string"}
        });
        if let (Some(target), Some(more)) = (object.as_object_mut(), extra.as_object()) {
            target.extend(more.clone());
        }
        object
    };
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["forks", "risks"],
        "properties": {
            "forks": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["question", "reply", "quote", "branches"],
                "properties": quoted(json!({
                    "question": {"type": "string"},
                    "branches": {"type": "array", "items": {
                        "type": "object", "additionalProperties": false,
                        "required": ["option", "state", "reason", "reply", "quote"],
                        "properties": quoted(json!({
                            "option": {"type": "string"},
                            "state": {"enum": ["chosen", "rejected", "deferred", "open", "agent_default", "abandoned"]},
                            "reason": {"type": "string"}
                        }))
                    }}
                }))
            }},
            "risks": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["description", "level", "status", "in_task", "reply", "quote"],
                "properties": quoted(json!({
                    "description": {"type": "string"},
                    "level": {"enum": ["critical", "high", "medium", "info"]},
                    "status": {"enum": ["open", "accepted", "resolved"]},
                    "in_task": {"type": "boolean"}
                }))
            }}
        }
    })
}

/// The replies in windows of whole replies, each at most `WINDOW_BYTES` once cut.
pub(super) fn windows(replies: &[Reply]) -> Vec<Vec<&Reply>> {
    let mut windows: Vec<Vec<&Reply>> = vec![Vec::new()];
    let mut size = 0;
    for reply in replies.iter().filter(|reply| !reply.text.trim().is_empty()) {
        let bytes = reply.text.len().min(REPLY_BYTES);
        if size + bytes > WINDOW_BYTES && !windows[windows.len() - 1].is_empty() {
            windows.push(Vec::new());
            size = 0;
        }
        size += bytes;
        let last = windows.len() - 1;
        windows[last].push(reply);
    }
    windows.retain(|window| !window.is_empty());
    windows
}

/// One window as the model reads it.
pub(super) fn rendered(window: &[&Reply]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(number: usize, bytes: usize) -> Reply {
        Reply {
            number,
            speaker: Speaker::Owner,
            text: "я".repeat(bytes / 2),
        }
    }

    #[test]
    fn windows_hold_whole_replies_and_a_long_reply_is_cut_where_it_is_said() {
        let replies = [reply(1, 40_000), reply(2, 40_000), reply(3, 100)];
        let cut: Vec<Vec<usize>> = windows(&replies)
            .iter()
            .map(|window| window.iter().map(|r| r.number).collect())
            .collect();
        assert_eq!(cut, [vec![1, 2, 3]]);
        let text = rendered(&windows(&replies)[0]);
        assert!(text.contains("[1] Владелец"));
        assert!(text.contains("…[обрезано: ещё"));
    }
}
