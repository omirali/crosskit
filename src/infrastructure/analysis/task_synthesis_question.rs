// The join question: a task's conversations' maps, by their nodes, asked to become one main line,
// risks followed across conversations, and findings outside the task (ADR 0056).
//
// The model reads nodes, not replies, and answers with the codes of the nodes each item stands
// for. The codes are checked by the program (`grounded`): a code no map holds is not a reference.
use serde_json::{Value, json};

use crate::application::{MapEngineFailure, MapMerger, MergeInput, MergedProposal};
use crate::domain::CivilDate;
use crate::domain::decision_map::{BranchState, RiskLevel, RiskStatus};
use crate::domain::task_synthesis::{LineStep, NodeRef, TaskRisk, TaskSynthesis};

use super::decision_map_question::{ClaudeCodeMapper, envelope_cost};

pub const MERGE_VERSION: &str = "task-synthesis-v1";
const DAY_MS: i64 = 86_400_000;

impl MapMerger for ClaudeCodeMapper {
    fn merge_version(&self) -> &str {
        MERGE_VERSION
    }

    fn merge(
        &self,
        task: &str,
        conversations: &[MergeInput<'_>],
    ) -> Result<MergedProposal, MapEngineFailure> {
        let prompt = format!("{}\n\n---\n\n{}", instructions(task), listed(conversations));
        let envelope = self.ask(&schema(), prompt)?;
        let (input_tokens, output_tokens, cost_micro_usd) = envelope_cost(&envelope);
        Ok(MergedProposal {
            synthesis: synthesis_of(&envelope["structured_output"]),
            input_tokens,
            output_tokens,
            cost_micro_usd,
        })
    }
}

fn instructions(task: &str) -> String {
    format!(
        "Ниже — карты решений отдельных бесед одной задачи «{task}», в порядке времени. У каждого \
         узла есть код: c<беседа>.f<развилка> или c<беседа>.r<риск>. Собери из них карту ВСЕЙ \
         задачи.\n\n\
         main_line — ключевые решения задачи по порядку: что выбирали и что выбрали. Одно и то \
         же решение, обсуждённое в нескольких беседах, — ОДИН шаг со всеми его узлами. Мелкие \
         развилки, не повлиявшие на ход задачи, пропусти. state — итог: chosen, rejected, \
         deferred, open, agent_default или abandoned, по самому позднему узлу.\n\n\
         risks — риски задачи. Один и тот же риск из разных бесед — ОДИН пункт со всеми его \
         узлами. status — по самому позднему узлу: закрыт в поздней беседе — resolved. level — \
         самый высокий из узлов.\n\n\
         findings — то же для узлов с пометкой «вне задачи».\n\n\
         В nodes — только коды из списка, символ в символ. Пункт без кодов будет отброшен. \
         title — коротко, по-русски."
    )
}

fn schema() -> Value {
    let states: Vec<&str> = BranchState::ALL.iter().map(|state| state.code()).collect();
    let levels: Vec<&str> = RiskLevel::ALL.iter().map(|level| level.code()).collect();
    let statuses: Vec<&str> = RiskStatus::ALL.iter().map(|status| status.code()).collect();
    let nodes = json!({"type": "array", "items": {"type": "string"}});
    let risk = json!({
        "type": "object", "additionalProperties": false,
        "required": ["title", "level", "status", "nodes"],
        "properties": {
            "title": {"type": "string"},
            "level": {"enum": levels},
            "status": {"enum": statuses},
            "nodes": nodes
        }
    });
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["main_line", "risks", "findings"],
        "properties": {
            "main_line": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "required": ["title", "state", "nodes"],
                "properties": {
                    "title": {"type": "string"},
                    "state": {"enum": states},
                    "nodes": nodes
                }
            }},
            "risks": {"type": "array", "items": risk.clone()},
            "findings": {"type": "array", "items": risk}
        }
    })
}

/// Every conversation's nodes with their codes, as the model reads them.
fn listed(conversations: &[MergeInput<'_>]) -> String {
    let mut text = String::new();
    for (conversation, input) in conversations.iter().enumerate() {
        let day = input.first_at_unix_ms.map_or_else(
            || "?".to_string(),
            |ms| CivilDate::from_days(ms.div_euclid(DAY_MS)).to_string(),
        );
        text.push_str(&format!("[c{conversation}] {day} «{}»\n", input.label));
        for (index, fork) in input.nodes.forks.iter().enumerate() {
            let code = NodeRef::Fork {
                conversation,
                index,
            }
            .code();
            text.push_str(&format!("  {code} развилка: {}\n", fork.question));
            for branch in &fork.branches {
                text.push_str(&format!(
                    "     {}: {} — {}\n",
                    branch.state.code(),
                    branch.option,
                    branch.reason
                ));
            }
        }
        for (index, risk) in input.nodes.risks.iter().enumerate() {
            let code = NodeRef::Risk {
                conversation,
                index,
            }
            .code();
            let outside = if risk.in_task {
                ""
            } else {
                " (вне задачи)"
            };
            text.push_str(&format!(
                "  {code} риск {} {}{outside}: {}\n",
                risk.level.code(),
                risk.status.code(),
                risk.description
            ));
        }
        text.push('\n');
    }
    text
}

fn synthesis_of(answer: &Value) -> TaskSynthesis {
    let nodes = |item: &Value| -> Vec<NodeRef> {
        item["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|code| NodeRef::parse(code.as_str()?))
            .collect()
    };
    let text = |item: &Value| item["title"].as_str().unwrap_or_default().to_string();
    let risks = |key: &str| -> Vec<TaskRisk> {
        answer[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                Some(TaskRisk {
                    title: text(item),
                    level: RiskLevel::parse(item["level"].as_str()?)?,
                    status: RiskStatus::parse(item["status"].as_str()?)?,
                    nodes: nodes(item),
                })
            })
            .collect()
    };
    TaskSynthesis {
        main_line: answer["main_line"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                Some(LineStep {
                    title: text(item),
                    state: BranchState::parse(item["state"].as_str()?)?,
                    nodes: nodes(item),
                })
            })
            .collect(),
        risks: risks("risks"),
        findings: risks("findings"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_becomes_steps_and_risks_by_their_codes() {
        let answer = json!({
            "main_line": [{"title": "Сначала данные", "state": "chosen", "nodes": ["c0.f3", "c1.f0", "не код"]}],
            "risks": [{"title": "Секреты в файле", "level": "critical", "status": "open", "nodes": ["c1.r2"]}],
            "findings": []
        });
        let synthesis = synthesis_of(&answer);
        assert_eq!(synthesis.main_line[0].nodes.len(), 2);
        assert_eq!(synthesis.risks[0].level, RiskLevel::Critical);
    }
}
