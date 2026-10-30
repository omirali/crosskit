// What the reader does with the lines a transcript actually holds.
//
// The fixtures are written here rather than taken from the corpus: the owner's transcripts
// are their working correspondence, and a test that carries a piece of one distributes it.
// Each fixture is the shape of a line, which is all the reader reads.
use std::io::Write;

use std::ops::ControlFlow;

use super::transcript_reader::{read_line, read_transcript};
use crate::application::conversation_assembly::ConversationAssembly;
use crate::domain::OwnerNaming;
use crate::domain::conversation::{
    NamedBy, SkippedLine, Speaker, TouchSource, TouchVerb, TranscriptLine, UnreadableLine,
};

fn meanings(text: &str) -> Vec<TranscriptLine> {
    let mut met = Vec::new();
    let _stop = read_line(text, |line| {
        met.push(line);
        ControlFlow::Continue(())
    });
    met
}

/// The one thing this line turned out to be, once the session it carries is set aside: every
/// line of a real transcript carries one, and no assertion here is about that.
fn spoken(text: &str) -> Vec<TranscriptLine> {
    meanings(text)
        .into_iter()
        .filter(|line| !matches!(line, TranscriptLine::Session(_)))
        .collect()
}

#[test]
fn a_reply_with_text_is_what_the_owner_said() {
    let met = spoken(
        r#"{"type":"user","sessionId":"s","timestamp":"2026-09-03T07:09:25.457Z",
            "message":{"role":"user","content":[{"type":"text","text":"что там?"}]}}"#,
    );
    let [TranscriptLine::Spoke(reply)] = met.as_slice() else {
        panic!("ожидалась одна реплика, вышло {met:?}");
    };
    assert_eq!(reply.speaker(), Speaker::Owner);
    assert_eq!(reply.text(), "что там?");
    assert_eq!(reply.at_unix_ms(), Some(1_788_419_365_457));
}

#[test]
fn a_reply_whose_content_is_a_bare_string_is_still_a_reply() {
    let met = spoken(r#"{"type":"user","message":{"role":"user","content":"проверь базу"}}"#);
    let [TranscriptLine::Spoke(reply)] = met.as_slice() else {
        panic!("ожидалась одна реплика, вышло {met:?}");
    };
    assert_eq!(reply.text(), "проверь базу");
    assert_eq!(reply.at_unix_ms(), None);
}

#[test]
fn a_line_that_is_only_a_tool_result_leaves_no_reply() {
    let met = spoken(
        r#"{"type":"user","toolUseResult":{"stdout":"ok"},
            "message":{"role":"user","content":[
              {"type":"tool_result","content":"жёлтый вывод на двести строк"}]}}"#,
    );
    assert_eq!(
        met,
        vec![
            TranscriptLine::Skipped(SkippedLine::ToolUseResult),
            TranscriptLine::Skipped(SkippedLine::ToolResult),
            TranscriptLine::Skipped(SkippedLine::NoTextLeft),
        ]
    );
}

#[test]
fn the_models_deliberation_and_its_tool_calls_are_not_what_it_said() {
    let met = spoken(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
              {"type":"thinking","thinking":"надо посмотреть в базу"},
              {"type":"tool_use","name":"Bash","input":{"command":"ls"}},
              {"type":"text","text":"Смотрю."}]}}"#,
    );
    assert_eq!(
        met,
        vec![
            TranscriptLine::Skipped(SkippedLine::Thinking),
            TranscriptLine::Skipped(SkippedLine::ToolUse),
            TranscriptLine::Spoke(
                crate::domain::conversation::Reply::new(Speaker::Model, "Смотрю.", None)
                    .expect("реплика")
            ),
        ]
    );
}

#[test]
fn a_message_the_harness_injected_is_nobodys_speech() {
    let met = spoken(
        r#"{"type":"user","isMeta":true,"message":{"role":"user","content":[
              {"type":"text","text":"Base directory for this skill: /skills/x\n# Навык"}]}}"#,
    );
    assert_eq!(met, vec![TranscriptLine::Skipped(SkippedLine::Injected)]);
}

#[test]
fn a_line_with_no_message_is_reported_and_not_skipped() {
    assert_eq!(
        spoken(r#"{"type":"user","uuid":"u"}"#),
        vec![TranscriptLine::Unreadable(UnreadableLine::MessageMissing)]
    );
}

#[test]
fn a_line_that_is_not_json_is_reported_by_that_reason() {
    assert_eq!(
        meanings("{\"type\":\"user\", это не json"),
        vec![TranscriptLine::Unreadable(UnreadableLine::NotJson)]
    );
    assert_eq!(
        meanings("[1, 2, 3]"),
        vec![TranscriptLine::Unreadable(UnreadableLine::NotAnObject)]
    );
    assert_eq!(
        meanings(r#"{"sessionId":"s"}"#),
        vec![TranscriptLine::Unreadable(UnreadableLine::Untyped)]
    );
    assert_eq!(
        spoken(r#"{"type":"assistant","message":{"role":"assistant","content":7}}"#),
        vec![TranscriptLine::Unreadable(UnreadableLine::ContentUnknown)]
    );
}

#[test]
fn an_empty_line_means_nothing_and_is_not_an_error() {
    assert!(meanings("   ").is_empty());
}

#[test]
fn a_name_line_carries_who_named_it() {
    let met = spoken(r#"{"type":"ai-title","sessionId":"s","aiTitle":"Беседа Б"}"#);
    let [TranscriptLine::Named(name)] = met.as_slice() else {
        panic!("ожидалось имя, вышло {met:?}");
    };
    assert_eq!(name.value(), "Беседа Б");
    assert_eq!(name.named_by(), NamedBy::Model);

    let met = spoken(r#"{"type":"custom-title","sessionId":"s","customTitle":"Беседа Е"}"#);
    let [TranscriptLine::Named(name)] = met.as_slice() else {
        panic!("ожидалось имя, вышло {met:?}");
    };
    assert_eq!(name.named_by(), NamedBy::Owner);
}

#[test]
fn a_name_line_with_nothing_in_it_does_not_name_the_conversation() {
    assert_eq!(
        spoken(r#"{"type":"ai-title","sessionId":"s","aiTitle":"   "}"#),
        vec![TranscriptLine::Skipped(SkippedLine::Bookkeeping)]
    );
    assert_eq!(
        spoken(r#"{"type":"ai-title","sessionId":"s"}"#),
        vec![TranscriptLine::Skipped(SkippedLine::Bookkeeping)]
    );
}

#[test]
fn the_session_is_reported_by_every_line_that_carries_one() {
    let met =
        meanings(r#"{"type":"queue-operation","sessionId":"9d21c7cc","operation":"enqueue"}"#);
    assert_eq!(
        met,
        vec![
            TranscriptLine::Session("9d21c7cc".to_string()),
            TranscriptLine::Skipped(SkippedLine::Bookkeeping),
        ]
    );
}

/// A whole small transcript, read from disk the way the corpus is read: several names in a
/// row, a reply, and a line that is not JSON at all.
#[test]
fn a_transcript_is_read_line_by_line_and_its_lines_are_counted() {
    let directory = std::env::temp_dir().join(format!(
        "crosskit-transcript-{}-{}",
        std::process::id(),
        line!()
    ));
    std::fs::create_dir_all(&directory).expect("каталог для фикстуры");
    let path = directory.join("session.jsonl");
    let mut file = std::fs::File::create(&path).expect("файл фикстуры");
    for line in [
        r#"{"type":"ai-title","sessionId":"s","aiTitle":"Первое"}"#,
        r#"{"type":"ai-title","sessionId":"s","aiTitle":"Второе"}"#,
        r#"{"type":"user","sessionId":"s","message":{"role":"user","content":[{"type":"text","text":"привет"}]}}"#,
        "",
        "не json вовсе",
    ] {
        writeln!(file, "{line}").expect("запись фикстуры");
    }
    drop(file);

    let mut met = Vec::new();
    let lines = read_transcript(&path, |line| met.push(line)).expect("чтение транскрипта");
    std::fs::remove_dir_all(&directory).expect("уборка фикстуры");

    assert_eq!(lines, 5, "пустая строка — это тоже строка файла");
    let names = met
        .iter()
        .filter(|line| matches!(line, TranscriptLine::Named(_)))
        .count();
    assert_eq!(names, 2);
    assert_eq!(
        met.iter()
            .filter(|line| matches!(line, TranscriptLine::Unreadable(UnreadableLine::NotJson)))
            .count(),
        1
    );
    assert_eq!(
        met.iter()
            .filter(|line| matches!(line, TranscriptLine::Spoke(_)))
            .count(),
        1
    );
}

/// The mark has to be made of what the owner said, and `isMeta` marks what the harness put
/// under their role: a skill's own text naming files is not a person asking about them.
///
/// It is checked here and not in the assembly, because the assembly never meets an injected
/// message — the reader turns it into a skip first, and this test is what makes that the
/// reason the mark comes out `not_named` rather than a coincidence.
#[test]
fn a_file_named_only_by_an_injected_message_is_not_a_file_the_owner_named() {
    let lines = [
        r#"{"type":"user","sessionId":"s","isMeta":true,"message":{"role":"user","content":[{"type":"text","text":"читай draft_service.py по правилам навыка"}]}}"#,
        r#"{"type":"user","sessionId":"s","message":{"role":"user","content":[{"type":"text","text":"разберись с расчётом"}]}}"#,
        r#"{"type":"assistant","sessionId":"s","message":{"role":"assistant","content":[{"type":"tool_use","name":"Read","input":{"file_path":"/work/draft_service.py"}}]}}"#,
    ];
    let mut assembly = ConversationAssembly::for_session("файл", u64::MAX);
    for text in lines {
        let _stop = read_line(text, |line| assembly.absorb(line));
    }
    let read = assembly.finish();
    let touches = read.conversation().touches();
    assert_eq!(touches.len(), 1, "{touches:?}");
    assert_eq!(
        touches[0].naming(),
        OwnerNaming::NotNamed,
        "имя файла прозвучало только в подставленном тексте"
    );
}

/// The top rung of section 5's scale: the tool was handed a path and read it off the disk.
#[test]
fn a_tool_call_with_a_file_path_is_a_touch_with_a_verb() {
    for (tool, verb) in [
        ("Read", TouchVerb::Read),
        ("Write", TouchVerb::Wrote),
        ("Edit", TouchVerb::Edited),
        ("NotebookEdit", TouchVerb::Edited),
    ] {
        let met = spoken(&format!(
            r#"{{"type":"assistant","timestamp":"2026-09-03T07:09:25.457Z",
                "message":{{"role":"assistant","content":[
                  {{"type":"tool_use","name":"{tool}","input":{{"file_path":"/a/b.rs"}}}}]}}}}"#
        ));
        let touch = met
            .iter()
            .find_map(|line| match line {
                TranscriptLine::Touched(touch) => Some(touch),
                _ => None,
            })
            .unwrap_or_else(|| panic!("инструмент {tool} должен дать ссылку, вышло {met:?}"));
        assert_eq!(touch.path(), "/a/b.rs");
        assert_eq!(touch.verb(), verb);
        assert_eq!(touch.source(), TouchSource::ToolInput);
        assert_eq!(touch.at_unix_ms(), Some(1_788_419_365_457));
    }
}

/// The set of tool names moves between releases of Claude Code. A name this build has no verb
/// for produces no touch and is counted, which is how anybody finds out that it moved.
#[test]
fn a_tool_this_build_does_not_know_is_counted_and_gives_no_touch() {
    let met = spoken(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
              {"type":"tool_use","name":"str_replace_editor","input":{"file_path":"/a/b.rs"}}]}}"#,
    );
    assert!(
        !met.iter()
            .any(|line| matches!(line, TranscriptLine::Touched(_))),
        "незнакомый инструмент не должен давать ссылку: {met:?}"
    );
    assert!(met.contains(&TranscriptLine::Skipped(SkippedLine::UnknownTool)));
}

/// A tool call with no file path at all — most of them — is just a tool call.
#[test]
fn a_tool_call_without_a_path_is_neither_a_touch_nor_an_unknown_tool() {
    let met = spoken(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
              {"type":"tool_use","name":"Bash","input":{"command":"ls /a"}}]}}"#,
    );
    assert_eq!(
        met,
        vec![
            TranscriptLine::Skipped(SkippedLine::ToolUse),
            TranscriptLine::Skipped(SkippedLine::NoTextLeft),
        ]
    );
}

#[test]
fn a_file_history_snapshot_names_the_files_it_backed_up() {
    let met = spoken(
        r#"{"type":"file-history-snapshot","messageId":"m",
            "snapshot":{"messageId":"m","timestamp":"2026-09-03T07:09:25.457Z",
            "trackedFileBackups":{"/a/one.rs":{"x":1},"/a/two.rs":{"x":2}}}}"#,
    );
    let touched: Vec<_> = met
        .iter()
        .filter_map(|line| match line {
            TranscriptLine::Touched(touch) => Some(touch),
            _ => None,
        })
        .collect();
    assert_eq!(touched.len(), 2, "две записи — две ссылки, вышло {met:?}");
    for touch in touched {
        assert_eq!(touch.verb(), TouchVerb::Edited);
        assert_eq!(touch.source(), TouchSource::FileHistory);
        assert_eq!(touch.at_unix_ms(), Some(1_788_419_365_457));
    }
}

/// The common case: the tool writes a snapshot line before it has anything to back up.
#[test]
fn an_empty_file_history_snapshot_names_nothing() {
    let met = spoken(
        r#"{"type":"file-history-snapshot","messageId":"m",
            "snapshot":{"messageId":"m","trackedFileBackups":{}}}"#,
    );
    assert_eq!(met, vec![TranscriptLine::Skipped(SkippedLine::Bookkeeping)]);
}

#[test]
fn a_transcript_that_is_not_there_is_an_error_and_not_an_empty_conversation() {
    let missing = std::env::temp_dir().join("crosskit-no-such-transcript.jsonl");
    assert!(read_transcript(&missing, |_| ()).is_err());
}
