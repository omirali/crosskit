// A proposal checked against the revision it was drawn from.
//
// This is where a model's answer becomes knowledge, and the rule is short: a fact is kept only
// when both its ends are entities of the same answer, its relation is a type of the vocabulary,
// the part it names is a part of this revision, and its quote occurs in that part. Everything
// else is refused and counted by why — the quote first, because that is the number which says
// whether the model is reading or writing.
//
// An entity is kept only when a kept fact joins it to another. An entity has no quote of its
// own, so one nothing proves is the model's word alone; the map shows what the source says.
use std::collections::{BTreeMap, HashMap};

use super::entity_key::EntityKey;
use super::proposal::KnowledgeProposal;
use super::quote::{quote_names, quote_occurs_in};
use super::vocabulary::KnowledgeVocabulary;

/// One part of a revision as the request numbered it: the chunk it is, and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePart {
    number: usize,
    chunk_id: String,
    text: String,
}

impl SourcePart {
    pub fn new(number: usize, chunk_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            number,
            chunk_id: chunk_id.into(),
            text: text.into(),
        }
    }

    pub fn number(&self) -> usize {
        self.number
    }

    pub fn chunk_id(&self) -> &str {
        &self.chunk_id
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// An entity as one revision states it: its key, the first type and description the answer
/// gave it, and every spelling the answer used for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityStatement {
    key: EntityKey,
    entity_type: String,
    description: String,
    spellings: Vec<String>,
}

impl EntityStatement {
    pub fn key(&self) -> &EntityKey {
        &self.key
    }

    pub fn entity_type(&self) -> &str {
        &self.entity_type
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// Distinct names as written, first first.
    pub fn spellings(&self) -> &[String] {
        &self.spellings
    }
}

/// A fact one revision proves: two entities by key, the relation, and the part and quote it
/// rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactStatement {
    subject: EntityKey,
    predicate: String,
    object: EntityKey,
    description: String,
    weight: u8,
    chunk_id: String,
    quote: String,
}

impl FactStatement {
    pub fn subject(&self) -> &EntityKey {
        &self.subject
    }

    pub fn predicate(&self) -> &str {
        &self.predicate
    }

    pub fn object(&self) -> &EntityKey {
        &self.object
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn weight(&self) -> u8 {
        self.weight
    }

    pub fn chunk_id(&self) -> &str {
        &self.chunk_id
    }

    pub fn quote(&self) -> &str {
        &self.quote
    }
}

/// What a check kept and what it refused, by reason. `facts_proposed` is the sum of the other
/// fact counters, so no fact is ever unaccounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KnowledgeTally {
    pub facts_kept: usize,
    /// The quote does not occur in the part the fact names.
    pub facts_rejected_by_quote: usize,
    /// The quote occurs and does not name both ends: «БИН» is not a proof that «Контрагент»
    /// is unique by it.
    pub facts_rejected_unnamed: usize,
    /// Over a limit, an unknown relation type, an end that is no entity of the answer, a part
    /// the revision does not have, or both ends one entity.
    pub facts_rejected_otherwise: usize,
    /// The same relation between the same two entities from the same part, said again.
    pub facts_repeated: usize,
    pub entities_kept: usize,
    /// Over a limit, an unknown type, or a name with no word in it.
    pub entities_refused: usize,
    /// Fitted and typed, and joined to nothing by any kept fact.
    pub entities_unproved: usize,
    /// Spellings beyond the first of each kept entity: what the key glued inside one answer.
    pub spellings_glued: usize,
}

impl KnowledgeTally {
    pub fn facts_proposed(&self) -> usize {
        self.facts_kept
            + self.facts_rejected_by_quote
            + self.facts_rejected_unnamed
            + self.facts_rejected_otherwise
            + self.facts_repeated
    }

    pub fn add(&mut self, other: &Self) {
        self.facts_kept += other.facts_kept;
        self.facts_rejected_by_quote += other.facts_rejected_by_quote;
        self.facts_rejected_unnamed += other.facts_rejected_unnamed;
        self.facts_rejected_otherwise += other.facts_rejected_otherwise;
        self.facts_repeated += other.facts_repeated;
        self.entities_kept += other.entities_kept;
        self.entities_refused += other.entities_refused;
        self.entities_unproved += other.entities_unproved;
        self.spellings_glued += other.spellings_glued;
    }
}

/// What one revision states, proved, with the count of what it did not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RevisionKnowledge {
    entities: Vec<EntityStatement>,
    facts: Vec<FactStatement>,
    tally: KnowledgeTally,
}

impl RevisionKnowledge {
    pub fn entities(&self) -> &[EntityStatement] {
        &self.entities
    }

    pub fn facts(&self) -> &[FactStatement] {
        &self.facts
    }

    pub fn tally(&self) -> KnowledgeTally {
        self.tally
    }
}

/// Checks `proposal` against `parts`. `key_of` reduces a name to its key; it is the same
/// function for the entities and for the ends of facts, so a fact may name an entity in another
/// form than the list did and still find it.
pub fn verify(
    proposal: &KnowledgeProposal,
    parts: &[SourcePart],
    vocabulary: &KnowledgeVocabulary,
    key_of: &dyn Fn(&str) -> Option<EntityKey>,
) -> RevisionKnowledge {
    let mut tally = KnowledgeTally {
        entities_refused: proposal.entities_refused(),
        facts_rejected_otherwise: proposal.facts_refused(),
        ..KnowledgeTally::default()
    };

    // Entities by key, in the order the answer first named them.
    let mut order: Vec<EntityKey> = Vec::new();
    let mut statements: HashMap<EntityKey, EntityStatement> = HashMap::new();
    for entity in proposal.entities() {
        let key = match key_of(entity.name()) {
            Some(key) if vocabulary.admits_entity_type(entity.entity_type()) => key,
            _ => {
                tally.entities_refused += 1;
                continue;
            }
        };
        match statements.get_mut(&key) {
            Some(statement) => {
                if !statement.spellings.iter().any(|seen| seen == entity.name()) {
                    statement.spellings.push(entity.name().to_string());
                }
            }
            None => {
                order.push(key.clone());
                statements.insert(
                    key.clone(),
                    EntityStatement {
                        key,
                        entity_type: entity.entity_type().to_string(),
                        description: entity.description().to_string(),
                        spellings: vec![entity.name().to_string()],
                    },
                );
            }
        }
    }

    let part_of: HashMap<usize, &SourcePart> =
        parts.iter().map(|part| (part.number(), part)).collect();
    let mut facts: Vec<FactStatement> = Vec::new();
    let mut said: BTreeMap<(EntityKey, String, EntityKey, String), ()> = BTreeMap::new();
    for fact in proposal.facts() {
        let ends = (key_of(fact.subject()), key_of(fact.object()));
        let (Some(subject), Some(object)) = ends else {
            tally.facts_rejected_otherwise += 1;
            continue;
        };
        let well_formed = statements.contains_key(&subject)
            && statements.contains_key(&object)
            && subject != object
            && vocabulary.admits_relation_type(fact.predicate());
        let Some(part) = part_of.get(&fact.part()).filter(|_| well_formed) else {
            tally.facts_rejected_otherwise += 1;
            continue;
        };
        if !quote_occurs_in(fact.quote(), part.text()) {
            tally.facts_rejected_by_quote += 1;
            continue;
        }
        let quote_key = key_of(fact.quote());
        let quote_words: Vec<&str> = quote_key
            .as_ref()
            .map(|key| key.as_str().split(' ').collect())
            .unwrap_or_default();
        let named = |key: &EntityKey, as_written: &str| {
            let key_words: Vec<&str> = key.as_str().split(' ').collect();
            let mut spellings: Vec<&str> = statements[key]
                .spellings
                .iter()
                .map(String::as_str)
                .collect();
            spellings.push(as_written);
            quote_names(fact.quote(), &quote_words, &key_words, &spellings)
        };
        if !named(&subject, fact.subject()) || !named(&object, fact.object()) {
            tally.facts_rejected_unnamed += 1;
            continue;
        }
        let identity = (
            subject.clone(),
            fact.predicate().to_string(),
            object.clone(),
            part.chunk_id().to_string(),
        );
        if said.insert(identity, ()).is_some() {
            tally.facts_repeated += 1;
            continue;
        }
        facts.push(FactStatement {
            subject,
            predicate: fact.predicate().to_string(),
            object,
            description: fact.description().to_string(),
            weight: fact.weight(),
            chunk_id: part.chunk_id().to_string(),
            quote: fact.quote().to_string(),
        });
    }
    tally.facts_kept = facts.len();

    let proved = |key: &EntityKey| {
        facts
            .iter()
            .any(|fact| fact.subject() == key || fact.object() == key)
    };
    let mut entities = Vec::new();
    for key in order {
        let Some(statement) = statements.remove(&key) else {
            continue;
        };
        if !proved(&key) {
            tally.entities_unproved += 1;
            continue;
        }
        tally.spellings_glued += statement.spellings.len() - 1;
        entities.push(statement);
    }
    tally.entities_kept = entities.len();

    RevisionKnowledge {
        entities,
        facts,
        tally,
    }
}

#[cfg(test)]
mod tests {
    use super::{SourcePart, verify};
    use crate::domain::knowledge::{
        EntityDraft, EntityKey, FactDraft, KnowledgeProposal, KnowledgeVocabulary,
    };

    const PART: &str = "Каждый пакет документов принадлежит одному контрагенту. \
                        Контрагент уникален по БИН.";

    fn vocabulary() -> KnowledgeVocabulary {
        KnowledgeVocabulary::new(
            vec!["concept".to_string(), "attribute".to_string()],
            vec!["belongs-to".to_string(), "identified-by".to_string()],
        )
        .unwrap()
    }

    /// A stand-in for the index's reduction: lowered, and a Russian plural ending cut. Enough to
    /// show that ends are matched by key and not by spelling.
    fn key_of(name: &str) -> Option<EntityKey> {
        EntityKey::from_words(name.split_whitespace().map(|word| {
            let lowered = word.to_lowercase();
            lowered
                .strip_suffix('ы')
                .map(str::to_string)
                .unwrap_or(lowered)
        }))
    }

    fn entity(name: &str, entity_type: &str) -> EntityDraft {
        EntityDraft {
            name: name.to_string(),
            entity_type: entity_type.to_string(),
            description: format!("{name} в реестре."),
        }
    }

    fn fact(subject: &str, predicate: &str, object: &str, part: i64, quote: &str) -> FactDraft {
        FactDraft {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            description: String::new(),
            weight: 5,
            part,
            quote: quote.to_string(),
        }
    }

    fn parts() -> Vec<SourcePart> {
        vec![SourcePart::new(1, "chunk:a", PART)]
    }

    #[test]
    fn a_fact_with_its_quote_in_its_part_is_kept_and_joins_entities_by_key() {
        let proposal = KnowledgeProposal::from_drafts(
            vec![
                entity("Пакет", "concept"),
                entity("Контрагент", "concept"),
                entity("Контрагенты", "concept"),
                entity("БИН", "attribute"),
            ],
            vec![
                fact(
                    "пакет",
                    "belongs-to",
                    "КОНТРАГЕНТ",
                    1,
                    "пакет документов принадлежит одному контрагенту",
                ),
                fact(
                    "Контрагенты",
                    "identified-by",
                    "БИН",
                    1,
                    "Контрагент уникален по БИН",
                ),
            ],
        );

        let knowledge = verify(&proposal, &parts(), &vocabulary(), &key_of);

        assert_eq!(knowledge.facts().len(), 2);
        let first = &knowledge.facts()[0];
        assert_eq!(first.subject().as_str(), "пакет");
        assert_eq!(first.object().as_str(), "контрагент");
        assert_eq!(first.chunk_id(), "chunk:a");
        let counterparty = &knowledge.entities()[1];
        assert_eq!(counterparty.key().as_str(), "контрагент");
        assert_eq!(counterparty.spellings(), ["Контрагент", "Контрагенты"]);
        let tally = knowledge.tally();
        assert_eq!(tally.facts_kept, 2);
        assert_eq!(tally.entities_kept, 3);
        assert_eq!(tally.spellings_glued, 1);
        assert_eq!(tally.facts_proposed(), 2);
    }

    #[test]
    fn a_quote_the_part_does_not_hold_rejects_the_fact_and_is_counted_apart() {
        let proposal = KnowledgeProposal::from_drafts(
            vec![entity("Пакет", "concept"), entity("Контрагент", "concept")],
            vec![fact(
                "Пакет",
                "belongs-to",
                "Контрагент",
                1,
                "пакет всегда принадлежит контрагенту",
            )],
        );

        let knowledge = verify(&proposal, &parts(), &vocabulary(), &key_of);

        assert!(knowledge.facts().is_empty());
        assert_eq!(knowledge.tally().facts_rejected_by_quote, 1);
        assert_eq!(knowledge.tally().facts_rejected_otherwise, 0);
        // Nothing proves either entity any more, so neither is kept.
        assert!(knowledge.entities().is_empty());
        assert_eq!(knowledge.tally().entities_unproved, 2);
    }

    /// A quote found in its part and naming only one end proves nothing about the other.
    #[test]
    fn a_quote_that_does_not_name_both_ends_rejects_the_fact_and_is_counted_apart() {
        let proposal = KnowledgeProposal::from_drafts(
            vec![entity("Контрагент", "concept"), entity("БИН", "attribute")],
            vec![
                fact("Контрагент", "identified-by", "БИН", 1, "уникален по БИН."),
                fact(
                    "Контрагент",
                    "identified-by",
                    "БИН",
                    1,
                    "Контрагент уникален по БИН",
                ),
            ],
        );

        let knowledge = verify(&proposal, &parts(), &vocabulary(), &key_of);

        assert_eq!(knowledge.tally().facts_rejected_unnamed, 1);
        assert_eq!(knowledge.tally().facts_rejected_by_quote, 0);
        assert_eq!(knowledge.facts().len(), 1);
        assert_eq!(knowledge.facts()[0].quote(), "Контрагент уникален по БИН");
        assert_eq!(knowledge.tally().facts_proposed(), 2);
    }

    #[test]
    fn every_other_way_a_fact_is_not_proved_is_counted_otherwise() {
        let quote = "Контрагент уникален по БИН";
        let proposal = KnowledgeProposal::from_drafts(
            vec![
                entity("Контрагент", "concept"),
                entity("БИН", "attribute"),
                entity("Договор", "document"),
            ],
            vec![
                fact("Контрагент", "unique-by", "БИН", 1, quote),
                fact("Контрагент", "identified-by", "ИИН", 1, quote),
                fact("Контрагент", "identified-by", "Договор", 1, quote),
                fact("Контрагент", "identified-by", "БИН", 2, quote),
                fact("Контрагент", "identified-by", "Контрагенты", 1, quote),
                fact("Контрагент", "identified-by", "БИН", 1, quote),
                fact("Контрагент", "identified-by", "БИН", 1, quote),
            ],
        );

        let knowledge = verify(&proposal, &parts(), &vocabulary(), &key_of);
        let tally = knowledge.tally();

        assert_eq!(tally.facts_kept, 1);
        assert_eq!(tally.facts_repeated, 1);
        // An unknown relation, an end that is no entity, an end of an untyped entity, a part
        // the revision does not have, and both ends one entity.
        assert_eq!(tally.facts_rejected_otherwise, 5);
        assert_eq!(tally.facts_rejected_by_quote, 0);
        assert_eq!(tally.facts_proposed(), 7);
        assert_eq!(
            tally.entities_refused, 1,
            "«document» is not in the vocabulary"
        );
    }
}
