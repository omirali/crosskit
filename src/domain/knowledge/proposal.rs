// What a model proposed about one revision, cut to size before anything is kept.
//
// Every field a model writes is bounded here, and an item that does not fit is refused whole
// rather than shortened: a description trimmed to its first 240 characters is still the first
// 240 characters of whatever the model chose to put there, and the point of the limit is that a
// description cannot carry a file. The same holds for the number of items — a model asked about
// one document does not get to return a hundred and fifty facts about it.
//
// Nothing here is checked against the source yet: a proposal is what a cache may keep, because
// it depends only on the content and the question. Whether a fact is proved is decided for each
// revision by `verification`, against that revision's own parts.

/// The longest name of an entity. A name is a handful of words.
pub const MAX_NAME_CHARS: usize = 80;
/// The longest description of an entity or of a fact: one sentence.
pub const MAX_DESCRIPTION_CHARS: usize = 240;
/// A quote shorter than this proves nothing in particular: «БИН» occurs in every part about it.
pub const MIN_QUOTE_CHARS: usize = 8;
/// The longest quote: long enough for the sentence that states a fact, short enough that the
/// facts of one revision cannot add up to the revision.
pub const MAX_QUOTE_CHARS: usize = 240;
/// The most entities and facts one answer may hold.
pub const MAX_ENTITIES: usize = 60;
pub const MAX_FACTS: usize = 80;
/// How central a fact is to its source, from 0 to this.
pub const MAX_WEIGHT: i64 = 10;

/// An entity as a model wrote it, before anything was checked. Public fields because this is
/// input and promises nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDraft {
    pub name: String,
    pub entity_type: String,
    pub description: String,
}

/// A fact as a model wrote it. `part` is the number the request gave the part it quotes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactDraft {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub description: String,
    pub weight: i64,
    pub part: i64,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedEntity {
    name: String,
    entity_type: String,
    description: String,
}

impl ProposedEntity {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn entity_type(&self) -> &str {
        &self.entity_type
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedFact {
    subject: String,
    predicate: String,
    object: String,
    description: String,
    weight: u8,
    part: usize,
    quote: String,
}

impl ProposedFact {
    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn predicate(&self) -> &str {
        &self.predicate
    }

    pub fn object(&self) -> &str {
        &self.object
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn weight(&self) -> u8 {
        self.weight
    }

    pub fn part(&self) -> usize {
        self.part
    }

    pub fn quote(&self) -> &str {
        &self.quote
    }
}

/// What one answer proposed, within the limits, and how many items it held that were not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnowledgeProposal {
    entities: Vec<ProposedEntity>,
    facts: Vec<ProposedFact>,
    entities_refused: usize,
    facts_refused: usize,
}

impl KnowledgeProposal {
    /// The drafts that fit, in the order they came; the rest counted and dropped.
    pub fn from_drafts(entities: Vec<EntityDraft>, facts: Vec<FactDraft>) -> Self {
        let mut proposal = Self::default();
        for draft in entities {
            match bounded_entity(draft) {
                Some(entity) if proposal.entities.len() < MAX_ENTITIES => {
                    proposal.entities.push(entity)
                }
                _ => proposal.entities_refused += 1,
            }
        }
        for draft in facts {
            match bounded_fact(draft) {
                Some(fact) if proposal.facts.len() < MAX_FACTS => proposal.facts.push(fact),
                _ => proposal.facts_refused += 1,
            }
        }
        proposal
    }

    /// The same proposal with refusals counted when it was first made. A cached proposal holds
    /// only what fitted; the refusals behind it are part of what it cost and are said again.
    pub fn with_earlier_refusals(mut self, entities: usize, facts: usize) -> Self {
        self.entities_refused += entities;
        self.facts_refused += facts;
        self
    }

    pub fn entities(&self) -> &[ProposedEntity] {
        &self.entities
    }

    pub fn facts(&self) -> &[ProposedFact] {
        &self.facts
    }

    pub fn entities_refused(&self) -> usize {
        self.entities_refused
    }

    pub fn facts_refused(&self) -> usize {
        self.facts_refused
    }
}

fn bounded_entity(draft: EntityDraft) -> Option<ProposedEntity> {
    Some(ProposedEntity {
        name: one_line(&draft.name, 1, MAX_NAME_CHARS)?,
        entity_type: one_line(&draft.entity_type, 1, MAX_NAME_CHARS)?,
        description: one_line(&draft.description, 0, MAX_DESCRIPTION_CHARS)?,
    })
}

fn bounded_fact(draft: FactDraft) -> Option<ProposedFact> {
    if !(0..=MAX_WEIGHT).contains(&draft.weight) {
        return None;
    }
    Some(ProposedFact {
        subject: one_line(&draft.subject, 1, MAX_NAME_CHARS)?,
        predicate: one_line(&draft.predicate, 1, MAX_NAME_CHARS)?,
        object: one_line(&draft.object, 1, MAX_NAME_CHARS)?,
        description: one_line(&draft.description, 0, MAX_DESCRIPTION_CHARS)?,
        weight: u8::try_from(draft.weight).ok()?,
        part: usize::try_from(draft.part).ok().filter(|part| *part >= 1)?,
        // A quote is matched with whitespace collapsed, so it is kept that way: a newline in a
        // stored quote would only be one more way for it to stop being one line.
        quote: one_line(
            &super::quote::collapse_whitespace(&draft.quote),
            MIN_QUOTE_CHARS,
            MAX_QUOTE_CHARS,
        )?,
    })
}

/// `text` trimmed, when it is one line of `least..=most` characters with no control character.
fn one_line(text: &str, least: usize, most: usize) -> Option<String> {
    let trimmed = text.trim();
    let chars = trimmed.chars().count();
    let fits = (least..=most).contains(&chars) && !trimmed.chars().any(char::is_control);
    fits.then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        EntityDraft, FactDraft, KnowledgeProposal, MAX_DESCRIPTION_CHARS, MAX_ENTITIES, MAX_FACTS,
        MAX_NAME_CHARS, MAX_QUOTE_CHARS, MIN_QUOTE_CHARS,
    };

    pub(super) fn entity(name: &str) -> EntityDraft {
        EntityDraft {
            name: name.to_string(),
            entity_type: "concept".to_string(),
            description: "Сторона договора.".to_string(),
        }
    }

    pub(super) fn fact(quote: &str) -> FactDraft {
        FactDraft {
            subject: "Пакет".to_string(),
            predicate: "belongs-to".to_string(),
            object: "Контрагент".to_string(),
            description: "Пакет документов принадлежит контрагенту.".to_string(),
            weight: 7,
            part: 1,
            quote: quote.to_string(),
        }
    }

    #[test]
    fn drafts_within_the_limits_are_kept_in_their_order() {
        let proposal = KnowledgeProposal::from_drafts(
            vec![entity(" Контрагент "), entity("Пакет")],
            vec![fact("каждый пакет принадлежит\n  одному контрагенту")],
        );

        let names: Vec<&str> = proposal.entities().iter().map(|e| e.name()).collect();
        assert_eq!(names, ["Контрагент", "Пакет"]);
        assert_eq!(
            proposal.facts()[0].quote(),
            "каждый пакет принадлежит одному контрагенту"
        );
        assert_eq!(proposal.facts()[0].weight(), 7);
        assert_eq!(proposal.facts()[0].part(), 1);
        assert_eq!(proposal.entities_refused(), 0);
        assert_eq!(proposal.facts_refused(), 0);
    }

    /// Refused whole, not shortened: the point of a limit is that a field cannot carry a file.
    #[test]
    fn an_item_over_any_limit_is_refused_whole_and_counted() {
        let long_description = EntityDraft {
            description: "д".repeat(MAX_DESCRIPTION_CHARS + 1),
            ..entity("Контрагент")
        };
        let many_lines = EntityDraft {
            description: "первая\nвторая".to_string(),
            ..entity("Контрагент")
        };
        let mut bad_facts = vec![
            fact("коротко"),
            fact(&"ц".repeat(MAX_QUOTE_CHARS + 1)),
            FactDraft {
                weight: 11,
                ..fact("каждый пакет принадлежит")
            },
            FactDraft {
                part: 0,
                ..fact("каждый пакет принадлежит")
            },
            FactDraft {
                subject: "и".repeat(MAX_NAME_CHARS + 1),
                ..fact("каждый пакет принадлежит")
            },
        ];
        bad_facts.push(fact(&"ц".repeat(MIN_QUOTE_CHARS)));

        let proposal = KnowledgeProposal::from_drafts(
            vec![long_description, many_lines, entity(""), entity("Пакет")],
            bad_facts,
        );

        assert_eq!(proposal.entities().len(), 1);
        assert_eq!(proposal.entities_refused(), 3);
        assert_eq!(
            proposal.facts().len(),
            1,
            "only the quote at the minimum fits"
        );
        assert_eq!(proposal.facts_refused(), 5);
    }

    #[test]
    fn an_answer_holds_no_more_items_than_the_ceiling() {
        let entities = (0..MAX_ENTITIES + 3)
            .map(|n| entity(&format!("Сущность {n}")))
            .collect();
        let facts = (0..MAX_FACTS + 2)
            .map(|_| fact("каждый пакет принадлежит"))
            .collect();

        let proposal = KnowledgeProposal::from_drafts(entities, facts).with_earlier_refusals(1, 1);

        assert_eq!(proposal.entities().len(), MAX_ENTITIES);
        assert_eq!(proposal.entities_refused(), 4);
        assert_eq!(proposal.facts().len(), MAX_FACTS);
        assert_eq!(proposal.facts_refused(), 3);
    }
}
