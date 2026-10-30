// End to end, with a real graph and a real scan: tasks stitched from what the graph kept of the
// conversations (ADR 0052).
//
// The rules of stitching are held by unit tests with no storage in the way. What is here needs
// the storage: that the graph hands back each conversation's name, the owner's words apart from
// the model's, and the files it wrote, so that a conversation opened on another's handoff is
// found in one task with it.
use super::period_pipeline_tests::iso;
use super::search_pipeline_tests::context;
use crate::application::{TaskCandidates, TaskMember, Tie};

const HOME: &str = "/Users/alice";
const HOUR: i64 = 60 * 60 * 1000;
/// 1 October 2026, 07:00 UTC.
const START: i64 = 1_790_838_000_000;

fn titled(session: &str, title: &str) -> String {
    format!(r#"{{"type":"ai-title","sessionId":"{session}","aiTitle":"{title}"}}"#)
}

fn said(session: &str, role: &str, at: i64, text: &str) -> String {
    format!(
        r#"{{"type":"{role}","sessionId":"{session}","timestamp":"{}","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#,
        iso(at)
    )
}

fn wrote(session: &str, at: i64, path: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{session}","timestamp":"{}","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Write","input":{{"file_path":"{path}"}}}}]}}}}"#,
        iso(at)
    )
}

fn transcript(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// A conversation that wrote a handoff, the next one opened on it a day later, and an unrelated
/// one a week on: the first two are one task, the third is none.
#[test]
fn a_conversation_opened_on_another_ones_handoff_is_found_in_one_task_with_it() {
    let context = context("tasks-handoff");
    let handoff = context.at("handoff.md");
    context.write("handoff.md", "что сделано и что дальше по скидкам");
    context.write(
        "discount.jsonl",
        &transcript(&[
            titled("discount", "Двойная скидка в корзине"),
            said("discount", "user", START, "Почему скидка считается дважды?"),
            said(
                "discount",
                "assistant",
                START + 1,
                "Нашёл: в cart.rs и в order.rs.",
            ),
            wrote("discount", START + 2, &handoff),
        ]),
    );
    context.write(
        "discount-next.jsonl",
        &transcript(&[
            titled("discount-next", "Скидка: продолжение"),
            said(
                "discount-next",
                "user",
                START + 24 * HOUR,
                &format!("Прочитай {handoff} и продолжай"),
            ),
            said(
                "discount-next",
                "assistant",
                START + 24 * HOUR + 1,
                "Читаю.",
            ),
        ]),
    );
    context.write(
        "report.jsonl",
        &transcript(&[
            titled("report", "Отчёт по продажам за квартал"),
            said(
                "report",
                "user",
                START + 7 * 24 * HOUR,
                "Собери отчёт по продажам",
            ),
            said(
                "report",
                "assistant",
                START + 7 * 24 * HOUR + 1,
                &format!("Как в {handoff}, только про продажи."),
            ),
        ]),
    );
    context.init();

    let tasks = TaskCandidates::new(&context.manager.repository())
        .find(HOME)
        .unwrap()
        .candidates;
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    let members: Vec<(&str, Tie)> = tasks[0]
        .members()
        .iter()
        .map(|member| (member.session_id(), member.tie()))
        .collect();
    assert_eq!(
        members,
        [("discount", Tie::Strong), ("discount-next", Tie::Strong)]
    );
    assert_eq!(
        tasks[0].members().first().and_then(TaskMember::name),
        Some("Двойная скидка в корзине")
    );
}
