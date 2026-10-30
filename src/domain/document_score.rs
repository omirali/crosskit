// What one document is worth as an answer, folded out of the matches inside it.
//
// A row of the results is a document, so the several places a query matched inside one file
// have to become one number. The rule is not their sum. Stage 29 measured what a sum does on
// real transcripts: one 15 MB conversation took all five places of a page on a single word
// stem, because it had said the word more times than anybody else — and Stage 31 measured the
// same effect as a ranking rule, where counting occurrences scored 0.00 strict precision
// against 0.53 for counting replies once.
//
// So the rule is: a document is worth its strongest match, and further matches are worth less
// and less. That is the assumption BM25 already makes about a word repeated inside one chunk,
// applied one level up — to the chunks of one document. It gives a conversation that said
// something in three separate replies more than one that said it four hundred times in a
// single pasted log, and it bounds what length alone can buy at `SATURATION + 1` times the
// best match, whether a document holds ten matches or ten thousand.

/// How fast a further match stops being worth anything.
///
/// 1.2 is BM25's own `k1`, and it is the same number for the same reason: it is the point
/// where a second mention is already worth much less than the first without being worth
/// nothing. Taking it rather than picking a new one keeps one assumption in the program
/// instead of two that could drift apart — and the number only sets how steeply the curve
/// flattens, never whether it does.
const SATURATION: f32 = 1.2;

/// The score of one document, while its matches are still being folded in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DocumentScore {
    best: f32,
    matches: u32,
}

impl DocumentScore {
    /// A document no match has been folded into yet.
    pub fn nothing() -> Self {
        Self {
            best: 0.0,
            matches: 0,
        }
    }

    /// The same document with one more of its matches folded in.
    pub fn and(self, matched: f32) -> Self {
        Self {
            best: self.best.max(matched),
            matches: self.matches.saturating_add(1),
        }
    }

    /// How many matches were folded in. A row says what it stands for, and "one file, six
    /// replies" is the thing folding took away from the page.
    pub fn matches(self) -> u32 {
        self.matches
    }

    /// The number the row is ranked by.
    ///
    /// The count is taken through `u16` so the conversion to a float is exact and this
    /// cannot quietly round. Past 65 535 matches it stops counting, and nothing is lost by
    /// that: the curve is within a thousandth of its ceiling long before then.
    pub fn value(self) -> f32 {
        let matches = f32::from(u16::try_from(self.matches).unwrap_or(u16::MAX));
        self.best * (matches * (SATURATION + 1.0)) / (matches + SATURATION)
    }
}

#[cfg(test)]
mod tests {
    use super::DocumentScore;

    fn folded(matches: &[f32]) -> f32 {
        matches
            .iter()
            .fold(DocumentScore::nothing(), |score, matched| {
                score.and(*matched)
            })
            .value()
    }

    /// A document with one match is worth that match, so folding changes nothing for the
    /// files that matched in one place — which is almost all of them outside a transcript.
    #[test]
    fn one_match_is_worth_itself() {
        assert!((folded(&[2.5]) - 2.5).abs() < 1e-5, "{}", folded(&[2.5]));
        assert_eq!(folded(&[]), 0.0);
    }

    /// The rule this whole type exists for: more places beat a better single place, and not
    /// by much, so neither length nor a single lucky line decides a page on its own.
    #[test]
    fn more_matches_are_worth_more_and_length_cannot_run_away_with_it() {
        assert!(folded(&[1.0, 1.0, 1.0]) > folded(&[1.0]));
        assert!(folded(&[1.0; 10_000]) < folded(&[1.0]) * 3.0);
    }

    /// Two thousand mediocre matches do not overtake a document whose best match is twice as
    /// good, which is the shape of the failure Stage 29 measured.
    #[test]
    fn a_long_document_does_not_pass_a_better_one_on_count_alone() {
        assert!(folded(&[1.0; 2_000]) < folded(&[3.0]));
    }
}
