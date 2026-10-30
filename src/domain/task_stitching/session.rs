// What is known of one session for stitching: where and when it ran, who it was, what it wrote,
// what it referred to and what its owner said.
//
// The facts are already reduced to the form they are compared in: files as keys
// (`mentions::file_key`), tracker links without queries, the owner's text joined. The readers
// of the two tools fill them; nothing here knows what a log line looked like.
use std::collections::BTreeSet;

use super::mentions::{file_key, path_mentions, tracker_links};

/// How much of the owner's first reply is kept as the opening: enough to recognise the
/// conversation by and to tell an imported copy from a different conversation.
const OPENING_CHARACTERS: usize = 200;
/// How many of the owner's replies make the start of a conversation. A conversation that
/// continues another says so as it starts — «прочитай handoff», «продолжаем задачу…» — and a
/// file or a conversation named later is consulted, not continued.
const OPENING_REPLIES: usize = 3;

/// The tool a conversation was held in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Platform {
    ClaudeCode,
    #[allow(
        dead_code,
        reason = "declared so the closed set is complete; the product does not read Codex yet (ADR 0052), the measurement does"
    )]
    Codex,
}

/// Where the record came from. A conversation held in Claude Code and imported by Codex is a
/// Claude Code conversation read from a copy: its platform is where it was held, not where it
/// is stored, and a copy carries the replies without the tool calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Original,
    #[allow(
        dead_code,
        reason = "declared so the closed set is complete; copies live in Codex, which only the measurement reads yet"
    )]
    ImportedCopy,
}

/// What kind of session it is. Only a conversation is a node of a task; the others are named
/// and counted so that a task does not show them as conversations (ADR 0051, decision 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SessionRole {
    /// A person talking to an agent.
    Conversation,
    /// A session another session started to do part of its work.
    #[allow(
        dead_code,
        reason = "declared so the closed set is complete; produced by the Codex reader of the measurement"
    )]
    Worker,
    /// A session that reviews another session's commands before they run.
    #[allow(
        dead_code,
        reason = "declared so the closed set is complete; produced by the Codex reader of the measurement"
    )]
    AutoReview,
}

/// One session.
#[derive(Debug, Clone)]
pub(crate) struct SessionFacts {
    pub(crate) id: String,
    pub(crate) platform: Platform,
    pub(crate) origin: Origin,
    pub(crate) role: SessionRole,
    /// The directory the session worked in, which relative paths it names resolve against.
    pub(crate) working_directory: Option<String>,
    /// Where the session was held, for telling neighbours: its working directory when the log
    /// records one, or the folder its transcript is filed under when only that is known.
    pub(crate) place: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) started_ms: Option<i64>,
    pub(crate) ended_ms: Option<i64>,
    pub(crate) title: Option<String>,
    /// The owner's first reply, as the readers cut it: what a person would recognise the
    /// conversation by when there is no title.
    pub(crate) opening: Option<String>,
    /// Everything the owner said, joined.
    pub(crate) owner_text: String,
    /// Keys of files the session wrote or edited.
    pub(crate) written: BTreeSet<String>,
    /// Keys of files the session read or named.
    pub(crate) referenced: BTreeSet<String>,
    pub(crate) tracker_links: BTreeSet<String>,
    /// What the owner said in the first replies, joined.
    pub(crate) start_text: String,
    /// Keys of files read or named before the owner's reply after the first ones.
    pub(crate) start_referenced: BTreeSet<String>,
    owner_replies: usize,
}

impl SessionFacts {
    pub(crate) fn new(id: impl Into<String>, platform: Platform) -> Self {
        Self {
            id: id.into(),
            platform,
            origin: Origin::Original,
            role: SessionRole::Conversation,
            working_directory: None,
            place: None,
            branch: None,
            started_ms: None,
            ended_ms: None,
            title: None,
            opening: None,
            owner_text: String::new(),
            written: BTreeSet::new(),
            referenced: BTreeSet::new(),
            tracker_links: BTreeSet::new(),
            start_text: String::new(),
            start_referenced: BTreeSet::new(),
            owner_replies: 0,
        }
    }

    fn is_starting(&self) -> bool {
        self.owner_replies <= OPENING_REPLIES
    }

    /// Widens the session's span to include `at_unix_ms`.
    pub(crate) fn saw(&mut self, at_unix_ms: i64) {
        self.started_ms = Some(self.started_ms.map_or(at_unix_ms, |t| t.min(at_unix_ms)));
        self.ended_ms = Some(self.ended_ms.map_or(at_unix_ms, |t| t.max(at_unix_ms)));
    }

    /// Takes in one reply of the owner: the opening if it is the first, the files and tracker
    /// tasks it names, and its words. Relative paths resolve against the session's working
    /// directory, so it has to be known first.
    pub(crate) fn take_owner_words(&mut self, text: &str, home: &str) {
        if self.opening.is_none() {
            self.opening = Some(text.trim().chars().take(OPENING_CHARACTERS).collect());
        }
        self.owner_replies += 1;
        if self.is_starting() {
            self.start_text.push_str(text);
            self.start_text.push('\n');
        }
        self.take_mentions(text, home);
        self.tracker_links.extend(tracker_links(text));
        self.owner_text.push_str(text);
        self.owner_text.push('\n');
    }

    /// Takes in the files `text` names as files the session referred to.
    pub(crate) fn take_mentions(&mut self, text: &str, home: &str) {
        for mention in path_mentions(text) {
            if let Some(key) = file_key(&mention, self.working_directory.as_deref(), home) {
                self.take_reference(key);
            }
        }
    }

    /// Takes in a file the session read or named, by its key.
    pub(crate) fn take_reference(&mut self, key: String) {
        if self.is_starting() {
            self.start_referenced.insert(key.clone());
        }
        self.referenced.insert(key);
    }

    /// Whether `other` is the same conversation: one is an imported copy of the other, and both
    /// open with the same words.
    #[cfg(test)]
    pub(crate) fn is_copy_of(&self, other: &Self) -> bool {
        self.origin == Origin::ImportedCopy
            && other.origin == Origin::Original
            && self.platform == other.platform
            && self.opening.is_some()
            && self.opening == other.opening
    }
}
