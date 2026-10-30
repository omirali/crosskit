// The whole knowledge of one context as a view: every entity with what the graph says about it,
// every fact with where it was read, and the bound past which the view is cut.
//
// An entity node holds only its key. Its name, type and description are what its mentions say,
// and they are chosen here, the same way every time: the spelling used most often — of the ones
// that differ only in letter case, the one with a capital, since a model writes «пакет документов»
// mid-sentence and «Пакет документов» when it names the thing — the type used most often, and
// the description of the first mention that has one. Choosing at reading time rather than
// writing time is what lets a new revision change the name a map shows without rewriting the
// node every other revision points at.
use std::collections::{BTreeMap, HashMap};

/// The most entities one map holds. Past it the view keeps the entities with the most facts and
/// says how many it found. Measured on the panel's own page (ADR 0048): the force layout grows
/// with the square of the map — 400 entities took 1.07 s, 800 took 4.0 s and 1 200 took 9.1 s on
/// a loaded machine — while reading the graph is not the cost (2 601 entities, 0.14 s).
pub const MAP_MOST_ENTITIES: usize = 400;

/// One mention as the graph holds it: which entity, under which name and type, from which file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMention {
    pub entity_id: String,
    pub revision_id: String,
    pub name: String,
    pub entity_type: String,
    pub description: String,
    pub file_id: String,
}

/// One fact as the graph holds it, before its source is named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFact {
    pub id: String,
    pub subject_id: String,
    pub predicate: String,
    pub object_id: String,
    pub description: String,
    pub weight: u8,
    pub quote: String,
    pub file_id: String,
    pub revision_id: String,
    /// The ordinal of the chunk the quote was found in.
    pub chunk_ordinal: i64,
}

/// Where a fact was read: a file, or a conversation, by the path a person can open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactSource {
    File {
        path: String,
    },
    Conversation {
        path: String,
        session_id: String,
        name: String,
        /// The reply the quote was cut from, when the chunks of the conversation line up with its
        /// replies one to one; nothing when a reply took several chunks and the reply is a span.
        reply: Option<u64>,
    },
}

/// An entity as a map shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEntity {
    id: String,
    name: String,
    entity_type: String,
    description: String,
    spellings: Vec<String>,
    files: usize,
    degree: usize,
}

impl MapEntity {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn entity_type(&self) -> &str {
        &self.entity_type
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// Every spelling the sources used, the shown one first.
    pub fn spellings(&self) -> &[String] {
        &self.spellings
    }

    /// How many files and conversations mention it.
    pub fn files(&self) -> usize {
        self.files
    }

    /// How many facts of the map have it at either end.
    pub fn degree(&self) -> usize {
        self.degree
    }
}

/// The mentions of one entity, gathered before its facts are counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionedEntity {
    id: String,
    name: String,
    entity_type: String,
    description: String,
    spellings: Vec<String>,
    files: usize,
}

impl MentionedEntity {
    /// Every entity named by `mentions`, each once, in the order of first appearance.
    pub fn gather(mentions: &[StoredMention]) -> Vec<Self> {
        let mut order: Vec<&str> = Vec::new();
        let mut by_entity: HashMap<&str, Vec<&StoredMention>> = HashMap::new();
        for mention in mentions {
            let group = by_entity.entry(&mention.entity_id).or_default();
            if group.is_empty() {
                order.push(&mention.entity_id);
            }
            group.push(mention);
        }
        order
            .into_iter()
            .map(|id| Self::of(id, &by_entity[id]))
            .collect()
    }

    fn of(id: &str, mentions: &[&StoredMention]) -> Self {
        let name = shown_name(mentions);
        let mut spellings = vec![name.clone()];
        for mention in mentions {
            if !spellings.contains(&mention.name) {
                spellings.push(mention.name.clone());
            }
        }
        let mut files: Vec<&str> = mentions.iter().map(|m| m.file_id.as_str()).collect();
        files.sort_unstable();
        files.dedup();
        Self {
            id: id.to_string(),
            entity_type: most_frequent(mentions.iter().map(|m| m.entity_type.as_str())),
            description: mentions
                .iter()
                .map(|mention| mention.description.as_str())
                .find(|description| !description.is_empty())
                .unwrap_or_default()
                .to_string(),
            name,
            spellings,
            files: files.len(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// The spelling a map shows: the most frequent once letter case is set aside, written with a
/// capital when one of its forms has one.
fn shown_name(mentions: &[&StoredMention]) -> String {
    let lowered: Vec<String> = mentions
        .iter()
        .map(|mention| mention.name.to_lowercase())
        .collect();
    let lowered = most_frequent(lowered.iter().map(String::as_str));
    let forms: Vec<&str> = mentions
        .iter()
        .map(|mention| mention.name.as_str())
        .filter(|name| name.to_lowercase() == lowered)
        .collect();
    forms
        .iter()
        .find(|name| name.chars().next().is_some_and(char::is_uppercase))
        .map(|name| name.to_string())
        .unwrap_or_else(|| most_frequent(forms.into_iter()))
}

/// The value given most often; of several given equally often, the one given first.
fn most_frequent<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for value in values {
        match counts.iter_mut().find(|(seen, _)| *seen == value) {
            Some((_, count)) => *count += 1,
            None => counts.push((value, 1)),
        }
    }
    let most = counts.iter().map(|(_, count)| *count).max().unwrap_or(0);
    counts
        .into_iter()
        .find(|(_, count)| *count == most)
        .map(|(value, _)| value.to_string())
        .unwrap_or_default()
}

/// A fact as a map shows it: the stored fact and where it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFact {
    fact: StoredFact,
    source: FactSource,
}

impl MapFact {
    pub fn new(fact: StoredFact, source: FactSource) -> Self {
        Self { fact, source }
    }

    pub fn fact(&self) -> &StoredFact {
        &self.fact
    }

    pub fn source(&self) -> &FactSource {
        &self.source
    }
}

/// The knowledge of one context, bounded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnowledgeMap {
    entities: Vec<MapEntity>,
    facts: Vec<MapFact>,
    found_entities: usize,
    found_facts: usize,
    limit: usize,
}

impl KnowledgeMap {
    /// The map of `entities` and `facts`, cut to `limit` entities.
    ///
    /// A fact whose end is no entity here is dropped: its source was hidden, or the entity's
    /// every mention was. An entity with no fact is dropped too — a map is of what is proved.
    /// Past the limit the entities with the most facts stay, ties by name, and a fact stays
    /// when both its ends did.
    pub fn bounded(entities: Vec<MentionedEntity>, facts: Vec<MapFact>, limit: usize) -> Self {
        let known: HashMap<&str, &MentionedEntity> = entities
            .iter()
            .map(|entity| (entity.id(), entity))
            .collect();
        let facts: Vec<MapFact> = facts
            .into_iter()
            .filter(|one| {
                known.contains_key(one.fact.subject_id.as_str())
                    && known.contains_key(one.fact.object_id.as_str())
            })
            .collect();
        let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
        for one in &facts {
            *degree.entry(one.fact.subject_id.as_str()).or_default() += 1;
            *degree.entry(one.fact.object_id.as_str()).or_default() += 1;
        }
        let mut ranked: Vec<MapEntity> = entities
            .iter()
            .filter_map(|entity| {
                let degree = *degree.get(entity.id())?;
                Some(MapEntity {
                    id: entity.id.clone(),
                    name: entity.name.clone(),
                    entity_type: entity.entity_type.clone(),
                    description: entity.description.clone(),
                    spellings: entity.spellings.clone(),
                    files: entity.files,
                    degree,
                })
            })
            .collect();
        ranked.sort_by(|left, right| {
            right
                .degree
                .cmp(&left.degree)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        let found_entities = ranked.len();
        let found_facts = facts.len();
        ranked.truncate(limit);
        let kept: std::collections::HashSet<String> =
            ranked.iter().map(|entity| entity.id.clone()).collect();
        let facts = facts
            .into_iter()
            .filter(|one| kept.contains(&one.fact.subject_id) && kept.contains(&one.fact.object_id))
            .collect();
        Self {
            entities: ranked,
            facts,
            found_entities,
            found_facts,
            limit,
        }
    }

    pub fn entities(&self) -> &[MapEntity] {
        &self.entities
    }

    pub fn facts(&self) -> &[MapFact] {
        &self.facts
    }

    pub fn found_entities(&self) -> usize {
        self.found_entities
    }

    pub fn found_facts(&self) -> usize {
        self.found_facts
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn truncated(&self) -> bool {
        self.entities.len() < self.found_entities || self.facts.len() < self.found_facts
    }

    pub fn entity(&self, id: &str) -> Option<&MapEntity> {
        self.entities.iter().find(|entity| entity.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::{FactSource, KnowledgeMap, MapFact, MentionedEntity, StoredFact, StoredMention};

    fn mention(entity: &str, name: &str, entity_type: &str, file: &str) -> StoredMention {
        StoredMention {
            entity_id: format!("entity:{entity}"),
            revision_id: format!("{file}@h"),
            name: name.to_string(),
            entity_type: entity_type.to_string(),
            description: if file == "f1" {
                String::new()
            } else {
                format!("о {name}")
            },
            file_id: file.to_string(),
        }
    }

    fn fact(id: &str, subject: &str, object: &str) -> MapFact {
        MapFact::new(
            StoredFact {
                id: id.to_string(),
                subject_id: format!("entity:{subject}"),
                predicate: "belongs-to".to_string(),
                object_id: format!("entity:{object}"),
                description: String::new(),
                weight: 5,
                quote: "цитата из файла".to_string(),
                file_id: "f1".to_string(),
                revision_id: "f1@h".to_string(),
                chunk_ordinal: 0,
            },
            FactSource::File {
                path: "/work/a.md".to_string(),
            },
        )
    }

    #[test]
    fn an_entity_shows_the_spelling_and_type_its_sources_use_most() {
        let entities = MentionedEntity::gather(&[
            mention("контрагент", "Контрагенты", "concept", "f1"),
            mention("контрагент", "Контрагент", "organization", "f2"),
            mention("контрагент", "Контрагент", "concept", "f3"),
            mention("пакет", "Пакет", "concept", "f1"),
        ]);

        let map = KnowledgeMap::bounded(entities, vec![fact("x", "пакет", "контрагент")], 10);
        let counterparty = map.entity("entity:контрагент").unwrap();

        assert_eq!(counterparty.name(), "Контрагент");
        assert_eq!(counterparty.entity_type(), "concept");
        assert_eq!(counterparty.spellings(), ["Контрагент", "Контрагенты"]);
        assert_eq!(counterparty.description(), "о Контрагент");
        assert_eq!(counterparty.files(), 3);
        assert_eq!(counterparty.degree(), 1);
    }

    #[test]
    fn of_two_spellings_that_differ_in_case_the_capital_one_is_shown() {
        let entities = MentionedEntity::gather(&[
            mention("пакет", "пакет документов", "document", "f1"),
            mention("пакет", "пакет документов", "document", "f2"),
            mention("пакет", "Пакет документов", "document", "f3"),
            mention("контрагент", "Контрагент", "concept", "f1"),
        ]);

        let map = KnowledgeMap::bounded(entities, vec![fact("x", "пакет", "контрагент")], 10);

        assert_eq!(
            map.entity("entity:пакет").unwrap().name(),
            "Пакет документов"
        );
        assert_eq!(
            map.entity("entity:пакет").unwrap().spellings(),
            ["Пакет документов", "пакет документов"]
        );
    }

    #[test]
    fn a_map_holds_only_what_a_fact_proves_and_cuts_the_least_connected() {
        let entities = MentionedEntity::gather(&[
            mention("a", "A", "concept", "f1"),
            mention("b", "B", "concept", "f1"),
            mention("c", "C", "concept", "f1"),
            mention("lonely", "Lonely", "concept", "f1"),
        ]);
        let facts = vec![
            fact("1", "a", "b"),
            fact("2", "a", "c"),
            fact("3", "a", "hidden"),
        ];

        let whole = KnowledgeMap::bounded(entities.clone(), facts.clone(), 10);
        assert_eq!(
            whole.found_entities(),
            3,
            "an entity with no fact is not on a map"
        );
        assert_eq!(whole.found_facts(), 2, "a fact to a hidden end is dropped");
        assert!(!whole.truncated());

        let cut = KnowledgeMap::bounded(entities, facts, 2);
        let kept: Vec<&str> = cut.entities().iter().map(|e| e.name()).collect();
        assert_eq!(kept, ["A", "B"], "most facts first, ties by name");
        assert_eq!(cut.facts().len(), 1);
        assert!(cut.truncated());
        assert_eq!(cut.limit(), 2);
    }
}
