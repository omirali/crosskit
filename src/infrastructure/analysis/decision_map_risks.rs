// The second pass of the decision map question: the risks and nothing else (ADR 0057).
//
// It carries version 3's levels, statuses and the checklist of production, secrets and data
// asked of every window: the checklist is what found both critical risks version 2 had missed
// (ADR 0056), and here it no longer competes with the owner's decisions for the model's
// attention.
use serde_json::{Value, json};

use crate::domain::decision_map::{RiskLevel, RiskStatus};

use super::decision_map_question::{MARKING, opening, with_evidence};

pub(super) fn instructions(task: &str) -> String {
    format!(
        "{}\n\n\
         РИСК — предупреждение, вопрос или непроверенное действие, на которое в беседе не \
         ответили или не закрыли. Уровень:\n\
         - critical — деньги, потеря или порча данных, правки на проде, безопасность, секреты;\n\
         - high — непроверенное поведение: проверка не запускалась или упала, команда отклонена;\n\
         - medium — вопрос Агента без ответа, допущение;\n\
         - info — TODO, мелочь.\n\
         Статус риска: open; accepted — Владелец осознанно принял, с причиной; resolved — закрыт \
         позже в этой же беседе. in_task = false, если риск не про задачу «{task}» (находка вне \
         задачи). Ищи риски и в длинных отчётах Агента: оговорки «не проверял», «предполагаю», \
         «осталось», вопросы в конце отчёта, на которые Владелец не ответил.\n\n\
         ОБЯЗАТЕЛЬНО ПРОВЕРЬ В КАЖДОМ ОКНЕ, и если было — заведи риск:\n\
         - прод: изменения данных или запуск скриптов на проде, релиз, порядок выката, работа \
         в обход ревью или MR;\n\
         - секреты: пароли, токены, ключи, kubeconfig, файлы .env — где оказались, кто их видит;\n\
         - данные: удаление, массовые правки, перенос, бэкфилл — чем проверены, можно ли откатить.\n\n\
         Развилки и выбор подхода здесь не размечай: их ищет другой проход.\n\n\
         {MARKING}",
        opening(task, "риски")
    )
}

pub(super) fn schema() -> Value {
    let levels: Vec<&str> = RiskLevel::ALL.iter().map(|level| level.code()).collect();
    let statuses: Vec<&str> = RiskStatus::ALL.iter().map(|status| status.code()).collect();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["risks"],
        "properties": {
            "risks": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["description", "level", "status", "in_task", "reply", "quote"],
                "properties": with_evidence(json!({
                    "description": {"type": "string"},
                    "level": {"enum": levels},
                    "status": {"enum": statuses},
                    "in_task": {"type": "boolean"}
                }))
            }}
        }
    })
}
