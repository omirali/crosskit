// The period phrases of every built-in language, read from the files beside the language packs.
//
// Data and not branches, for the reason the packs themselves are (ADR 0017): which words of a
// language name "last week" is a fact about that language, and a person adding one should edit
// a list of words rather than a `match`. The files are compiled in, so a build carries exactly
// the phrases it was tested with.
use std::collections::BTreeMap;

use serde::Deserialize;

use crate::domain::{PeriodPhrase, PeriodPhrases, PeriodPhrasesError};

const PACKS: [(&str, &str); 2] = [
    ("ru", include_str!("language_packs/periods/ru.toml")),
    ("en", include_str!("language_packs/periods/en.toml")),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhraseFile {
    language: String,
    phrases: BTreeMap<String, Vec<String>>,
}

/// What a phrase file cannot say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeriodPackError {
    Unreadable {
        language: &'static str,
    },
    /// The file names a language other than the one it is filed under.
    WrongLanguage {
        language: &'static str,
    },
    /// A key no build of this program knows.
    UnknownPeriod {
        language: &'static str,
        key: String,
    },
    Phrases(PeriodPhrasesError),
}

impl std::fmt::Display for PeriodPackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable { language } => {
                write!(formatter, "the period phrases of {language} cannot be read")
            }
            Self::WrongLanguage { language } => write!(
                formatter,
                "the period phrases filed under {language} name another language"
            ),
            Self::UnknownPeriod { language, key } => write!(
                formatter,
                "the period phrases of {language} name a period this build does not know: {key}"
            ),
            Self::Phrases(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PeriodPackError {}

/// Every phrase of every built-in language.
pub fn built_in_period_phrases() -> Result<PeriodPhrases, PeriodPackError> {
    let mut entries = Vec::new();
    for (language, text) in PACKS {
        for (phrase, written) in parse_pack(language, text)? {
            entries.push((phrase, written));
        }
    }
    PeriodPhrases::new(entries).map_err(PeriodPackError::Phrases)
}

fn parse_pack(
    language: &'static str,
    text: &str,
) -> Result<Vec<(PeriodPhrase, String)>, PeriodPackError> {
    let file: PhraseFile =
        toml::from_str(text).map_err(|_| PeriodPackError::Unreadable { language })?;
    if file.language != language {
        return Err(PeriodPackError::WrongLanguage { language });
    }
    let mut entries = Vec::new();
    for (key, phrases) in file.phrases {
        let Some(period) = PeriodPhrase::from_key(&key) else {
            return Err(PeriodPackError::UnknownPeriod { language, key });
        };
        entries.extend(phrases.into_iter().map(|phrase| (period, phrase)));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{PACKS, PeriodPackError, built_in_period_phrases, parse_pack};
    use crate::domain::PeriodPhrase;

    /// Every period of the closed set, as the files spell their keys.
    fn every_period() -> HashSet<PeriodPhrase> {
        let mut all: HashSet<PeriodPhrase> = [
            PeriodPhrase::Today,
            PeriodPhrase::Yesterday,
            PeriodPhrase::ThisWeek,
            PeriodPhrase::LastWeek,
            PeriodPhrase::ThisMonth,
            PeriodPhrase::LastMonth,
        ]
        .into_iter()
        .collect();
        all.extend((1..=12).map(PeriodPhrase::Month));
        all
    }

    #[test]
    fn the_built_in_phrases_load() {
        built_in_period_phrases().unwrap();
    }

    /// Each language names every period: a phrase that works in one language and is silent
    /// in the other is a period nobody would know to rely on.
    #[test]
    fn every_language_names_every_period() {
        for (language, text) in PACKS {
            let named: HashSet<PeriodPhrase> = parse_pack(language, text)
                .unwrap()
                .into_iter()
                .map(|(period, _)| period)
                .collect();
            assert_eq!(named, every_period(), "{language}");
        }
    }

    /// The phrases the stage was written for, in both languages, each read as what it says.
    #[test]
    fn the_phrases_of_the_stage_are_read_as_what_they_say() {
        let phrases = built_in_period_phrases().unwrap();
        for (question, period, remaining) in [
            (
                "над чем я работал на прошлой неделе?",
                PeriodPhrase::LastWeek,
                "над чем я работал",
            ),
            ("что я менял вчера", PeriodPhrase::Yesterday, "что я менял"),
            ("что сделано сегодня", PeriodPhrase::Today, "что сделано"),
            (
                "что обсуждали на этой неделе",
                PeriodPhrase::ThisWeek,
                "что обсуждали",
            ),
            (
                "что было в этом месяце",
                PeriodPhrase::ThisMonth,
                "что было",
            ),
            (
                "что было в прошлом месяце",
                PeriodPhrase::LastMonth,
                "что было",
            ),
            (
                "что решили в сентябре",
                PeriodPhrase::Month(9),
                "что решили",
            ),
            (
                "what did I fix last week",
                PeriodPhrase::LastWeek,
                "what did I fix",
            ),
            (
                "what changed in May?",
                PeriodPhrase::Month(5),
                "what changed",
            ),
            ("anything new today", PeriodPhrase::Today, "anything new"),
        ] {
            let stripped = phrases.strip(question);
            assert_eq!(
                stripped.found().map(|found| found.phrase()),
                Some(period),
                "{question}"
            );
            assert_eq!(stripped.remaining(), remaining, "{question}");
        }
    }

    #[test]
    fn a_file_naming_an_unknown_period_or_another_language_is_refused() {
        assert_eq!(
            parse_pack(
                "ru",
                "language = \"ru\"\n[phrases]\nfortnight = [\"за две недели\"]\n"
            ),
            Err(PeriodPackError::UnknownPeriod {
                language: "ru",
                key: "fortnight".to_string()
            })
        );
        assert_eq!(
            parse_pack("ru", "language = \"en\"\n[phrases]\n"),
            Err(PeriodPackError::WrongLanguage { language: "ru" })
        );
        assert_eq!(
            parse_pack("ru", "language = \"ru\"\nextra = 1\n[phrases]\n"),
            Err(PeriodPackError::Unreadable { language: "ru" })
        );
    }
}
