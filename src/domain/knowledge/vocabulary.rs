// The closed lists a model chooses from: which kinds of entity and which kinds of relation one
// installation extracts.
//
// Closed because an open list is not a vocabulary. Given free choice a model calls one thing a
// `concept` in one file and a `term` in the next, and a graph whose types drift that way cannot
// be coloured, filtered or counted. The lists live in the configuration, so the owner changes
// them without a build; this type is what the configuration has to be to be accepted at all.
use std::fmt;

use super::super::cache_key::compose;

/// The longest a type may be. A type is a word or two; anything longer is a description.
pub const MAX_TYPE_CHARS: usize = 40;

/// The most types one list may hold. The list travels in every request, and a model choosing
/// among a hundred kinds chooses no better than among twenty.
pub const MAX_TYPES: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeVocabulary {
    entity_types: Vec<String>,
    relation_types: Vec<String>,
}

impl KnowledgeVocabulary {
    /// Both lists, refused when either is not a list a model can be held to.
    pub fn new(
        entity_types: Vec<String>,
        relation_types: Vec<String>,
    ) -> Result<Self, VocabularyError> {
        Ok(Self {
            entity_types: checked(entity_types, VocabularyList::Entity)?,
            relation_types: checked(relation_types, VocabularyList::Relation)?,
        })
    }

    pub fn entity_types(&self) -> &[String] {
        &self.entity_types
    }

    pub fn relation_types(&self) -> &[String] {
        &self.relation_types
    }

    pub fn admits_entity_type(&self, candidate: &str) -> bool {
        self.entity_types.iter().any(|known| known == candidate)
    }

    pub fn admits_relation_type(&self, candidate: &str) -> bool {
        self.relation_types.iter().any(|known| known == candidate)
    }

    /// The lists as one readable string, for the cache key: a changed list is a different
    /// question, and an answer chosen from the old list must not be served for the new one.
    pub fn fingerprint(&self) -> String {
        let entity: Vec<&str> = self.entity_types.iter().map(String::as_str).collect();
        let relation: Vec<&str> = self.relation_types.iter().map(String::as_str).collect();
        compose(
            "vocabulary",
            &[&compose("entity", &entity), &compose("relation", &relation)],
        )
    }
}

fn checked(types: Vec<String>, list: VocabularyList) -> Result<Vec<String>, VocabularyError> {
    if types.is_empty() {
        return Err(VocabularyError::Empty(list));
    }
    if types.len() > MAX_TYPES {
        return Err(VocabularyError::TooMany {
            list,
            count: types.len(),
        });
    }
    let mut accepted: Vec<String> = Vec::with_capacity(types.len());
    for candidate in types {
        let trimmed = candidate.trim();
        let fits = !trimmed.is_empty()
            && trimmed.chars().count() <= MAX_TYPE_CHARS
            && !trimmed.chars().any(char::is_control);
        if !fits {
            return Err(VocabularyError::NotAType {
                list,
                candidate: trimmed.chars().take(MAX_TYPE_CHARS).collect(),
            });
        }
        if accepted.iter().any(|known| known == trimmed) {
            return Err(VocabularyError::Repeated {
                list,
                candidate: trimmed.to_string(),
            });
        }
        accepted.push(trimmed.to_string());
    }
    Ok(accepted)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabularyList {
    Entity,
    Relation,
}

impl VocabularyList {
    fn key(self) -> &'static str {
        match self {
            Self::Entity => "entity_types",
            Self::Relation => "relation_types",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VocabularyError {
    Empty(VocabularyList),
    TooMany {
        list: VocabularyList,
        count: usize,
    },
    NotAType {
        list: VocabularyList,
        candidate: String,
    },
    Repeated {
        list: VocabularyList,
        candidate: String,
    },
}

impl fmt::Display for VocabularyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(list) => write!(
                f,
                "[knowledge] {} is empty; a model given no type to choose cannot name one",
                list.key()
            ),
            Self::TooMany { list, count } => write!(
                f,
                "[knowledge] {} holds {count} types; at most {MAX_TYPES} are accepted",
                list.key()
            ),
            Self::NotAType { list, candidate } => write!(
                f,
                "[knowledge] {} has {candidate:?}, which is not a type: one line of 1 to {MAX_TYPE_CHARS} characters",
                list.key()
            ),
            Self::Repeated { list, candidate } => {
                write!(f, "[knowledge] {} names {candidate:?} twice", list.key())
            }
        }
    }
}

impl std::error::Error for VocabularyError {}

#[cfg(test)]
mod tests {
    use super::{KnowledgeVocabulary, MAX_TYPE_CHARS, MAX_TYPES, VocabularyError, VocabularyList};

    fn list(types: &[&str]) -> Vec<String> {
        types.iter().map(|one| one.to_string()).collect()
    }

    #[test]
    fn a_vocabulary_admits_exactly_its_own_types() {
        let vocabulary =
            KnowledgeVocabulary::new(list(&["concept", " role "]), list(&["belongs-to"])).unwrap();

        assert!(vocabulary.admits_entity_type("concept"));
        assert!(vocabulary.admits_entity_type("role"));
        assert!(!vocabulary.admits_entity_type("Concept"));
        assert!(!vocabulary.admits_entity_type("belongs-to"));
        assert!(vocabulary.admits_relation_type("belongs-to"));
        assert!(!vocabulary.admits_relation_type("related"));
    }

    #[test]
    fn a_list_a_model_cannot_be_held_to_is_refused() {
        assert_eq!(
            KnowledgeVocabulary::new(vec![], list(&["a"])).unwrap_err(),
            VocabularyError::Empty(VocabularyList::Entity)
        );
        assert!(matches!(
            KnowledgeVocabulary::new(list(&["a"]), list(&["x", "x"])).unwrap_err(),
            VocabularyError::Repeated {
                list: VocabularyList::Relation,
                ..
            }
        ));
        for not_a_type in ["", "two\nlines", &"t".repeat(MAX_TYPE_CHARS + 1)] {
            assert!(
                matches!(
                    KnowledgeVocabulary::new(list(&[not_a_type]), list(&["a"])).unwrap_err(),
                    VocabularyError::NotAType { .. }
                ),
                "{not_a_type:?}"
            );
        }
        let many: Vec<String> = (0..=MAX_TYPES).map(|n| format!("t{n}")).collect();
        assert!(matches!(
            KnowledgeVocabulary::new(many, list(&["a"])).unwrap_err(),
            VocabularyError::TooMany { .. }
        ));
    }

    /// A changed list is a different question, so its fingerprint differs — and moving a type
    /// from one list to the other is a change too.
    #[test]
    fn the_fingerprint_changes_with_either_list() {
        let base = KnowledgeVocabulary::new(list(&["a", "b"]), list(&["r"])).unwrap();
        let more = KnowledgeVocabulary::new(list(&["a", "b", "c"]), list(&["r"])).unwrap();
        let moved = KnowledgeVocabulary::new(list(&["a"]), list(&["b", "r"])).unwrap();

        assert_eq!(base.fingerprint(), base.clone().fingerprint());
        assert_ne!(base.fingerprint(), more.fingerprint());
        assert_ne!(base.fingerprint(), moved.fingerprint());
    }
}
