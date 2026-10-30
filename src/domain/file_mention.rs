// How a file gets referred to: by the owner inside a conversation, and by whoever asks about
// one at the command line. Both are the same question — does this text name this file — and
// they are one module so the two answers cannot drift apart.
//
// An agent opens files the person never asked for: it greps, it follows an import, it reads
// a neighbour to check a convention. Those touches are facts and the graph keeps them, but
// they are not what the person means by "we worked on this file". What separates the two is
// not the number of reads — Stage 37 measured that 219 of 236 read-only pairs were read
// exactly once, so a single read is the normal case, not the suspicious one. What separates
// them is whether the person said the name.
//
// The rule is a type here rather than a condition in a query, because the choice between its
// two forms was made by measurement and the losing form has to stay readable beside the
// winner.
use std::fmt;

/// What is known about the owner naming a file, for one file in one conversation.
///
/// Three states and not a boolean, for the same reason `TIME_NOT_RECORDED` is a sentinel and
/// not a zero: a row written before this build could tell has not been found to be unnamed —
/// nobody looked at it. A reader that cannot tell those apart quietly reports the whole of an
/// older graph as touched in passing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OwnerNaming {
    /// The owner named this file in a reply of their own.
    Named,
    /// The owner did not name it, and this build looked.
    NotNamed,
    /// Nobody looked: the row was written before the mark existed. It is computed when the
    /// transcript is published as a new revision — a pass over an unchanged one writes
    /// nothing, so a conversation that has ended keeps this until the context is reindexed.
    NotComputed,
}

impl OwnerNaming {
    pub fn of_naming(named: bool) -> Self {
        match named {
            true => Self::Named,
            false => Self::NotNamed,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Named => "named",
            Self::NotNamed => "not_named",
            Self::NotComputed => "not_computed",
        }
    }

    /// The read side of the closed set. A value this build does not know is read as
    /// `NotComputed`: a mark whose word changed is a mark nobody computed with these rules.
    pub fn parse(value: &str) -> Self {
        match value {
            "named" => Self::Named,
            "not_named" => Self::NotNamed,
            _ => Self::NotComputed,
        }
    }

    #[cfg(test)]
    pub const ALL: [Self; 3] = [Self::Named, Self::NotNamed, Self::NotComputed];
}

impl fmt::Display for OwnerNaming {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a mention is looked for. Both forms were measured on the owner's corpus; the one the
/// product uses is `MentionRule::PRODUCT` and the other stays because a rule that was chosen
/// against a measured alternative reads differently from a rule somebody picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionRule {
    /// The path, as the transcript recorded it, occurs in a reply. Measured against the
    /// other form on the owner's corpus and not chosen; it stays because a rule that beat a
    /// measured alternative reads differently from a rule somebody picked, and because the
    /// measurement that chose it has to stay runnable.
    #[allow(
        dead_code,
        reason = "the candidate this build did not choose; see ADR 0041 for both numbers"
    )]
    WholePath,
    /// The last segment of the path occurs in a reply, on its own boundaries.
    ///
    /// Measured and not chosen either: it finds everything `WholePath` finds and more, and
    /// most of that more is not a person naming a file. See `SpokenName`.
    #[allow(
        dead_code,
        reason = "the candidate this build did not choose; see ADR 0041 for all three numbers"
    )]
    FileName,
    /// The last segment occurs in what the person actually typed: the same rule as
    /// `FileName`, with the blocks a tool wrapped around its own text left out.
    ///
    /// A transcript records, inside one message under the owner's role, both what they typed
    /// and what the harness put there: a background task reporting itself, the editor saying
    /// which file is open. Stage 37 measured 482 such blocks in the owner's corpus, and the
    /// names inside them are the whole of what `FileName` finds beyond `WholePath` among
    /// files nobody wrote to. A quoted name is not a request.
    SpokenName,
}

impl MentionRule {
    /// The rule this build writes marks with. See ADR 0041 for what all three gave.
    pub const PRODUCT: Self = Self::SpokenName;

    /// Whether `said` names `path` under this rule.
    pub fn names(self, path: &str, said: &str) -> bool {
        match self {
            Self::WholePath => said.contains(path),
            Self::FileName => match file_name_of(path) {
                Some(name) => mentions_name(said, name),
                None => false,
            },
            Self::SpokenName => match file_name_of(path) {
                Some(name) => outside_quoted_blocks(said, |spoken| mentions_name(spoken, name)),
                None => false,
            },
        }
    }
}

/// Runs `spoken` over the parts of a text that nobody wrapped in a tag, and stops at the
/// first one that answers yes.
///
/// A tagged block is quoted material: the tool put it under a person's role, so a file name
/// inside it was reported to them rather than asked for by them. The rule is the shape of the
/// markup and not the name of any one tag — a tool that invents a new envelope tomorrow is
/// still quoting, and a rule listing today's tags would silently stop working.
///
/// Backticks are deliberately not quoting: `посмотри `draft_service.py`` is how a person
/// names a file, and dropping it would throw away the plainest mention there is.
///
/// Only a block that is **closed** is one. A tool closes what it opens; a person types
/// `<KEY>` or `<IntakeFormState>` in the middle of a sentence and closes nothing. Treating an
/// unclosed tag as an envelope was measured first and threw away the rest of the message it
/// appeared in — four files the owner did name, in the corpus of one machine.
fn outside_quoted_blocks(said: &str, mut spoken: impl FnMut(&str) -> bool) -> bool {
    let mut plain_from = 0;
    let mut at = 0;
    while let Some(found) = said[at..].find('<') {
        let open = at + found;
        let Some((tag, after_open)) = opening_tag_at(said, open) else {
            at = open + 1;
            continue;
        };
        let closing = format!("</{tag}>");
        let Some(offset) = said[after_open..].find(&closing) else {
            at = after_open;
            continue;
        };
        if spoken(&said[plain_from..open]) {
            return true;
        }
        let end = after_open + offset + closing.len();
        plain_from = end;
        at = end;
    }
    spoken(&said[plain_from..])
}

/// How many bytes of a reply the person actually typed: everything outside the blocks a tool
/// wrapped around its own text.
///
/// Only the measurement asks this — the rule itself asks a yes-or-no question — and it lives
/// here rather than in the measurement so that both answers come from one scanner. Stage 37
/// measured with it that a third of what every document of this project calls the owner's
/// words was said by the harness.
///
/// Named for what it counts and not `spoken_bytes`, although that is what it means: two other
/// things in this program already carry that name, both for the bytes of everything said, and
/// this stage cost two corrections precisely because "the owner's words" meant different
/// things in different places. One name, one quantity.
#[cfg(test)]
pub fn bytes_outside_quoted_blocks(said: &str) -> usize {
    let mut bytes = 0;
    outside_quoted_blocks(said, |span| {
        bytes += span.len();
        false
    });
    bytes
}

/// The tag name at `open` and where its `>` ends, when what stands there is an opening tag.
fn opening_tag_at(said: &str, open: usize) -> Option<(&str, usize)> {
    let rest = &said[open + 1..];
    let name_length = rest.find(|character: char| {
        !(character.is_ascii_alphanumeric() || character == '-' || character == '_')
    })?;
    let name = &rest[..name_length];
    if name.is_empty() || !name.starts_with(|character: char| character.is_ascii_alphabetic()) {
        return None;
    }
    let after_name = &rest[name_length..];
    let close = after_name.find('>')?;
    // Attributes are allowed and a self-closing tag is not an envelope: it opens nothing.
    if after_name[..close].ends_with('/') {
        return None;
    }
    Some((name, open + 1 + name_length + close + 1))
}

/// The last segment of a path, or nothing when the path ends in a separator or is empty.
///
/// Separators are both, because a transcript carries the paths of whichever machine wrote it
/// and a Windows path in a corpus read on macOS is still a path with a name at the end.
pub fn file_name_of(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.is_empty() {
        true => None,
        false => Some(name),
    }
}

/// Whether a file name occurs in a text as a name rather than as part of a longer word.
///
/// `draft_service.py` inside `my_draft_service.py` is a different file, and a name matched
/// with `contains` alone reports the longer one as named. The boundary is: what comes before
/// and after the name is not a character a file name is made of.
fn mentions_name(said: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(found) = said[from..].find(name) {
        let start = from + found;
        let end = start + name.len();
        let before_is_a_name_character = said[..start]
            .chars()
            .next_back()
            .is_some_and(is_name_character);
        let after_is_a_name_character = said[end..].chars().next().is_some_and(is_name_character);
        if !before_is_a_name_character && !after_is_a_name_character {
            return true;
        }
        // Past this occurrence, by one character rather than by the name's length: an
        // occurrence can start inside the previous one.
        from = start + said[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Whether a path is the one somebody meant by typing its tail.
///
/// The owner types `draft_service.py`, not the twelve segments above it. A tail matches on a
/// segment boundary and never inside a name: `service.py` does not name `draft_service.py`,
/// because answering about a file somebody did not ask about is worse than answering nothing.
pub fn path_ends_with_segment(path: &str, tail: &str) -> bool {
    if tail.is_empty() {
        return false;
    }
    if path == tail {
        return true;
    }
    let Some(before) = path.len().checked_sub(tail.len()) else {
        return false;
    };
    path.ends_with(tail)
        && path[..before]
            .chars()
            .next_back()
            .is_some_and(|character| character == '/' || character == '\\')
}

/// What a file name is made of, for deciding where one ends. A path separator is deliberately
/// not one: `.../counterparty/draft_service.py` names `draft_service.py`, and a person who
/// typed the directory too named the file no less.
fn is_name_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_closed_set_round_trips_through_what_the_graph_stores() {
        for naming in OwnerNaming::ALL {
            assert_eq!(OwnerNaming::parse(naming.as_str()), naming);
        }
    }

    /// A word this build does not know is not a fourth state: it is a mark nobody computed
    /// the way this build computes them.
    #[test]
    fn a_mark_written_by_other_rules_reads_as_not_computed() {
        assert_eq!(OwnerNaming::parse(""), OwnerNaming::NotComputed);
        assert_eq!(OwnerNaming::parse("maybe"), OwnerNaming::NotComputed);
    }

    #[test]
    fn the_name_is_the_last_segment_and_both_separators_end_one() {
        assert_eq!(
            file_name_of("/a/b/draft_service.py"),
            Some("draft_service.py")
        );
        assert_eq!(file_name_of(r"C:\work\notes.md"), Some("notes.md"));
        assert_eq!(file_name_of("bare.md"), Some("bare.md"));
        assert_eq!(file_name_of("/a/b/"), None);
        assert_eq!(file_name_of(""), None);
    }

    #[test]
    fn the_whole_path_rule_asks_for_the_whole_path() {
        let rule = MentionRule::WholePath;
        assert!(rule.names("/a/b/draft_service.py", "открой /a/b/draft_service.py"));
        assert!(!rule.names("/a/b/draft_service.py", "открой draft_service.py"));
    }

    /// The name has to be the name, not a tail of a longer one: a person who names
    /// `draft_service.py` has not named `my_draft_service.py`.
    #[test]
    fn a_name_inside_a_longer_name_is_a_different_file() {
        let rule = MentionRule::FileName;
        assert!(rule.names("/a/draft_service.py", "посмотри draft_service.py, там NDS"));
        assert!(rule.names(
            "/a/draft_service.py",
            "в modules/counterparty/draft_service.py"
        ));
        assert!(rule.names("/a/draft_service.py", "«draft_service.py»"));
        assert!(!rule.names("/a/draft_service.py", "посмотри my_draft_service.py"));
        assert!(!rule.names("/a/draft_service.py", "посмотри draft_service.pyc"));
    }

    /// A second occurrence decides when the first one was inside a longer word, so the
    /// search does not stop at the first near miss.
    #[test]
    fn a_later_occurrence_still_counts_when_an_earlier_one_was_part_of_a_word() {
        let rule = MentionRule::FileName;
        assert!(rule.names("/a/notes.md", "сначала my_notes.md, потом notes.md"));
    }

    #[test]
    fn a_tail_names_a_path_only_on_a_segment_boundary() {
        assert!(path_ends_with_segment(
            "/a/b/draft_service.py",
            "draft_service.py"
        ));
        assert!(path_ends_with_segment(
            "/a/b/draft_service.py",
            "b/draft_service.py"
        ));
        assert!(path_ends_with_segment(
            "draft_service.py",
            "draft_service.py"
        ));
        assert!(path_ends_with_segment(r"C:\\work\\notes.md", "notes.md"));
        assert!(!path_ends_with_segment(
            "/a/b/draft_service.py",
            "service.py"
        ));
        assert!(!path_ends_with_segment("/a/b/draft_service.py", ""));
        assert!(!path_ends_with_segment("short.py", "much_longer_name.py"));
    }

    /// The rule the product uses, on the shape that made it necessary: the harness reported a
    /// background task under the owner's role, and the file it names is not a file the owner
    /// asked about.
    #[test]
    fn a_name_inside_a_block_the_tool_wrote_is_not_the_person_naming_it() {
        let notification = "<task-notification>\n<output-file>/tmp/tasks/bw0eoigv6.output</output-file>\n<status>completed</status>\n</task-notification>";
        assert!(MentionRule::FileName.names("/tmp/tasks/bw0eoigv6.output", notification));
        assert!(!MentionRule::SpokenName.names("/tmp/tasks/bw0eoigv6.output", notification));
    }

    #[test]
    fn what_the_person_typed_around_a_quoted_block_still_names_a_file() {
        let said = "смотри <task-notification><output-file>/tmp/x.output</output-file></task-notification> и почини draft_service.py";
        assert!(MentionRule::SpokenName.names("/a/draft_service.py", said));
    }

    /// A tag nobody closed is not an envelope. A tool closes what it opens; a person writes
    /// `<KEY>` inside a sentence, and the sentence goes on.
    #[test]
    fn an_unclosed_tag_is_a_person_typing_an_angle_bracket() {
        let said = "положи <KEY> в docker-compose.yml";
        assert!(MentionRule::SpokenName.names("/a/docker-compose.yml", said));
    }

    /// The inner tags of an envelope are inside it, so the whole block goes with the outer
    /// one and a name in it is still quoted.
    #[test]
    fn a_name_inside_a_nested_block_is_quoted_too() {
        let said =
            "<task-notification><output-file>/tmp/notes.md</output-file></task-notification>";
        assert!(!MentionRule::SpokenName.names("/tmp/notes.md", said));
    }

    /// Backticks are how a person names a file, and a comparison is not a tag.
    #[test]
    fn backticks_and_arithmetic_are_not_quoting() {
        assert!(MentionRule::SpokenName.names("/a/draft_service.py", "открой `draft_service.py`"));
        assert!(MentionRule::SpokenName.names("/a/b.rs", "если a < b, то b.rs не при чём"));
        assert!(MentionRule::SpokenName.names("/a/b.rs", "<br/> b.rs"));
    }

    #[test]
    fn a_path_with_no_name_is_named_by_nothing() {
        assert!(!MentionRule::FileName.names("/a/b/", "что угодно про /a/b/"));
    }
}
