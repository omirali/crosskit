// The terms one piece of text gives, taken through the analyzer the index is written with.
//
// Stage 31 needed a key per word that survives morphology: `контрагентом`, `контрагентам`
// and `контрагенты` have to be one thing or no count of them means anything. That rule
// already exists and is already data — the language packs — and writing a second word
// splitter for a measurement would mean measuring a corpus the product does not see.
//
// One reading per word rather than every reading. An index holds every reading so that a
// question in either language reaches the document; a measurement counting how often a word
// occurs must not count an ambiguous word twice, and `Reading::Only(0)` is what a question
// already goes through.
//
// There is a second reduction here, and it exists to be beaten. Stage 29 priced the
// conversation name by matching the first five letters of each word — no morphology, no
// library — and got 0.67. Comparing this stage's 0.87 against that number compares two
// rulers, because Stage 29 never counted what its own ties were doing. So the crude rule is
// reproduced here and run through the same ranking, the same scoring and the same column, and
// the two differ by the reduction and by nothing else: the splitting rule, the case folding
// and everything downstream are shared.
use tantivy::tokenizer::{TextAnalyzer, TokenStream};

use super::analyzer::{Reading, analyzer};
use super::language_pack::built_in_normalizations;
use crate::domain::NormalizationSet;

/// How a word is reduced to the key a measurement counts it under.
enum Reduction {
    /// The stem the language packs give it. Applied inside the analyzer, so nothing is left
    /// to do here.
    Stem,
    /// Its first `n` characters, lowercased — Stage 29's crude rule. A word shorter than `n`
    /// is its own key.
    FirstCharacters(usize),
}

/// Splits text into the terms one reduction gives for it.
///
/// Holds a prepared analyzer, because preparing a Snowball stemmer is not free and a corpus
/// is millions of tokens.
pub(crate) struct TermReader {
    analyzer: TextAnalyzer,
    reduce: Reduction,
}

impl TermReader {
    /// The reader for `ru` and `en`, which is what an index is written with unless a
    /// configuration says otherwise.
    pub(crate) fn with_built_in_packs() -> Self {
        Self {
            analyzer: analyzer(&built_in_normalizations(), Reading::Only(0)),
            reduce: Reduction::Stem,
        }
    }

    /// Stage 29's rule: the same splitting and case folding, then the first `characters` of
    /// each word and nothing else.
    ///
    /// The normalization set is empty on purpose rather than absent: a set that claims no
    /// token lets every token through exactly as written, which is precisely "no morphology"
    /// and keeps the splitting rule identical to the other reader's.
    pub(crate) fn with_the_first_characters(characters: usize) -> Self {
        let claims_nothing =
            NormalizationSet::new(Vec::new()).expect("пустой набор нормализаций допустим");
        Self {
            analyzer: analyzer(&claims_nothing, Reading::Only(0)),
            reduce: Reduction::FirstCharacters(characters),
        }
    }

    /// Appends the terms of `text` to `into`, in the order they were written.
    ///
    /// Order is kept and repeats are kept, because what the caller cuts into windows is the
    /// sequence of words and not the set of them.
    pub(crate) fn terms_into(&mut self, text: &str, into: &mut Vec<String>) {
        let mut stream = self.analyzer.token_stream(text);
        while stream.advance() {
            let term = &stream.token().text;
            into.push(match self.reduce {
                Reduction::Stem => term.clone(),
                // By characters and never by bytes: a Russian letter is two bytes, and
                // cutting five bytes off `контрагент` would cut two and a half letters and
                // land in the middle of the third.
                Reduction::FirstCharacters(characters) => term.chars().take(characters).collect(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TermReader;

    fn terms(text: &str) -> Vec<String> {
        let mut reader = TermReader::with_built_in_packs();
        let mut into = Vec::new();
        reader.terms_into(text, &mut into);
        into
    }

    fn crude_terms(text: &str) -> Vec<String> {
        let mut reader = TermReader::with_the_first_characters(5);
        let mut into = Vec::new();
        reader.terms_into(text, &mut into);
        into
    }

    #[test]
    fn the_forms_of_one_word_share_one_key() {
        let forms = terms("контрагентом контрагентам контрагенты контрагент");
        assert_eq!(forms.len(), 4);
        assert_eq!(
            forms
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1,
            "все формы должны сойтись в один ключ, вышло {forms:?}"
        );
    }

    #[test]
    fn a_word_gets_one_key_and_not_one_per_language() {
        // Both declared packs could have written a token of digits and Latin letters; a
        // measurement must count it once.
        assert_eq!(terms("ocr OCR Ocr").len(), 3);
        assert_eq!(terms("привет hello 2026").len(), 3);
    }

    #[test]
    fn the_order_of_the_words_is_kept_and_so_are_repeats() {
        assert_eq!(terms("раз два раз").len(), 3);
        let words = terms("договор поставки договор");
        assert_eq!(words[0], words[2]);
        assert_ne!(words[0], words[1]);
    }

    #[test]
    fn punctuation_is_a_separator_and_not_a_word() {
        assert_eq!(terms("«контрагент», договор!").len(), 2);
        assert!(terms("  \n — ").is_empty());
    }

    /// Stage 29's rule cuts by letters, not by bytes. Five bytes of `контрагент` would be two
    /// and a half Cyrillic letters.
    #[test]
    fn the_crude_rule_keeps_the_first_five_letters_of_a_word() {
        assert_eq!(crude_terms("контрагент"), vec!["контр"]);
        assert_eq!(crude_terms("КОНТРАГЕНТАМИ"), vec!["контр"]);
        assert_eq!(crude_terms("чат"), vec!["чат"]);
        assert_eq!(crude_terms("Implement"), vec!["imple"]);
    }

    /// Where the two rules disagree, and it is not a detail: the crude rule joins words that
    /// share a beginning and splits words that share a meaning.
    #[test]
    fn the_crude_rule_and_morphology_disagree_in_both_directions() {
        // Morphology keeps them apart; five letters make them one word.
        assert_ne!(terms("контрагент")[0], terms("контроль")[0]);
        assert_eq!(crude_terms("контрагент")[0], crude_terms("контроль")[0]);

        // Morphology joins them; five letters keep them apart. It happens wherever the stem
        // is shorter than five letters, which in Russian is most short nouns — `чат` among
        // them, and this corpus is made of chats.
        assert_eq!(terms("чате")[0], terms("чатом")[0]);
        assert_ne!(crude_terms("чате")[0], crude_terms("чатом")[0]);
    }

    /// Both readers split the same way, so a comparison between them is about the reduction
    /// and about nothing else.
    #[test]
    fn both_rules_split_words_identically() {
        let text = "«Сверка счетов», раздел 7 — draft_note для v1!";
        assert_eq!(terms(text).len(), crude_terms(text).len());
    }
}
