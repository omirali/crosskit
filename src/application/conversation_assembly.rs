// Turning the lines of one transcript into one conversation.
//
// A reader hands over what each line turned out to be and decides nothing. The decisions are
// here, and there are three:
//
//   * which name wins when a transcript records several — the tool renames the session as
//     the subject moves, so the last one it wrote is the one it means;
//   * whose name wins when both parties named it — the owner's rename is deliberate and the
//     model's is a guess, so the owner's wins even if it came first;
//   * what a line that could not be read does to the result — it is counted by its reason and
//     named in the report, because a transcript is somebody else's format, it changes, and a
//     silently dropped line is a measurement that quietly stops measuring.
use std::collections::{BTreeMap, HashMap};
use std::ops::ControlFlow;

use crate::domain::conversation::{
    Conversation, ConversationName, FileTouch, NamedBy, Reply, SkippedLine, Speaker,
    TranscriptLine, UnreadableLine,
};
use crate::domain::{MentionRule, OwnerNaming};

/// What reading one transcript, or a corpus of them, met on the way.
///
/// Every accessor is `cfg(test)`: the pass publishes a conversation and has nowhere to report
/// a tally to, and the only thing that reads these numbers today is the measurement that
/// prints them. They move out together with the first command that reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptTally {
    observations: u64,
    replies: u64,
    names: u64,
    renames: u64,
    skipped: BTreeMap<SkippedLine, u64>,
    unreadable: BTreeMap<UnreadableLine, u64>,
}

impl TranscriptTally {
    /// How many things the reader had to say about the transcript, which is not how many
    /// lines it held: one message line holding three deliberations and a sentence is four
    /// observations. The count of lines belongs to the reader, which is the only thing that
    /// sees them.
    #[cfg(test)]
    pub fn observations(&self) -> u64 {
        self.observations
    }

    #[cfg(test)]
    pub fn replies(&self) -> u64 {
        self.replies
    }

    /// How many name lines the transcript held, which is not how many names it had: the tool
    /// writes the name again every time it reconsiders it.
    #[cfg(test)]
    pub fn names(&self) -> u64 {
        self.names
    }

    /// How many times a name line actually changed the name. This is the evidence for "the
    /// last one wins": a transcript with hundreds of name lines and no rename was never
    /// renamed, and one with renames had a name that would have been wrong taken earlier.
    #[cfg(test)]
    pub fn renames(&self) -> u64 {
        self.renames
    }

    #[cfg(test)]
    pub fn skipped(&self, reason: SkippedLine) -> u64 {
        self.skipped.get(&reason).copied().unwrap_or_default()
    }

    /// Unreadable lines, by reason. Unlike a skip, this is one per line: a line the reader
    /// could not read produces this and nothing else.
    #[cfg(test)]
    pub fn unreadable(&self, reason: UnreadableLine) -> u64 {
        self.unreadable.get(&reason).copied().unwrap_or_default()
    }

    #[cfg(test)]
    pub fn unreadable_total(&self) -> u64 {
        self.unreadable.values().sum()
    }

    /// Folds another transcript's tally in, so a corpus is counted the same way one file is.
    #[cfg(test)]
    pub fn absorb(&mut self, other: &Self) {
        self.observations += other.observations;
        self.replies += other.replies;
        self.names += other.names;
        self.renames += other.renames;
        for (reason, count) in &other.skipped {
            *self.skipped.entry(*reason).or_default() += count;
        }
        for (reason, count) in &other.unreadable {
            *self.unreadable.entry(*reason).or_default() += count;
        }
    }
}

/// One transcript read: the conversation it held and what reading it met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadTranscript {
    conversation: Conversation,
    tally: TranscriptTally,
    outgrew_the_budget: bool,
}

impl ReadTranscript {
    #[cfg(test)]
    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    #[cfg(test)]
    pub fn tally(&self) -> &TranscriptTally {
        &self.tally
    }

    /// The conversation and what reading it met. The pass publishes the conversation and has
    /// nowhere to report a tally to, so it uses `into_conversation`; the measurement wants
    /// both.
    #[cfg(all(test, feature = "insight-layer-measurement"))]
    pub fn into_parts(self) -> (Conversation, TranscriptTally) {
        (self.conversation, self.tally)
    }

    /// The conversation alone, for the caller that publishes it and has nowhere to report a
    /// tally to.
    pub fn into_conversation(self) -> Conversation {
        self.conversation
    }

    /// Whether the transcript held more speech than the caller allowed, and reading stopped.
    ///
    /// The conversation in hand is then a prefix of the real one and must not be published:
    /// it is the answer "too large", not a shorter conversation.
    pub fn outgrew_the_budget(&self) -> bool {
        self.outgrew_the_budget
    }
}

/// Collects the lines of one transcript into one conversation.
///
/// `fallback_session` is what the conversation is called when no line carried a session of
/// its own — for Claude Code that is the transcript's own file name, which is a convention of
/// one tool and therefore the caller's to supply rather than this type's to assume.
#[derive(Debug)]
pub struct ConversationAssembly {
    fallback_session: String,
    session_id: Option<String>,
    name: Option<ConversationName>,
    replies: Vec<Reply>,
    touches: Vec<FileTouch>,
    /// Where each touch already met sits in `touches`, so meeting it again is a count rather
    /// than a row.
    seen_touches: HashMap<(String, &'static str, &'static str), usize>,
    /// How many times each touch of `touches` happened, by the same index.
    times: Vec<u32>,
    tally: TranscriptTally,
    /// How many bytes of speech this assembly may hold, and how many it holds.
    ///
    /// The budget is checked as each reply arrives and not at the end, and that is the whole
    /// point of it. Stage 2S paid for this lesson on the other extractor and wrote it down in
    /// `plain_text_evidence`: releasing the text after cutting does not lower the peak,
    /// because the peak has already happened — it has to never exist. A ceiling applied to a
    /// finished sum is not a ceiling on memory, it is a verdict delivered after the cost.
    most_spoken_bytes: u64,
    spoken_bytes: u64,
    outgrew_the_budget: bool,
}

impl ConversationAssembly {
    /// An assembly that will hold at most `most_spoken_bytes` of speech.
    ///
    /// The budget is not optional, because every caller has one: the pass has the content
    /// limit of its policy, and a measurement that must drop nothing says so by passing
    /// `u64::MAX` where a reader can see it.
    pub fn for_session(fallback_session: impl Into<String>, most_spoken_bytes: u64) -> Self {
        Self {
            fallback_session: fallback_session.into(),
            session_id: None,
            name: None,
            replies: Vec::new(),
            touches: Vec::new(),
            seen_touches: HashMap::new(),
            times: Vec::new(),
            tally: TranscriptTally::default(),
            most_spoken_bytes,
            spoken_bytes: 0,
            outgrew_the_budget: false,
        }
    }

    /// Takes one line, and says whether reading should go on.
    ///
    /// It stops on exactly one condition: the speech would pass the budget. Everything else
    /// is counted and carried on from, because a transcript is somebody else's format and a
    /// reader that stops at the first surprise reports a corpus it did not read.
    pub fn absorb(&mut self, line: TranscriptLine) -> ControlFlow<()> {
        self.tally.observations += 1;
        match line {
            TranscriptLine::Session(id) => {
                self.session_id.get_or_insert(id);
            }
            TranscriptLine::Named(name) => {
                self.tally.names += 1;
                if keeps_the_new_name(self.name.as_ref(), &name) {
                    if self
                        .name
                        .as_ref()
                        .is_some_and(|held| held.value() != name.value())
                    {
                        self.tally.renames += 1;
                    }
                    self.name = Some(name);
                }
            }
            TranscriptLine::Spoke(reply) => {
                // Checked before the reply is kept, so what is held never exceeds the budget
                // by more than the one reply being weighed.
                let would_hold = self.spoken_bytes.saturating_add(reply.text().len() as u64);
                if would_hold > self.most_spoken_bytes {
                    self.outgrew_the_budget = true;
                    return ControlFlow::Break(());
                }
                self.spoken_bytes = would_hold;
                self.tally.replies += 1;
                self.replies.push(reply);
            }
            TranscriptLine::Touched(touch) => {
                // The same file, verb and source met again is the same fact, and it is also
                // a second time it happened. Keeping the first row keeps the earliest time,
                // which is when the conversation first touched the file; the repeat is
                // counted on it, because "read once in passing" and "gone back to nine
                // times" are the same row and not the same touch.
                let key = (
                    touch.path().to_string(),
                    touch.verb().as_str(),
                    touch.source().as_str(),
                );
                match self.seen_touches.get(&key) {
                    Some(&at) => self.times[at] = self.times[at].saturating_add(1),
                    None => {
                        self.seen_touches.insert(key, self.touches.len());
                        self.touches.push(touch);
                        self.times.push(1);
                    }
                }
            }
            TranscriptLine::Skipped(reason) => {
                *self.tally.skipped.entry(reason).or_default() += 1;
            }
            TranscriptLine::Unreadable(reason) => {
                *self.tally.unreadable.entry(reason).or_default() += 1;
            }
        }
        ControlFlow::Continue(())
    }

    pub fn finish(self) -> ReadTranscript {
        let session_id = self.session_id.unwrap_or(self.fallback_session);
        // The marks are settled here and not as the lines arrive, because both of them are
        // properties of the whole conversation: a file is named whether the owner said it
        // before the agent opened it or after, and a repeat is only known once there are no
        // more lines.
        let replies = self.replies;
        let touches = self
            .touches
            .into_iter()
            .zip(self.times)
            .map(|(touch, times)| {
                let named = owner_named(&replies, touch.path());
                touch.marked(OwnerNaming::of_naming(named), times)
            })
            .collect();
        ReadTranscript {
            conversation: Conversation::new(session_id, self.name, replies, touches),
            tally: self.tally,
            outgrew_the_budget: self.outgrew_the_budget,
        }
    }
}

/// Whether the owner named this path in something they said themselves.
///
/// Only the owner's replies, because the question is what the person asked for; and only
/// replies, because what the harness injected under their role is not theirs — the reader
/// drops those as `Injected` before this ever sees them.
fn owner_named(replies: &[Reply], path: &str) -> bool {
    replies
        .iter()
        .filter(|reply| reply.speaker() == Speaker::Owner)
        .any(|reply| MentionRule::PRODUCT.names(path, reply.text()))
}

/// The owner's name beats the model's wherever both are present; between two names by the
/// same party the later one wins, because that is the one the writer settled on.
fn keeps_the_new_name(held: Option<&ConversationName>, offered: &ConversationName) -> bool {
    match held {
        None => true,
        Some(held) => !matches!(
            (held.named_by(), offered.named_by()),
            (NamedBy::Owner, NamedBy::Model)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation::{TouchSource, TouchVerb};

    fn named(value: &str, by: NamedBy) -> TranscriptLine {
        TranscriptLine::Named(ConversationName::new(value, by).expect("непустое имя"))
    }

    fn touched(
        path: &str,
        verb: TouchVerb,
        source: TouchSource,
        at_unix_ms: Option<i64>,
    ) -> TranscriptLine {
        TranscriptLine::Touched(
            FileTouch::new(path, verb, source, at_unix_ms).expect("непустой путь"),
        )
    }

    fn spoke(speaker: Speaker, text: &str) -> TranscriptLine {
        TranscriptLine::Spoke(Reply::new(speaker, text, Some(1)).expect("непустая реплика"))
    }

    fn assembled(lines: Vec<TranscriptLine>) -> ReadTranscript {
        assembled_within(lines, u64::MAX)
    }

    fn assembled_within(lines: Vec<TranscriptLine>, most_spoken_bytes: u64) -> ReadTranscript {
        let mut assembly = ConversationAssembly::for_session("файл", most_spoken_bytes);
        for line in lines {
            if assembly.absorb(line).is_break() {
                break;
            }
        }
        assembly.finish()
    }

    #[test]
    fn the_last_name_the_model_wrote_is_the_one_it_meant() {
        let read = assembled(vec![
            named("Первое", NamedBy::Model),
            spoke(Speaker::Owner, "что там?"),
            named("Второе", NamedBy::Model),
            named("Третье", NamedBy::Model),
        ]);
        let name = read.conversation().name().expect("имя");
        assert_eq!(name.value(), "Третье");
        assert_eq!(name.named_by(), NamedBy::Model);
        assert_eq!(read.tally().names(), 3);
        assert_eq!(read.tally().renames(), 2);
    }

    #[test]
    fn the_owners_own_name_wins_wherever_both_are_written() {
        let owner_first = assembled(vec![
            named("Беседа Е", NamedBy::Owner),
            named("Implement peer-to-peer debt tracking", NamedBy::Model),
        ]);
        assert_eq!(
            owner_first.conversation().name().expect("имя").value(),
            "Беседа Е"
        );

        let model_first = assembled(vec![
            named("Implement peer-to-peer debt tracking", NamedBy::Model),
            named("Беседа Е", NamedBy::Owner),
        ]);
        assert_eq!(
            model_first.conversation().name().expect("имя").value(),
            "Беседа Е"
        );
    }

    #[test]
    fn a_transcript_with_no_name_line_has_no_name() {
        let read = assembled(vec![spoke(Speaker::Owner, "что там?")]);
        assert!(read.conversation().name().is_none());
        assert_eq!(read.tally().names(), 0);
    }

    #[test]
    fn the_session_comes_from_the_first_line_that_carried_one() {
        let read = assembled(vec![
            TranscriptLine::Session("сессия".to_string()),
            TranscriptLine::Session("другая".to_string()),
        ]);
        assert_eq!(read.conversation().session_id(), "сессия");
    }

    #[test]
    fn a_transcript_whose_lines_carry_no_session_falls_back_to_its_file() {
        let read = assembled(vec![spoke(Speaker::Model, "готово")]);
        assert_eq!(read.conversation().session_id(), "файл");
    }

    #[test]
    fn every_line_is_counted_by_what_it_turned_out_to_be() {
        let read = assembled(vec![
            spoke(Speaker::Owner, "раз"),
            spoke(Speaker::Model, "два"),
            TranscriptLine::Skipped(SkippedLine::Thinking),
            TranscriptLine::Skipped(SkippedLine::Thinking),
            TranscriptLine::Skipped(SkippedLine::ToolResult),
            TranscriptLine::Unreadable(UnreadableLine::NotJson),
        ]);
        let tally = read.tally();
        assert_eq!(tally.observations(), 6);
        assert_eq!(tally.replies(), 2);
        assert_eq!(tally.skipped(SkippedLine::Thinking), 2);
        assert_eq!(tally.skipped(SkippedLine::ToolResult), 1);
        assert_eq!(tally.skipped(SkippedLine::Attachment), 0);
        assert_eq!(tally.unreadable(UnreadableLine::NotJson), 1);
        assert_eq!(tally.unreadable_total(), 1);
    }

    /// A session reads one file forty times. The graph holds one row, and the earliest time
    /// it was touched — a repeat is the same fact, not a new one.
    #[test]
    fn the_same_file_named_again_with_the_same_verb_is_one_touch() {
        let read = assembled(vec![
            touched("/a/b.rs", TouchVerb::Read, TouchSource::ToolInput, Some(10)),
            touched("/a/b.rs", TouchVerb::Read, TouchSource::ToolInput, Some(99)),
            touched(
                "/a/b.rs",
                TouchVerb::Wrote,
                TouchSource::ToolInput,
                Some(20),
            ),
            touched(
                "/a/b.rs",
                TouchVerb::Read,
                TouchSource::FileHistory,
                Some(30),
            ),
            touched("/a/c.rs", TouchVerb::Read, TouchSource::ToolInput, Some(40)),
        ]);
        let touches = read.conversation().touches();
        assert_eq!(
            touches.len(),
            4,
            "повтор пути с тем же глаголом и источником — тот же факт: {touches:?}"
        );
        assert_eq!(touches[0].at_unix_ms(), Some(10), "остаётся первое время");
        assert_eq!(touches[0].times(), 2, "повтор считается на той же строке");
    }

    /// The mark the whole stage is for: the owner asked about this file by name, so a touch
    /// that is only a read is still what they meant.
    #[test]
    fn a_file_the_owner_named_is_marked_named_even_when_it_was_only_read() {
        let read = assembled(vec![
            spoke(Speaker::Owner, "посмотри draft_service.py, там НДС"),
            touched(
                "/work/modules/counterparty/draft_service.py",
                TouchVerb::Read,
                TouchSource::ToolInput,
                Some(10),
            ),
        ]);
        let touches = read.conversation().touches();
        assert_eq!(touches[0].naming(), OwnerNaming::Named);
        assert_eq!(touches[0].times(), 1);
    }

    /// The other half of the same rule: the agent opened a neighbour nobody asked about.
    #[test]
    fn a_file_nobody_named_is_marked_not_named_rather_than_left_unknown() {
        let read = assembled(vec![
            spoke(Speaker::Owner, "посмотри draft_service.py"),
            touched(
                "/work/modules/counterparty/registry.py",
                TouchVerb::Read,
                TouchSource::ToolInput,
                Some(10),
            ),
        ]);
        let touches = read.conversation().touches();
        assert_eq!(touches[0].naming(), OwnerNaming::NotNamed);
    }

    /// Only the owner's own words count. What the model said is not the owner asking, and a
    /// file the model announced it was opening is exactly the touch the mark exists to sort
    /// out.
    #[test]
    fn the_model_naming_a_file_is_not_the_owner_naming_it() {
        let read = assembled(vec![
            spoke(Speaker::Owner, "разберись с расчётом"),
            spoke(Speaker::Model, "Смотрю draft_service.py."),
            touched(
                "/work/draft_service.py",
                TouchVerb::Read,
                TouchSource::ToolInput,
                Some(10),
            ),
        ]);
        assert_eq!(
            read.conversation().touches()[0].naming(),
            OwnerNaming::NotNamed
        );
    }

    /// The owner names the file after the agent has already opened it. The mark is a property
    /// of the conversation and not of the order its lines arrived in.
    #[test]
    fn naming_a_file_later_in_the_conversation_still_names_it() {
        let read = assembled(vec![
            touched(
                "/work/draft_service.py",
                TouchVerb::Read,
                TouchSource::ToolInput,
                Some(10),
            ),
            spoke(Speaker::Owner, "да, именно draft_service.py"),
        ]);
        assert_eq!(
            read.conversation().touches()[0].naming(),
            OwnerNaming::Named
        );
    }

    /// The fix the budget exists for: reading stops **when** the ceiling is reached, not
    /// after everything has been held and weighed. A transcript whose speech runs past the
    /// budget never has more than the budget plus the one reply being weighed in memory.
    #[test]
    fn reading_stops_at_the_budget_instead_of_holding_everything_and_refusing() {
        let long = "я".repeat(100); // 200 bytes apiece
        let lines: Vec<TranscriptLine> = (0..50).map(|_| spoke(Speaker::Owner, &long)).collect();

        let read = assembled_within(lines, 500);
        assert!(read.outgrew_the_budget(), "должно было упереться в предел");
        assert_eq!(
            read.conversation().replies().len(),
            2,
            "удержано ровно столько реплик, сколько влезло: две по 200 байт"
        );
        assert!(
            read.conversation().spoken_bytes() <= 500,
            "удержанное не превышает бюджет: {}",
            read.conversation().spoken_bytes()
        );
    }

    /// A transcript that fits is not marked, and nothing about it changes.
    #[test]
    fn a_transcript_inside_the_budget_is_read_whole() {
        let read = assembled_within(
            vec![spoke(Speaker::Owner, "раз"), spoke(Speaker::Model, "два")],
            1_000,
        );
        assert!(!read.outgrew_the_budget());
        assert_eq!(read.conversation().replies().len(), 2);
    }

    #[test]
    fn a_corpus_is_tallied_the_same_way_one_transcript_is() {
        let one = assembled(vec![
            spoke(Speaker::Owner, "раз"),
            TranscriptLine::Unreadable(UnreadableLine::NotJson),
        ]);
        let other = assembled(vec![
            spoke(Speaker::Model, "два"),
            TranscriptLine::Unreadable(UnreadableLine::NotJson),
            TranscriptLine::Unreadable(UnreadableLine::Untyped),
        ]);
        let mut corpus = TranscriptTally::default();
        corpus.absorb(one.tally());
        corpus.absorb(other.tally());
        assert_eq!(corpus.observations(), 5);
        assert_eq!(corpus.replies(), 2);
        assert_eq!(corpus.unreadable(UnreadableLine::NotJson), 2);
        assert_eq!(corpus.unreadable(UnreadableLine::Untyped), 1);
        assert_eq!(corpus.unreadable_total(), 3);
    }
}
