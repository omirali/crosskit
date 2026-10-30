// The first pass of the decision map question: the owner's decisions and nothing else (ADR 0057).
//
// It carries version 3's words on where decisions are made, what a fork is and what became of
// each branch, and no word on risks: with the checklist of risks beside them, a long
// conversation kept 6 forks of the 21 version 2 had found (ADR 0056). A decision of the owner to
// do something on production stays here, as a fork; the risk of it is the other pass's.
use serde_json::{Value, json};

use crate::domain::decision_map::BranchState;

use super::decision_map_question::{MARKING, opening, with_evidence};

pub(super) fn instructions(task: &str) -> String {
    format!(
        "{}\n\n\
         ГДЕ РЕШАЮТ. Решения принимает Владелец, и обычно короткой репликой: «давай сначала X», \
         «нет, не так», «оставь как есть», «потом». Проверь КАЖДУЮ реплику Владельца: если она \
         выбирает, отклоняет, откладывает или поворачивает работу, это развилка. Самые важные \
         развилки — где Владелец пошёл не туда, куда предлагал Агент.\n\n\
         РАЗВИЛКА — выбор подхода, который меняет направление кода или данных или который дорого \
         откатить. Мелкие выборы не развилки. У развилки ветки — варианты. Состояние ветки:\n\
         - chosen — выбрал ВЛАДЕЛЕЦ своими словами (цитата из его реплики);\n\
         - rejected — отклонена Владельцем или по ходу работы (обязательно причина);\n\
         - deferred — отложена (условие возврата);\n\
         - open — ждала решения Владельца и не получила его;\n\
         - agent_default — Владелец не ответил, Агент выбрал сам;\n\
         - abandoned — Владелец ответил на другое или сменил тему.\n\
         Предложение Агента без слов Владельца — НЕ chosen, а agent_default, open или abandoned.\n\n\
         Решение Владельца сделать что-то на проде — изменить данные, запустить скрипт, выкатить \
         релиз, обойти ревью или MR — это тоже развилка: что выбрано и что было альтернативой, \
         даже если он сказал это одной фразой.\n\n\
         Риски, предупреждения и непроверенное здесь не размечай: их ищет другой проход.\n\n\
         {MARKING}",
        opening(task, "решения Владельца — развилки")
    )
}

pub(super) fn schema() -> Value {
    let states: Vec<&str> = BranchState::ALL.iter().map(|state| state.code()).collect();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["forks"],
        "properties": {
            "forks": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["question", "reply", "quote", "branches"],
                "properties": with_evidence(json!({
                    "question": {"type": "string"},
                    "branches": {"type": "array", "items": {
                        "type": "object", "additionalProperties": false,
                        "required": ["option", "state", "reason", "reply", "quote"],
                        "properties": with_evidence(json!({
                            "option": {"type": "string"},
                            "state": {"enum": states},
                            "reason": {"type": "string"}
                        }))
                    }}
                }))
            }}
        }
    })
}
