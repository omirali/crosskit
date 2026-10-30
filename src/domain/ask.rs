// Asking a question about a context, and what an answer to it may be.
//
// The rules here are about three things, and all three exist because the answer comes from
// a model: how much of the context's text may leave in one question, what shape an answer
// has to have to be accepted at all, and which sources it is allowed to name.
//
// The last one is the point. The model is given excerpts numbered from one and nothing else
// — no path, no file name, no identifier of any kind — so the only thing it can cite is a
// number this program handed it. A number outside the range it was given is not a source it
// knows about; it is an answer that does not fit, and it is refused rather than printed.
use std::fmt;

use super::cache_key::compose;
use super::period::Period;

/// What a person asked. Not empty, and not longer than a question anybody types: the whole
/// of it travels to the model, and a question the size of a file is a file being sent under
/// another name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question(String);

/// Long enough for a paragraph of a question, short enough that nothing else fits.
const MAX_QUESTION_CHARS: usize = 1_000;

impl Question {
    pub fn parse(asked: &str) -> Result<Self, AskError> {
        let asked = asked.trim();
        if asked.is_empty() {
            return Err(AskError::EmptyQuestion);
        }
        if asked.chars().count() > MAX_QUESTION_CHARS {
            return Err(AskError::QuestionTooLong {
                chars: asked.chars().count(),
                limit: MAX_QUESTION_CHARS,
            });
        }
        Ok(Self(asked.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The question as words to look for: everything that is not a letter or a digit becomes
    /// a space.
    ///
    /// A person asking a question is not writing a query. `+`, `-`, `"`, `:` and the rest
    /// mean things to a query parser — a field that does not exist, a term that must be
    /// there, a phrase that never closes — and a question carrying any of them would be
    /// refused or, worse, quietly answered from the wrong set. The words are what was meant;
    /// which of them matter is the index's business.
    pub fn search_words(&self) -> String {
        let mut words = String::with_capacity(self.0.len());
        for character in self.0.chars() {
            if character.is_alphanumeric() {
                words.push(character);
            } else if !words.ends_with(' ') {
                words.push(' ');
            }
        }
        words.trim().to_string()
    }
}

impl fmt::Display for Question {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How much of a context may leave in one question: how many excerpts, and how many bytes
/// of them together.
///
/// Two numbers rather than one because they bound different things. The byte limit is what
/// leaves the machine; the count is how thinly it is spread — fifty excerpts of four hundred
/// bytes answer worse than five of four thousand, and a limit on bytes alone would not say
/// which of the two happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AskBudget {
    max_sources: usize,
    max_bytes: usize,
}

impl AskBudget {
    pub fn new(max_sources: usize, max_bytes: usize) -> Self {
        Self {
            max_sources,
            max_bytes,
        }
    }

    pub fn max_sources(&self) -> usize {
        self.max_sources
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Whether one more excerpt of `bytes` fits beside what is already taken.
    ///
    /// The first excerpt is admitted even when it is larger than the whole budget, and then
    /// cut to it by the caller: a question whose best source is one long paragraph is
    /// answered from part of that paragraph rather than from nothing.
    pub fn admits(&self, taken: usize, bytes_taken: usize, bytes: usize) -> bool {
        if taken >= self.max_sources {
            return false;
        }
        if taken == 0 {
            return true;
        }
        bytes_taken + bytes <= self.max_bytes
    }

    /// How much of an excerpt may still be sent, given what is already taken.
    pub fn remaining_bytes(&self, bytes_taken: usize) -> usize {
        self.max_bytes.saturating_sub(bytes_taken)
    }
}

/// An answer this program accepts: what it says, and which of the excerpts it was given it
/// says it used.
///
/// Both are checked here rather than trusted. The text must be there and must be a text
/// rather than a document; the sources must be numbers of excerpts this question actually
/// sent. An answer citing a source nobody gave it is refused whole: it is the one shape of
/// wrong answer that would let a model put a file in front of a reader by inventing its
/// number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    text: String,
    used: Vec<usize>,
}

/// A terminal answer, not a document. A model asked for prose and answering with a book is
/// answering something else, and nothing downstream is prepared to print it.
const MAX_ANSWER_CHARS: usize = 4_000;

impl Answer {
    /// `sent` is how many excerpts the question carried; sources are numbered from one.
    pub fn parse(text: &str, used: &[i64], sent: usize) -> Result<Self, AskError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(AskError::EmptyAnswer);
        }
        if text.chars().count() > MAX_ANSWER_CHARS {
            return Err(AskError::AnswerTooLong {
                chars: text.chars().count(),
                limit: MAX_ANSWER_CHARS,
            });
        }
        let mut ordinals = Vec::with_capacity(used.len());
        for cited in used {
            let ordinal = usize::try_from(*cited).map_err(|_| AskError::SourceOutOfRange {
                cited: *cited,
                sent,
            })?;
            if ordinal == 0 || ordinal > sent {
                return Err(AskError::SourceOutOfRange {
                    cited: *cited,
                    sent,
                });
            }
            if !ordinals.contains(&ordinal) {
                ordinals.push(ordinal);
            }
        }
        ordinals.sort_unstable();
        Ok(Self {
            text: text.to_string(),
            used: ordinals,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The excerpts it cited, numbered from one, in order and without repeats.
    pub fn used(&self) -> &[usize] {
        &self.used
    }
}

/// What makes two questions the same piece of work: the question itself, the model, the
/// wording of the prompt, and the sources it carried — each named by the chunk it came from
/// and how much of that chunk was sent.
///
/// The chunk names the content without carrying it: its identity is derived from the file
/// and the hash of what it holds. The length is there because a budget may have cut an
/// excerpt short, and half a paragraph is a different question from the whole of it.
///
/// A question about a period carries the period's days, and not the words that named it:
/// "last week" asked next Thursday is a different seven days, and the model was told which.
/// A question without one composes exactly the key it composed before periods existed, so
/// every answer already cached is still found.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AskCacheKey(String);

impl AskCacheKey {
    pub fn of(
        question: &Question,
        model_id: &str,
        prompt_version: &str,
        sources: &[(&str, usize)],
        period: Option<&Period>,
    ) -> Self {
        let mut parts: Vec<String> = vec![
            question.as_str().to_string(),
            model_id.to_string(),
            prompt_version.to_string(),
        ];
        for (chunk_id, bytes) in sources {
            parts.push(format!("{chunk_id}@{bytes}"));
        }
        if let Some(period) = period {
            parts.push(format!(
                "period:{}..{}",
                period.start_unix_ms(),
                period.end_unix_ms()
            ));
        }
        let borrowed: Vec<&str> = parts.iter().map(String::as_str).collect();
        Self(compose("ask-cache", &borrowed))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AskCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a question or an answer cannot be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskError {
    EmptyQuestion,
    QuestionTooLong { chars: usize, limit: usize },
    EmptyAnswer,
    AnswerTooLong { chars: usize, limit: usize },
    SourceOutOfRange { cited: i64, sent: usize },
}

impl fmt::Display for AskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyQuestion => write!(f, "a question with no words in it"),
            Self::QuestionTooLong { chars, limit } => write!(
                f,
                "a question of {chars} characters; {limit} is the most that is asked rather than sent"
            ),
            Self::EmptyAnswer => write!(f, "an answer with no text in it"),
            Self::AnswerTooLong { chars, limit } => {
                write!(f, "an answer of {chars} characters; the limit is {limit}")
            }
            Self::SourceOutOfRange { cited, sent } => write!(
                f,
                "an answer citing source {cited} when the question carried {sent}"
            ),
        }
    }
}

impl std::error::Error for AskError {}

#[cfg(test)]
mod tests {
    use super::super::cache_key::compose;
    use super::super::calendar::CivilDate;
    use super::super::period::{LocalDay, Period, PeriodPhrase};
    use super::{Answer, AskBudget, AskCacheKey, AskError, Question};

    #[test]
    fn a_question_is_what_was_typed_without_its_whitespace() {
        assert_eq!(
            Question::parse("  чем занят этот проект? ")
                .unwrap()
                .as_str(),
            "чем занят этот проект?"
        );
        assert_eq!(Question::parse("   ").unwrap_err(), AskError::EmptyQuestion);
        assert!(matches!(
            Question::parse(&"а".repeat(1_001)).unwrap_err(),
            AskError::QuestionTooLong { .. }
        ));
    }

    /// A question is prose, and prose is full of characters a query parser reads as
    /// instructions. What is looked for is the words.
    #[test]
    fn a_question_becomes_the_words_it_is_made_of() {
        let question =
            Question::parse("что такое `ContentEgressPermit`: зачем он? (+нужен -нет)").unwrap();

        assert_eq!(
            question.search_words(),
            "что такое ContentEgressPermit зачем он нужен нет"
        );
        assert_eq!(
            Question::parse("кітаптар және индексы?")
                .unwrap()
                .search_words(),
            "кітаптар және индексы",
            "letters of every declared language are letters"
        );
    }

    /// The budget is two limits, and the count is the one that decides first: it is what
    /// keeps a question from being spread over fifty excerpts of nothing.
    #[test]
    fn a_budget_bounds_both_the_number_of_excerpts_and_their_bytes() {
        let budget = AskBudget::new(3, 1_000);

        assert!(budget.admits(0, 0, 400));
        assert!(budget.admits(2, 600, 400));
        assert!(!budget.admits(3, 0, 1), "three is all this budget takes");
        assert!(
            !budget.admits(2, 900, 400),
            "and only a thousand bytes of them"
        );
        assert_eq!(budget.remaining_bytes(900), 100);
        assert_eq!(budget.remaining_bytes(2_000), 0);
    }

    /// The first excerpt is taken whatever its size, and cut to the budget by the caller:
    /// one long paragraph is a better answer than none.
    #[test]
    fn the_first_excerpt_is_admitted_even_when_it_fills_the_budget_alone() {
        let budget = AskBudget::new(5, 100);

        assert!(budget.admits(0, 0, 10_000));
        assert_eq!(budget.remaining_bytes(0), 100);
    }

    #[test]
    fn an_answer_keeps_its_text_and_the_sources_it_cited() {
        let answer = Answer::parse("  потому что так измерено  ", &[2, 1, 2], 3).unwrap();

        assert_eq!(answer.text(), "потому что так измерено");
        assert_eq!(answer.used(), [1, 2], "in order, and each one once");
    }

    /// The check that matters: a model cannot put a source in front of a reader by naming a
    /// number nobody gave it.
    #[test]
    fn an_answer_citing_a_source_that_was_never_sent_is_refused() {
        for cited in [0, 4, -1, i64::MAX] {
            assert_eq!(
                Answer::parse("текст", &[cited], 3).unwrap_err(),
                AskError::SourceOutOfRange { cited, sent: 3 }
            );
        }
    }

    /// An answer that cites nothing is still an answer: "the excerpts do not say" is the
    /// honest one when they do not.
    #[test]
    fn an_answer_may_cite_nothing() {
        let answer = Answer::parse("в этих выдержках этого нет", &[], 3).unwrap();
        assert!(answer.used().is_empty());
    }

    #[test]
    fn an_answer_that_is_not_one_is_refused() {
        assert_eq!(
            Answer::parse("   ", &[], 1).unwrap_err(),
            AskError::EmptyAnswer
        );
        assert!(matches!(
            Answer::parse(&"a".repeat(4_001), &[], 1).unwrap_err(),
            AskError::AnswerTooLong { .. }
        ));
    }

    /// Everything the answer depends on is in the key, and nothing else is.
    #[test]
    fn a_key_changes_with_the_question_the_model_the_prompt_and_the_sources() {
        let question = Question::parse("что это").unwrap();
        let other = Question::parse("что то").unwrap();
        let sources = [("chunk:a", 100usize), ("chunk:b", 50)];
        let key = |question: &Question, model: &str, prompt: &str, sources: &[(&str, usize)]| {
            AskCacheKey::of(question, model, prompt, sources, None)
        };

        let base = key(&question, "m", "p", &sources);
        assert_eq!(base, key(&question, "m", "p", &sources));
        for different in [
            key(&other, "m", "p", &sources),
            key(&question, "other-model", "p", &sources),
            key(&question, "m", "other-prompt", &sources),
            key(&question, "m", "p", &[("chunk:a", 100)]),
            key(&question, "m", "p", &[("chunk:b", 50), ("chunk:a", 100)]),
            // The same source, cut differently: half a paragraph is another question.
            key(&question, "m", "p", &[("chunk:a", 40), ("chunk:b", 50)]),
        ] {
            assert_ne!(base, different);
        }
    }

    fn thursday(year: i32, month: u8, day: u8) -> LocalDay {
        LocalDay::new(CivilDate::new(year, month, day).unwrap(), 5 * 3600)
    }

    /// "Last week" asked one Thursday and the next is two questions: the period's days are in
    /// the key, the words that named it are not what tells them apart.
    #[test]
    fn the_same_question_in_two_weeks_is_two_keys() {
        let question = Question::parse("над чем я работал на прошлой неделе?").unwrap();
        let sources = [("chunk:a", 100usize)];
        let this_week = Period::of_phrase(PeriodPhrase::LastWeek, thursday(2026, 10, 1));
        let next_week = Period::of_phrase(PeriodPhrase::LastWeek, thursday(2026, 10, 8));

        let first = AskCacheKey::of(&question, "m", "p", &sources, Some(&this_week));
        assert_eq!(
            first,
            AskCacheKey::of(&question, "m", "p", &sources, Some(&this_week))
        );
        assert_ne!(
            first,
            AskCacheKey::of(&question, "m", "p", &sources, Some(&next_week))
        );
        assert_ne!(first, AskCacheKey::of(&question, "m", "p", &sources, None));
    }

    /// A question without a period composes the key this program composed before periods
    /// existed, part for part: every answer already in the cache is still found.
    #[test]
    fn a_question_without_a_period_keeps_the_key_it_had() {
        let question = Question::parse("что делает сверка").unwrap();
        let key = AskCacheKey::of(
            &question,
            "a-model",
            "ask-v1",
            &[("chunk:a", 100), ("chunk:b", 7)],
            None,
        );
        assert_eq!(
            key.as_str(),
            compose(
                "ask-cache",
                &[
                    "что делает сверка",
                    "a-model",
                    "ask-v1",
                    "chunk:a@100",
                    "chunk:b@7"
                ]
            )
        );
    }
}
