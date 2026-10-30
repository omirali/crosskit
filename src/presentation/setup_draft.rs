// The configuration `setup` writes, rendered from the person's answers.
//
// It is printed whole, asked about, and then written as the very string that was printed, the
// way the draft of `contexts propose` always was: a command that showed one thing and wrote
// another would be asking for consent to something nobody saw. Every `include` carries, as a
// comment, the answer that put it there.
use std::fmt::Write;

use crate::domain::{
    ContextLayout, LaidOutContext, LaidOutRoot, MachineGroups, NormalizedPath, RootKind,
    RootSource, TreeEvidence,
};

use super::context_draft::{toml_string, workspace_header, write_walk};
use super::context_wording::{counted, describe};
use super::setup_wording::{ORGANIZATION_MARK, group_reason};

/// What lies inside a session's directory beside its transcript and is not the conversation:
/// the full transcripts of its subagents and the tool output it saved to files. Named by their
/// place in a transcript folder, never by the bare name — a directory called `tool-results` in
/// a project is somebody's source code.
const NOT_THE_CONVERSATION: [&str; 2] = ["subagents", "tool-results"];
/// A session's own directory: named by the session's id, a UUID. Measured on the owner's
/// machine on 2026-10-01, it holds `subagents/`, `tool-results/`,
/// `auto-mode-classifier-error.txt` and `custom-title.json` — and nothing of the conversation,
/// whose transcript lies beside it as `<id>.jsonl`.
const SESSION_DIRECTORY: &str = "????????-????-????-????-????????????";

/// The whole file for `layout`.
pub(super) fn render(
    layout: &ContextLayout,
    groups: &MachineGroups,
    evidence: &TreeEvidence,
    transcripts_root: Option<&NormalizedPath>,
) -> String {
    let mut draft = String::new();
    let _ = writeln!(
        draft,
        "# Contexts for {}, from your answers",
        toml_string(evidence.root.as_str())
    );
    let _ = writeln!(draft, "#");
    let _ = writeln!(
        draft,
        "# This is the whole of the file `crosskit setup` is about to write, and nothing\n\
         # has been written yet. Every directory below is here because of one answer of\n\
         # yours, named beside it; nothing was placed without one. No context lets its\n\
         # content leave this machine: none of them says `send_content_outside`."
    );
    let _ = writeln!(draft, "#");
    let _ = writeln!(draft, "# Walked: {}.", describe(evidence.whole));

    let holds_transcripts = layout.contexts().iter().any(|context| {
        context
            .roots()
            .iter()
            .any(|root| matches!(root.kind(), RootKind::Transcripts { .. }))
    });
    if let (true, Some(root)) = (holds_transcripts, transcripts_root) {
        write_transcript_exclusions(&mut draft, root);
    }
    for context in layout.contexts() {
        draft.push('\n');
        write_context(&mut draft, context, groups);
    }
    write_walk(&mut draft, &evidence.walk);
    draft
}

fn write_transcript_exclusions(draft: &mut String, root: &NormalizedPath) {
    let _ = writeln!(draft);
    let _ = writeln!(draft, "[index]");
    let _ = writeln!(
        draft,
        "# A transcript folder holds its conversations, `<session>.jsonl`, and the notes the\n\
         # tool keeps about the project in `memory/`; both are indexed. Each session also has\n\
         # a directory of its own, named by its id: the transcripts of subagents, tool output\n\
         # saved to files, the tool's own dumps. None of that is the conversation, and the\n\
         # last line leaves the whole directory out."
    );
    let _ = writeln!(draft, "exclude = [");
    for pattern in transcript_exclusions(root) {
        let _ = writeln!(draft, "  {},", toml_string(&pattern));
    }
    let _ = writeln!(draft, "]");
}

/// The exclusions written under the transcript folders of `root`. The session directory is
/// matched by the shape of its name — a UUID, 8-4-4-4-12 — and not by "any directory": the
/// folder also holds `memory/`, which is the project's and stays, and the conversation's
/// own file is `<id>.jsonl`, one segment and longer than the id.
pub(super) fn transcript_exclusions(root: &NormalizedPath) -> Vec<String> {
    NOT_THE_CONVERSATION
        .iter()
        .map(|name| format!("{}/*/*/{name}/**", root.as_str()))
        .chain([format!("{}/*/{SESSION_DIRECTORY}/**", root.as_str())])
        .collect()
}

fn write_context(draft: &mut String, context: &LaidOutContext, groups: &MachineGroups) {
    let _ = writeln!(draft, "{}", workspace_header(context.name()));
    let _ = writeln!(
        draft,
        "# {}, {}, {}",
        counted(context.projects() as u64, "project"),
        format_args!(
            "{} without git",
            counted(context.document_folders() as u64, "folder")
        ),
        counted(context.conversations() as u64, "conversation")
    );
    let _ = writeln!(draft, "include = [");
    for root in context.roots() {
        let _ = writeln!(
            draft,
            "  {},  # {}",
            toml_string(root.path().as_str()),
            why_here(root, groups)
        );
    }
    let _ = writeln!(draft, "]");
}

/// The answer a root is here by, and what the root is.
fn why_here(root: &LaidOutRoot, groups: &MachineGroups) -> String {
    let answer = match root.source() {
        RootSource::Group(index) => format!("group {}", index + 1),
        RootSource::UnansweredGroup(index) => {
            format!(
                "group {}, left without an answer, with the remainder",
                index + 1
            )
        }
        RootSource::Remainder => "remainder".to_string(),
        RootSource::RemainderFolder => "remainder, this folder by your answer".to_string(),
    };
    let what = match root.kind() {
        RootKind::Project { nested } => {
            let reason = match root.source() {
                RootSource::Group(index) | RootSource::UnansweredGroup(index) => groups
                    .groups()
                    .get(index)
                    .map(|group| {
                        let mark = match group.looks_like_your_organization() {
                            true => format!(" ({ORGANIZATION_MARK})"),
                            false => String::new(),
                        };
                        format!("{}{mark}", group_reason(group.reason()))
                    })
                    .unwrap_or_default(),
                RootSource::Remainder | RootSource::RemainderFolder => String::new(),
            };
            match nested {
                0 => format!("project; {reason}"),
                count => format!(
                    "project with {} inside it; {reason}",
                    counted(*count as u64, "repository")
                ),
            }
        }
        RootKind::Transcripts {
            directory,
            conversations,
        } => format!(
            "{} started in {}",
            counted(*conversations as u64, "conversation"),
            toml_string(directory.as_str())
        ),
        RootKind::Document { files } => format!("folder without git; {}", counted(*files, "file")),
    };
    format!("{answer}: {what}")
}

#[cfg(test)]
mod tests {
    use super::transcript_exclusions;
    use crate::domain::{NormalizedPath, PathPattern};

    const ROOT: &str = "/Users/alice/.claude/projects";
    const SESSION: &str = "0f3a9c21-7b5b-4a47-9e10-5d2c8b6a1f00";

    fn excluded(relative: &str) -> bool {
        let root = NormalizedPath::from_absolute(ROOT).unwrap();
        let path = NormalizedPath::from_absolute(&format!("{ROOT}/{relative}")).unwrap();
        transcript_exclusions(&root)
            .iter()
            .any(|pattern| PathPattern::parse(pattern).unwrap().matches(&path))
    }

    /// The conversation and the project's notes stay; everything inside a session's own
    /// directory goes, whatever it is called.
    #[test]
    fn the_exclusions_take_a_sessions_directory_and_leave_its_conversation_and_the_notes() {
        let folder = "-Users-alice-Projects-payments-api";
        assert!(!excluded(&format!("{folder}/{SESSION}.jsonl")));
        assert!(!excluded(&format!("{folder}/memory/MEMORY.md")));
        assert!(!excluded(&format!("{folder}/memory/deploy-notes.md")));
        for inside in [
            "auto-mode-classifier-error.txt",
            "custom-title.json",
            "subagents/agent-1.jsonl",
            "tool-results/output.txt",
            "anything-new.bin",
        ] {
            assert!(
                excluded(&format!("{folder}/{SESSION}/{inside}")),
                "{inside} inside the session directory must be left out"
            );
        }
    }
}
