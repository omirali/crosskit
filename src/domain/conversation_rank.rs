// What one term of a question is worth to one conversation.
//
// Three places a term can be, and they are not equally good evidence. Its own name is the
// shortest and most deliberate description a conversation has — Stage 29 measured that a
// crude match against names alone beats the whole body index, 0.67 against 0.60. What the
// owner said is what they were asking for. What the model said is mostly the owner's words
// read back, plus everything the model volunteered.
//
// The counting rule matters more than the coefficients. A term is counted once per reply
// that holds it and never once per occurrence: a conversation where somebody pasted a log
// with the word four hundred times in it is not four hundred times more about that word than
// a conversation where the owner said it three times. Counting occurrences is exactly how the
// 15 MB conversation took all five places of one page in Stage 29.
use std::fmt;

/// Where in one conversation a term turned up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermPresence {
    in_name: bool,
    owner_replies: u32,
    model_replies: u32,
}

impl TermPresence {
    pub fn new(in_name: bool, owner_replies: u32, model_replies: u32) -> Self {
        Self {
            in_name,
            owner_replies,
            model_replies,
        }
    }

    pub fn in_name(self) -> bool {
        self.in_name
    }

    pub fn owner_replies(self) -> u32 {
        self.owner_replies
    }

    pub fn model_replies(self) -> u32 {
        self.model_replies
    }

    pub fn is_absent(self) -> bool {
        !self.in_name && self.owner_replies == 0 && self.model_replies == 0
    }
}

/// How a count of replies becomes a number in the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counting {
    /// The count itself. A conversation ten times as long can score ten times as high.
    Replies,
    /// `log2(1 + count)`. The second mention of a word says much less than the first, which
    /// is the assumption BM25 already makes about term frequency, applied to replies.
    Saturated,
    /// Said or not said. The strongest possible answer to length: a conversation of two
    /// thousand replies and a conversation of six are worth the same for a term both used,
    /// and what separates them has to come from somewhere other than their size.
    Once,
}

impl Counting {
    fn of(self, replies: u32) -> f64 {
        match self {
            Self::Replies => f64::from(replies),
            Self::Saturated => (f64::from(replies) + 1.0).log2(),
            Self::Once => f64::from(u8::from(replies > 0)),
        }
    }
}

/// One weighting of the three places, and one counting rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoringRule {
    name: f64,
    owner: f64,
    model: f64,
    counting: Counting,
}

impl ScoringRule {
    /// The stage's hypothesis: the name is worth three replies, the owner's reply two, the
    /// model's one.
    pub fn three_two_one() -> Self {
        Self {
            name: 3.0,
            owner: 2.0,
            model: 1.0,
            counting: Counting::Replies,
        }
    }

    /// The same weighting with the name taken out, which is what says how much the name was
    /// contributing on its own.
    pub fn without_the_name() -> Self {
        Self {
            name: 0.0,
            ..Self::three_two_one()
        }
    }

    /// The name and nothing else, which is Stage 29's 0.67 baseline recomputed over the same
    /// morphology the index uses instead of a five-letter prefix match.
    pub fn the_name_alone() -> Self {
        Self {
            name: 3.0,
            owner: 0.0,
            model: 0.0,
            counting: Counting::Replies,
        }
    }

    /// The hypothesis with the reply counts saturated, which is the shortest answer to a
    /// conversation outscoring another by being longer.
    pub fn three_two_one_saturated() -> Self {
        Self {
            counting: Counting::Saturated,
            ..Self::three_two_one()
        }
    }

    /// The hypothesis with a party's replies counted once however many there were, which is
    /// the only weighting here that a long conversation cannot win by being long.
    pub fn three_two_one_at_most_once() -> Self {
        Self {
            counting: Counting::Once,
            ..Self::three_two_one()
        }
    }

    /// The body counted once per party, with the name taken out. Against
    /// `three_two_one_at_most_once` this is what the name is worth under a weighting where it
    /// can be worth anything at all — under raw counts the body outscores it by hundreds and
    /// the difference says nothing.
    pub fn without_the_name_at_most_once() -> Self {
        Self {
            name: 0.0,
            ..Self::three_two_one_at_most_once()
        }
    }

    /// What one term of a question adds to one conversation's score.
    pub fn score(self, presence: TermPresence) -> f64 {
        let named = if presence.in_name() { self.name } else { 0.0 };
        named
            + self.owner * self.counting.of(presence.owner_replies())
            + self.model * self.counting.of(presence.model_replies())
    }
}

impl fmt::Display for ScoringRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let counting = match self.counting {
            Counting::Replies => "реплики",
            Counting::Saturated => "log2(1+реплики)",
            Counting::Once => "сказано или нет",
        };
        write!(
            formatter,
            "имя×{} + владелец×{} + модель×{}, счёт: {counting}",
            self.name, self.owner, self.model
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_places_weigh_three_two_and_one() {
        let rule = ScoringRule::three_two_one();
        assert_eq!(rule.score(TermPresence::new(true, 0, 0)), 3.0);
        assert_eq!(rule.score(TermPresence::new(false, 1, 0)), 2.0);
        assert_eq!(rule.score(TermPresence::new(false, 0, 1)), 1.0);
        assert_eq!(rule.score(TermPresence::new(true, 2, 3)), 3.0 + 4.0 + 3.0);
    }

    /// The rule the whole ranking rests on. A term repeated inside one reply is that one
    /// reply, so the conversation where the owner said a word in three separate replies
    /// outscores the conversation where a pasted log holds it four hundred times in one.
    #[test]
    fn a_term_counts_once_per_reply_and_never_once_per_occurrence() {
        let rule = ScoringRule::three_two_one();
        let pasted_log = TermPresence::new(false, 1, 0);
        let said_three_times = TermPresence::new(false, 3, 0);
        assert!(rule.score(said_three_times) > rule.score(pasted_log));
        assert_eq!(rule.score(pasted_log), 2.0);
    }

    #[test]
    fn saturating_the_count_stops_length_alone_from_winning() {
        let plain = ScoringRule::three_two_one();
        let saturated = ScoringRule::three_two_one_saturated();
        let long = TermPresence::new(false, 400, 0);
        let named = TermPresence::new(true, 2, 0);
        assert!(plain.score(long) > plain.score(named));
        assert!(saturated.score(named) > saturated.score(long) / 4.0);
        // 2·log2(401) ≈ 17.3, against 3 + 2·log2(3) ≈ 6.2: the ceiling is not a cure, it is
        // a much slower climb.
        assert!((saturated.score(long) - 2.0 * 401f64.log2()).abs() < 1e-9);
    }

    #[test]
    fn taking_the_name_out_leaves_only_what_was_said() {
        let rule = ScoringRule::without_the_name();
        assert_eq!(rule.score(TermPresence::new(true, 0, 0)), 0.0);
        assert_eq!(rule.score(TermPresence::new(true, 1, 1)), 3.0);

        let rule = ScoringRule::without_the_name_at_most_once();
        assert_eq!(rule.score(TermPresence::new(true, 0, 0)), 0.0);
        assert_eq!(rule.score(TermPresence::new(true, 900, 900)), 3.0);
    }

    #[test]
    fn the_name_alone_ignores_the_body() {
        let rule = ScoringRule::the_name_alone();
        assert_eq!(rule.score(TermPresence::new(false, 40, 40)), 0.0);
        assert_eq!(rule.score(TermPresence::new(true, 40, 40)), 3.0);
    }

    /// The one weighting whose score has a ceiling, which is what makes the longest
    /// conversation in the corpus stop winning every question by itself.
    #[test]
    fn counting_a_party_once_puts_a_ceiling_on_what_a_term_can_be_worth() {
        let rule = ScoringRule::three_two_one_at_most_once();
        assert_eq!(rule.score(TermPresence::new(true, 2000, 4000)), 6.0);
        assert_eq!(rule.score(TermPresence::new(true, 1, 1)), 6.0);
        assert_eq!(rule.score(TermPresence::new(false, 400, 0)), 2.0);
        assert_eq!(rule.score(TermPresence::new(false, 0, 0)), 0.0);
    }

    #[test]
    fn a_term_nowhere_in_the_conversation_is_absent() {
        assert!(TermPresence::default().is_absent());
        assert!(!TermPresence::new(false, 0, 1).is_absent());
    }
}
