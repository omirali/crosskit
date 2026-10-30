// Reducing a word by stripping the endings its language glues on, in the order they were
// glued.
//
// This is the whole of what a language pack does to a token, and it is data: an ordered list
// of steps, each holding the endings of one grammatical layer. Kazakh writes them one after
// another — `кітап` + `тар` (plural) + `ымыз` (ours) + `да` (in) — so they come off in the
// reverse order, one layer at a time, and the pack says which layers exist and what is in
// them. Nothing here knows which language it is working on.
//
// The two built-in languages reduce by Snowball instead, which is an algorithm rather than a
// list; both are named by `StemRule` so every caller applies "the rule this token got"
// without asking which kind it is.
use std::fmt;

/// The ordered layers of endings one language pack strips, and how short a word may get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuffixRules {
    minimum_stem_chars: usize,
    steps: Vec<SuffixStep>,
}

/// One grammatical layer: the endings that may be stripped at this point, at most one of
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuffixStep {
    name: String,
    /// Sorted longest first, so `кітаптарымызда` loses `ымыз` rather than `ы`.
    suffixes: Vec<String>,
}

impl SuffixStep {
    /// A layer with `name` and the endings it may strip.
    ///
    /// The order they were written in does not decide anything: the longest match wins, and
    /// sorting here means a pack author cannot make the rule wrong by listing `ы` before
    /// `ымыз`.
    pub fn new(name: impl Into<String>, suffixes: Vec<String>) -> Result<Self, SuffixRuleError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SuffixRuleError::UnnamedStep);
        }
        if suffixes.is_empty() {
            return Err(SuffixRuleError::EmptyStep(name));
        }
        if let Some(empty) = suffixes.iter().find(|suffix| suffix.trim().is_empty()) {
            return Err(SuffixRuleError::EmptySuffix {
                step: name,
                suffix: empty.clone(),
            });
        }
        let mut sorted = suffixes;
        sorted.sort_by(|left, right| {
            right
                .chars()
                .count()
                .cmp(&left.chars().count())
                .then_with(|| left.cmp(right))
        });
        sorted.dedup();
        Ok(Self {
            name,
            suffixes: sorted,
        })
    }
}

impl SuffixRules {
    /// The steps in the order they are applied, and the shortest stem this pack accepts.
    ///
    /// The minimum is not a nicety: without it `тар` loses its own three letters and every
    /// short word collapses into nothing, which would make one term out of words that share
    /// no meaning at all.
    pub fn new(minimum_stem_chars: usize, steps: Vec<SuffixStep>) -> Result<Self, SuffixRuleError> {
        if minimum_stem_chars == 0 {
            return Err(SuffixRuleError::NoMinimumStem);
        }
        if steps.is_empty() {
            return Err(SuffixRuleError::NoSteps);
        }
        Ok(Self {
            minimum_stem_chars,
            steps,
        })
    }

    /// The stem of `token`: at most one ending off each layer, in order, never cutting below
    /// the minimum.
    ///
    /// Works in characters rather than bytes, because every letter this is for is two bytes
    /// long and a byte-wise cut would split one in half.
    pub fn stem(&self, token: &str) -> String {
        let mut letters: Vec<char> = token.chars().collect();
        for step in &self.steps {
            if let Some(shortened) = strip_one(&letters, step, self.minimum_stem_chars) {
                letters = shortened;
            }
        }
        letters.into_iter().collect()
    }
}

/// The longest ending of `step` that `letters` ends with, removed. `None` when this layer
/// takes nothing off this word, including when the ending it matched would leave too short a
/// stem.
///
/// The longest match decides, and if it cannot apply nothing else is tried. Falling back to a
/// shorter ending would strip letters the longer one said belong together: `елдің` loses
/// `дің` or stays as it is, and `ің` — leaving `елд` — is a reading of the word nobody wrote
/// a rule for.
fn strip_one(letters: &[char], step: &SuffixStep, minimum: usize) -> Option<Vec<char>> {
    for suffix in &step.suffixes {
        let suffix: Vec<char> = suffix.chars().collect();
        if suffix.len() >= letters.len() || !letters.ends_with(&suffix) {
            continue;
        }
        if letters.len() - suffix.len() < minimum {
            return None;
        }
        return Some(letters[..letters.len() - suffix.len()].to_vec());
    }
    None
}

/// What a pack said that cannot be a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuffixRuleError {
    UnnamedStep,
    EmptyStep(String),
    EmptySuffix { step: String, suffix: String },
    NoSteps,
    NoMinimumStem,
}

impl fmt::Display for SuffixRuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnnamedStep => write!(f, "a stemming step has no name"),
            Self::EmptyStep(step) => write!(f, "stemming step {step:?} lists no suffixes"),
            Self::EmptySuffix { step, suffix } => {
                write!(f, "stemming step {step:?} lists an empty suffix {suffix:?}")
            }
            Self::NoSteps => write!(f, "a suffix rule with no steps strips nothing"),
            Self::NoMinimumStem => {
                write!(f, "the minimum stem length must be at least one character")
            }
        }
    }
}

impl std::error::Error for SuffixRuleError {}

#[cfg(test)]
mod tests {
    use super::{SuffixRuleError, SuffixRules, SuffixStep};

    fn step(name: &str, suffixes: &[&str]) -> SuffixStep {
        SuffixStep::new(
            name,
            suffixes
                .iter()
                .map(|suffix| (*suffix).to_string())
                .collect(),
        )
        .unwrap()
    }

    /// The three layers of a Kazakh noun, in the order they come off.
    fn kazakh() -> SuffixRules {
        SuffixRules::new(
            3,
            vec![
                step(
                    "case",
                    &[
                        "да", "де", "та", "те", "дан", "ден", "тан", "тен", "ға", "ге",
                    ],
                ),
                step("possessive", &["ымыз", "іміз", "сы", "сі", "ы", "і"]),
                step("plural", &["лар", "лер", "дар", "дер", "тар", "тер"]),
            ],
        )
        .unwrap()
    }

    #[test]
    fn every_layer_comes_off_in_its_turn() {
        let rules = kazakh();
        assert_eq!(rules.stem("кітаптарымызда"), "кітап");
        assert_eq!(rules.stem("кітаптар"), "кітап");
        assert_eq!(rules.stem("кітапта"), "кітап");
        assert_eq!(rules.stem("кітап"), "кітап");
    }

    /// One ending per layer. A word that ends in two endings of the same layer keeps the
    /// second, because stripping both would be a rule nobody wrote.
    #[test]
    fn a_layer_strips_at_most_one_ending() {
        let rules = SuffixRules::new(2, vec![step("case", &["да", "де"])]).unwrap();
        assert_eq!(rules.stem("сөздеде"), "сөзде");
    }

    /// The longest ending of a layer wins, whatever order the pack listed them in.
    #[test]
    fn the_longest_ending_of_a_layer_wins() {
        let rules = SuffixRules::new(3, vec![step("possessive", &["ы", "ымыз"])]).unwrap();
        assert_eq!(rules.stem("кітабымыз"), "кітаб");
    }

    /// And when the longest one would cut below the minimum, the layer strips nothing rather
    /// than falling back to a shorter ending: the shorter one would take letters the longer
    /// one said belong to the same ending.
    #[test]
    fn a_layer_blocked_by_the_minimum_strips_nothing_instead_of_less() {
        let rules = SuffixRules::new(3, vec![step("case", &["дің", "ің"])]).unwrap();
        assert_eq!(rules.stem("елдің"), "елдің");
        let shorter_minimum = SuffixRules::new(2, vec![step("case", &["дің", "ің"])]).unwrap();
        assert_eq!(shorter_minimum.stem("елдің"), "ел");
    }

    /// Short words keep themselves: a rule that cut `тар` to nothing would make one term of
    /// every short word in the language.
    #[test]
    fn nothing_is_cut_below_the_minimum_stem() {
        let rules = kazakh();
        assert_eq!(rules.stem("тар"), "тар");
        assert_eq!(rules.stem("дала"), "дала");
        assert_eq!(rules.stem("тау"), "тау");
    }

    #[test]
    fn a_rule_that_cannot_be_one_is_refused() {
        assert_eq!(
            SuffixStep::new("case", vec![]).unwrap_err(),
            SuffixRuleError::EmptyStep("case".to_string())
        );
        assert_eq!(
            SuffixStep::new("", vec!["да".to_string()]).unwrap_err(),
            SuffixRuleError::UnnamedStep
        );
        assert_eq!(
            SuffixStep::new("case", vec![" ".to_string()]).unwrap_err(),
            SuffixRuleError::EmptySuffix {
                step: "case".to_string(),
                suffix: " ".to_string()
            }
        );
        assert_eq!(
            SuffixRules::new(0, vec![step("case", &["да"])]).unwrap_err(),
            SuffixRuleError::NoMinimumStem
        );
        assert_eq!(
            SuffixRules::new(3, vec![]).unwrap_err(),
            SuffixRuleError::NoSteps
        );
    }
}
