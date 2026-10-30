// Which parser a file is read by, decided from its name.
//
// Until Stage 32 there was one parser and no decision: every file went through `plain-text`,
// cut into 4 KB pieces. A transcript of a coding session read that way is mostly not a
// conversation — Stage 31 measured 2.46% of its bytes as speech and the rest as tool output
// carried twice, attachments and envelope — so the index answered questions about logs.
//
// The rule is the name, and the type says so the way `SourceLanguage` does: the input is a
// path, nothing here opens a file, and an unknown name is not a guess but the default. What
// the name cannot settle, the parser settles by failing to parse — a `.jsonl` that is not a
// transcript is read as plain text, and that is the parser's report rather than a sniff.
use super::source_path::NormalizedPath;

/// The parser one file is read by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionProfile {
    /// Bytes, cut into fixed pieces. Everything that is not something else.
    PlainText,
    /// A session transcript: one JSON object per line, of which the conversation is a small
    /// part. Chunks are replies.
    Conversation,
}

/// The suffix a Claude Code transcript is written under. One suffix and not a list, because
/// this build reads one tool's transcripts; Codex and Cursor write their sessions elsewhere
/// and in another shape, and a rule that claimed them here would claim files it cannot read.
const TRANSCRIPT_SUFFIX: &str = ".jsonl";

/// How large a transcript this build will open at all.
///
/// **Not the content limit, and it does not replace it.** The content limit is unchanged and
/// still decides what goes into the index; for a conversation it is applied to the
/// conversation, because a transcript is a container and Stage 31 measured the talking in it
/// at 2.46% of its bytes. The owner's largest conversation is 58.5 MiB of transcript holding
/// 1.5 MB of talking: under a rule that compares the container against a content limit it was
/// invisible to search entirely, and it is the right answer to one of the fifteen questions.
///
/// This number is a different thing: a ceiling on *work*. Reading a transcript is a pass over
/// its lines whatever comes out, and without a ceiling one enormous file would hold a scan for
/// as long as it took. It is set far above anything measured — eight times the largest
/// transcript on this machine — because it is there to stop the absurd case and not to make a
/// judgement about content.
const MOST_TRANSCRIPT_BYTES: u64 = 512 * 1024 * 1024;

impl ExtractionProfile {
    /// The profile a path's name selects.
    pub fn of_path(path: &NormalizedPath) -> Self {
        if path.as_str().ends_with(TRANSCRIPT_SUFFIX) {
            return Self::Conversation;
        }
        Self::PlainText
    }

    /// The suffix a transcript's name ends with: what a reader of the graph asks for when it
    /// wants the files this profile reads, without a second copy of the rule.
    pub fn transcript_suffix() -> &'static str {
        TRANSCRIPT_SUFFIX
    }

    /// Whether a file of this size is worth opening under this profile.
    ///
    /// For text the two questions are the same one: the file is the content, so a file over
    /// the limit holds content over the limit and is never opened. For a transcript they are
    /// different questions, and answering the first with the second is what kept the owner's
    /// largest conversation out of the index.
    pub fn may_read(self, size_bytes: u64, max_content_bytes: u64) -> bool {
        match self {
            Self::PlainText => size_bytes <= max_content_bytes,
            Self::Conversation => size_bytes <= MOST_TRANSCRIPT_BYTES,
        }
    }

    /// The name this profile records in the provenance of everything it produces, and the
    /// version of its rules.
    ///
    /// The version is part of a chunk's identity, so the same bytes cut by a later version of
    /// the same parser are different chunks and both can be in the graph at once. That is
    /// what makes "which files must be read again" answerable at all.
    pub fn extractor_id(self) -> &'static str {
        match self {
            Self::PlainText => "plain-text",
            Self::Conversation => "conversation",
        }
    }

    pub fn extractor_version(self) -> &'static str {
        match self {
            Self::PlainText => "1",
            Self::Conversation => "1",
        }
    }

    /// How the text of a chunk was arrived at, which for both profiles so far is the profile
    /// itself. It is a separate column because one parser can extract by more than one method
    /// — a PDF read by its text layer and the same PDF read by OCR are the same extractor and
    /// not the same evidence.
    pub fn extraction_method(self) -> &'static str {
        self.extractor_id()
    }

    /// The profile that recorded itself under this name, and nothing for a name no build of
    /// this program writes.
    pub fn of_extractor_id(id: &str) -> Option<Self> {
        match id {
            "plain-text" => Some(Self::PlainText),
            "conversation" => Some(Self::Conversation),
            _ => None,
        }
    }

    /// Whether the chunk at this ordinal is the conversation's own name.
    ///
    /// One rule in one place, because two different callers derive it: the pass, which has
    /// the conversation in hand, and the rebuild, which has only rows out of the graph. If
    /// they disagreed, a rebuilt index would hold different name documents from a freshly
    /// written one and nothing would say so.
    pub fn chunk_is_the_conversation_name(self, ordinal: u32) -> bool {
        matches!(self, Self::Conversation) && ordinal == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(value: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(value).expect("абсолютный путь")
    }

    #[test]
    fn a_transcript_is_recognised_by_its_suffix() {
        assert_eq!(
            ExtractionProfile::of_path(&path("/Users/a/.claude/projects/p/9c21.jsonl")),
            ExtractionProfile::Conversation
        );
    }

    #[test]
    fn everything_else_is_read_as_plain_text() {
        for name in [
            "/a/b/notes.md",
            "/a/b/main.rs",
            "/a/b/data.json",
            "/a/b/jsonl",
            "/a/b/x.jsonl.bak",
            "/a/b",
        ] {
            assert_eq!(
                ExtractionProfile::of_path(&path(name)),
                ExtractionProfile::PlainText,
                "{name} должен читаться как обычный текст"
            );
        }
    }

    /// The rule that decides whether a file is opened at all, and the one Stage 32 changed
    /// for one profile only.
    #[test]
    fn a_transcript_is_opened_past_the_content_limit_and_a_document_is_not() {
        let limit = 16 * 1024 * 1024;
        let big = 61_384_989; // the owner's largest transcript, to the byte
        assert!(!ExtractionProfile::PlainText.may_read(big, limit));
        assert!(ExtractionProfile::Conversation.may_read(big, limit));
        // Both refuse the absurd case; the ceilings differ, but neither is absent.
        assert!(!ExtractionProfile::Conversation.may_read(u64::MAX, limit));
        assert!(ExtractionProfile::PlainText.may_read(limit, limit));
    }

    #[test]
    fn the_name_is_the_first_chunk_of_a_conversation_and_nothing_of_a_document() {
        assert!(ExtractionProfile::Conversation.chunk_is_the_conversation_name(0));
        assert!(!ExtractionProfile::Conversation.chunk_is_the_conversation_name(1));
        assert!(!ExtractionProfile::PlainText.chunk_is_the_conversation_name(0));
        assert_eq!(
            ExtractionProfile::of_extractor_id("conversation"),
            Some(ExtractionProfile::Conversation)
        );
        assert_eq!(ExtractionProfile::of_extractor_id("ocr"), None);
    }

    /// The two profiles must never share provenance, or a chunk of a file read one way would
    /// be indistinguishable from a chunk of the same file read the other.
    #[test]
    fn the_two_profiles_are_told_apart_by_what_they_record() {
        assert_ne!(
            ExtractionProfile::PlainText.extractor_id(),
            ExtractionProfile::Conversation.extractor_id()
        );
    }
}
