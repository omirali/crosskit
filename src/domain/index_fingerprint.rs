// What an index records about how it was written.
//
// An index is a derivative, and everything in it is the output of rules that change: a
// language pack gains a suffix, a tokenizer learns a separator, a field is added, the table
// that names a file's language by its suffix gets a row. An index written by the old rules
// holds what it holds in a form the new ones never ask for, and nothing about the answers it
// gives says so — they are simply fewer, or a field a query filters on is missing from half
// the documents. So every index carries a description of how it was written, and every read
// compares.
//
// One description rather than several, and one string rather than a record: it is compared,
// never parsed, and it is written in the same commit as the documents it describes. Two
// values could disagree if one of two writes failed; a description that had to be parsed
// would turn a change of format into a new way to be wrong.
use std::fmt;

use super::text_normalization::NormalizationSet;

/// How an index was written: the fields it holds, the version of the suffix table its
/// language field was written from, the tokenizer that split its text and every active
/// normalization with its version, in order.
///
/// Stored in the index and compared on every read. Without it a change to any of those
/// rules would leave an index whose contents were produced by rules nobody applies any
/// more, answering fewer queries than it should with nothing to show that anything is
/// wrong. Language packs and the suffix table are both data, and the versions in that data
/// are what the reader compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFingerprint(String);

impl IndexFingerprint {
    /// The fingerprint of an index with these `fields`, written from suffix table
    /// `suffix_table_version` by the analyzer `tokenizer` and `set` describe.
    ///
    /// `fields` is the schema's own list, in the order it declares them, and it is passed in
    /// rather than known here: which fields an index has belongs to the adapter that builds
    /// its schema, and a second list written down beside it would be the one that goes
    /// stale.
    pub fn of(
        fields: &[&str],
        suffix_table_version: u32,
        tokenizer: &str,
        set: &NormalizationSet,
    ) -> Self {
        let normalizations = set
            .active()
            .iter()
            .map(|normalization| format!("{}/{}", normalization.id(), normalization.version()))
            .collect::<Vec<_>>()
            .join(",");
        Self(format!(
            "fields={};suffixes={suffix_table_version};tokenizer={tokenizer};normalizations={normalizations}",
            fields.join(",")
        ))
    }

    /// A fingerprint as an index recorded it. Not parsed: it is compared, and a string this
    /// program did not write is simply a fingerprint that does not match.
    pub fn stored(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IndexFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::IndexFingerprint;
    use crate::domain::alphabet::{Alphabet, Script};
    use crate::domain::text_normalization::{
        Normalization, NormalizationSet, SnowballLanguage, StemRule,
    };

    /// A normalization is a name and a version here; what it writes and how it stems is
    /// somebody else's test.
    fn named(id: &str, version: &str) -> Normalization {
        Normalization::new(
            id,
            version,
            Script::Cyrillic,
            Alphabet::new(
                "абвгдеёжзийклмнопрстуфхцчшщъыьэюя".chars().collect(),
                Vec::new(),
            )
            .unwrap(),
            BTreeSet::new(),
            StemRule::Snowball(SnowballLanguage::Russian),
        )
        .unwrap()
    }

    fn set(normalizations: Vec<Normalization>) -> NormalizationSet {
        NormalizationSet::new(normalizations).unwrap()
    }

    fn fingerprint(fields: &[&str], suffixes: u32, tokenizer: &str) -> IndexFingerprint {
        IndexFingerprint::of(
            fields,
            suffixes,
            tokenizer,
            &set(vec![named("ru", "1"), named("en", "1")]),
        )
    }

    #[test]
    fn it_names_the_fields_the_suffix_table_the_tokenizer_and_every_normalization() {
        assert_eq!(
            fingerprint(&["chunk_id", "text"], 1, "simple-lowercase/1").as_str(),
            "fields=chunk_id,text;suffixes=1;tokenizer=simple-lowercase/1;normalizations=ru/1,en/1"
        );
    }

    #[test]
    fn a_set_that_normalizes_nothing_still_has_one() {
        assert_eq!(
            IndexFingerprint::of(&["text"], 1, "simple-lowercase/1", &set(Vec::new())).as_str(),
            "fields=text;suffixes=1;tokenizer=simple-lowercase/1;normalizations="
        );
    }

    /// A field added or removed changes the fingerprint, which is the whole reason the
    /// fields are in it: an index the new field is missing from answers a query that filters
    /// on it with silence, and the counts agree while it does.
    #[test]
    fn a_different_set_of_fields_is_a_different_fingerprint() {
        let both = fingerprint(&["chunk_id", "text"], 1, "simple-lowercase/1");
        for other in [
            &["chunk_id"][..],
            &["chunk_id", "source_language", "text"][..],
            &["text", "chunk_id"][..],
            &[][..],
        ] {
            assert_ne!(both, fingerprint(other, 1, "simple-lowercase/1"));
        }
    }

    /// The same fields written from a different suffix table are different documents: the
    /// language field of one was decided by rows the other does not have.
    #[test]
    fn a_new_version_of_the_suffix_table_is_a_new_fingerprint() {
        let fields = &["chunk_id", "source_language"];
        assert_ne!(
            fingerprint(fields, 1, "simple-lowercase/1"),
            fingerprint(fields, 2, "simple-lowercase/1")
        );
    }

    /// Changing what a pack does means changing its version, and this is where that change
    /// becomes visible to an index written by the old one.
    #[test]
    fn a_new_version_of_one_normalization_is_a_new_fingerprint() {
        let of = |normalizations| {
            IndexFingerprint::of(&["text"], 1, "simple-lowercase/1", &set(normalizations))
        };

        assert_ne!(of(vec![named("ru", "1")]), of(vec![named("ru", "2")]));
    }

    /// The set and its order are part of it too, so an index written with one set is not read
    /// as though it had been written with another.
    #[test]
    fn a_different_set_of_normalizations_gives_a_different_fingerprint() {
        let of = |normalizations| {
            IndexFingerprint::of(&["text"], 1, "simple-lowercase/1", &set(normalizations))
        };
        let both = of(vec![named("ru", "1"), named("en", "1")]);
        for other in [
            vec![named("ru", "1")],
            vec![named("en", "1"), named("ru", "1")],
            vec![named("ru", "1"), named("en", "1"), named("kk", "1")],
            Vec::new(),
        ] {
            assert_ne!(both, of(other));
        }
        assert_ne!(both, fingerprint(&["text"], 1, "other-tokenizer/1"));
    }

    /// A fingerprint an index recorded is compared, never parsed: it matches the build that
    /// would write the same string and nothing else, including a string this program never
    /// wrote and the shorter string written before the fields were part of it.
    #[test]
    fn a_stored_fingerprint_is_compared_and_not_read() {
        let current = fingerprint(&["chunk_id", "text"], 1, "simple-lowercase/1");

        assert_eq!(
            IndexFingerprint::stored(
                "fields=chunk_id,text;suffixes=1;tokenizer=simple-lowercase/1;normalizations=ru/1,en/1"
            ),
            current
        );
        assert_ne!(
            IndexFingerprint::stored("tokenizer=simple-lowercase/1;normalizations=ru/1,en/1"),
            current
        );
        assert_ne!(
            IndexFingerprint::stored("written by something else"),
            current
        );
    }
}
