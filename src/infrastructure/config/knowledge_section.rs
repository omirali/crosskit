// `[knowledge]`: the closed lists of entity and relation types a model chooses from.
//
// A file with no section, or a section that names only one list, gets the built-in list for
// what it left out. The built-in lists are argued in ADR 0048; changing one is the owner's
// decision and a line in this file, and it makes every revision a new question — the lists are
// part of the cache key, so nothing chosen from the old list is served for the new one.
use serde::Deserialize;

use crate::domain::knowledge::{KnowledgeVocabulary, VocabularyError};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct KnowledgeSection {
    entity_types: Option<Vec<String>>,
    relation_types: Option<Vec<String>>,
}

/// The kinds of thing a work document talks about. Kept short on purpose: a model choosing
/// among a dozen kinds is consistent from one file to the next, and the map is coloured by
/// these, so each has to be told apart at a glance.
const ENTITY_TYPES: &[&str] = &[
    "concept",
    "attribute",
    "organization",
    "person",
    "role",
    "system",
    "dataset",
    "document",
    "process",
    "rule",
    "task",
    "issue",
];

/// How two of those relate, read as "subject predicate object". `related-to` is the last
/// resort and is said to be one, so a model that cannot place a fact does not stretch another.
const RELATION_TYPES: &[&str] = &[
    "is-a",
    "part-of",
    "belongs-to",
    "has-attribute",
    "identified-by",
    "uses",
    "stored-in",
    "produces",
    "depends-on",
    "references",
    "responsible-for",
    "governs",
    "replaces",
    "contradicts",
    "related-to",
];

pub(super) fn parse_knowledge(
    section: &KnowledgeSection,
) -> Result<KnowledgeVocabulary, VocabularyError> {
    KnowledgeVocabulary::new(
        section
            .entity_types
            .clone()
            .unwrap_or_else(|| owned(ENTITY_TYPES)),
        section
            .relation_types
            .clone()
            .unwrap_or_else(|| owned(RELATION_TYPES)),
    )
}

/// What a file with no `[knowledge]` section, or no file at all, extracts.
pub(super) fn default_vocabulary() -> KnowledgeVocabulary {
    parse_knowledge(&KnowledgeSection::default()).expect("the built-in lists must be accepted")
}

fn owned(types: &[&str]) -> Vec<String> {
    types.iter().map(|one| one.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ENTITY_TYPES, KnowledgeSection, RELATION_TYPES, default_vocabulary, parse_knowledge,
    };

    #[test]
    fn no_section_means_the_built_in_lists() {
        let vocabulary = default_vocabulary();

        assert_eq!(vocabulary.entity_types().len(), ENTITY_TYPES.len());
        assert_eq!(vocabulary.relation_types().len(), RELATION_TYPES.len());
        assert!(vocabulary.admits_entity_type("concept"));
        assert!(vocabulary.admits_relation_type("identified-by"));
    }

    #[test]
    fn a_section_replaces_the_list_it_names_and_keeps_the_other() {
        let section: KnowledgeSection =
            toml::from_str("entity_types = [\"понятие\", \"система\"]").unwrap();

        let vocabulary = parse_knowledge(&section).unwrap();

        assert_eq!(vocabulary.entity_types(), ["понятие", "система"]);
        assert_eq!(vocabulary.relation_types().len(), RELATION_TYPES.len());
    }

    #[test]
    fn a_list_that_is_not_a_vocabulary_is_refused() {
        let empty: KnowledgeSection = toml::from_str("relation_types = []").unwrap();
        assert!(parse_knowledge(&empty).is_err());
        assert!(toml::from_str::<KnowledgeSection>("types = [\"a\"]").is_err());
    }
}
