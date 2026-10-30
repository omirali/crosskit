// The analyzer: how text becomes the terms an index holds, and a query becomes the terms
// it is looked up by.
//
// It is a splitting rule followed by a set of normalizations, and the set is data — the
// configuration names it, the domain routes each token to the ones that may reduce it, and
// this file only applies the rules that routing chose. There is no `match` on a language
// here, which is what makes adding one a matter of adding a data file rather than of editing
// a branch.
//
// A token several languages could have written gets a term from each of them, all at the
// same position in the document. That is what makes an ambiguous word findable by both
// readings: the index holds `индекс` and whatever Kazakh makes of the same letters, and a
// query in either language reaches the document. Duplicates collapse — most words that two
// languages could have written they reduce the same way — so the cost is only paid where the
// readings actually differ.
//
// Expansion happens when writing and not when asking, and the difference is not symmetry for
// its own sake. A document holds every reading of its words, so a query needs one of them to
// find it; a query that expanded too would hand the parser several terms in one position,
// which it reads as a phrase — words that must follow one another — and a phrase of two
// readings of one word matches nothing. Measured on this repository''s corpus: Russian recall
// fell from 0.95 to 0.39 with an expanding query, and stayed at 0.95 with a single-reading
// one.
use rust_stemmers::{Algorithm, Stemmer};
use tantivy::tokenizer::{
    LowerCaser, SimpleTokenizer, TextAnalyzer, Token, TokenFilter, TokenStream, Tokenizer,
};

use crate::domain::{NormalizationSet, SnowballLanguage, StemRule, SuffixRules};

/// The splitting rule and its version, as the fingerprint records it.
///
/// `SimpleTokenizer` splits on everything that is not alphanumeric and `LowerCaser` folds
/// case. The splitting rule is worth naming: it treats Russian quotation marks as
/// separators, which the engine's own tokenizer did not — there a word inside guillemets
/// became the token `«слово»` and could never be found by its bare form, 108 of them in the
/// corpus of ADR 0011. The version changes when the rule does, which makes every index
/// written by the old rule report itself as written by another analyzer.
pub(super) const TOKENIZER: &str = "simple-lowercase/1";

/// The name a tantivy schema refers to the prefix analyzer by: the second way a conversation
/// name is written into the index.
// Keeps the product's earlier name: it is written in the schema of every existing index.
pub(super) const PREFIX_ANALYZER: &str = "crosscode-prefix";

/// How many characters of a word the prefix rule keeps, and the version of that rule.
///
/// Five, because that is what Stage 29 used and Stage 31 measured: run through one column on
/// one corpus, the crude rule scored 0.80 strict against 0.73 for morphology, and the two
/// fail in opposite directions — a typo kills the stemmer (`разборали` stems to `разбора`,
/// which is not `разбор`) while the first five letters survive it, and an inflection kills the
/// prefix (`форму` is not `формы`) while the stemmer folds both to `форм`. Neither is better;
/// the conversation name is written both ways and the question asks both.
pub(super) const PREFIX_CHARACTERS: usize = 5;
pub(super) const PREFIX_RULE: &str = "prefix-5/1";

/// What the fingerprint records about how this build reduces words, both ways at once.
///
/// One string because a fingerprint is compared and never parsed; both rules because an index
/// whose name field was cut at five letters is not the same index as one cut at six, and
/// nothing else in the fingerprint would move if that number changed.
pub(super) fn analyzer_rules() -> String {
    format!("{TOKENIZER}+{PREFIX_RULE}")
}

/// The analyzer that keeps the first letters of each word and applies no morphology at all.
pub(super) fn prefix_analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(PrefixFilter {
            characters: PREFIX_CHARACTERS,
        })
        .build()
}

/// Cuts every token to its first `characters` characters.
///
/// By characters and never by bytes: a Cyrillic letter is two bytes, and five bytes of
/// `контрагент` would be two and a half letters and not a word at all.
#[derive(Clone)]
struct PrefixFilter {
    characters: usize,
}

impl TokenFilter for PrefixFilter {
    type Tokenizer<T: Tokenizer> = PrefixTokenizer<T>;

    fn transform<T: Tokenizer>(self, tokenizer: T) -> PrefixTokenizer<T> {
        PrefixTokenizer {
            inner: tokenizer,
            characters: self.characters,
        }
    }
}

#[derive(Clone)]
struct PrefixTokenizer<T> {
    inner: T,
    characters: usize,
}

impl<T: Tokenizer> Tokenizer for PrefixTokenizer<T> {
    type TokenStream<'a> = PrefixTokenStream<T::TokenStream<'a>>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        PrefixTokenStream {
            tail: self.inner.token_stream(text),
            characters: self.characters,
            current: Token::default(),
        }
    }
}

struct PrefixTokenStream<T> {
    tail: T,
    characters: usize,
    current: Token,
}

impl<T: TokenStream> TokenStream for PrefixTokenStream<T> {
    fn advance(&mut self) -> bool {
        if !self.tail.advance() {
            return false;
        }
        self.current = self.tail.token().clone();
        self.current.text = self.current.text.chars().take(self.characters).collect();
        true
    }

    fn token(&self) -> &Token {
        &self.current
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.current
    }
}

/// The name a tantivy schema refers to the analyzer by. The schema stores the name, not the
/// rule, so an index reopened later gets whatever is registered under this name — which is
/// exactly why the fingerprint is stored beside it.
// Keeps the product's earlier name: it is written in the schema of every existing index.
pub(super) const ANALYZER: &str = "crosscode-normalizing";

/// Whether the analyzer is writing an index or reading a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reading {
    /// Every reading of every token, so the document is findable by any of them.
    Every,
    /// One reading per word — the n-th, or the first when a word has fewer than that.
    ///
    /// A question is parsed once per reading and the parses are joined by "or", because
    /// several terms in one position are a phrase to the parser and a phrase of two readings
    /// of one word matches nothing.
    Only(usize),
}

/// The analyzer for one set of normalizations.
pub(super) fn analyzer(normalizations: &NormalizationSet, reading: Reading) -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(NormalizingFilter {
            normalizations: normalizations.clone(),
            reading,
        })
        .build()
}

/// Reduces each token by every normalization that claims it, and leaves every other token
/// exactly as it was written.
#[derive(Clone)]
struct NormalizingFilter {
    normalizations: NormalizationSet,
    reading: Reading,
}

impl TokenFilter for NormalizingFilter {
    type Tokenizer<T: Tokenizer> = NormalizingTokenizer<T>;

    fn transform<T: Tokenizer>(self, tokenizer: T) -> NormalizingTokenizer<T> {
        NormalizingTokenizer {
            inner: tokenizer,
            normalizations: self.normalizations,
            reading: self.reading,
        }
    }
}

#[derive(Clone)]
struct NormalizingTokenizer<T> {
    inner: T,
    normalizations: NormalizationSet,
    reading: Reading,
}

impl<T: Tokenizer> Tokenizer for NormalizingTokenizer<T> {
    type TokenStream<'a> = NormalizingTokenStream<T::TokenStream<'a>>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        NormalizingTokenStream {
            tail: self.inner.token_stream(text),
            // One prepared stemmer per active normalization, in the same order, built once
            // per stream rather than once per token: creating a Snowball stemmer is not
            // free and a chunk is thousands of tokens.
            stemmers: self
                .normalizations
                .active()
                .iter()
                .map(|normalization| match normalization.stem() {
                    StemRule::Snowball(language) => {
                        Reducer::Snowball(Stemmer::create(algorithm(*language)))
                    }
                    StemRule::Suffixes(rules) => Reducer::Suffixes(rules.clone()),
                })
                .collect(),
            normalizations: self.normalizations.clone(),
            reading: self.reading,
            current: Token::default(),
            pending: Vec::new(),
        }
    }
}

/// The prepared form of one normalization's rule: an algorithm from a library, or the
/// endings a pack listed.
enum Reducer {
    Snowball(Stemmer),
    Suffixes(SuffixRules),
}

impl Reducer {
    fn stem(&self, token: &str) -> String {
        match self {
            Self::Snowball(stemmer) => stemmer.stem(token).into_owned(),
            Self::Suffixes(rules) => rules.stem(token),
        }
    }
}

struct NormalizingTokenStream<T> {
    tail: T,
    normalizations: NormalizationSet,
    reading: Reading,
    stemmers: Vec<Reducer>,
    /// The token this stream is showing, which is the source token with its text replaced.
    current: Token,
    /// The readings of the token in hand that have not been shown yet, in reverse order so
    /// the first one is popped first.
    pending: Vec<String>,
}

impl<T: TokenStream> NormalizingTokenStream<T> {
    /// Every distinct stem of `token`, in the configured order of the normalizations that
    /// claimed it.
    fn readings(&self, token: &str) -> Vec<String> {
        let claims = self.normalizations.claiming(token);
        let wanted = match self.reading {
            Reading::Every => claims.as_slice(),
            Reading::Only(ordinal) => {
                let chosen = ordinal.min(claims.len().saturating_sub(1));
                &claims.as_slice()[chosen..(chosen + 1).min(claims.len())]
            }
        };
        let mut readings: Vec<String> = Vec::with_capacity(wanted.len());
        for position in wanted {
            let stem = self.stemmers[*position].stem(token);
            if !readings.contains(&stem) {
                readings.push(stem);
            }
        }
        readings
    }
}

impl<T: TokenStream> TokenStream for NormalizingTokenStream<T> {
    fn advance(&mut self) -> bool {
        // Another reading of the token already in hand: same position, same offsets, so the
        // two are alternatives of one word rather than two words.
        if let Some(reading) = self.pending.pop() {
            self.current.text = reading;
            return true;
        }
        if !self.tail.advance() {
            return false;
        }
        self.current = self.tail.token().clone();
        let mut readings = self.readings(&self.current.text);
        if readings.is_empty() {
            // A token no normalization claims enters the index exactly as written. That is
            // the whole answer for a word in a language this build does not normalize: its
            // own form, which at least matches itself, instead of a stem invented for it by
            // rules meant for another language.
            return true;
        }
        self.current.text = readings.remove(0);
        readings.reverse();
        self.pending = readings;
        true
    }

    fn token(&self) -> &Token {
        &self.current
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.current
    }
}

/// The stem one normalization gives one word, for a test that pins what a pack does. The
/// analyzer itself goes through `Reducer`, which prepares the stemmer once per stream.
#[cfg(test)]
pub(super) fn stem_with_snowball(
    normalization: &crate::domain::Normalization,
    word: &str,
) -> String {
    match normalization.stem() {
        StemRule::Snowball(language) => Stemmer::create(algorithm(*language))
            .stem(word)
            .into_owned(),
        StemRule::Suffixes(rules) => rules.stem(word),
    }
}

fn algorithm(language: SnowballLanguage) -> Algorithm {
    match language {
        SnowballLanguage::Russian => Algorithm::Russian,
        SnowballLanguage::English => Algorithm::English,
    }
}

#[cfg(test)]
mod tests {
    use super::super::language_pack::{built_in_normalizations, load_normalizations};
    use super::{Reading, analyzer};
    use crate::domain::NormalizationSet;
    use tantivy::tokenizer::TokenStream;

    fn terms_with(set: &NormalizationSet, text: &str) -> Vec<String> {
        placed_with(set, text)
            .into_iter()
            .map(|(term, _)| term)
            .collect()
    }

    /// Every term with the position it occupies, which is what says whether two terms are
    /// two words or two readings of one. Written the way an index is written: every reading.
    fn placed_with(set: &NormalizationSet, text: &str) -> Vec<(String, usize)> {
        placed_reading(set, text, Reading::Every)
    }

    /// The same, for whichever side is being asked about.
    fn placed_reading(
        set: &NormalizationSet,
        text: &str,
        reading: Reading,
    ) -> Vec<(String, usize)> {
        let mut analyzer = analyzer(set, reading);
        let mut stream = analyzer.token_stream(text);
        let mut terms = Vec::new();
        while stream.advance() {
            let token = stream.token();
            terms.push((token.text.clone(), token.position));
        }
        terms
    }

    fn terms(text: &str) -> Vec<String> {
        terms_with(&built_in_normalizations(), text)
    }

    /// One sentence, two languages, each word reduced by the rules of its own — which is
    /// the case a single stemmer per index cannot serve at all (ADR 0011).
    #[test]
    fn a_mixed_sentence_is_normalized_word_by_word() {
        assert_eq!(
            terms("The walker обходит trees и пишет индексы"),
            vec!["the", "walker", "обход", "tree", "и", "пишет", "индекс"]
        );
    }

    #[test]
    fn russian_and_english_forms_each_reduce_to_one_term() {
        for forms in [
            ["индекс", "индексы", "индексов"],
            ["источник", "источника", "источников"],
            ["index", "indexes", "indexed"],
            ["commit", "commits", "committed"],
        ] {
            let stems: Vec<String> = forms.iter().map(|form| terms(form)[0].clone()).collect();
            assert!(
                stems.windows(2).all(|pair| pair[0] == pair[1]),
                "{forms:?} must share a stem, got {stems:?}"
            );
        }
    }

    /// With Kazakh undeclared, a Kazakh word keeps its own form and a Russian word beside it
    /// is still stemmed: the rule is per token, and a language nobody declared normalizes
    /// nothing.
    #[test]
    fn an_undeclared_language_keeps_its_own_forms() {
        assert_eq!(
            terms("қазақша индексы мүмкіндік"),
            vec!["қазақша", "индекс", "мүмкіндік"]
        );
        assert_ne!(terms("кітап")[0], terms("кітаптар")[0]);
    }

    fn with_kazakh() -> NormalizationSet {
        load_normalizations(&["ru", "en", "kk"]).unwrap()
    }

    /// Declared, Kazakh reduces its own words: a letter only it writes settles the token,
    /// and its forms meet at one term.
    #[test]
    fn a_letter_only_kazakh_writes_sends_the_token_to_kazakh_alone() {
        let set = with_kazakh();
        for forms in [
            ["кітап", "кітаптар", "кітаптарымызда"],
            ["мүмкіндік", "мүмкіндіктер", "мүмкіндікте"],
            ["қала", "қалалар", "қаласында"],
        ] {
            let stems: Vec<String> = forms
                .iter()
                .map(|form| terms_with(&set, form)[0].clone())
                .collect();
            assert!(
                stems.windows(2).all(|pair| pair[0] == pair[1]),
                "{forms:?} must meet at one term, got {stems:?}"
            );
            assert_eq!(
                terms_with(&set, forms[1]).len(),
                1,
                "a token its own letters give away needs no second reading"
            );
        }
    }

    /// A token neither language gives away is read by both, at one position. Two terms for
    /// one word is the price of not knowing which language wrote it — and the reason a
    /// query in either language reaches the document.
    #[test]
    fn a_token_no_letter_gives_away_is_indexed_by_both_readings_at_one_position() {
        let placed = placed_with(&with_kazakh(), "хранилище");

        assert_eq!(
            placed,
            vec![("хранилищ".to_string(), 0), ("хранилище".to_string(), 0)],
            "both readings, same position"
        );
    }

    /// Where the two readings agree the duplicate collapses and the index holds one term,
    /// exactly as before Kazakh was declared. Where they disagree it holds two — and they
    /// disagree whenever a Russian ending happens to look like a Kazakh one, which is what
    /// makes the size of this index a thing to measure rather than to assume.
    #[test]
    fn a_reading_costs_a_term_only_where_the_two_disagree() {
        let set = with_kazakh();
        assert_eq!(terms_with(&set, "обход"), vec!["обход"]);
        assert_eq!(terms_with(&set, "и"), vec!["и"]);
        // `сы` is a Kazakh possessive ending, so Kazakh reads one letter more off this word
        // than Russian does, and the document is findable by either.
        assert_eq!(terms_with(&set, "индексы"), vec!["индекс", "индек"]);
    }

    /// Declaring Kazakh changes nothing about the other script: a Latin token is reduced by
    /// English, once, whatever else is declared.
    #[test]
    fn declaring_kazakh_leaves_the_other_script_alone() {
        let latin =
            |set: &NormalizationSet| -> Vec<String> { terms_with(set, "The walker reports trees") };

        assert_eq!(latin(&with_kazakh()), latin(&built_in_normalizations()));
        assert_eq!(latin(&with_kazakh()), ["the", "walker", "report", "tree"]);
    }

    #[test]
    fn a_token_of_mixed_alphabets_or_no_letters_is_left_alone() {
        assert_eq!(terms("сервerный"), vec!["сервerный"]);
        assert_eq!(terms("2026"), vec!["2026"]);
    }

    /// The defect that decided against the engine's extension: there a word in guillemets
    /// became one token with the quotes inside it, unfindable by its own spelling.
    #[test]
    fn russian_quotation_marks_are_separators() {
        assert_eq!(
            terms("раздел «Контексты» описан"),
            terms("раздел Контексты описан")
        );
    }

    #[test]
    fn identifiers_split_on_punctuation() {
        assert_eq!(terms("assert_eq!(a, b)"), vec!["assert", "eq", "a", "b"]);
    }

    /// The configured set is what the analyzer applies: with Russian switched off, Russian
    /// words are indexed in the form they were written in.
    #[test]
    fn the_configured_set_decides_what_is_normalized() {
        let english_only = load_normalizations(&["en"]).unwrap();
        assert_eq!(
            terms_with(&english_only, "индексы и trees"),
            vec!["индексы", "и", "tree"]
        );
        let nothing = load_normalizations::<&str>(&[]).unwrap();
        assert_eq!(
            terms_with(&nothing, "индексы и trees"),
            vec!["индексы", "и", "trees"]
        );
    }

    /// A question is read as one term per word, because several terms in one position are a
    /// phrase to the parser and a phrase of two readings of one word matches nothing. The
    /// document holds both readings, so one of them finds it.
    #[test]
    fn a_question_is_read_as_one_term_per_word() {
        let set = with_kazakh();
        let asked: Vec<String> = placed_reading(&set, "хранилище индексы", Reading::Only(0))
            .into_iter()
            .map(|(term, _)| term)
            .collect();

        assert_eq!(asked, ["хранилищ", "индекс"]);
        // And what the index holds for the same words is every reading of them.
        assert_eq!(
            terms_with(&set, "хранилище индексы"),
            ["хранилищ", "хранилище", "индекс", "индек"]
        );
    }
}
