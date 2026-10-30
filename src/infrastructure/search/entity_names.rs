// The key of an entity, read by the search index's own analyzer.
//
// One rule for when two words are the same word: the analyzer that writes the index lowers a
// token and reduces it by every language pack that claims it. A name's key is the first reading
// of each of its words — the reading a question is parsed with first — so «Контрагенты» and
// «контрагент» are one key exactly when search would find one by the other. The one thing
// added is `ё` read as `е`: Russian writes both for one letter, and a key that told them apart
// would split «счёт» from «счет» for no reason a reader could see.
use std::cell::RefCell;

use tantivy::tokenizer::TextAnalyzer;

use crate::application::EntityNames;
use crate::domain::NormalizationSet;
use crate::domain::knowledge::EntityKey;

use super::analyzer::{Reading, analyzer};

pub struct AnalyzedEntityNames {
    /// The analyzer needs `&mut` to stream; one worker on one thread is the whole of the
    /// sharing a cell has to serve.
    analyzer: RefCell<TextAnalyzer>,
}

impl AnalyzedEntityNames {
    /// The names of a context whose index is written with `normalizations`.
    pub fn new(normalizations: &NormalizationSet) -> Self {
        Self {
            analyzer: RefCell::new(analyzer(normalizations, Reading::Only(0))),
        }
    }
}

impl EntityNames for AnalyzedEntityNames {
    fn key_of(&self, name: &str) -> Option<EntityKey> {
        let folded = name.replace('ё', "е").replace('Ё', "Е");
        let mut analyzer = self.analyzer.borrow_mut();
        let mut stream = analyzer.token_stream(&folded);
        let mut words = Vec::new();
        while stream.advance() {
            words.push(stream.token().text.clone());
        }
        EntityKey::from_words(words)
    }
}

#[cfg(test)]
mod tests {
    use super::AnalyzedEntityNames;
    use crate::application::EntityNames;
    use crate::infrastructure::search::built_in_normalizations;

    fn key(names: &AnalyzedEntityNames, name: &str) -> String {
        names.key_of(name).unwrap().as_str().to_string()
    }

    /// What the key glues and what it does not, as ADR 0048 names it. The second list is not a
    /// wish list: each pair is two entities on the map, and the map shows both.
    #[test]
    fn a_key_glues_the_forms_of_one_word_and_nothing_further() {
        let names = AnalyzedEntityNames::new(&built_in_normalizations());

        for same in [
            ["Контрагент", "контрагенты"],
            ["Контрагент", "Контрагентов"],
            ["пакет документов", "Пакеты документов"],
            ["БИН", "бин"],
            ["Счёт", "счет"],
            ["counterparty", "Counterparties"],
            ["Реестр  контрагентов", "реестр контрагента"],
        ] {
            assert_eq!(key(&names, same[0]), key(&names, same[1]), "{same:?}");
        }
        for apart in [
            ["counterparty", "Контрагент"],
            ["БИН", "бизнес-идентификационный номер"],
            ["Поставщик", "Контрагент"],
            ["реестр контрагентов", "контрагентов реестр"],
        ] {
            assert_ne!(key(&names, apart[0]), key(&names, apart[1]), "{apart:?}");
        }
    }

    #[test]
    fn a_name_of_punctuation_alone_has_no_key() {
        let names = AnalyzedEntityNames::new(&built_in_normalizations());

        assert_eq!(names.key_of(" — «» "), None);
        assert_eq!(key(&names, "ТОО «Ромашка»"), key(&names, "тоо ромашка"));
    }
}
