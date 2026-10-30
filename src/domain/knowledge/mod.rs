// The knowledge layer: entities and the facts that join them, each fact proved by a quote the
// program found in its source.
//
// The map:
//
//   * `vocabulary` — the closed lists of entity and relation types an installation extracts.
//   * `proposal` — what a model proposed for one revision, already cut to the limits that keep
//     a description from becoming a channel for a whole file.
//   * `quote` — whether a quote occurs in the text of the part it names.
//   * `entity_key` — the identity that glues «Контрагент» and «контрагенты» into one entity.
//   * `verification` — a proposal checked against the revision's own parts: what is kept, and
//     why the rest is not.
//   * `map` — the whole graph of one context as a bounded view, and what was cut.
mod entity_key;
mod map;
mod proposal;
mod quote;
mod verification;
mod vocabulary;

pub use entity_key::EntityKey;
pub use map::{
    FactSource, KnowledgeMap, MAP_MOST_ENTITIES, MapEntity, MapFact, MentionedEntity, StoredFact,
    StoredMention,
};
pub use proposal::{EntityDraft, FactDraft, KnowledgeProposal, ProposedEntity, ProposedFact};
/// The quote rule, which proves the nodes of a decision map the way it proves facts.
pub use quote::quote_occurs_in;
pub use verification::{KnowledgeTally, RevisionKnowledge, SourcePart, verify};
pub use vocabulary::{KnowledgeVocabulary, VocabularyError};
