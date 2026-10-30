// What makes two names one entity.
//
// A key is the name reduced word by word — lowered, and each word cut to its stem by the same
// language packs the search index uses — and joined by single spaces. «Контрагент»,
// «контрагенты» and «Контрагентов» reduce to one key; so do «Пакет документов» and «пакеты
// документов». Reducing is infrastructure (it is the index's analyzer); this type only says
// what a reduction has to be to be a key, and how a key is written into an id.
//
// What a key does not glue, measured and named in ADR 0048: a name and its translation
// (`counterparty` and «Контрагент»), an abbreviation and its expansion («БИН» and «бизнес-
// идентификационный номер»), and a synonym. Those stay two entities, and the map shows both.
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityKey(String);

impl EntityKey {
    /// The key of a name already reduced to words, or nothing when no word is left: a name of
    /// punctuation alone names nothing that could be met again.
    pub fn from_words<Word: AsRef<str>>(words: impl IntoIterator<Item = Word>) -> Option<Self> {
        let joined = words
            .into_iter()
            .map(|word| word.as_ref().trim().to_string())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        (!joined.is_empty()).then_some(Self(joined))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The id of the entity node this key names, in its context's graph.
    pub fn entity_id(&self) -> String {
        format!("entity:{}", self.0)
    }
}

impl fmt::Display for EntityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::EntityKey;

    #[test]
    fn a_key_is_its_words_joined_by_one_space() {
        let key = EntityKey::from_words(["контрагент", " ", "", "бин "]).unwrap();

        assert_eq!(key.as_str(), "контрагент бин");
        assert_eq!(key.entity_id(), "entity:контрагент бин");
    }

    #[test]
    fn a_name_with_no_word_left_names_nothing() {
        assert_eq!(EntityKey::from_words(Vec::<String>::new()), None);
        assert_eq!(EntityKey::from_words([" ", ""]), None);
    }
}
