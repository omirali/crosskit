// Use case: which documents of a context have an event inside a period.
//
// The period is decided by this program; what falls inside it is decided by the graph, and
// only by facts that carry their own moment. Two such facts exist and nothing else counts:
//
//   * a reply of a conversation, said at the moment its transcript recorded;
//   * a touch of a file by a conversation, at the moment of the tool call.
//
// When the indexer saw a revision is not one of them. A context reindexed on the first of
// October would otherwise make its whole tree "this week", which is the error this stage
// exists to remove rather than a smaller version of it.
//
// The answer is a scope: the files every chunk of which belongs to the period (a file a
// conversation wrote in it) and the chunks that belong to it one by one (replies of a
// conversation said in it). The search then ranks inside the scope — candidate Б of ADR 0043,
// chosen by measurement over filtering the page afterwards and over a moment in the index.
use std::collections::{BTreeMap, HashMap};

use crate::domain::conversation::TouchVerb;
use crate::domain::{ChunkMoment, NormalizedPath, OwnerNaming, Period, Unplaceable, chunk_moments};

use super::repository::RepositoryError;

/// A conversation of the current version of a transcript that said something in a period,
/// with everything needed to place its chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTimeline {
    file_id: String,
    named: bool,
    reply_moments: Vec<Option<i64>>,
    chunks: Vec<(u32, String)>,
}

impl ConversationTimeline {
    /// `reply_moments` by reply ordinal; `chunks` as ordinal and id, in any order.
    pub fn new(
        file_id: impl Into<String>,
        named: bool,
        reply_moments: Vec<Option<i64>>,
        chunks: Vec<(u32, String)>,
    ) -> Self {
        Self {
            file_id: file_id.into(),
            named,
            reply_moments,
            chunks,
        }
    }
}

/// What placing one conversation's chunks met, in numbers: Stage 40's census.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TimelineCensus {
    pub(crate) chunks: usize,
    pub(crate) replies: usize,
    pub(crate) replies_without_moment: usize,
    pub(crate) unplaced: BTreeMap<Unplaceable, usize>,
}

#[cfg(test)]
impl ConversationTimeline {
    pub(crate) fn census(&self) -> TimelineCensus {
        let mut unplaced = BTreeMap::new();
        for (_, moment) in moments_of(self) {
            if let ChunkMoment::NotKnown(why) = moment {
                *unplaced.entry(why).or_default() += 1;
            }
        }
        TimelineCensus {
            chunks: self.chunks.len(),
            replies: self.reply_moments.len(),
            replies_without_moment: self.reply_moments.iter().filter(|at| at.is_none()).count(),
            unplaced,
        }
    }
}

/// A touch inside a period, of a file this context holds, by a conversation of a current
/// transcript revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodTouch {
    file_id: String,
    at_unix_ms: i64,
    verb: TouchVerb,
    naming: OwnerNaming,
}

impl PeriodTouch {
    pub fn new(
        file_id: impl Into<String>,
        at_unix_ms: i64,
        verb: TouchVerb,
        naming: OwnerNaming,
    ) -> Self {
        Self {
            file_id: file_id.into(),
            at_unix_ms,
            verb,
            naming,
        }
    }

    /// The strict reading of ADR 0041, the same one `touches` answers with: written, edited,
    /// or named by the owner. A file the agent opened on its way to another one was not worked
    /// on, and "what did I work on" must not be answered with it.
    fn is_deliberate(&self) -> bool {
        self.naming == OwnerNaming::Named || self.verb.is_deliberate()
    }
}

/// Reading the events of a period out of one context's graph.
pub trait PeriodEventReader {
    /// Every conversation of a current transcript revision with at least one reply said in
    /// `[start, end)`.
    fn conversations_speaking_between(
        &self,
        start_unix_ms: i64,
        end_unix_ms: i64,
    ) -> Result<Vec<ConversationTimeline>, RepositoryError>;

    /// Every touch made in `[start, end)` by a conversation of a current transcript revision,
    /// of a file at a path this context holds.
    fn touches_between(
        &self,
        start_unix_ms: i64,
        end_unix_ms: i64,
    ) -> Result<Vec<PeriodTouch>, RepositoryError>;

    /// The paths of transcripts a pass saw written at or after `start` and could not read: the
    /// graph's copy is older than the file the pass found, or there is none — the conversation
    /// outgrew the content limit, or changed while it was read. What they said after the
    /// graph's copy is in no index, and a period has to say so. A file written since the last
    /// pass is not among them: the graph has not seen that write, and claims nothing about it.
    fn transcripts_behind_since(&self, start_unix_ms: i64) -> Result<Vec<String>, RepositoryError>;
}

/// What a period could not show, and why. Never a reason to show something from outside it:
/// the point is that the person is told, instead of reading a short answer as a whole one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeriodGaps {
    /// Transcripts written during or after the period whose newer part the graph does not hold.
    behind_transcripts: usize,
    /// Conversations that said something in the period with chunks that could not be placed
    /// in time, and those chunks by reason.
    unplaced_conversations: usize,
    unplaced_chunks: BTreeMap<Unplaceable, usize>,
}

impl PeriodGaps {
    pub fn is_empty(&self) -> bool {
        self.behind_transcripts == 0 && self.unplaced_conversations == 0
    }

    /// Transcripts written during or after the period whose newer part the graph lacks.
    pub fn behind_transcripts(&self) -> usize {
        self.behind_transcripts
    }

    /// Conversations that spoke in the period with chunks that could not be placed in it.
    pub fn unplaced_conversations(&self) -> usize {
        self.unplaced_conversations
    }

    /// One sentence for both the person and the model, or nothing when nothing is missing.
    /// Built here once, so the terminal and the model are never told two different things.
    pub fn sentence(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if self.behind_transcripts > 0 {
            parts.push(format!(
                "{} conversation(s) written to during or after this period whose newer part is \
                 not in the index — the last pass could not read it (larger than the content \
                 limit, or changed while read)",
                self.behind_transcripts
            ));
        }
        if self.unplaced_conversations > 0 {
            let reasons: Vec<String> = self
                .unplaced_chunks
                .iter()
                .map(|(why, chunks)| {
                    let why = match why {
                        Unplaceable::ReplyWithoutMoment => "a reply without a recorded moment",
                        Unplaceable::CountsDoNotFit => "chunks and replies that do not line up",
                    };
                    format!("{chunks} chunk(s): {why}")
                })
                .collect();
            parts.push(format!(
                "{} conversation(s) that spoke in this period with chunks that could not be \
                 placed in time ({})",
                self.unplaced_conversations,
                reasons.join("; ")
            ));
        }
        Some(format!("Not shown: {}.", parts.join("; ")))
    }
}

/// The event that put a document inside a period, as a source line prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodEvent {
    /// A reply was said. For a chunk whose reply is known only to a span, the latest moment.
    Reply { at_unix_ms: i64 },
    /// A conversation touched the file.
    Touch { at_unix_ms: i64, verb: TouchVerb },
}

impl PeriodEvent {
    pub fn at_unix_ms(self) -> i64 {
        match self {
            Self::Reply { at_unix_ms } | Self::Touch { at_unix_ms, .. } => at_unix_ms,
        }
    }

    /// The later of two events: a document is shown with the last time it was worked on.
    fn later(self, other: Self) -> Self {
        if other.at_unix_ms() > self.at_unix_ms() {
            other
        } else {
            self
        }
    }
}

/// What a period admits: whole files and single chunks, each with the event that admitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodScope {
    period: Period,
    files: HashMap<String, PeriodEvent>,
    chunks: HashMap<String, PeriodEvent>,
    files_with_events: usize,
    gaps: PeriodGaps,
}

impl PeriodScope {
    /// Reads the events of `period` and turns them into a scope. `is_visible` is the run's own
    /// rule for which paths it may name: a transcript the policy hides is not counted as a gap
    /// either, or the count would describe files this run is not allowed to talk about.
    pub fn gather(
        reader: &dyn PeriodEventReader,
        period: Period,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<Self, RepositoryError> {
        let (start, end) = (period.start_unix_ms(), period.end_unix_ms());
        let mut files: HashMap<String, PeriodEvent> = HashMap::new();
        let mut chunks: HashMap<String, PeriodEvent> = HashMap::new();
        let mut speaking_files: Vec<String> = Vec::new();
        let mut gaps = PeriodGaps::default();

        for conversation in reader.conversations_speaking_between(start, end)? {
            let placed = place(&conversation, &period);
            if !placed.unplaced.is_empty() {
                gaps.unplaced_conversations += 1;
                for (why, count) in placed.unplaced {
                    *gaps.unplaced_chunks.entry(why).or_default() += count;
                }
            }
            let admitted = placed.admitted;
            if admitted.is_empty() {
                continue;
            }
            speaking_files.push(conversation.file_id.clone());
            for (chunk_id, event) in admitted {
                insert_later(&mut chunks, chunk_id, event);
            }
        }
        for touch in reader.touches_between(start, end)? {
            if !touch.is_deliberate() || !period.contains(touch.at_unix_ms) {
                continue;
            }
            let event = PeriodEvent::Touch {
                at_unix_ms: touch.at_unix_ms,
                verb: touch.verb,
            };
            insert_later(&mut files, touch.file_id, event);
        }

        gaps.behind_transcripts = reader
            .transcripts_behind_since(start)?
            .iter()
            .filter(|path| NormalizedPath::from_absolute(path).is_ok_and(|path| is_visible(&path)))
            .count();

        speaking_files.retain(|file_id| !files.contains_key(file_id));
        speaking_files.sort();
        speaking_files.dedup();
        let files_with_events = files.len() + speaking_files.len();
        Ok(Self {
            period,
            files,
            chunks,
            files_with_events,
            gaps,
        })
    }

    /// What the period could not show.
    pub fn gaps(&self) -> &PeriodGaps {
        &self.gaps
    }

    pub fn period(&self) -> &Period {
        &self.period
    }

    /// Whether a chunk of a file belongs to the period: every chunk of a file touched in it,
    /// and the chunks of conversations said in it. The index asks this by ordinals of its own
    /// columns (`whole_files`, `single_chunks`); this form is for indexes written by tests.
    #[cfg(test)]
    pub fn admits(&self, chunk_id: &str, file_id: &str) -> bool {
        self.files.contains_key(file_id) || self.chunks.contains_key(chunk_id)
    }

    /// The event that put this chunk inside, when it is inside.
    pub fn event_of(&self, chunk_id: &str, file_id: &str) -> Option<PeriodEvent> {
        match (self.chunks.get(chunk_id), self.files.get(file_id)) {
            (Some(chunk), Some(file)) => Some(chunk.later(*file)),
            (Some(event), None) | (None, Some(event)) => Some(*event),
            (None, None) => None,
        }
    }

    /// Files every chunk of which the period admits.
    pub fn whole_files(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// Chunks the period admits one by one.
    pub fn single_chunks(&self) -> impl Iterator<Item = &str> {
        self.chunks.keys().map(String::as_str)
    }

    /// How many files have any event in the period at all, whether or not a query matches
    /// them: what tells "nothing happened then" from "nothing that happened matched".
    pub fn files_with_events(&self) -> usize {
        self.files_with_events
    }

    /// A scope admitting these chunks one by one, each by a reply at the moment given: for
    /// tests of the layers that only read a scope, so they need not stage a graph's events.
    #[cfg(test)]
    pub(crate) fn admitting_chunks(period: Period, chunks: &[(&str, i64)]) -> Self {
        Self {
            period,
            files: HashMap::new(),
            chunks: chunks
                .iter()
                .map(|(id, at_unix_ms)| {
                    (
                        (*id).to_string(),
                        PeriodEvent::Reply {
                            at_unix_ms: *at_unix_ms,
                        },
                    )
                })
                .collect(),
            files_with_events: chunks.len(),
            gaps: PeriodGaps::default(),
        }
    }
}

/// Each chunk of one conversation, by ordinal, with what the graph can say about its moment.
///
/// The rule counts chunks by position; an ordinal missing from the middle would move every
/// chunk after it onto the wrong reply, so such a conversation places nothing and says why.
fn moments_of(conversation: &ConversationTimeline) -> Vec<(&str, ChunkMoment)> {
    let mut by_ordinal: Vec<(u32, &str)> = conversation
        .chunks
        .iter()
        .map(|(ordinal, id)| (*ordinal, id.as_str()))
        .collect();
    by_ordinal.sort_unstable();
    let lined_up = by_ordinal
        .iter()
        .enumerate()
        .all(|(position, (ordinal, _))| *ordinal as usize == position);
    let moments = if lined_up {
        chunk_moments(
            conversation.named,
            by_ordinal.len(),
            &conversation.reply_moments,
        )
    } else {
        vec![ChunkMoment::NotKnown(Unplaceable::CountsDoNotFit); by_ordinal.len()]
    };
    by_ordinal
        .into_iter()
        .map(|(_, id)| id)
        .zip(moments)
        .collect()
}

/// What placing one conversation in a period gave: the chunks it admits, with the reply that
/// admitted each, and the chunks nothing could be said about, by reason.
struct Placed {
    admitted: Vec<(String, PeriodEvent)>,
    unplaced: BTreeMap<Unplaceable, usize>,
}

/// A reply chunk is admitted when every reply it could have come from was said inside the
/// period (`domain::reply_chunk`). The conversation's name is admitted with the conversation:
/// it is what lets a question find the conversation by what it is called, and a period that
/// dropped it would rank the conversation as if it had no name.
fn place(conversation: &ConversationTimeline, period: &Period) -> Placed {
    let moments = moments_of(conversation);
    let mut admitted = Vec::new();
    let mut unplaced: BTreeMap<Unplaceable, usize> = BTreeMap::new();
    let mut latest_inside: Option<i64> = None;
    for (chunk_id, moment) in &moments {
        match *moment {
            ChunkMoment::Said { earliest, latest }
                if period.contains(earliest) && period.contains(latest) =>
            {
                latest_inside = latest_inside.max(Some(latest));
                admitted.push((
                    chunk_id.to_string(),
                    PeriodEvent::Reply { at_unix_ms: latest },
                ));
            }
            ChunkMoment::NotKnown(why) => *unplaced.entry(why).or_default() += 1,
            ChunkMoment::Said { .. } | ChunkMoment::Name => {}
        }
    }
    if let (Some(at_unix_ms), Some((name, ChunkMoment::Name))) = (latest_inside, moments.first()) {
        admitted.push((name.to_string(), PeriodEvent::Reply { at_unix_ms }));
    }
    Placed { admitted, unplaced }
}

fn insert_later(into: &mut HashMap<String, PeriodEvent>, key: String, event: PeriodEvent) {
    into.entry(key)
        .and_modify(|held| *held = held.later(event))
        .or_insert(event);
}

#[cfg(test)]
mod tests {
    use super::{ConversationTimeline, PeriodEvent, PeriodEventReader, PeriodScope, PeriodTouch};
    use crate::application::RepositoryError;
    use crate::domain::conversation::TouchVerb;
    use crate::domain::{CivilDate, LocalDay, OwnerNaming, Period, PeriodPhrase};

    /// Last week of Thursday 1 October 2026, at UTC+05:00: 21–27 September.
    fn last_week() -> Period {
        Period::of_phrase(
            PeriodPhrase::LastWeek,
            LocalDay::new(CivilDate::new(2026, 10, 1).unwrap(), 5 * 3600),
        )
    }

    /// Noon, local, on a day of September 2026.
    fn noon_of_september(day: u8) -> i64 {
        let days = CivilDate::new(2026, 9, day).unwrap().days();
        (days * 86_400 + 7 * 3600) * 1000
    }

    struct Events {
        conversations: Vec<ConversationTimeline>,
        touches: Vec<PeriodTouch>,
        behind: Vec<String>,
    }

    impl PeriodEventReader for Events {
        fn conversations_speaking_between(
            &self,
            _start: i64,
            _end: i64,
        ) -> Result<Vec<ConversationTimeline>, RepositoryError> {
            Ok(self.conversations.clone())
        }

        fn touches_between(
            &self,
            _start: i64,
            _end: i64,
        ) -> Result<Vec<PeriodTouch>, RepositoryError> {
            Ok(self.touches.clone())
        }

        fn transcripts_behind_since(&self, _start: i64) -> Result<Vec<String>, RepositoryError> {
            Ok(self.behind.clone())
        }
    }

    fn chunks(ids: &[&str]) -> Vec<(u32, String)> {
        ids.iter()
            .enumerate()
            .map(|(ordinal, id)| (ordinal as u32, (*id).to_string()))
            .collect()
    }

    /// A conversation that ran over three weeks gives the period only the replies said in
    /// it, and its name with them.
    #[test]
    fn only_the_replies_said_inside_the_period_are_admitted() {
        let events = Events {
            conversations: vec![ConversationTimeline::new(
                "file:chat",
                true,
                vec![
                    Some(noon_of_september(15)),
                    Some(noon_of_september(23)),
                    Some(noon_of_september(29)),
                ],
                chunks(&["c:name", "c:15", "c:23", "c:29"]),
            )],
            touches: vec![],
            behind: vec![],
        };
        let scope = PeriodScope::gather(&events, last_week(), &|_| true).unwrap();

        assert!(scope.admits("c:23", "file:chat"));
        assert!(scope.admits("c:name", "file:chat"));
        assert!(!scope.admits("c:15", "file:chat"));
        assert!(!scope.admits("c:29", "file:chat"));
        assert_eq!(
            scope.event_of("c:name", "file:chat"),
            Some(PeriodEvent::Reply {
                at_unix_ms: noon_of_september(23)
            })
        );
        assert_eq!(scope.files_with_events(), 1);
    }

    /// A file a conversation wrote inside the period is admitted whole; one it only read is
    /// not, and neither is a touch from outside the period.
    #[test]
    fn a_file_is_admitted_by_a_deliberate_touch_inside_the_period() {
        let events = Events {
            conversations: vec![],
            touches: vec![
                PeriodTouch::new(
                    "file:edited",
                    noon_of_september(24),
                    TouchVerb::Edited,
                    OwnerNaming::NotNamed,
                ),
                PeriodTouch::new(
                    "file:glanced",
                    noon_of_september(24),
                    TouchVerb::Read,
                    OwnerNaming::NotNamed,
                ),
                PeriodTouch::new(
                    "file:named",
                    noon_of_september(25),
                    TouchVerb::Read,
                    OwnerNaming::Named,
                ),
                PeriodTouch::new(
                    "file:earlier",
                    noon_of_september(2),
                    TouchVerb::Wrote,
                    OwnerNaming::NotNamed,
                ),
            ],
            behind: vec![],
        };
        let scope = PeriodScope::gather(&events, last_week(), &|_| true).unwrap();

        assert!(scope.admits("any chunk", "file:edited"));
        assert!(scope.admits("any chunk", "file:named"));
        assert!(!scope.admits("any chunk", "file:glanced"));
        assert!(!scope.admits("any chunk", "file:earlier"));
        assert_eq!(
            scope.event_of("any chunk", "file:edited"),
            Some(PeriodEvent::Touch {
                at_unix_ms: noon_of_september(24),
                verb: TouchVerb::Edited
            })
        );
        assert_eq!(scope.files_with_events(), 2);
    }

    /// A gap in the ordinals would put every later chunk on the wrong reply; nothing of such a
    /// conversation is placed.
    #[test]
    fn a_conversation_whose_chunks_do_not_line_up_places_nothing() {
        let events = Events {
            conversations: vec![ConversationTimeline::new(
                "file:chat",
                false,
                vec![Some(noon_of_september(23)), Some(noon_of_september(24))],
                vec![(0, "c:0".to_string()), (2, "c:2".to_string())],
            )],
            touches: vec![],
            behind: vec![],
        };
        let scope = PeriodScope::gather(&events, last_week(), &|_| true).unwrap();
        assert!(!scope.admits("c:0", "file:chat"));
        assert_eq!(scope.files_with_events(), 0);
    }

    /// A conversation that spoke in the period with replies the transcript gave no moment for:
    /// those chunks are not shown, and the scope says how many and why.
    #[test]
    fn chunks_that_cannot_be_placed_are_counted_with_their_reason() {
        let events = Events {
            conversations: vec![ConversationTimeline::new(
                "file:chat",
                false,
                vec![
                    Some(noon_of_september(23)),
                    None,
                    Some(noon_of_september(24)),
                ],
                chunks(&["c:23", "c:none", "c:24"]),
            )],
            touches: vec![],
            behind: vec![],
        };
        let scope = PeriodScope::gather(&events, last_week(), &|_| true).unwrap();

        assert!(scope.admits("c:23", "file:chat") && scope.admits("c:24", "file:chat"));
        assert!(!scope.admits("c:none", "file:chat"));
        assert_eq!(
            scope.gaps().sentence().as_deref(),
            Some(
                "Not shown: 1 conversation(s) that spoke in this period with chunks that could \
                 not be placed in time (1 chunk(s): a reply without a recorded moment)."
            )
        );
    }

    /// Nothing missing, nothing said: the ordinary period prints no extra line.
    #[test]
    fn a_period_with_nothing_missing_has_no_sentence() {
        let events = Events {
            conversations: vec![],
            touches: vec![],
            behind: vec![],
        };
        let scope = PeriodScope::gather(&events, last_week(), &|_| true).unwrap();
        assert_eq!(scope.gaps().sentence(), None);
    }
}
