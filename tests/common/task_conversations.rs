// Conversations of a task and a stand-in `claude` that maps them: what the tests of `tasks map`
// and of the service answering it are built from (ADR 0055–0057).
use std::fs;
use std::path::{Path, PathBuf};

/// Two conversations handing a note on, and one about something else a week later, written as
/// Claude Code transcripts into `documents`.
pub fn write_task_conversations(documents: &Path) {
    let documents = fs::canonicalize(documents).unwrap();
    let handoff = documents.join("handoff.md").display().to_string();
    fs::write(documents.join("handoff.md"), "что сделано по скидкам").unwrap();
    let said = |session: &str, role: &str, at: &str, text: &str| {
        format!(
            r#"{{"type":"{role}","sessionId":"{session}","timestamp":"{at}","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    };
    let titled = |session: &str, title: &str| {
        format!(r#"{{"type":"ai-title","sessionId":"{session}","aiTitle":"{title}"}}"#)
    };
    let transcripts = [
        (
            "discount.jsonl",
            vec![
                titled("aaaa1111-discount", "Двойная скидка в корзине"),
                said(
                    "aaaa1111-discount",
                    "user",
                    "2026-10-01T07:00:00.000Z",
                    "Почему скидка дважды?",
                ),
                format!(
                    r#"{{"type":"assistant","sessionId":"aaaa1111-discount","timestamp":"2026-10-01T07:00:02.000Z","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Write","input":{{"file_path":"{handoff}"}}}}]}}}}"#
                ),
            ],
        ),
        (
            "discount-next.jsonl",
            vec![
                titled("bbbb2222-next", "Скидка: продолжение"),
                said(
                    "bbbb2222-next",
                    "user",
                    "2026-10-02T07:00:00.000Z",
                    &format!("Прочитай {handoff} и продолжай"),
                ),
                said(
                    "bbbb2222-next",
                    "assistant",
                    "2026-10-02T07:00:01.000Z",
                    "Читаю.",
                ),
            ],
        ),
        (
            "report.jsonl",
            vec![
                titled("cccc3333-report", "Отчёт по продажам за квартал"),
                said(
                    "cccc3333-report",
                    "user",
                    "2026-10-09T07:00:00.000Z",
                    "Собери отчёт",
                ),
            ],
        ),
    ];
    for (name, lines) in transcripts {
        fs::write(documents.join(name), format!("{}\n", lines.join("\n"))).unwrap();
    }
}

/// The decisions pass's answer: one fork the first reply of the first conversation proves.
const FORKS: &str = r#"{"forks":[{"question":"Где считать скидку?","reply":1,"quote":"Почему скидка дважды","branches":[{"option":"разобраться","state":"chosen","reason":"","reply":1,"quote":"Почему скидка дважды"}]}]}"#;
/// The risks pass's answer: one critical risk a quote proves and one risk it does not.
const RISKS: &str = r#"{"risks":[{"description":"причина не найдена","level":"critical","status":"open","in_task":true,"reply":1,"quote":"Почему скидка дважды"},{"description":"выдумано","level":"high","status":"open","in_task":true,"reply":1,"quote":"этих слов в реплике нет"}]}"#;
/// The join's answer: the fork as the main line and the risk as the task's.
const JOINED: &str = r#"{"main_line":[{"title":"Разобраться со скидкой","state":"chosen","nodes":["c0.f0"]}],"risks":[{"title":"Причина не найдена","level":"critical","status":"open","nodes":["c0.r0"]}],"findings":[]}"#;

/// A `claude` that answers every map question by its schema and writes the pass it answered
/// (`forks`, `risks` or `join`) as a line of `calls`. The directory to put first on PATH, and
/// the root to remove.
pub fn fake_map_engine(calls: &Path) -> (PathBuf, String) {
    let envelope = |answer: &str| {
        format!(
            r#"{{"type":"result","is_error":false,"structured_output":{answer},"usage":{{"input_tokens":10,"output_tokens":5}},"total_cost_usd":0.01}}"#
        )
    };
    // Each question is told by the schema it is asked in, as the tool receives it: the join's
    // has a main line, the risks pass's has `in_task`, the decisions pass's has neither.
    super::fake_engine(&format!(
        "cat >/dev/null; echo \"$*\" | grep -q main_line && pass=join || {{ echo \"$*\" | grep -q in_task && pass=risks || pass=forks; }}; echo $pass >> '{}'; case $pass in join) printf '%s' '{}' ;; risks) printf '%s' '{}' ;; *) printf '%s' '{}' ;; esac",
        calls.display(),
        envelope(JOINED),
        envelope(RISKS),
        envelope(FORKS)
    ))
}
