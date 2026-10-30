// Which normalizations a token belongs to, decided by the letters in the token itself.
//
// A normalization is a named unit — the name a configuration uses for a language, the
// version of its rules, the script and letters it writes, the letters that give it away, and
// how it reduces a token to a stem. Every one of them is data: the two that reduce by
// Snowball and the ones that reduce by a list of endings are the same shape, and adding a
// language is adding that data rather than a branch in this file.
//
// Routing is per token and by evidence, never by a language assigned to a document. A file
// of Russian prose with English identifiers in it has both in one sentence, and the letters
// are the only thing about a token this program has actually read. That is the same narrow
// claim `ChunkScript` makes about a chunk, applied one token at a time.
//
// Three cases, and the third is what this file is really about:
//
//   * a letter only one declared language of that script has — `қ` among Kazakh and Russian
//     — settles it, and that language alone reduces the token;
//   * no such letter, and several declared languages of that script could have written it:
//     every one of them reduces it, and each distinct stem enters the index at the same
//     position. A word that is Russian keeps being found by its Russian forms, and the same
//     word read as Kazakh is found by its Kazakh ones. Ambiguity costs terms, not words;
//   * letters nobody declared, or two alphabets in one token: it enters the index exactly as
//     written, which at least matches itself.
use std::collections::BTreeSet;
use std::fmt;

pub use super::alphabet::{Alphabet, Script};

pub use super::suffix_stemming::{SuffixRuleError, SuffixRules, SuffixStep};

/// How a normalization reduces a token that it claims.
///
/// A value rather than a `match` on the name: one language's rules are an algorithm somebody
/// else wrote, another's are a list of endings in a data file, and every place that applies a
/// rule handles both without asking which language it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StemRule {
    /// The Snowball algorithm for this language.
    Snowball(SnowballLanguage),
    /// The endings this language glues on, stripped layer by layer.
    Suffixes(SuffixRules),
}

/// The Snowball algorithms this program applies. Named here, applied in infrastructure: the
/// domain says which rule a token gets, not which library runs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnowballLanguage {
    Russian,
    English,
}

/// One named way to reduce a token to a stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalization {
    id: String,
    version: String,
    script: Script,
    alphabet: Alphabet,
    evidence: BTreeSet<char>,
    stem: StemRule,
}

impl Normalization {
    /// A normalization as a pack describes it, refused when the description cannot be one.
    ///
    /// The evidence letters must be letters this language writes: a letter it does not have
    /// could not be evidence of it, and a pack claiming one would quietly take tokens away
    /// from the language that does.
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        script: Script,
        alphabet: Alphabet,
        evidence: BTreeSet<char>,
        stem: StemRule,
    ) -> Result<Self, NormalizationError> {
        let id = id.into();
        let version = version.into();
        if id.trim().is_empty() {
            return Err(NormalizationError::Unnamed);
        }
        if version.trim().is_empty() {
            return Err(NormalizationError::Unversioned(id));
        }
        if let Some(outside) = evidence.iter().find(|letter| !alphabet.holds(**letter)) {
            return Err(NormalizationError::EvidenceOutsideAlphabet {
                id,
                letter: *outside,
            });
        }
        Ok(Self {
            id,
            version,
            script,
            alphabet,
            evidence,
            stem,
        })
    }

    /// The name a configuration writes and the fingerprint records.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The version of this normalization's rules. It is part of the analyzer fingerprint, so
    /// changing what the rules do means changing this, and every index written by the old
    /// rules is then reported as written by another analyzer.
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn stem(&self) -> &StemRule {
        &self.stem
    }

    /// Whether every letter of `token` is one this normalization writes, and there is at
    /// least one letter.
    ///
    /// Every letter, not most of them: a token mixing two alphabets is something no stemming
    /// rule was written for, and applying one invents a stem. Digits and punctuation decide
    /// nothing, so `сервер2` is a Cyrillic token while `2026` is nobody's.
    fn could_have_written(&self, token: &str) -> bool {
        let mut letters = false;
        for character in token.chars() {
            if !character.is_alphabetic() {
                continue;
            }
            if !self.alphabet.holds(character) {
                return false;
            }
            letters = true;
        }
        letters
    }

    fn is_given_away_by(&self, token: &str) -> bool {
        !self.evidence.is_empty() && token.chars().any(|letter| self.evidence.contains(&letter))
    }
}

/// How many normalizations may reduce one token.
///
/// A ceiling rather than a preference: every extra variant is another term in the index for
/// the same word, and a configuration that declared a dozen languages of one script would
/// multiply the index by a dozen without anybody deciding to. Four is past what any declared
/// set here reaches and small enough to hold on the stack.
pub const MAX_CLAIMS: usize = 4;

/// Which active normalizations reduce one token, by their position in the set.
///
/// Positions rather than the normalizations themselves, so a caller holding one prepared
/// stemmer per active normalization reaches it without a lookup. No allocation: this is
/// asked once per token, and a corpus is millions of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Claims {
    positions: [usize; MAX_CLAIMS],
    count: usize,
}

impl Claims {
    fn push(&mut self, position: usize) {
        if self.count < MAX_CLAIMS {
            self.positions[self.count] = position;
            self.count += 1;
        }
    }

    pub fn as_slice(&self) -> &[usize] {
        &self.positions[..self.count]
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn len(&self) -> usize {
        self.count
    }
}

/// The normalizations one index is written and read with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizationSet {
    active: Vec<Normalization>,
}

impl NormalizationSet {
    /// The set a configuration named, in the order it named them.
    ///
    /// Two checks decide whether this set can route at all. A name given twice is refused —
    /// a set holds a normalization once, and the fingerprint must not list one twice. And a
    /// letter one language declares as evidence must not be a letter another declared
    /// language of the same script writes: evidence that two languages share is not evidence,
    /// and a set built on it would send tokens to whichever pack was listed first.
    pub fn new(active: Vec<Normalization>) -> Result<Self, NormalizationError> {
        for (position, normalization) in active.iter().enumerate() {
            if active[..position]
                .iter()
                .any(|earlier| earlier.id == normalization.id)
            {
                return Err(NormalizationError::Repeated(normalization.id.clone()));
            }
            for other in active.iter().filter(|other| other.id != normalization.id) {
                if other.script != normalization.script {
                    continue;
                }
                if let Some(shared) = normalization
                    .evidence
                    .iter()
                    .find(|letter| other.alphabet.holds(**letter))
                {
                    return Err(NormalizationError::EvidenceIsNotEvidence {
                        id: normalization.id.clone(),
                        other: other.id.clone(),
                        letter: *shared,
                    });
                }
            }
        }
        Ok(Self { active })
    }

    /// Which active normalizations reduce `token`, and nothing when it belongs in the index
    /// exactly as written.
    ///
    /// A letter that gives one language away settles the token for that language alone. With
    /// no such letter every language of that script that could have written it reduces it,
    /// in the configured order, up to `MAX_CLAIMS`.
    pub fn claiming(&self, token: &str) -> Claims {
        let mut claims = Claims::default();
        let mut given_away = Claims::default();
        for (position, normalization) in self.active.iter().enumerate() {
            if !normalization.could_have_written(token) {
                continue;
            }
            if normalization.is_given_away_by(token) {
                given_away.push(position);
            }
            claims.push(position);
        }
        if given_away.is_empty() {
            claims
        } else {
            given_away
        }
    }

    /// The active normalizations in their configured order.
    pub fn active(&self) -> &[Normalization] {
        &self.active
    }

    /// The most readings any one token can have in this set: the largest number of declared
    /// languages sharing a script, never more than the ceiling.
    ///
    /// A question has to be asked in every reading a word may have been written in, and this
    /// says how many that is. One, for every set that declares one language per script —
    /// which is what a reader of a single-language corpus pays.
    pub fn max_readings(&self) -> usize {
        let mut per_script = [0usize; 2];
        for normalization in &self.active {
            let slot = match normalization.script {
                Script::Latin => 0,
                Script::Cyrillic => 1,
            };
            per_script[slot] += 1;
        }
        per_script
            .into_iter()
            .max()
            .unwrap_or(1)
            .clamp(1, MAX_CLAIMS)
    }
}

/// What a configuration or a pack asked for that this program cannot give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizationError {
    Unnamed,
    Unversioned(String),
    Repeated(String),
    EmptyAlphabet,
    BackwardRange {
        from: char,
        to: char,
    },
    EvidenceOutsideAlphabet {
        id: String,
        letter: char,
    },
    EvidenceIsNotEvidence {
        id: String,
        other: String,
        letter: char,
    },
}

impl fmt::Display for NormalizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unnamed => write!(f, "a normalization has no name"),
            Self::Unversioned(id) => write!(f, "normalization {id:?} has no version"),
            Self::Repeated(id) => write!(f, "normalization {id:?} is named twice"),
            Self::EmptyAlphabet => {
                write!(f, "a normalization that writes no letters claims nothing")
            }
            Self::BackwardRange { from, to } => {
                write!(
                    f,
                    "the alphabet range {from:?}..{to:?} ends before it starts"
                )
            }
            Self::EvidenceOutsideAlphabet { id, letter } => write!(
                f,
                "normalization {id:?} offers {letter:?} as evidence of itself, but does not write that letter"
            ),
            Self::EvidenceIsNotEvidence { id, other, letter } => write!(
                f,
                "normalization {id:?} offers {letter:?} as evidence of itself, but {other:?} writes that letter too, so it gives neither away"
            ),
        }
    }
}

impl std::error::Error for NormalizationError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        Alphabet, MAX_CLAIMS, Normalization, NormalizationError, NormalizationSet, Script,
        SnowballLanguage, StemRule, SuffixRules, SuffixStep,
    };

    fn letters(source: &str) -> BTreeSet<char> {
        source.chars().collect()
    }

    fn alphabet(source: &str) -> Alphabet {
        Alphabet::new(letters(source), Vec::new()).unwrap()
    }

    const RUSSIAN: &str = "абвгдеёжзийклмнопрстуфхцчшщъыьэюя";
    const KAZAKH: &str = "аәбвгғдеёжзийкқлмнңоөпрстуұүфхһцчшщъыіьэюя";

    fn russian() -> Normalization {
        Normalization::new(
            "ru",
            "1",
            Script::Cyrillic,
            alphabet(RUSSIAN),
            BTreeSet::new(),
            StemRule::Snowball(SnowballLanguage::Russian),
        )
        .unwrap()
    }

    /// A pack with the shape a data file has: its own alphabet, the letters that give it
    /// away and a list of endings.
    fn kazakh() -> Normalization {
        Normalization::new(
            "kk",
            "1",
            Script::Cyrillic,
            alphabet(KAZAKH),
            letters("әғқңөұүһі"),
            StemRule::Suffixes(
                SuffixRules::new(
                    3,
                    vec![
                        SuffixStep::new("plural", vec!["тар".to_string(), "лар".to_string()])
                            .unwrap(),
                    ],
                )
                .unwrap(),
            ),
        )
        .unwrap()
    }

    fn english() -> Normalization {
        Normalization::new(
            "en",
            "1",
            Script::Latin,
            Alphabet::new(
                BTreeSet::new(),
                vec![('a', 'z'), ('A', 'Z'), ('\u{00C0}', '\u{024F}')],
            )
            .unwrap(),
            BTreeSet::new(),
            StemRule::Snowball(SnowballLanguage::English),
        )
        .unwrap()
    }

    fn claimed_by<'a>(set: &'a NormalizationSet, token: &str) -> Vec<&'a str> {
        set.claiming(token)
            .as_slice()
            .iter()
            .map(|position| set.active()[*position].id())
            .collect()
    }

    /// A letter only one declared language writes settles the token for that language alone,
    /// even though the other one could have written every other letter of it.
    #[test]
    fn a_letter_that_gives_a_language_away_settles_the_token() {
        let set = NormalizationSet::new(vec![russian(), kazakh(), english()]).unwrap();
        for token in ["қазақша", "әдебиет", "мүмкіндік", "кітаптар", "өнер"]
        {
            assert_eq!(claimed_by(&set, token), ["kk"], "{token}");
        }
    }

    /// Without such a letter both languages of that script reduce the token, in the
    /// configured order. This is the case the whole multi-stem question is about: the word
    /// is Russian or Kazakh, the letters do not say, and the index holds both readings.
    #[test]
    fn a_token_no_letter_gives_away_is_claimed_by_every_language_of_its_script() {
        let set = NormalizationSet::new(vec![russian(), kazakh(), english()]).unwrap();
        assert_eq!(claimed_by(&set, "индексы"), ["ru", "kk"]);
        assert_eq!(claimed_by(&set, "сервер2"), ["ru", "kk"]);
        // Latin is another script, so nothing Cyrillic reaches it and it reaches nothing.
        assert_eq!(claimed_by(&set, "reconcile"), ["en"]);
        assert_eq!(claimed_by(&set, "café"), ["en"]);
    }

    /// Declaring one language of a script is the old behaviour, unchanged: one claim.
    #[test]
    fn one_declared_language_of_a_script_claims_alone() {
        let set = NormalizationSet::new(vec![russian(), english()]).unwrap();
        assert_eq!(claimed_by(&set, "индексы"), ["ru"]);
        assert_eq!(claimed_by(&set, "қазақша"), Vec::<&str>::new());
    }

    #[test]
    fn a_token_of_mixed_or_unknown_letters_is_claimed_by_nobody() {
        let set = NormalizationSet::new(vec![russian(), kazakh(), english()]).unwrap();
        for token in ["сервerный", "2026", "", "—", "κόσμος", "日本語"] {
            assert!(set.claiming(token).is_empty(), "{token:?}");
        }
    }

    /// The ceiling holds whatever a configuration declares: five languages of one script
    /// cannot make five terms out of one word.
    #[test]
    fn no_token_is_claimed_more_than_the_ceiling_allows() {
        let many: Vec<Normalization> = (0..MAX_CLAIMS + 2)
            .map(|ordinal| {
                Normalization::new(
                    format!("l{ordinal}"),
                    "1",
                    Script::Cyrillic,
                    alphabet(RUSSIAN),
                    BTreeSet::new(),
                    StemRule::Snowball(SnowballLanguage::Russian),
                )
                .unwrap()
            })
            .collect();
        let set = NormalizationSet::new(many).unwrap();

        let claims = set.claiming("индексы");

        assert_eq!(claims.len(), MAX_CLAIMS);
        assert_eq!(
            claimed_by(&set, "индексы"),
            ["l0", "l1", "l2", "l3"],
            "the configured order decides which ones fit"
        );
    }

    #[test]
    fn a_name_declared_twice_is_refused() {
        assert_eq!(
            NormalizationSet::new(vec![russian(), russian()]).unwrap_err(),
            NormalizationError::Repeated("ru".to_string())
        );
    }

    /// Evidence another declared language of the same script also writes is not evidence,
    /// and a set built on it would route by the order packs happened to be listed in.
    #[test]
    fn evidence_another_declared_language_writes_too_is_refused() {
        let pretender = Normalization::new(
            "xx",
            "1",
            Script::Cyrillic,
            alphabet(KAZAKH),
            letters("а"),
            StemRule::Snowball(SnowballLanguage::Russian),
        )
        .unwrap();

        let error = NormalizationSet::new(vec![russian(), pretender]).unwrap_err();

        assert!(
            matches!(error, NormalizationError::EvidenceIsNotEvidence { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("gives neither away"), "{error}");
    }

    /// A letter offered as evidence of a language that does not write it would take tokens
    /// away from the language that does.
    #[test]
    fn evidence_outside_a_language_s_own_alphabet_is_refused() {
        let error = Normalization::new(
            "ru",
            "1",
            Script::Cyrillic,
            alphabet(RUSSIAN),
            letters("қ"),
            StemRule::Snowball(SnowballLanguage::Russian),
        )
        .unwrap_err();

        assert!(
            matches!(error, NormalizationError::EvidenceOutsideAlphabet { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_normalization_without_a_name_or_a_version_is_refused() {
        let unnamed = Normalization::new(
            " ",
            "1",
            Script::Cyrillic,
            alphabet(RUSSIAN),
            BTreeSet::new(),
            StemRule::Snowball(SnowballLanguage::Russian),
        );
        assert_eq!(unnamed.unwrap_err(), NormalizationError::Unnamed);

        let unversioned = Normalization::new(
            "ru",
            "",
            Script::Cyrillic,
            alphabet(RUSSIAN),
            BTreeSet::new(),
            StemRule::Snowball(SnowballLanguage::Russian),
        );
        assert_eq!(
            unversioned.unwrap_err(),
            NormalizationError::Unversioned("ru".to_string())
        );
    }

    #[test]
    fn an_empty_set_normalizes_nothing() {
        let set = NormalizationSet::new(Vec::new()).unwrap();
        assert!(set.active().is_empty());
        assert!(set.claiming("обход").is_empty());
    }
}
