// The question both engines ask about a revision, and the one way their answers are read.
//
// One question, three answers: the topic of stage 6, and the entities and facts of the
// knowledge layer (ADR 0048), in the shape of the owner's GraphRAG prompt — entity: name, type,
// description; relation: source, target, type, description, weight — with the two things that
// prompt left open closed here. A relation names its ends by the names of the answer's own
// entities, and the program turns them into ids; and it names the part it was read in and the
// words of that part that state it, which the program finds or refuses.
//
// The types are an `enum` in the schema, so an engine that honours it cannot answer outside the
// vocabulary. The domain checks again: a schema is a promise, and the check is the boundary.
use serde_json::{Value, json};

use crate::domain::knowledge::{
    EntityDraft, FactDraft, KnowledgeProposal, KnowledgeVocabulary, ProposedEntity, ProposedFact,
};
use crate::domain::{InsightFailureCategory, RevisionTopic};

/// The question in words. Part of the cache key through each engine's prompt version: change a
/// word here and raise both versions, or an answer to the old wording is served for the new.
///
/// `v2` tells the model the rule the program checks (ADR 0048, «Поправка 2»): a quote names both
/// ends. On the owner's first run under `v1` the rule refused 144 of 185 facts — the model drew
/// «section part-of document» from structure and quoted one line.
pub(in crate::infrastructure) fn instructions(vocabulary: &KnowledgeVocabulary) -> String {
    format!(
        "You read one document of a person's own work. It is given in numbered parts, each \
under a line \"[part N]\". Answer with three things.\n\
\n\
topic: the single subject the document is about — a noun phrase of at most a dozen words, in \
the language the document is written in, naming the subject rather than describing the file. \
confidence: a number between 0 and 1 for how clearly the document supports that topic.\n\
\n\
entities: the things the document is about — concepts of its domain, organisations, people \
and roles, systems, data, documents, processes, rules, tasks and problems. For each: name — \
its canonical name, singular, in the nominative case, in the document's main language; when \
the text calls it by a code identifier or in another language, still name it as the document's \
language does, and give one entity one name throughout. type — one of: {entity_types}. \
description — one sentence, at most 200 characters, saying what it is in this document.\n\
\n\
relations: facts the document states, each between two of your entities, read as \"subject \
predicate object\". subject and object — names exactly as in your entities. predicate — one \
of: {relation_types}; use related-to only when no other fits. description — one sentence. \
weight — an integer from 0 to 10, how central the fact is to the document. part — the number \
of the part that states it. quote — the words of that part that state the fact, copied \
exactly, character for character, with the same letter case and punctuation, 15 to 200 \
characters; never paraphrase, never join words from different places, never add or drop \
markup. The quote must name both ends of the relation: it contains the subject and the object \
as this text writes them. A relation that no single passage of its part states with both its \
ends named — one you read from the document's structure, its headings, or from several places \
together — is not proposed at all.\n\
\n\
Only what the text states; fewer facts are better than invented ones. At most 40 entities and \
60 relations. Outside quote, do not copy the document's text.",
        entity_types = vocabulary.entity_types().join(", "),
        relation_types = vocabulary.relation_types().join(", "),
    )
}

/// The shape the answer must have, with the vocabulary as enums.
pub(in crate::infrastructure) fn answer_schema(vocabulary: &KnowledgeVocabulary) -> Value {
    json!({
        "type": "object",
        "properties": {
            "topic": {"type": "string"},
            "confidence": {"type": "number"},
            "entities": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "type": {"type": "string", "enum": vocabulary.entity_types()},
                        "description": {"type": "string"}
                    },
                    "required": ["name", "type", "description"],
                    "additionalProperties": false
                }
            },
            "relations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string"},
                        "predicate": {"type": "string", "enum": vocabulary.relation_types()},
                        "object": {"type": "string"},
                        "description": {"type": "string"},
                        "weight": {"type": "integer"},
                        "part": {"type": "integer"},
                        "quote": {"type": "string"}
                    },
                    "required": ["subject", "predicate", "object", "description", "weight", "part", "quote"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["topic", "confidence", "entities", "relations"],
        "additionalProperties": false
    })
}

/// The topic and the proposal of one structured answer, or `Schema` when it is not the shape
/// agreed. A missing list is not an empty one: the schema requires both, and an answer
/// without them is not this answer.
pub(in crate::infrastructure) fn parse_structured(
    structured: &Value,
) -> Result<(RevisionTopic, KnowledgeProposal), InsightFailureCategory> {
    let label = structured["topic"]
        .as_str()
        .ok_or(InsightFailureCategory::Schema)?;
    let confidence = structured["confidence"]
        .as_f64()
        .ok_or(InsightFailureCategory::Schema)?;
    let topic =
        RevisionTopic::parse(label, confidence).map_err(|_| InsightFailureCategory::Schema)?;
    Ok((topic, proposal_of(structured)?))
}

fn proposal_of(value: &Value) -> Result<KnowledgeProposal, InsightFailureCategory> {
    let entities = value["entities"]
        .as_array()
        .ok_or(InsightFailureCategory::Schema)?
        .iter()
        .map(entity_draft)
        .collect::<Result<Vec<_>, _>>()?;
    let facts = value["relations"]
        .as_array()
        .ok_or(InsightFailureCategory::Schema)?
        .iter()
        .map(fact_draft)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(KnowledgeProposal::from_drafts(entities, facts))
}

fn entity_draft(value: &Value) -> Result<EntityDraft, InsightFailureCategory> {
    Ok(EntityDraft {
        name: string(value, "name")?,
        entity_type: string(value, "type")?,
        description: string(value, "description")?,
    })
}

fn fact_draft(value: &Value) -> Result<FactDraft, InsightFailureCategory> {
    Ok(FactDraft {
        subject: string(value, "subject")?,
        predicate: string(value, "predicate")?,
        object: string(value, "object")?,
        description: string(value, "description")?,
        weight: integer(value, "weight")?,
        part: integer(value, "part")?,
        quote: string(value, "quote")?,
    })
}

fn string(value: &Value, field: &str) -> Result<String, InsightFailureCategory> {
    value[field]
        .as_str()
        .map(str::to_string)
        .ok_or(InsightFailureCategory::Schema)
}

fn integer(value: &Value, field: &str) -> Result<i64, InsightFailureCategory> {
    value[field].as_i64().ok_or(InsightFailureCategory::Schema)
}

/// A proposal as the cache keeps it: only what fitted, in the answer's own shape, with what did
/// not counted beside it. Read back through the same bounds as a fresh answer.
pub(in crate::infrastructure) fn stored_proposal(proposal: &KnowledgeProposal) -> String {
    let entities: Vec<Value> = proposal.entities().iter().map(entity_value).collect();
    let relations: Vec<Value> = proposal.facts().iter().map(fact_value).collect();
    json!({
        "entities": entities,
        "relations": relations,
        "entities_refused": proposal.entities_refused(),
        "relations_refused": proposal.facts_refused(),
    })
    .to_string()
}

/// The proposal a cache entry holds; an entry written before the knowledge layer holds none.
pub(in crate::infrastructure) fn proposal_from_stored(
    text: &str,
) -> Result<KnowledgeProposal, InsightFailureCategory> {
    if text.is_empty() {
        return Ok(KnowledgeProposal::default());
    }
    let value: Value = serde_json::from_str(text).map_err(|_| InsightFailureCategory::Schema)?;
    let refused = |field: &str| {
        value[field]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .ok_or(InsightFailureCategory::Schema)
    };
    Ok(proposal_of(&value)?
        .with_earlier_refusals(refused("entities_refused")?, refused("relations_refused")?))
}

fn entity_value(entity: &ProposedEntity) -> Value {
    json!({
        "name": entity.name(),
        "type": entity.entity_type(),
        "description": entity.description(),
    })
}

fn fact_value(fact: &ProposedFact) -> Value {
    json!({
        "subject": fact.subject(),
        "predicate": fact.predicate(),
        "object": fact.object(),
        "description": fact.description(),
        "weight": fact.weight(),
        "part": fact.part(),
        "quote": fact.quote(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        answer_schema, instructions, parse_structured, proposal_from_stored, stored_proposal,
    };
    use crate::domain::InsightFailureCategory;
    use crate::domain::knowledge::KnowledgeVocabulary;

    fn vocabulary() -> KnowledgeVocabulary {
        KnowledgeVocabulary::new(
            vec!["concept".to_string(), "attribute".to_string()],
            vec!["identified-by".to_string(), "related-to".to_string()],
        )
        .unwrap()
    }

    fn answer() -> Value {
        json!({
            "topic": "реестр контрагентов",
            "confidence": 0.9,
            "entities": [
                {"name": "Контрагент", "type": "concept", "description": "Сторона договора."},
                {"name": "БИН", "type": "attribute", "description": "Номер юрлица."}
            ],
            "relations": [{
                "subject": "Контрагент", "predicate": "identified-by", "object": "БИН",
                "description": "Контрагент уникален по БИН.", "weight": 9, "part": 1,
                "quote": "Контрагент уникален по БИН"
            }]
        })
    }

    #[test]
    fn the_schema_holds_the_vocabulary_as_enums_and_requires_every_field() {
        let schema = answer_schema(&vocabulary());

        assert_eq!(
            schema["properties"]["entities"]["items"]["properties"]["type"]["enum"],
            json!(["concept", "attribute"])
        );
        assert_eq!(
            schema["properties"]["relations"]["items"]["properties"]["predicate"]["enum"],
            json!(["identified-by", "related-to"])
        );
        assert_eq!(
            schema["properties"]["relations"]["items"]["required"],
            json!([
                "subject",
                "predicate",
                "object",
                "description",
                "weight",
                "part",
                "quote"
            ])
        );
        assert_eq!(schema["additionalProperties"], json!(false));
        let words = instructions(&vocabulary());
        assert!(words.contains("one of: concept, attribute."));
        assert!(words.contains("one of: identified-by, related-to;"));
        // The rule the program checks is the rule the model is told.
        assert!(words.contains("The quote must name both ends of the relation"));
    }

    #[test]
    fn an_answer_in_the_agreed_shape_is_a_topic_and_a_proposal() {
        let (topic, proposal) = parse_structured(&answer()).unwrap();

        assert_eq!(topic.label(), "реестр контрагентов");
        assert_eq!(proposal.entities().len(), 2);
        assert_eq!(proposal.facts()[0].part(), 1);
        assert_eq!(proposal.facts()[0].weight(), 9);
    }

    #[test]
    fn an_answer_missing_a_list_or_a_field_is_not_this_answer() {
        let mut no_relations = answer();
        no_relations.as_object_mut().unwrap().remove("relations");
        let mut part_as_text = answer();
        part_as_text["relations"][0]["part"] = json!("1");
        let mut no_topic = answer();
        no_topic["topic"] = json!("");

        for wrong in [no_relations, part_as_text, no_topic] {
            assert_eq!(
                parse_structured(&wrong).unwrap_err(),
                InsightFailureCategory::Schema
            );
        }
    }

    #[test]
    fn a_stored_proposal_reads_back_as_itself_with_its_refusals() {
        let mut over = answer();
        over["relations"]
            .as_array_mut()
            .unwrap()
            .push(json!({"subject": "a", "predicate": "related-to", "object": "b",
                         "description": "", "weight": 99, "part": 1, "quote": "достаточно длинная"}));
        let (_, proposal) = parse_structured(&over).unwrap();
        assert_eq!(proposal.facts_refused(), 1);

        let restored = proposal_from_stored(&stored_proposal(&proposal)).unwrap();

        assert_eq!(restored, proposal);
        assert_eq!(
            proposal_from_stored("").unwrap(),
            Default::default(),
            "an entry from before the knowledge layer proposes nothing"
        );
        assert!(proposal_from_stored("{").is_err());
    }
}
