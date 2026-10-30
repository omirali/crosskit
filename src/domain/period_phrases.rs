// The phrases of a question that name a period, and what is left of the question without
// them.
//
// Two rules, and both come from measurements of this project rather than from taste.
//
// The set is closed and it is data. Which words mean "last week" is a fact about a language,
// it lives in the language pack's directory beside the stemming rules, and a phrase nobody
// wrote down is not a period — the question is then answered exactly as it was before this
// stage, which is the one outcome a guess could not promise.
//
// The phrase is taken out of what is looked for. Stages 29 and 31 measured the frame of a
// question — "in which chat did we …" — as the thing that spoils a search: BM25 matches its
// words in every document, and anything that expands them multiplies the damage. "Last week"
// is the same kind of word. It says which documents to look among, not what to look for.

/// One phrase as written in a pack: its words, already lower case, and what it names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Phrase {
    words: Vec<String>,
    names: super::period::PeriodPhrase,
}

/// Every phrase this build recognises, longest first so "на прошлой неделе" is found before a
/// shorter phrase inside it could be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodPhrases {
    phrases: Vec<Phrase>,
}

/// A phrase a question held and the question without it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrippedQuestion {
    found: Option<FoundPhrase>,
    remaining: String,
}

impl StrippedQuestion {
    pub fn found(&self) -> Option<&FoundPhrase> {
        self.found.as_ref()
    }

    /// What is left to look for. Exactly the text given when no phrase was found — not a
    /// rejoined copy of it — so a question without a period is the same query it always was.
    pub fn remaining(&self) -> &str {
        &self.remaining
    }
}

/// The phrase as it appeared, and what it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundPhrase {
    phrase: super::period::PeriodPhrase,
    as_written: String,
}

impl FoundPhrase {
    pub fn phrase(&self) -> super::period::PeriodPhrase {
        self.phrase
    }

    /// The words of the question it was read from, as typed.
    pub fn as_written(&self) -> &str {
        &self.as_written
    }
}

impl PeriodPhrases {
    /// The phrases of every pack, each a list of words. A phrase written in a form a question
    /// can never match — upper case, punctuation, an empty word — is refused, and so is one
    /// phrase naming two periods: a pack that says both would answer one question two ways.
    pub fn new(
        entries: Vec<(super::period::PeriodPhrase, String)>,
    ) -> Result<Self, PeriodPhrasesError> {
        let mut phrases: Vec<Phrase> = Vec::with_capacity(entries.len());
        for (names, written) in entries {
            let words: Vec<String> = written.split_whitespace().map(str::to_string).collect();
            if words.is_empty() || words.iter().any(|word| normalized(word) != *word) {
                return Err(PeriodPhrasesError::NotInMatchingForm(written));
            }
            if let Some(other) = phrases.iter().find(|phrase| phrase.words == words) {
                if other.names != names {
                    return Err(PeriodPhrasesError::NamesTwoPeriods(written));
                }
                continue;
            }
            phrases.push(Phrase { words, names });
        }
        phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.words.len()));
        Ok(Self { phrases })
    }

    /// The first phrase the text holds, earliest in the text and longest at that place, and
    /// the text without it. Words are compared lower case and without the punctuation around
    /// them, so "неделе?" is the word "неделе"; the words that remain keep their spelling.
    ///
    /// One phrase and not every one: "вчера и сегодня" names two periods, and choosing their
    /// union or either one would be this program deciding what was meant. The second stays a
    /// word of the question.
    pub fn strip(&self, text: &str) -> StrippedQuestion {
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let words: Vec<String> = tokens.iter().map(|token| normalized(token)).collect();
        for start in 0..words.len() {
            let Some(phrase) = self.phrases.iter().find(|phrase| {
                words
                    .get(start..start + phrase.words.len())
                    .is_some_and(|window| window == phrase.words.as_slice())
            }) else {
                continue;
            };
            let end = start + phrase.words.len();
            let remaining: Vec<&str> = tokens[..start]
                .iter()
                .chain(&tokens[end..])
                .copied()
                .collect();
            return StrippedQuestion {
                found: Some(FoundPhrase {
                    phrase: phrase.names,
                    as_written: tokens[start..end].join(" "),
                }),
                remaining: remaining.join(" "),
            };
        }
        StrippedQuestion {
            found: None,
            remaining: text.to_string(),
        }
    }
}

/// A word as a phrase is compared against: its letters and digits, lower case.
fn normalized(token: &str) -> String {
    token
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// What a phrase pack cannot say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeriodPhrasesError {
    NotInMatchingForm(String),
    NamesTwoPeriods(String),
}

impl std::fmt::Display for PeriodPhrasesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInMatchingForm(phrase) => write!(
                formatter,
                "the period phrase {phrase:?} is not lower-case words a question can match"
            ),
            Self::NamesTwoPeriods(phrase) => {
                write!(formatter, "the period phrase {phrase:?} names two periods")
            }
        }
    }
}

impl std::error::Error for PeriodPhrasesError {}

#[cfg(test)]
mod tests {
    use super::super::period::PeriodPhrase;
    use super::{PeriodPhrases, PeriodPhrasesError};

    fn phrases() -> PeriodPhrases {
        PeriodPhrases::new(vec![
            (PeriodPhrase::LastWeek, "на прошлой неделе".to_string()),
            (PeriodPhrase::LastWeek, "last week".to_string()),
            (PeriodPhrase::Yesterday, "вчера".to_string()),
            (PeriodPhrase::Today, "сегодня".to_string()),
            (PeriodPhrase::Month(9), "в сентябре".to_string()),
        ])
        .unwrap()
    }

    /// The owner's question: the phrase names the period and leaves the query.
    #[test]
    fn the_period_phrase_is_taken_out_of_what_is_looked_for() {
        let stripped = phrases().strip("над чем я работал на прошлой неделе");
        let found = stripped.found().unwrap();
        assert_eq!(found.phrase(), PeriodPhrase::LastWeek);
        assert_eq!(found.as_written(), "на прошлой неделе");
        assert_eq!(stripped.remaining(), "над чем я работал");
        for word in ["прошлой", "неделе"] {
            assert!(!stripped.remaining().contains(word));
        }
    }

    /// Case and the punctuation around a word do not hide it; the words that stay keep their
    /// spelling.
    #[test]
    fn case_and_punctuation_do_not_hide_a_phrase() {
        let stripped = phrases().strip("Что обсуждали В Сентябре? про `Индекс`");
        assert_eq!(stripped.found().unwrap().phrase(), PeriodPhrase::Month(9));
        assert_eq!(stripped.remaining(), "Что обсуждали про `Индекс`");
        assert_eq!(
            phrases().strip("What did I fix LAST WEEK?").remaining(),
            "What did I fix"
        );
    }

    /// A question without a phrase is the same text, character for character: not a rejoined
    /// copy with its spacing changed.
    #[test]
    fn a_question_without_a_phrase_is_left_exactly_as_it_was() {
        for question in [
            "над  чем я работал",
            "неделе прошлой на",
            "в каком чате мы разбирали сверку",
            "сентябрь",
            "",
        ] {
            let stripped = phrases().strip(question);
            assert_eq!(stripped.found(), None, "{question}");
            assert_eq!(stripped.remaining(), question);
        }
    }

    /// A phrase is whole words: "вчерашний" is not "вчера".
    #[test]
    fn a_phrase_matches_whole_words_only() {
        assert_eq!(phrases().strip("вчерашний отчёт").found(), None);
        assert_eq!(phrases().strip("на прошлой").found(), None);
    }

    /// Two phrases: the first is the period, the second stays a word.
    #[test]
    fn only_the_first_phrase_is_read() {
        let stripped = phrases().strip("вчера и сегодня");
        assert_eq!(stripped.found().unwrap().phrase(), PeriodPhrase::Yesterday);
        assert_eq!(stripped.remaining(), "и сегодня");
    }

    #[test]
    fn a_pack_that_cannot_be_matched_or_says_two_things_is_refused() {
        assert!(matches!(
            PeriodPhrases::new(vec![(PeriodPhrase::Today, "Сегодня".to_string())]),
            Err(PeriodPhrasesError::NotInMatchingForm(_))
        ));
        assert!(matches!(
            PeriodPhrases::new(vec![(PeriodPhrase::Today, "  ".to_string())]),
            Err(PeriodPhrasesError::NotInMatchingForm(_))
        ));
        assert!(matches!(
            PeriodPhrases::new(vec![
                (PeriodPhrase::Today, "сегодня".to_string()),
                (PeriodPhrase::Yesterday, "сегодня".to_string()),
            ]),
            Err(PeriodPhrasesError::NamesTwoPeriods(_))
        ));
    }
}
