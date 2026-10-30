// The machine answer of `crosskit knowledge map --json`: the knowledge of one context as one
// object of the schema `machine_answer` declares and numbers.
//
// The same rules as for a page of `search` and a view of `graph`: one object on stdout, a
// failure included; a field holds only what the core knows. A new object beside the others,
// so the number does not change (ADR 0044): no field of an existing answer changed meaning.
//
// `types` is the vocabulary in its configured order, so a reader can give each type the same
// colour on every map, whichever types this map happens to hold.
use serde::Serialize;

use crate::application::EntityNames;
use crate::domain::WorkspaceName;
use crate::domain::knowledge::{FactSource, KnowledgeMap, KnowledgeVocabulary, MapEntity, MapFact};

use super::machine_answer::{SCHEMA, object_line};
use super::search::one_line;

#[derive(Serialize)]
struct Map<'a> {
    schema: u32,
    context: &'a str,
    /// The most entities this map may hold.
    limit: usize,
    /// Whether anything found is not in `entities` or `facts`.
    truncated: bool,
    counts: Counts,
    entity_types: &'a [String],
    relation_types: &'a [String],
    /// The roots of the context, so a reader can offer a source to filter by.
    roots: &'a [String],
    entities: Vec<Entity<'a>>,
    facts: Vec<Fact<'a>>,
}

#[derive(Serialize)]
struct Counts {
    entities: Count,
    facts: Count,
}

#[derive(Serialize)]
struct Count {
    shown: usize,
    found: usize,
}

#[derive(Serialize)]
struct Entity<'a> {
    id: &'a str,
    name: String,
    #[serde(rename = "type")]
    entity_type: &'a str,
    description: String,
    /// Every spelling the sources used, `name` first.
    spellings: Vec<String>,
    /// How many files and conversations mention it.
    sources: usize,
    /// How many facts of this map have it at either end.
    degree: usize,
    /// The stems of its name, spellings and description, by the analyzer search uses: what a
    /// reader matches a typed word against, so «контракты» finds «контракт» in a description.
    terms: Vec<String>,
}

#[derive(Serialize)]
struct Fact<'a> {
    id: &'a str,
    from: &'a str,
    to: &'a str,
    predicate: &'a str,
    description: String,
    weight: u8,
    /// The words of the source that state the fact, found there by the program.
    quote: String,
    source: Source<'a>,
}

#[derive(Serialize)]
struct Source<'a> {
    /// `file` or `conversation`.
    kind: &'static str,
    path: &'a str,
    /// The chunk the quote was found in, counted from one.
    part: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation: Option<String>,
    /// The reply, counted from one, when the conversation's chunks line up with its replies.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<u64>,
}

/// The map as the line `knowledge map --json` prints.
pub(super) fn map_line(
    context: &WorkspaceName,
    map: &KnowledgeMap,
    vocabulary: &KnowledgeVocabulary,
    roots: &[String],
    names: &dyn EntityNames,
) -> String {
    object_line(&Map {
        schema: SCHEMA,
        context: context.as_str(),
        limit: map.limit(),
        truncated: map.truncated(),
        counts: Counts {
            entities: Count {
                shown: map.entities().len(),
                found: map.found_entities(),
            },
            facts: Count {
                shown: map.facts().len(),
                found: map.found_facts(),
            },
        },
        entity_types: vocabulary.entity_types(),
        relation_types: vocabulary.relation_types(),
        roots,
        entities: map
            .entities()
            .iter()
            .map(|one| entity(one, names))
            .collect(),
        facts: map.facts().iter().map(fact).collect(),
    })
}

/// Every string here came out of a model's answer about somebody's file, and stays one line.
fn entity<'a>(entity: &'a MapEntity, names: &dyn EntityNames) -> Entity<'a> {
    let mut terms: Vec<String> = Vec::new();
    let texts = entity
        .spellings()
        .iter()
        .map(String::as_str)
        .chain([entity.name(), entity.description()]);
    for text in texts {
        let Some(key) = names.key_of(text) else {
            continue;
        };
        for word in key.as_str().split(' ') {
            if !terms.iter().any(|seen| seen == word) {
                terms.push(word.to_string());
            }
        }
    }
    Entity {
        id: entity.id(),
        name: one_line(entity.name()),
        entity_type: entity.entity_type(),
        description: one_line(entity.description()),
        spellings: entity.spellings().iter().map(|one| one_line(one)).collect(),
        sources: entity.files(),
        degree: entity.degree(),
        terms,
    }
}

fn fact(fact: &MapFact) -> Fact<'_> {
    let stored = fact.fact();
    let part = stored.chunk_ordinal + 1;
    let source = match fact.source() {
        FactSource::File { path } => Source {
            kind: "file",
            path,
            part,
            session_id: None,
            conversation: None,
            reply: None,
        },
        FactSource::Conversation {
            path,
            session_id,
            name,
            reply,
        } => Source {
            kind: "conversation",
            path,
            part,
            session_id: Some(session_id),
            conversation: Some(one_line(name)),
            reply: reply.map(|reply| reply + 1),
        },
    };
    Fact {
        id: &stored.id,
        from: &stored.subject_id,
        to: &stored.object_id,
        predicate: &stored.predicate,
        description: one_line(&stored.description),
        weight: stored.weight,
        quote: one_line(&stored.quote),
        source,
    }
}
