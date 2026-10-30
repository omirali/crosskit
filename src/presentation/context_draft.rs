// A draft of contexts as configuration text, for `contexts propose` to print and the operator
// to paste.
//
// Everything here is valid TOML, the sentences included — they are comments, so a draft is
// a file whether it is pasted whole or in part. `setup` renders its own file from the person's
// answers (`setup_draft`) and shares two pieces with this one: how a path is written as a TOML
// string, and what the walk met on the way.
use std::fmt::Write;

use crate::domain::{
    ContextProposal, ProposedContext, UnattributedSubtree, WalkOutcome, WorkspaceName,
};

use super::context_wording::{
    basis, counted, describe, evidence_of, not_attributed, remaining, repository_evidence,
};
use super::format_number;

/// How many unreadable paths are named before the rest are only counted. A tree with
/// thousands of them is a permissions problem, not a list worth printing.
const NAMED_FAILURES: usize = 10;

/// Said above every draft: what it is and what to do with it.
const PREAMBLE: &str = "# Nothing was written. Paste the sections you agree with into the policy file; a\n\
     # directory given to `crosskit index init` is included for that run, so these\n\
     # sections are enough to start. The names are suggestions.";

/// The draft for `proposal`, or nothing when the tree holds no files to draft for.
pub(super) fn render(proposal: &ContextProposal) -> Option<String> {
    if proposal.is_empty() {
        return None;
    }
    let mut draft = String::new();
    write_header(&mut draft, proposal);
    for context in proposal.contexts() {
        draft.push('\n');
        write_context(&mut draft, context);
    }
    write_unattributed(&mut draft, proposal.unattributed());
    write_walk(&mut draft, proposal.walk());
    Some(draft)
}

/// What the walk met, for a tree that gave nothing to draft. Printed on its own, because a
/// directory that holds no files and a directory the walk could not enter look the same
/// from the outside.
pub(super) fn render_empty(proposal: &ContextProposal) -> String {
    let mut printed = format!("Nothing to propose: {} holds no files.\n", proposal.root());
    write_walk(&mut printed, proposal.walk());
    printed
}

fn write_header(draft: &mut String, proposal: &ContextProposal) {
    let _ = writeln!(draft, "# Draft contexts for {}", proposal.root());
    let _ = writeln!(draft, "#");
    let _ = writeln!(draft, "{PREAMBLE}");
    let _ = writeln!(draft, "#");
    let _ = writeln!(draft, "# Walked: {}.", describe(proposal.whole()));
    let _ = writeln!(
        draft,
        "# Every count here is of what lies on disk. The index policy reads less than that:\n\
         # build output, dependency caches and `.git` are excluded by default, so a context\n\
         # over a directory of source code indexes a fraction of the bytes named below."
    );
    if let Some(repository) = proposal.root_repository() {
        let _ = writeln!(
            draft,
            "# The walked root is itself a repository ({}). One context over the whole root is\n\
             # the decision this command prepares rather than makes, so nothing is proposed for it.",
            repository_evidence(repository)
        );
    }
}

fn write_context(draft: &mut String, context: &ProposedContext) {
    let _ = writeln!(draft, "{}", workspace_header(context.name()));
    let _ = writeln!(
        draft,
        "# {}",
        basis(context.basis(), context.includes().len())
    );
    let _ = writeln!(draft, "include = [");
    for include in context.includes() {
        let _ = writeln!(
            draft,
            "  {},  # {}; {}",
            toml_string(include.path().as_str()),
            evidence_of(include),
            describe(include.volume())
        );
    }
    let _ = writeln!(draft, "]");
}

fn write_unattributed(draft: &mut String, subtrees: &[UnattributedSubtree]) {
    if subtrees.is_empty() {
        return;
    }
    let _ = writeln!(draft);
    let _ = writeln!(
        draft,
        "# Not attributed to any context. These are yours to decide:"
    );
    for subtree in subtrees {
        let _ = writeln!(draft, "#");
        let _ = writeln!(draft, "#   {}", toml_string(subtree.path().as_str()));
        let _ = writeln!(draft, "#     {}", remaining(subtree));
        let _ = writeln!(draft, "#     {}", not_attributed(subtree.reason()));
    }
}

/// What the walk could not do. Written even when everything was proposed, because a
/// proposal made over a directory the walk could not enter is a proposal about less than
/// the operator thinks.
pub(super) fn write_walk(draft: &mut String, walk: &WalkOutcome) {
    let mut lines = Vec::new();
    if walk.symlinks > 0 {
        lines.push(format!(
            "{} symbolic links, none of them followed",
            format_number(walk.symlinks)
        ));
    }
    if walk.generated_directories > 0 {
        lines.push(format!(
            "{} directories a tool made, each skipped whole on its own marker",
            format_number(walk.generated_directories)
        ));
    }
    if walk.special_entries > 0 {
        lines.push(format!(
            "{} entries that are neither files nor directories",
            format_number(walk.special_entries)
        ));
    }
    if walk.unrepresentable_names > 0 {
        lines.push(format!(
            "{} entries whose names are not valid UTF-8, skipped before any system call",
            format_number(walk.unrepresentable_names)
        ));
    }
    if walk.unreadable_git_configs > 0 {
        lines.push(format!(
            "{} repositories whose `.git/config` could not be read as text",
            format_number(walk.unreadable_git_configs)
        ));
    }
    if walk.closed_by_the_system > 0 {
        lines.push(format!(
            "{} the system keeps closed to this terminal (on macOS: Full Disk Access is not \
             granted); none of them is needed for a context",
            counted(walk.closed_by_the_system, "folder")
        ));
    }
    if !walk.failures.is_empty() {
        lines.push(format!(
            "{} paths the walk could not inspect",
            format_number(walk.failures.len() as u64)
        ));
    }
    if lines.is_empty() {
        return;
    }

    let _ = writeln!(draft);
    let _ = writeln!(draft, "# What the walk met on the way:");
    for line in lines {
        let _ = writeln!(draft, "#   {line}");
    }
    for failure in walk.failures.iter().take(NAMED_FAILURES) {
        let _ = writeln!(draft, "#     {failure}");
    }
    if walk.failures.len() > NAMED_FAILURES {
        let _ = writeln!(
            draft,
            "#     and {} more",
            format_number((walk.failures.len() - NAMED_FAILURES) as u64)
        );
    }
}

/// The header of a context's section. A context may be called by letters of any script and
/// may hold a dot, and TOML reads neither as a bare key — `[workspace.работа]` does not parse
/// and `[workspace.a.b]` is a table inside a table — so such a name is written quoted.
pub(super) fn workspace_header(name: &WorkspaceName) -> String {
    let bare = name
        .as_str()
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    match bare {
        true => format!("[workspace.{name}]"),
        false => format!("[workspace.{}]", toml_string(name.as_str())),
    }
}

/// A path as a TOML basic string. Paths are untrusted input: a quote, a backslash or a
/// newline in one would otherwise end the string or the comment it sits in and turn the
/// rest of the line into something the reader never wrote.
pub(super) fn toml_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => {
                escaped.push_str(&format!("\\u{:04X}", control as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(test)]
mod tests {
    use super::{PREAMBLE, toml_string, workspace_header, write_walk};
    use crate::domain::{WalkOutcome, WorkspaceName};

    /// The places the system keeps closed are one line however many there are; a failure
    /// that is not that is still named.
    #[test]
    fn closed_places_are_one_line_and_other_failures_keep_their_names() {
        let walk = WalkOutcome {
            closed_by_the_system: 122,
            failures: vec!["/Users/alice/odd: source scan failed: Permission denied".to_string()],
            ..WalkOutcome::default()
        };
        let mut draft = String::new();

        write_walk(&mut draft, &walk);

        assert_eq!(draft.matches("keeps closed").count(), 1, "{draft}");
        assert!(
            draft.contains("122 folders the system keeps closed"),
            "{draft}"
        );
        assert!(
            draft.contains("#     /Users/alice/odd: source scan failed"),
            "{draft}"
        );
    }

    #[test]
    fn a_path_that_could_end_the_line_it_is_printed_on_is_escaped() {
        assert_eq!(toml_string("/home/alice/notes"), "\"/home/alice/notes\"");
        assert_eq!(
            toml_string("/home/alice/say \"hi\""),
            "\"/home/alice/say \\\"hi\\\"\""
        );
        assert_eq!(toml_string("/home/a\nb"), "\"/home/a\\nb\"");
        assert_eq!(toml_string("/home/a\u{7}b"), "\"/home/a\\u0007b\"");
    }

    /// What a name may be is wider than what TOML reads bare, and the header must parse back to
    /// the same name — the whole point of the file is that it loads.
    #[test]
    fn a_name_toml_cannot_read_bare_is_quoted_and_reads_back_as_itself() {
        for (name, header) in [
            ("client-a", "[workspace.client-a]"),
            ("работа", "[workspace.\"работа\"]"),
            ("notes.2026", "[workspace.\"notes.2026\"]"),
        ] {
            let name = WorkspaceName::parse(name).unwrap();
            assert_eq!(workspace_header(&name), header);
            let parsed: toml::Table = toml::from_str(&format!("{header}\ninclude = []\n")).unwrap();
            assert!(
                parsed["workspace"]
                    .as_table()
                    .unwrap()
                    .contains_key(name.as_str()),
                "{header} reads back as another key: {parsed:?}"
            );
        }
    }

    /// The preamble is comments, so it cannot change what the draft declares.
    #[test]
    fn the_preamble_is_a_comment() {
        for line in PREAMBLE.lines() {
            assert!(
                line.trim_start().starts_with('#'),
                "{line:?} would be read as configuration"
            );
        }
    }
}
