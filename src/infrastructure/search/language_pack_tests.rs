// What the packs this build carries actually do, and what a pack that cannot be one is
// answered with.
//
// The rules live in data files, which is what makes a language a matter of adding data — and
// also what makes a rule easy to change without noticing that every index written by the old
// one now answers by rules nobody applies. So the behaviour of each pack is pinned here
// against its version: change the data, and either the stems stay the same or this test
// fails and asks for the version to move.
use super::language_pack::{
    DEFAULT_LANGUAGES, LanguagePackError, available, built_in_normalizations, load_normalizations,
    parse_pack,
};
use crate::domain::{IndexFingerprint, NormalizationSet, StemRule};

fn versions(set: &NormalizationSet) -> Vec<String> {
    set.active()
        .iter()
        .map(|normalization| format!("{}/{}", normalization.id(), normalization.version()))
        .collect()
}

/// The stem each pack gives these words, applied on its own.
fn stems(language: &str, words: &[&str]) -> Vec<String> {
    let set = load_normalizations(&[language]).unwrap();
    let normalization = &set.active()[0];
    words
        .iter()
        .map(|word| match normalization.stem() {
            StemRule::Suffixes(rules) => rules.stem(word),
            StemRule::Snowball(_) => super::analyzer::stem_with_snowball(normalization, word),
        })
        .collect()
}

#[test]
fn this_build_carries_three_packs_and_declares_two() {
    assert_eq!(available(), ["ru", "en", "kk"]);
    assert_eq!(DEFAULT_LANGUAGES, ["ru", "en"]);
    assert_eq!(versions(&built_in_normalizations()), ["ru/1", "en/1"]);
}

/// What the Kazakh pack does to the forms of one word, pinned to `kk/1`.
///
/// This is the test to read when changing `kk.toml`: if a change is meant, the stems below
/// change with it and so does the version in the file — and every index written by the old
/// rules then reports itself as written by another analyzer.
#[test]
fn the_kazakh_pack_at_version_one_reduces_these_words_this_way() {
    assert_eq!(versions(&load_normalizations(&["kk"]).unwrap()), ["kk/1"]);
    let words = [
        "кітап",
        "кітаптар",
        "кітапта",
        "кітаптарда",
        "кітаптарымызда",
        "қала",
        "қалалар",
        "қаласында",
        "мүмкіндік",
        "мүмкіндіктер",
        "тау",
        "тар",
    ];
    assert_eq!(
        stems("kk", &words),
        [
            "кітап",
            "кітап",
            "кітап",
            "кітап",
            "кітап",
            "қала",
            "қала",
            "қала",
            "мүмкіндік",
            "мүмкіндік",
            "тау",
            "тар",
        ]
    );
}

/// The same for the two Snowball packs: their rules are somebody else's algorithm, and a
/// new version of that library changing what it does is exactly what this catches.
#[test]
fn the_snowball_packs_at_version_one_reduce_these_words_this_way() {
    assert_eq!(
        stems("ru", &["индекс", "индексы", "индексов", "хранилище"]),
        ["индекс", "индекс", "индекс", "хранилищ"]
    );
    assert_eq!(
        stems("en", &["index", "indexes", "indexed", "committing"]),
        ["index", "index", "index", "commit"]
    );
}

/// A name this build has no pack for fails the load and says what it does have. Skipping it
/// quietly would index that language by no rule at all while the configuration said
/// otherwise.
#[test]
fn a_language_this_build_does_not_carry_is_refused() {
    let error = load_normalizations(&["ru", "tr"]).unwrap_err();
    assert_eq!(error, LanguagePackError::Unknown("tr".to_string()));
    assert!(error.to_string().contains("ru, en, kk"), "{error}");
}

#[test]
fn a_language_declared_twice_is_refused() {
    let error = load_normalizations(&["ru", "kk", "ru"]).unwrap_err();
    assert!(error.to_string().contains("named twice"), "{error}");
}

/// Kazakh and Russian together are the pair this stage is about: both Cyrillic, one of them
/// with letters of its own, and the set builds because those letters really are its own.
#[test]
fn russian_and_kazakh_can_be_declared_together() {
    let set = load_normalizations(&["ru", "en", "kk"]).unwrap();
    assert_eq!(versions(&set), ["ru/1", "en/1", "kk/1"]);
    assert_eq!(
        IndexFingerprint::of(&["text"], 1, "simple-lowercase/1", &set).as_str(),
        "fields=text;suffixes=1;tokenizer=simple-lowercase/1;normalizations=ru/1,en/1,kk/1"
    );
}

/// A pack that says something this build cannot use is refused when the configuration that
/// declares it loads, and the refusal says which pack and what about it.
///
/// Every one of these is a file somebody could write: the packs this build carries are
/// correct, and what has to be shown is what happens to one that is not.
#[test]
fn a_pack_that_cannot_be_read_is_refused_with_the_reason() {
    let cases: &[(&str, &str, &str)] = &[
        ("not toml", "name = kk", "cannot be read"),
        (
            "no stemming section",
            "name = \"kk\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"аб\"\n",
            "cannot be read",
        ),
        (
            "an unknown key",
            "name = \"kk\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"аб\"\nspelling = \"loose\"\n[stemming]\nkind = \"snowball\"\nalgorithm = \"russian\"\n",
            "cannot be read",
        ),
        (
            "a script this build does not route by",
            "name = \"kk\"\nversion = \"1\"\nscript = \"runic\"\nalphabet = \"аб\"\n[stemming]\nkind = \"snowball\"\nalgorithm = \"russian\"\n",
            "is not a script",
        ),
        (
            "an algorithm this build does not have",
            "name = \"kk\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"аб\"\n[stemming]\nkind = \"snowball\"\nalgorithm = \"kazakh\"\n",
            "no snowball algorithm",
        ),
        (
            "a stemming kind that is not one",
            "name = \"kk\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"аб\"\n[stemming]\nkind = \"neural\"\n",
            "not one this build applies",
        ),
        (
            "suffix rules with no minimum stem",
            "name = \"kk\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"аб\"\n[stemming]\nkind = \"suffixes\"\n[[stemming.step]]\nname = \"case\"\nsuffixes = [\"да\"]\n",
            "no minimum stem length",
        ),
        (
            "a step with no suffixes",
            "name = \"kk\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"аб\"\n[stemming]\nkind = \"suffixes\"\nminimum_stem_chars = 2\n[[stemming.step]]\nname = \"case\"\nsuffixes = []\n",
            "lists no suffixes",
        ),
        (
            "no letters at all",
            "name = \"kk\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"\"\n[stemming]\nkind = \"snowball\"\nalgorithm = \"russian\"\n",
            "writes no letters",
        ),
        (
            "evidence it does not write",
            "name = \"kk\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"аб\"\nevidence = \"қ\"\n[stemming]\nkind = \"snowball\"\nalgorithm = \"russian\"\n",
            "does not write that letter",
        ),
        (
            "a name that is not the one declared",
            "name = \"tr\"\nversion = \"1\"\nscript = \"cyrillic\"\nalphabet = \"аб\"\n[stemming]\nkind = \"snowball\"\nalgorithm = \"russian\"\n",
            "calls itself",
        ),
    ];

    for (what, text, expected) in cases {
        let error = parse_pack("kk", text)
            .err()
            .unwrap_or_else(|| panic!("{what} must be refused"));
        assert!(error.to_string().contains(expected), "{what}: {error}");
    }
}

/// The packs this build carries parse, which is the other half of the test above: the
/// refusals are about data, not about the parser refusing everything.
#[test]
fn every_pack_this_build_carries_parses() {
    for name in available() {
        load_normalizations(&[name]).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}
