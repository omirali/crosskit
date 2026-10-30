// The letters one language writes, and the writing system they belong to.
//
// Routing asks one question of a token — who could have written it — and this is what
// answers it. A pack declares its letters as data; the two shapes here are the two shapes
// alphabets come in, a list for one that is a list and ranges for one that is blocks of the
// standard.
use std::collections::BTreeSet;

use super::text_normalization::NormalizationError;

/// The writing system a normalization is for. Routing only ever compares languages of one
/// script: an English pack and a Kazakh one never claim the same token, so neither has to
/// say anything about the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Latin,
    Cyrillic,
}

/// The letters one normalization writes: single letters, ranges, or both.
///
/// Ranges because Latin is a few blocks of code points rather than a list anybody wants to
/// read, and single letters because an alphabet like Kazakh's is exactly forty-two of them.
/// Every candidate must also be a letter, which is what keeps the multiplication sign out of
/// the Latin range it sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alphabet {
    letters: BTreeSet<char>,
    ranges: Vec<(char, char)>,
}

impl Alphabet {
    pub fn new(
        letters: BTreeSet<char>,
        ranges: Vec<(char, char)>,
    ) -> Result<Self, NormalizationError> {
        if letters.is_empty() && ranges.is_empty() {
            return Err(NormalizationError::EmptyAlphabet);
        }
        if let Some((from, to)) = ranges.iter().find(|(from, to)| from > to) {
            return Err(NormalizationError::BackwardRange {
                from: *from,
                to: *to,
            });
        }
        Ok(Self { letters, ranges })
    }

    pub fn holds(&self, character: char) -> bool {
        character.is_alphabetic()
            && (self.letters.contains(&character)
                || self
                    .ranges
                    .iter()
                    .any(|(from, to)| (*from..=*to).contains(&character)))
    }
}
