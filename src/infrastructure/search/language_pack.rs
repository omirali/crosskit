// Reading a language pack: the data that says how one language's words are reduced.
//
// A pack is data, never code. It says what the language is called, which version of its
// rules this is, the script and letters it writes, the letters that give it away and how it
// stems — an algorithm named by its language, or the endings it glues on. Adding a language
// is adding one of these files and naming it in the registry below; nothing in this program
// grows a branch for it.
//
// The packs travel inside the binary because they are part of what this build does: an index
// is written by the rules this binary carries, and its fingerprint names their versions. The
// file is parsed when a configuration declares the language, not when the program is built,
// so a pack that cannot be read is a configuration that does not load rather than a build
// that does not happen.
use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;

use crate::domain::{
    Alphabet, Normalization, NormalizationError, NormalizationSet, Script, SnowballLanguage,
    StemRule, SuffixRuleError, SuffixRules, SuffixStep,
};

/// The packs this build carries. A list, not a match: the only thing a new language changes
/// here is one more line.
const PACKS: &[(&str, &str)] = &[
    ("ru", include_str!("language_packs/ru.toml")),
    ("en", include_str!("language_packs/en.toml")),
    ("kk", include_str!("language_packs/kk.toml")),
];

/// What a configuration that says nothing about languages means: the two this program was
/// measured on, in this order.
///
/// Kazakh is carried but not declared by default. Declaring it would change the analyzer of
/// every existing index — every Cyrillic word would be indexed by two languages instead of
/// one — and that is a corpus owner's decision, not a default (ADR 0017).
pub const DEFAULT_LANGUAGES: &[&str] = &["ru", "en"];

/// The set a configuration named, in the order it named them.
pub fn load_normalizations<Name: AsRef<str>>(
    names: &[Name],
) -> Result<NormalizationSet, LanguagePackError> {
    let mut active = Vec::with_capacity(names.len());
    for name in names {
        active.push(read(name.as_ref())?);
    }
    NormalizationSet::new(active).map_err(LanguagePackError::Refused)
}

/// `ru` and `en`: what an index is written with unless a configuration says otherwise.
pub fn built_in_normalizations() -> NormalizationSet {
    load_normalizations(DEFAULT_LANGUAGES).expect("the packs this build carries must parse")
}

/// The names this build has packs for, for a message that has to say what it does have.
pub fn available() -> Vec<&'static str> {
    PACKS.iter().map(|(name, _)| *name).collect()
}

fn read(name: &str) -> Result<Normalization, LanguagePackError> {
    let (_, text) = PACKS
        .iter()
        .find(|(packed, _)| *packed == name)
        .ok_or_else(|| LanguagePackError::Unknown(name.to_string()))?;
    parse_pack(name, text)
}

/// One pack's data as a normalization, or the reason it cannot be one.
///
/// Named apart from the packs this build carries so a test can hand it text: the packs here
/// are correct by the time anybody runs, and what has to be shown is what happens to one
/// that is not.
pub(super) fn parse_pack(name: &str, text: &str) -> Result<Normalization, LanguagePackError> {
    let file: PackFile = toml::from_str(text).map_err(|error| LanguagePackError::Unreadable {
        name: name.to_string(),
        reason: error.message().to_string(),
    })?;
    file.into_normalization(name)
}

/// The shape of a pack file. Unknown keys are refused: a rule nobody reads is a rule the
/// author believes is in force.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackFile {
    name: String,
    version: String,
    script: String,
    #[serde(default)]
    alphabet: String,
    #[serde(default)]
    alphabet_ranges: Vec<String>,
    #[serde(default)]
    evidence: String,
    stemming: StemmingSection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StemmingSection {
    kind: String,
    #[serde(default)]
    algorithm: Option<String>,
    #[serde(default)]
    minimum_stem_chars: Option<usize>,
    #[serde(default)]
    step: Vec<StepSection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepSection {
    name: String,
    suffixes: Vec<String>,
}

impl PackFile {
    fn into_normalization(self, declared: &str) -> Result<Normalization, LanguagePackError> {
        if self.name != declared {
            return Err(LanguagePackError::Unreadable {
                name: declared.to_string(),
                reason: format!("the pack calls itself {:?}", self.name),
            });
        }
        let alphabet = Alphabet::new(
            self.alphabet.chars().collect(),
            parse_ranges(&self.alphabet_ranges, declared)?,
        )
        .map_err(LanguagePackError::Refused)?;
        let normalization = Normalization::new(
            self.name,
            self.version,
            parse_script(&self.script, declared)?,
            alphabet,
            self.evidence.chars().collect::<BTreeSet<char>>(),
            self.stemming.into_rule(declared)?,
        )
        .map_err(LanguagePackError::Refused)?;
        Ok(normalization)
    }
}

impl StemmingSection {
    fn into_rule(self, name: &str) -> Result<StemRule, LanguagePackError> {
        match self.kind.as_str() {
            "snowball" => {
                let algorithm = self
                    .algorithm
                    .ok_or_else(|| LanguagePackError::Unreadable {
                        name: name.to_string(),
                        reason: "a snowball rule names no algorithm".to_string(),
                    })?;
                let language = match algorithm.as_str() {
                    "russian" => SnowballLanguage::Russian,
                    "english" => SnowballLanguage::English,
                    other => {
                        return Err(LanguagePackError::Unreadable {
                            name: name.to_string(),
                            reason: format!("this build has no snowball algorithm {other:?}"),
                        });
                    }
                };
                Ok(StemRule::Snowball(language))
            }
            "suffixes" => {
                let minimum =
                    self.minimum_stem_chars
                        .ok_or_else(|| LanguagePackError::Unreadable {
                            name: name.to_string(),
                            reason: "a suffix rule names no minimum stem length".to_string(),
                        })?;
                let mut steps = Vec::with_capacity(self.step.len());
                for step in self.step {
                    steps.push(SuffixStep::new(step.name, step.suffixes).map_err(|error| {
                        LanguagePackError::BadRule {
                            name: name.to_string(),
                            error,
                        }
                    })?);
                }
                SuffixRules::new(minimum, steps)
                    .map(StemRule::Suffixes)
                    .map_err(|error| LanguagePackError::BadRule {
                        name: name.to_string(),
                        error,
                    })
            }
            other => Err(LanguagePackError::Unreadable {
                name: name.to_string(),
                reason: format!("a stemming rule of kind {other:?} is not one this build applies"),
            }),
        }
    }
}

fn parse_script(script: &str, name: &str) -> Result<Script, LanguagePackError> {
    match script {
        "latin" => Ok(Script::Latin),
        "cyrillic" => Ok(Script::Cyrillic),
        other => Err(LanguagePackError::Unreadable {
            name: name.to_string(),
            reason: format!("{other:?} is not a script this build routes by"),
        }),
    }
}

/// `"00C0-024F"` as a pair of characters. Hexadecimal code points, because the ranges this
/// is for are blocks of the standard and everybody writing one has the block chart in front
/// of them.
fn parse_ranges(ranges: &[String], name: &str) -> Result<Vec<(char, char)>, LanguagePackError> {
    let mut parsed = Vec::with_capacity(ranges.len());
    for range in ranges {
        let unreadable = || LanguagePackError::Unreadable {
            name: name.to_string(),
            reason: format!("{range:?} is not an alphabet range like \"0041-005A\""),
        };
        let (from, to) = range.split_once('-').ok_or_else(unreadable)?;
        parsed.push((
            code_point(from).ok_or_else(unreadable)?,
            code_point(to).ok_or_else(unreadable)?,
        ));
    }
    Ok(parsed)
}

fn code_point(text: &str) -> Option<char> {
    char::from_u32(u32::from_str_radix(text.trim(), 16).ok()?)
}

/// What a declared language could not become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguagePackError {
    /// No pack of that name travels in this build.
    Unknown(String),
    /// The pack is there and does not say what it has to.
    Unreadable { name: String, reason: String },
    /// Its stemming rules are not rules.
    BadRule {
        name: String,
        error: SuffixRuleError,
    },
    /// The set they make cannot route.
    Refused(NormalizationError),
}

impl fmt::Display for LanguagePackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(
                f,
                "no language pack is called {name:?}; this build carries {}",
                available().join(", ")
            ),
            Self::Unreadable { name, reason } => {
                write!(f, "the language pack {name:?} cannot be read: {reason}")
            }
            Self::BadRule { name, error } => {
                write!(
                    f,
                    "the language pack {name:?} has a rule that is not one: {error}"
                )
            }
            Self::Refused(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LanguagePackError {}
