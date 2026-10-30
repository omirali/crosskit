// A conversation with an AI coding tool: what was said in it, and which files it touched.
//
// A transcript on disk is a log of a session: every tool call, every file the model read
// back, every internal deliberation, and around each of them an envelope of identifiers.
// None of that is the conversation. What two parties said to each other is a small part of
// the file — Stage 31 measured 2.5% of its bytes — and it is the only part this type holds.
//
// The types here say nothing about JSON, because a second tool writes its sessions in a
// different shape and the same conversation has to come out of it. A reader turns a file
// into `TranscriptLine` values; nothing below that boundary knows what a line looked like.
use std::fmt;

use super::file_mention::OwnerNaming;

/// Who said a reply. Two parties, because a transcript records two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// The person whose machine this is.
    Owner,
    /// The model answering them.
    Model,
}

impl fmt::Display for Speaker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owner => write!(formatter, "владелец"),
            Self::Model => write!(formatter, "модель"),
        }
    }
}

/// One thing that was said: who said it, what they said, and when.
///
/// The text is never empty — a reply with nothing in it is a line the reader ignored, not a
/// reply — and the time is optional because a transcript can record a line without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    speaker: Speaker,
    text: String,
    at_unix_ms: Option<i64>,
}

impl Reply {
    /// A reply, or nothing when there is no text to hold.
    pub fn new(speaker: Speaker, text: impl Into<String>, at_unix_ms: Option<i64>) -> Option<Self> {
        let text = text.into();
        if text.trim().is_empty() {
            return None;
        }
        Some(Self {
            speaker,
            text,
            at_unix_ms,
        })
    }

    pub fn speaker(&self) -> Speaker {
        self.speaker
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn at_unix_ms(&self) -> Option<i64> {
        self.at_unix_ms
    }
}

/// Who named the conversation.
///
/// Both names live in the same file and they are not the same claim. The model writes its
/// own name as the session goes and rewrites it when the subject moves; the owner's name is
/// a deliberate rename, so it wins wherever both are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NamedBy {
    Model,
    Owner,
}

/// The name of a conversation and who gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationName {
    value: String,
    named_by: NamedBy,
}

impl ConversationName {
    /// A name, or nothing when the transcript wrote an empty one.
    pub fn new(value: impl Into<String>, named_by: NamedBy) -> Option<Self> {
        let value = value.into().trim().to_string();
        if value.is_empty() {
            return None;
        }
        Some(Self { value, named_by })
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn named_by(&self) -> NamedBy {
        self.named_by
    }
}

/// One conversation: its session, its name, and what was said in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    session_id: String,
    name: Option<ConversationName>,
    replies: Vec<Reply>,
    touches: Vec<FileTouch>,
}

impl Conversation {
    pub fn new(
        session_id: impl Into<String>,
        name: Option<ConversationName>,
        replies: Vec<Reply>,
        touches: Vec<FileTouch>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            name,
            replies,
            touches,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn name(&self) -> Option<&ConversationName> {
        self.name.as_ref()
    }

    pub fn replies(&self) -> &[Reply] {
        &self.replies
    }

    /// The files this conversation touched, deduplicated by path and verb where the reader
    /// met the same pair twice.
    pub fn touches(&self) -> &[FileTouch] {
        &self.touches
    }

    /// When the first thing was said. With `last_spoke_at_unix_ms` this is the span a panel
    /// shows as "when the conversation ran", which is not when its file was last written.
    pub fn first_spoke_at_unix_ms(&self) -> Option<i64> {
        self.replies.iter().filter_map(Reply::at_unix_ms).min()
    }

    /// When the conversation last had something said in it, which is not when its file was
    /// last written: a transcript is appended to by the tool, and Stage 32 needs the time of
    /// the talking.
    pub fn last_spoke_at_unix_ms(&self) -> Option<i64> {
        self.replies.iter().filter_map(Reply::at_unix_ms).max()
    }

    /// The bytes of what was said.
    ///
    /// Not read by the pass: the content limit is counted as the replies arrive, in
    /// `ConversationAssembly`, so that reading stops at the ceiling instead of a finished
    /// conversation being weighed and refused. What is left here is the weighing — the
    /// measurement that puts a corpus of talking beside a corpus of transcripts, and the test
    /// that checks the budget held.
    #[cfg(test)]
    pub fn spoken_bytes(&self) -> u64 {
        self.replies
            .iter()
            .map(|reply| reply.text().len() as u64)
            .sum()
    }
}

/// What a conversation did to a file.
///
/// A closed set and never the tool's own name. Claude Code renames its tools between
/// versions — `Edit` was `str_replace_editor`, `NotebookEdit` did not exist — and a graph
/// that stored whatever string the transcript carried would answer "which conversations wrote
/// this file" differently depending on which weeks the transcripts came from. A tool this
/// build does not know produces no touch at all and is counted as unknown, which is a number
/// in the report rather than a guess in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TouchVerb {
    Read,
    Wrote,
    Edited,
}

impl TouchVerb {
    /// The verb a tool's name means, and nothing for a tool this build has not been told
    /// about.
    pub fn of_tool(tool: &str) -> Option<Self> {
        match tool {
            "Read" => Some(Self::Read),
            "Write" => Some(Self::Wrote),
            "Edit" | "MultiEdit" | "NotebookEdit" => Some(Self::Edited),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Wrote => "wrote",
            Self::Edited => "edited",
        }
    }

    /// The read side of the closed set, out of `cfg(test)` since Stage 37: the command that
    /// answers "which conversations touched this file" reads verbs back out of the graph.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "wrote" => Some(Self::Wrote),
            "edited" => Some(Self::Edited),
            _ => None,
        }
    }

    /// Whether this verb alone makes a touch deliberate. Writing and editing are things
    /// nobody does to a file they met by accident; reading is what an agent does to forty
    /// files while looking for one. ADR 0041.
    pub fn is_deliberate(self) -> bool {
        match self {
            Self::Wrote | Self::Edited => true,
            Self::Read => false,
        }
    }

    #[cfg(test)]
    pub const ALL: [Self; 3] = [Self::Read, Self::Wrote, Self::Edited];
}

impl fmt::Display for TouchVerb {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Where the knowledge that a file was touched came from, and therefore how much it is worth.
///
/// Section 5 of the architecture puts these on different rungs of the auto-accept scale, and
/// the difference is not a nuance. A path the tool itself was handed and read from disk is
/// `auto-accept`; a path scraped out of a shell command is `observation only`, because the
/// command line does not say what happened to it — `cat` reads, `sed -i` writes, `rg` does
/// neither — and Stage 32 measured that only 16% of such paths exist at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TouchSource {
    /// The argument a file tool was called with. Fact.
    ToolInput,
    /// The file-history snapshot the tool writes before it edits. Fact.
    FileHistory,
    /// A path read out of a shell command line. Observation only, and **this build produces
    /// none**: Stage 32 measured that only a small minority of such paths exist at all, and
    /// the command line does not say what happened to the file. The variant is declared
    /// anyway, because the column already holds a closed set and a source added later must
    /// be a value this set always had rather than a migration of every row.
    #[allow(
        dead_code,
        reason = "declared so the closed set is complete; measured, not produced — see ADR 0037"
    )]
    Command,
}

impl TouchSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolInput => "tool_input",
            Self::FileHistory => "file_history",
            Self::Command => "command",
        }
    }

    /// The read side of the closed set; see `TouchVerb::parse`.
    #[cfg(test)]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "tool_input" => Some(Self::ToolInput),
            "file_history" => Some(Self::FileHistory),
            "command" => Some(Self::Command),
            _ => None,
        }
    }

    /// Whether a fact from this source may be written to the graph without review.
    #[cfg(test)]
    pub fn is_auto_accepted(self) -> bool {
        match self {
            Self::ToolInput | Self::FileHistory => true,
            Self::Command => false,
        }
    }

    #[cfg(test)]
    pub const ALL: [Self; 3] = [Self::ToolInput, Self::FileHistory, Self::Command];
}

impl fmt::Display for TouchSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One file a conversation touched: the path as the transcript wrote it, what was done to it,
/// when, and on whose word.
///
/// **The path is a string and not an edge to `File`, and that is a decision, not a
/// shortcut.** Stage 32 measured where these paths lead: 31% of them lie in no configured
/// context at all, and of the rest most belong to a context other than `chats`. A context is
/// its own database (ADR 0008) — a cross-context query is impossible by construction, which
/// is the guarantee itself — so an edge to a `File` node could only ever be drawn to a file
/// that happens to live in the same database, and would silently be absent for the 69% that
/// do not. A path is the same fact in every context, and joining it to a file is the reader's
/// job, in whichever context they are standing.
///
/// **Two marks travel with it, and both are written rather than filtered.** A touch nobody
/// asked for is still a fact, and a fact that was never recorded cannot be recovered — the
/// transcripts it comes from are live files the tool prunes. So nothing is dropped on the way
/// in: the marks say what is known about a touch, and how strict an answer is, is the
/// reader's to decide and to change its mind about. ADR 0041.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTouch {
    path: String,
    verb: TouchVerb,
    source: TouchSource,
    at_unix_ms: Option<i64>,
    naming: OwnerNaming,
    times: u32,
}

impl FileTouch {
    /// A touch, or nothing when the transcript wrote no path.
    ///
    /// It is born having happened once and with nobody having looked at whether the owner
    /// named it: a reader of one line knows neither. Both are settled by the assembly, which
    /// is the only thing that has met the whole conversation.
    pub fn new(
        path: impl Into<String>,
        verb: TouchVerb,
        source: TouchSource,
        at_unix_ms: Option<i64>,
    ) -> Option<Self> {
        let path = path.into().trim().to_string();
        if path.is_empty() {
            return None;
        }
        Some(Self {
            path,
            verb,
            source,
            at_unix_ms,
            naming: OwnerNaming::NotComputed,
            times: 1,
        })
    }

    /// The same touch, with what the whole conversation says about it.
    pub fn marked(mut self, naming: OwnerNaming, times: u32) -> Self {
        self.naming = naming;
        self.times = times.max(1);
        self
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn verb(&self) -> TouchVerb {
        self.verb
    }

    pub fn source(&self) -> TouchSource {
        self.source
    }

    pub fn at_unix_ms(&self) -> Option<i64> {
        self.at_unix_ms
    }

    /// Whether the owner named this file in their own words in this conversation.
    pub fn naming(&self) -> OwnerNaming {
        self.naming
    }

    /// How many times the conversation did this to this file. One is the ordinary case and
    /// the reason the mark exists is the other one: a file opened once in passing and a file
    /// gone back to nine times are not the same touch.
    pub fn times(&self) -> u32 {
        self.times
    }
}

/// Why a reader passed a line by. Named rather than counted as one number, because "we
/// dropped 380 000 lines" says nothing about whether the right ones were dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkippedLine {
    /// `toolUseResult` in the envelope: what a tool printed, carried beside the message.
    ToolUseResult,
    /// A `tool_result` block in the message body — the same text a second time.
    ToolResult,
    /// A tool call the model made.
    ToolUse,
    /// The model's own deliberation.
    Thinking,
    /// An image block; nothing here reads pixels.
    Image,
    /// An `attachment` line: a file the tool folded into the session.
    Attachment,
    /// A line the session bookkeeping writes and nobody said: queue operations, latches,
    /// file-history snapshots, mode changes.
    Bookkeeping,
    /// A message the harness injected under a party's role — a skill's text, an image
    /// placeholder. It is marked `isMeta` in the transcript, and it is nobody's speech.
    Injected,
    /// A message whose blocks held nothing but the above, so no text survived.
    NoTextLeft,
    /// A tool call carrying a file path, made by a tool this build has no verb for. Counted
    /// rather than guessed: the set of tools moves, and a number here is how anybody learns
    /// that it moved.
    UnknownTool,
}

impl SkippedLine {
    /// Every reason, so a report can print a full table rather than the reasons that
    /// happened to occur. Only a report needs the whole list.
    #[cfg(all(test, feature = "insight-layer-measurement"))]
    pub const ALL: [Self; 10] = [
        Self::ToolUseResult,
        Self::ToolResult,
        Self::ToolUse,
        Self::Thinking,
        Self::Image,
        Self::Attachment,
        Self::Bookkeeping,
        Self::Injected,
        Self::NoTextLeft,
        Self::UnknownTool,
    ];
}

impl fmt::Display for SkippedLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let said = match self {
            Self::ToolUseResult => "вывод инструмента в обвязке строки",
            Self::ToolResult => "вывод инструмента в теле сообщения",
            Self::ToolUse => "вызов инструмента",
            Self::Thinking => "размышление модели",
            Self::Image => "картинка",
            Self::Attachment => "вложение",
            Self::Bookkeeping => "служебная строка сессии",
            Self::Injected => "подставленный текст (isMeta)",
            Self::NoTextLeft => "сообщение без текста после отбора",
            Self::UnknownTool => "инструмент с путём, глагол которого неизвестен",
        };
        write!(formatter, "{said}")
    }
}

/// Why a line could not be read at all. A transcript is somebody else's format and it
/// changes, so this is a result of the measurement and not an error to swallow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnreadableLine {
    /// The line is not JSON.
    NotJson,
    /// It is JSON, but not an object with fields.
    NotAnObject,
    /// An object with no `type`, so nothing says what it is.
    Untyped,
    /// A typed message line whose `message` is missing or is not an object.
    MessageMissing,
    /// A message whose `content` is neither a string nor a list of blocks.
    ContentUnknown,
    /// A line whose bytes are not UTF-8.
    NotUtf8,
}

impl UnreadableLine {
    #[cfg(all(test, feature = "insight-layer-measurement"))]
    pub const ALL: [Self; 6] = [
        Self::NotJson,
        Self::NotAnObject,
        Self::Untyped,
        Self::MessageMissing,
        Self::ContentUnknown,
        Self::NotUtf8,
    ];
}

impl fmt::Display for UnreadableLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let said = match self {
            Self::NotJson => "не JSON",
            Self::NotAnObject => "JSON, но не объект",
            Self::Untyped => "объект без поля type",
            Self::MessageMissing => "строка сообщения без message",
            Self::ContentUnknown => "content незнакомой формы",
            Self::NotUtf8 => "байты не UTF-8",
        };
        write!(formatter, "{said}")
    }
}

/// What one line of a transcript turned out to be.
///
/// A reader yields one of these per line and never decides anything: the rules for which
/// name wins and what a conversation ends up being belong above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptLine {
    /// The session this line belongs to, seen on any line that carries it.
    Session(String),
    /// A name the transcript records, with who gave it.
    Named(ConversationName),
    /// Something a party said.
    Spoke(Reply),
    /// A file the conversation touched, on the word of a source that knows the verb.
    Touched(FileTouch),
    /// A line read and deliberately passed by.
    Skipped(SkippedLine),
    /// A line that could not be read.
    Unreadable(UnreadableLine),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_of_whitespace_is_not_a_reply() {
        assert!(Reply::new(Speaker::Owner, "   \n\t ", Some(1)).is_none());
        assert!(Reply::new(Speaker::Owner, "что там?", Some(1)).is_some());
    }

    #[test]
    fn a_name_is_trimmed_and_an_empty_one_is_no_name() {
        assert!(ConversationName::new("  \n ", NamedBy::Model).is_none());
        let name = ConversationName::new("  Беседа Б \n", NamedBy::Owner).expect("непустое имя");
        assert_eq!(name.value(), "Беседа Б");
        assert_eq!(name.named_by(), NamedBy::Owner);
    }

    #[test]
    fn a_conversation_reports_when_it_started_and_when_it_ended() {
        let conversation = Conversation::new(
            "s",
            None,
            vec![
                Reply::new(Speaker::Owner, "раз", Some(10)).expect("реплика"),
                Reply::new(Speaker::Model, "два", None).expect("реплика"),
                Reply::new(Speaker::Owner, "три", Some(30)).expect("реплика"),
            ],
            Vec::new(),
        );
        assert_eq!(conversation.first_spoke_at_unix_ms(), Some(10));
        assert_eq!(conversation.last_spoke_at_unix_ms(), Some(30));
        assert_eq!(conversation.spoken_bytes(), 6 + 6 + 6);
    }

    /// The verb is this build's, not the transcript's. A tool nobody taught this build about
    /// gets no verb, and therefore no touch.
    #[test]
    fn a_tool_this_build_does_not_know_has_no_verb() {
        assert_eq!(TouchVerb::of_tool("Read"), Some(TouchVerb::Read));
        assert_eq!(TouchVerb::of_tool("Write"), Some(TouchVerb::Wrote));
        assert_eq!(TouchVerb::of_tool("Edit"), Some(TouchVerb::Edited));
        assert_eq!(TouchVerb::of_tool("NotebookEdit"), Some(TouchVerb::Edited));
        assert_eq!(TouchVerb::of_tool("Bash"), None);
        assert_eq!(TouchVerb::of_tool("str_replace_editor"), None);
        assert_eq!(TouchVerb::of_tool(""), None);
    }

    /// Both closed sets survive a round trip through the string the graph stores, so a row
    /// written today is readable by the same build tomorrow.
    #[test]
    fn the_closed_sets_round_trip_through_what_the_graph_stores() {
        for verb in TouchVerb::ALL {
            assert_eq!(TouchVerb::parse(verb.as_str()), Some(verb));
        }
        for source in TouchSource::ALL {
            assert_eq!(TouchSource::parse(source.as_str()), Some(source));
        }
        assert_eq!(TouchVerb::parse("touched"), None);
        assert_eq!(TouchSource::parse("guess"), None);
    }

    /// Section 5 of the architecture, as a property of the type: a path scraped from a
    /// command line is observation only, and this build writes none.
    #[test]
    fn only_a_path_the_tool_was_handed_is_auto_accepted() {
        assert!(TouchSource::ToolInput.is_auto_accepted());
        assert!(TouchSource::FileHistory.is_auto_accepted());
        assert!(!TouchSource::Command.is_auto_accepted());
    }

    #[test]
    fn a_touch_without_a_path_is_not_a_touch() {
        assert!(FileTouch::new("  ", TouchVerb::Read, TouchSource::ToolInput, None).is_none());
        let touch = FileTouch::new(
            " /a/b.rs ",
            TouchVerb::Read,
            TouchSource::ToolInput,
            Some(7),
        )
        .expect("непустой путь");
        assert_eq!(touch.path(), "/a/b.rs");
        assert_eq!(touch.at_unix_ms(), Some(7));
    }
}
