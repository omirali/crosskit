// What `setup` says before its questions and after its indexing: what it found and left out,
// and, at the end, what each context holds and what stays outside all of them and why.
//
// Both are counts of the answers and of what was found, not of the index: the index says what
// it holds through `crosskit status`, and the two are asked separately on purpose.
use std::collections::BTreeMap;

use crate::domain::{ContextLayout, GroupMembers, LeftOutKind, MachineGroups, NormalizedPath};

use super::context_wording::counted;
use super::setup_wording::{contents, left_out_heading, left_out_reason, shown};

/// The inventory, the things left out one by one, and the line about Codex.
pub(super) fn found(
    groups: &MachineGroups,
    root: &NormalizedPath,
    home: Option<&NormalizedPath>,
    codex_sessions: usize,
) -> String {
    let mut text = String::new();
    let projects: usize = groups
        .groups()
        .iter()
        .map(|group| group.members().projects().len())
        .sum();
    let folders: usize = groups
        .groups()
        .iter()
        .map(|group| group.members().documents().len())
        .sum::<usize>()
        + groups.remainder().documents().len();
    let every: Vec<&GroupMembers> = groups
        .groups()
        .iter()
        .map(|group| group.members())
        .chain([groups.remainder()])
        .collect();
    let transcript_folders: usize = every
        .iter()
        .map(|members| members.transcripts().len())
        .sum();
    let conversations: usize = every.iter().map(|members| members.conversations()).sum();

    text.push_str(&format!(
        "Found under {}: {} in {}, {}, {} with {}.\n",
        shown(root, home),
        counted(projects as u64, "project"),
        counted(groups.groups().len() as u64, "group"),
        format_args!("{} without git", counted(folders as u64, "folder")),
        counted(transcript_folders as u64, "transcript folder"),
        counted(conversations as u64, "conversation"),
    ));
    if !groups.left_out().is_empty() {
        text.push_str("Left out of every group, and why:\n");
        for left in groups.left_out() {
            let conversations = match left.kind() {
                LeftOutKind::TranscriptFolder => {
                    format!(
                        " ({})",
                        counted(left.conversations() as u64, "conversation")
                    )
                }
                LeftOutKind::Project | LeftOutKind::DocumentFolder => String::new(),
            };
            text.push_str(&format!(
                "  {}{conversations}: {}\n",
                shown(left.path(), home),
                left_out_reason(left.reason(), home)
            ));
        }
    }
    text.push_str(&codex_line(codex_sessions));
    text
}

fn codex_line(sessions: usize) -> String {
    format!(
        "Codex: {} found; they are outside every context, because Crosskit has no reader for \
         them.\n",
        counted(sessions as u64, "session")
    )
}

/// One line per context and one per reason something stays outside.
pub(super) fn summary(
    layout: &ContextLayout,
    groups: &MachineGroups,
    codex_sessions: usize,
) -> String {
    let mut text = String::from("\nWhat each context holds, by your answers:\n");
    for context in layout.contexts() {
        text.push_str(&format!(
            "  {}: {}\n",
            context.name(),
            contents(
                context.projects(),
                context.document_folders(),
                0,
                context.conversations()
            )
        ));
    }
    for name in layout.empty() {
        text.push_str(&format!(
            "  {name}: nothing was sent to it, so it was not written\n"
        ));
    }

    text.push_str("Outside every context:\n");
    for (heading, members) in [
        ("skipped by your answer", layout.skipped()),
        ("left without an answer", layout.unanswered()),
    ] {
        if !members.is_empty() {
            text.push_str(&format!("  {heading}: {}\n", of_members(members)));
        }
    }
    let mut by_heading: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for left in groups.left_out() {
        let entry = by_heading
            .entry(left_out_heading(left.reason()))
            .or_default();
        entry.0 += 1;
        entry.1 += left.conversations();
    }
    for (heading, (things, conversations)) in by_heading {
        text.push_str(&format!(
            "  {heading}: {}, {}\n",
            counted(things as u64, "place"),
            counted(conversations as u64, "conversation")
        ));
    }
    text.push_str(&format!("  {}", codex_line(codex_sessions)));
    text
}

fn of_members(members: &GroupMembers) -> String {
    contents(
        members.projects().len(),
        members.documents().len(),
        members.transcripts().len(),
        members.conversations(),
    )
}

/// The one line about egress, said when the file is written: every new context keeps its
/// content, and turning egress on is what makes its conversations sendable.
pub(super) fn egress_line(layout: &ContextLayout) -> String {
    let per_context: Vec<String> = layout
        .contexts()
        .iter()
        .map(|context| {
            format!(
                "{} {}",
                context.name(),
                counted(context.conversations() as u64, "conversation")
            )
        })
        .collect();
    format!(
        "Content egress is off in every context written. Turning it on for a context makes its \
         conversations sendable to the model — excerpts in `ask` and analysis jobs: {}.",
        per_context.join(", ")
    )
}
