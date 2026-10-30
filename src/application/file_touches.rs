// Use case: where a file was worked on, and what a conversation worked on.
//
// Two directions of one fact. The graph holds every file a conversation touched — including
// the forty an agent opened while looking for one — and the strictness lives here, not in the
// graph: what is written is everything, what is answered is what somebody meant. ADR 0041.
//
// A conversation is identified by its session and not by its row. A transcript is appended
// to, so the same session is stored again at every revision that saw it grow, and a reader
// that counted rows would answer "three conversations" about one. Pairs of session and path
// are folded, and the count of times takes the largest rather than the sum: the later
// revision saw everything the earlier one did.
use std::collections::BTreeMap;

use crate::domain::conversation::TouchVerb;
use crate::domain::{OwnerNaming, path_ends_with_segment};

use super::repository::RepositoryError;

/// One touch as the graph holds it, with the conversation that made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTouch {
    session_id: String,
    conversation_name: String,
    path: String,
    verb: TouchVerb,
    naming: OwnerNaming,
    times: u32,
    at_unix_ms: Option<i64>,
}

impl StoredTouch {
    pub fn new(
        session_id: impl Into<String>,
        conversation_name: impl Into<String>,
        path: impl Into<String>,
        verb: TouchVerb,
        naming: OwnerNaming,
        times: u32,
        at_unix_ms: Option<i64>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            conversation_name: conversation_name.into(),
            path: path.into(),
            verb,
            naming,
            times,
            at_unix_ms,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Where the touches of one query come from.
///
/// Both methods narrow in the database and neither decides anything: which of the rows they
/// return belong to the answer is settled here, by rules that have to be readable.
pub trait TouchReader {
    /// Every touch whose path ends with `needle`, by string. The segment rule is applied
    /// afterwards, so a needle that ends inside a longer name costs a row rather than an
    /// answer.
    fn touches_ending_with(&self, needle: &str) -> Result<Vec<StoredTouch>, RepositoryError>;

    /// Every touch of the conversations whose session or name answers to `needle`.
    fn touches_of_conversations_matching(
        &self,
        needle: &str,
    ) -> Result<Vec<StoredTouch>, RepositoryError>;
}

/// One conversation and one file, folded from every touch row of that pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchedPair {
    session_id: String,
    conversation_name: String,
    path: String,
    /// One entry per verb, with how many times that verb happened. Kept per verb because the
    /// two folds are different: the same verb met again in a later revision of the transcript
    /// is the same count over again, and a different verb is a touch of its own.
    verbs: Vec<(TouchVerb, u32)>,
    naming: OwnerNaming,
    at_unix_ms: Option<i64>,
}

impl TouchedPair {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The name of the conversation, or an empty string when nobody named it.
    pub fn conversation_name(&self) -> &str {
        &self.conversation_name
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn verbs(&self) -> impl Iterator<Item = TouchVerb> + '_ {
        self.verbs.iter().map(|(verb, _)| *verb)
    }

    pub fn naming(&self) -> OwnerNaming {
        self.naming
    }

    /// How many times this conversation touched this file, over every verb, or zero when the
    /// rows were written before the count existed. Zero is not "never": it is the same
    /// "nobody looked" that `OwnerNaming::NotComputed` says about the other mark.
    pub fn times(&self) -> u32 {
        self.verbs.iter().map(|(_, times)| *times).sum()
    }

    pub fn at_unix_ms(&self) -> Option<i64> {
        self.at_unix_ms
    }

    /// Whether this pair belongs in an answer nobody asked to widen.
    ///
    /// Written, edited, or named by the owner. Everything else is a file the agent opened on
    /// its way to something — a fact worth keeping and not worth answering with.
    pub fn is_deliberate(&self) -> bool {
        self.naming == OwnerNaming::Named || self.verbs.iter().any(|(verb, _)| verb.is_deliberate())
    }

    /// Whether this pair was left out only because nobody ever looked at it. A row written
    /// before the marks existed is not a row that failed the rule.
    pub fn is_unmarked(&self) -> bool {
        self.naming == OwnerNaming::NotComputed
    }
}

/// What was asked and of which side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// A file, answered with the conversations that touched it.
    WhichConversations,
    /// A conversation, answered with the files it touched.
    WhichFiles,
}

/// One question about touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchQuery {
    needle: String,
    direction: Direction,
    include_in_passing: bool,
}

impl TouchQuery {
    /// A query, or a refusal when there is nothing to look for. An empty needle would match
    /// every path in the graph, which is not a question anybody asked.
    pub fn new(
        needle: &str,
        direction: Direction,
        include_in_passing: bool,
    ) -> Result<Self, TouchQueryError> {
        let needle = needle.trim();
        if needle.is_empty() {
            return Err(TouchQueryError::NothingToLookFor);
        }
        Ok(Self {
            needle: needle.to_string(),
            direction,
            include_in_passing,
        })
    }

    pub fn needle(&self) -> &str {
        &self.needle
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchQueryError {
    NothingToLookFor,
}

impl std::fmt::Display for TouchQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingToLookFor => formatter.write_str(
                "a file or a conversation has to be named; there is nothing to look for",
            ),
        }
    }
}

/// One heading of an answer and what stands under it.
///
/// A group is a path when the question was about a file, and a conversation when it was about
/// a conversation. There is more than one group only when what was asked matched more than
/// one thing, and that is the case the answer has to name rather than resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchGroup {
    heading: String,
    pairs: Vec<TouchedPair>,
    in_passing: usize,
    unmarked_in_passing: usize,
}

impl TouchGroup {
    pub fn heading(&self) -> &str {
        &self.heading
    }

    pub fn pairs(&self) -> &[TouchedPair] {
        &self.pairs
    }

    /// How many pairs this group holds that the answer did not show: read, never named.
    pub fn in_passing(&self) -> usize {
        self.in_passing
    }

    /// Of those, how many were left out with nothing measured about them, because they were
    /// written before the marks existed.
    pub fn unmarked_in_passing(&self) -> usize {
        self.unmarked_in_passing
    }
}

/// The answer to one question about touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchReport {
    direction: Direction,
    groups: Vec<TouchGroup>,
    widened: bool,
}

impl TouchReport {
    pub fn direction(&self) -> Direction {
        self.direction
    }

    pub fn groups(&self) -> &[TouchGroup] {
        &self.groups
    }

    /// Whether the caller asked for the pairs a strict answer leaves out.
    pub fn widened(&self) -> bool {
        self.widened
    }

    /// Whether what was asked named more than one thing. The answer shows all of them; this
    /// is what makes the ambiguity something the reader is told rather than something the
    /// program decided quietly.
    pub fn is_ambiguous(&self) -> bool {
        self.groups.len() > 1
    }

    pub fn in_passing(&self) -> usize {
        self.groups.iter().map(TouchGroup::in_passing).sum()
    }

    pub fn unmarked_in_passing(&self) -> usize {
        self.groups
            .iter()
            .map(TouchGroup::unmarked_in_passing)
            .sum()
    }
}

pub struct TouchService<'a, Reader> {
    reader: &'a Reader,
}

impl<'a, Reader> TouchService<'a, Reader>
where
    Reader: TouchReader,
{
    pub fn new(reader: &'a Reader) -> Self {
        Self { reader }
    }

    pub fn execute(&self, query: &TouchQuery) -> Result<TouchReport, RepositoryError> {
        let found = match query.direction {
            Direction::WhichConversations => {
                let rows = self.reader.touches_ending_with(query.needle())?;
                rows.into_iter()
                    .filter(|touch| path_ends_with_segment(touch.path(), query.needle()))
                    .collect()
            }
            Direction::WhichFiles => self
                .reader
                .touches_of_conversations_matching(query.needle())?,
        };
        Ok(report(fold(found), query))
    }
}

/// Folds touch rows into pairs of one conversation and one file. The graph view folds with
/// it too, so a pair means the same thing in both answers.
pub(super) fn fold(rows: Vec<StoredTouch>) -> Vec<TouchedPair> {
    let mut folded: BTreeMap<(String, String), TouchedPair> = BTreeMap::new();
    for row in rows {
        let key = (row.session_id.clone(), row.path.clone());
        match folded.get_mut(&key) {
            None => {
                folded.insert(
                    key,
                    TouchedPair {
                        session_id: row.session_id,
                        conversation_name: row.conversation_name,
                        path: row.path,
                        verbs: vec![(row.verb, row.times)],
                        naming: row.naming,
                        at_unix_ms: row.at_unix_ms,
                    },
                );
            }
            Some(held) => {
                match held.verbs.iter_mut().find(|(verb, _)| *verb == row.verb) {
                    // The largest and not the sum: the same session stored again at a later
                    // revision counted the same touches over, from the beginning of the
                    // transcript.
                    Some((_, times)) => *times = (*times).max(row.times),
                    None => held.verbs.push((row.verb, row.times)),
                }
                held.naming = stronger(held.naming, row.naming);
                held.at_unix_ms = earliest(held.at_unix_ms, row.at_unix_ms);
                if held.conversation_name.is_empty() {
                    held.conversation_name = row.conversation_name;
                }
            }
        }
    }
    let mut pairs: Vec<TouchedPair> = folded.into_values().collect();
    for pair in &mut pairs {
        pair.verbs.sort_unstable_by_key(|(verb, _)| *verb);
    }
    pairs
}

impl TouchedPair {
    /// The same conversation's touches of one file written under a second spelling of its
    /// path — relative once, whole once. Those are different touch rows, not one row seen at
    /// two revisions, so the counts of one verb add up rather than take the larger.
    pub(super) fn absorb(&mut self, other: TouchedPair) {
        for (verb, times) in other.verbs {
            match self.verbs.iter_mut().find(|(held, _)| *held == verb) {
                Some((_, held)) => *held += times,
                None => self.verbs.push((verb, times)),
            }
        }
        self.verbs.sort_unstable_by_key(|(verb, _)| *verb);
        self.naming = stronger(self.naming, other.naming);
        self.at_unix_ms = earliest(self.at_unix_ms, other.at_unix_ms);
    }
}

/// What two rows of one pair say together. One row having found the owner naming the file
/// settles it; a row nobody looked at never overrules a row somebody did.
fn stronger(held: OwnerNaming, offered: OwnerNaming) -> OwnerNaming {
    match (held, offered) {
        (OwnerNaming::Named, _) | (_, OwnerNaming::Named) => OwnerNaming::Named,
        (OwnerNaming::NotNamed, _) | (_, OwnerNaming::NotNamed) => OwnerNaming::NotNamed,
        _ => OwnerNaming::NotComputed,
    }
}

fn earliest(held: Option<i64>, offered: Option<i64>) -> Option<i64> {
    match (held, offered) {
        (Some(held), Some(offered)) => Some(held.min(offered)),
        (held, offered) => held.or(offered),
    }
}

/// Groups the pairs under the side that was not asked about, and applies the strictness.
fn report(pairs: Vec<TouchedPair>, query: &TouchQuery) -> TouchReport {
    let mut groups: BTreeMap<String, TouchGroup> = BTreeMap::new();
    for pair in pairs {
        // Grouped by what is unique and headed by what can be read: two conversations can
        // carry one name, and a session is what tells them apart.
        let key = match query.direction {
            Direction::WhichConversations => pair.path.clone(),
            Direction::WhichFiles => pair.session_id.clone(),
        };
        let heading = match query.direction {
            Direction::WhichConversations => pair.path.clone(),
            Direction::WhichFiles => match pair.conversation_name.trim().is_empty() {
                true => format!("(unnamed) session {}", pair.session_id),
                false => format!("{} — session {}", pair.conversation_name, pair.session_id),
            },
        };
        let group = groups.entry(key).or_insert_with(|| TouchGroup {
            heading,
            pairs: Vec::new(),
            in_passing: 0,
            unmarked_in_passing: 0,
        });
        if pair.is_deliberate() || query.include_in_passing {
            group.pairs.push(pair);
            continue;
        }
        group.in_passing += 1;
        if pair.is_unmarked() {
            group.unmarked_in_passing += 1;
        }
    }
    let mut groups: Vec<TouchGroup> = groups.into_values().collect();
    for group in &mut groups {
        // The most worked on first, and the path or session second so that two runs of the
        // same question print the same page.
        group.pairs.sort_by(|left, right| {
            right
                .is_deliberate()
                .cmp(&left.is_deliberate())
                .then(right.times().cmp(&left.times()))
                .then(left.path.cmp(&right.path))
                .then(left.session_id.cmp(&right.session_id))
        });
    }
    // A group nothing is left in is still a group: it is how a reader learns that the file is
    // known and was only ever glanced at.
    groups.retain(|group| !group.pairs.is_empty() || group.in_passing > 0);
    TouchReport {
        direction: query.direction,
        groups,
        widened: query.include_in_passing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(session: &str, path: &str, verb: TouchVerb, naming: OwnerNaming) -> StoredTouch {
        StoredTouch::new(session, "беседа", path, verb, naming, 1, Some(10))
    }

    struct Rows(Vec<StoredTouch>);

    impl TouchReader for Rows {
        fn touches_ending_with(&self, _needle: &str) -> Result<Vec<StoredTouch>, RepositoryError> {
            Ok(self.0.clone())
        }

        fn touches_of_conversations_matching(
            &self,
            _needle: &str,
        ) -> Result<Vec<StoredTouch>, RepositoryError> {
            Ok(self.0.clone())
        }
    }

    fn answered(rows: Vec<StoredTouch>, needle: &str, include_in_passing: bool) -> TouchReport {
        let reader = Rows(rows);
        let query = TouchQuery::new(needle, Direction::WhichConversations, include_in_passing)
            .expect("непустой запрос");
        TouchService::new(&reader).execute(&query).expect("ответ")
    }

    /// The whole rule of the default answer, in one test: read and never named is left out
    /// and counted; written, edited or named is shown.
    #[test]
    fn the_default_answer_holds_nothing_that_was_only_read() {
        let report = answered(
            vec![
                touch("s1", "/a/b.rs", TouchVerb::Read, OwnerNaming::NotNamed),
                touch("s2", "/a/b.rs", TouchVerb::Edited, OwnerNaming::NotNamed),
                touch("s3", "/a/b.rs", TouchVerb::Read, OwnerNaming::Named),
                touch("s4", "/a/b.rs", TouchVerb::Wrote, OwnerNaming::NotNamed),
            ],
            "b.rs",
            false,
        );
        let sessions: Vec<&str> = report.groups()[0]
            .pairs()
            .iter()
            .map(TouchedPair::session_id)
            .collect();
        assert_eq!(sessions, vec!["s2", "s3", "s4"], "{report:?}");
        assert_eq!(report.in_passing(), 1);
        assert_eq!(report.unmarked_in_passing(), 0);
    }

    /// A pair left out with nothing measured about it is counted apart, because "nobody
    /// looked" and "looked and the owner never named it" are not the same answer.
    #[test]
    fn a_pair_nobody_marked_is_counted_apart_from_one_that_failed_the_rule() {
        let report = answered(
            vec![
                touch("s1", "/a/b.rs", TouchVerb::Read, OwnerNaming::NotComputed),
                touch("s2", "/a/b.rs", TouchVerb::Read, OwnerNaming::NotNamed),
            ],
            "b.rs",
            false,
        );
        assert_eq!(report.in_passing(), 2);
        assert_eq!(report.unmarked_in_passing(), 1);
        assert!(report.groups()[0].pairs().is_empty());
    }

    #[test]
    fn widening_shows_what_the_strict_answer_counted() {
        let rows = vec![touch(
            "s1",
            "/a/b.rs",
            TouchVerb::Read,
            OwnerNaming::NotNamed,
        )];
        let widened = answered(rows, "b.rs", true);
        assert_eq!(widened.groups()[0].pairs().len(), 1);
        assert_eq!(widened.in_passing(), 0);
        assert!(widened.widened());
    }

    /// The same session stored again at a later revision is one conversation, and the times
    /// it touched a file are not counted twice.
    #[test]
    fn one_session_stored_twice_is_one_conversation() {
        let rows = vec![
            StoredTouch::new(
                "s1",
                "беседа",
                "/a/b.rs",
                TouchVerb::Read,
                OwnerNaming::Named,
                2,
                Some(10),
            ),
            StoredTouch::new(
                "s1",
                "беседа",
                "/a/b.rs",
                TouchVerb::Read,
                OwnerNaming::Named,
                5,
                Some(10),
            ),
        ];
        let report = answered(rows, "b.rs", false);
        let pairs = report.groups()[0].pairs();
        assert_eq!(pairs.len(), 1, "{pairs:?}");
        assert_eq!(
            pairs[0].times(),
            5,
            "позже записанная ревизия считала с начала"
        );
    }

    /// Two verbs of one pair are one row, and the row is deliberate because one of them is.
    #[test]
    fn a_pair_read_and_then_edited_is_one_row() {
        let report = answered(
            vec![
                touch("s1", "/a/b.rs", TouchVerb::Read, OwnerNaming::NotNamed),
                touch("s1", "/a/b.rs", TouchVerb::Edited, OwnerNaming::NotNamed),
            ],
            "b.rs",
            false,
        );
        let pairs = report.groups()[0].pairs();
        assert_eq!(pairs.len(), 1);
        assert_eq!(
            pairs[0].verbs().collect::<Vec<_>>(),
            vec![TouchVerb::Read, TouchVerb::Edited]
        );
        assert_eq!(pairs[0].times(), 2, "два глагола — два касания");
        assert!(pairs[0].is_deliberate());
    }

    /// A tail is matched on a segment boundary, and a needle that only ends inside a longer
    /// name answers nothing rather than answering about another file.
    #[test]
    fn a_tail_that_ends_inside_a_longer_name_is_not_that_file() {
        let report = answered(
            vec![
                touch("s1", "/a/my_b.rs", TouchVerb::Edited, OwnerNaming::NotNamed),
                touch("s2", "/a/b.rs", TouchVerb::Edited, OwnerNaming::NotNamed),
            ],
            "b.rs",
            false,
        );
        assert_eq!(report.groups().len(), 1);
        assert_eq!(report.groups()[0].heading(), "/a/b.rs");
    }

    /// A tail matching two different files is answered about both, and the answer says so.
    #[test]
    fn a_tail_matching_two_files_is_not_resolved_quietly() {
        let report = answered(
            vec![
                touch("s1", "/one/b.rs", TouchVerb::Edited, OwnerNaming::NotNamed),
                touch("s2", "/two/b.rs", TouchVerb::Edited, OwnerNaming::NotNamed),
            ],
            "b.rs",
            false,
        );
        assert!(report.is_ambiguous());
        let headings: Vec<&str> = report.groups().iter().map(TouchGroup::heading).collect();
        assert_eq!(headings, vec!["/one/b.rs", "/two/b.rs"]);
    }

    #[test]
    fn a_question_about_nothing_is_refused() {
        assert_eq!(
            TouchQuery::new("  ", Direction::WhichConversations, false),
            Err(TouchQueryError::NothingToLookFor)
        );
    }
}
