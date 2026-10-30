// Use case: the knowledge of one context, as the map and the commands read it.
//
// The graph holds facts and mentions of revisions. What is shown is narrower, and the rule is
// the one search follows (ADR 0034, ADR 0041): only the current revision of a file says
// anything, and only through a path this context's policy still admits. A fact of a revision a
// newer one replaced is not shown even before a sweep deletes it; a fact of a file whose every
// path the policy now hides is not shown and is not named.
//
// A fact's source is said the way a person can follow it: the path of the file, or the
// conversation with its session and, when the chunks of that conversation line up with its
// replies, the reply.
use std::collections::{BTreeSet, HashMap};

use crate::domain::NormalizedPath;
use crate::domain::knowledge::{
    FactSource, KnowledgeMap, MapFact, MentionedEntity, StoredFact, StoredMention,
};

use super::insight_analysis::EntityNames;
use super::repository::RepositoryError;

/// The reads the knowledge layer needs. Every one of them is of what is already durable, so
/// none takes a transaction or the write lock (ADR 0034).
pub trait KnowledgeReader {
    /// Every mention whose revision is the current version of its file.
    fn current_mentions(&self) -> Result<Vec<StoredMention>, RepositoryError>;

    /// Every fact whose revision is the current version of its file.
    fn current_facts(&self) -> Result<Vec<StoredFact>, RepositoryError>;

    /// Where each of these revisions can be opened.
    fn revision_sources(
        &self,
        revision_ids: &[String],
    ) -> Result<Vec<RevisionSource>, RepositoryError>;

    /// What the knowledge layer has cost and kept, and what the queue still holds.
    fn knowledge_totals(&self) -> Result<KnowledgeTotals, RepositoryError>;
}

/// A revision's place: the paths its file is present at, and the conversation it is, if it is
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionSource {
    pub revision_id: String,
    pub present_paths: Vec<NormalizedPath>,
    pub conversation: Option<SourceConversation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceConversation {
    pub session_id: String,
    pub name: String,
    /// Whether chunk zero is the conversation's name rather than a reply.
    pub named: bool,
    pub message_count: u64,
    pub chunk_count: u64,
}

impl SourceConversation {
    /// The reply chunk `ordinal` was cut from, when every reply took one chunk; nothing when
    /// some reply took several, because then the reply is a span (`domain::reply_chunk`).
    fn reply_of(&self, ordinal: i64) -> Option<u64> {
        let first_reply_chunk = u64::from(self.named);
        let reply_chunks = self.chunk_count.checked_sub(first_reply_chunk)?;
        let ordinal = u64::try_from(ordinal).ok()?;
        (reply_chunks == self.message_count && ordinal >= first_reply_chunk)
            .then(|| ordinal - first_reply_chunk)
    }
}

/// One call that was paid for: how many bytes it sent and what the engine said it cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaidCall {
    pub bytes: i64,
    pub cost_micro_usd: i64,
}

/// What a call costs, read off the calls already paid: a fixed part — the tool's own
/// instructions, which every call carries whatever it sends — and a part per byte of content.
/// Two numbers because one would mislead either way: a rate per part measured on short notes
/// is mostly the fixed part and overstates a long document's parts tenfold, and a rate per
/// byte measured on long documents forgets that every short note still pays a whole call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostRate {
    pub per_call_micro_usd: f64,
    pub per_byte_micro_usd: f64,
    pub calls: usize,
}

impl CostRate {
    /// The least-squares line through the paid calls, or the mean cost of a call when the calls
    /// do not differ in size enough to tell the two parts apart. Nothing before the first price.
    pub fn fitted(calls: &[PaidCall]) -> Option<Self> {
        let priced: Vec<(f64, f64)> = calls
            .iter()
            .filter(|call| call.cost_micro_usd > 0)
            .map(|call| (call.bytes as f64, call.cost_micro_usd as f64))
            .collect();
        if priced.is_empty() {
            return None;
        }
        let count = priced.len() as f64;
        let mean_bytes = priced.iter().map(|(bytes, _)| bytes).sum::<f64>() / count;
        let mean_cost = priced.iter().map(|(_, cost)| cost).sum::<f64>() / count;
        let spread: f64 = priced
            .iter()
            .map(|(bytes, _)| (bytes - mean_bytes).powi(2))
            .sum();
        let together: f64 = priced
            .iter()
            .map(|(bytes, cost)| (bytes - mean_bytes) * (cost - mean_cost))
            .sum();
        let average = Self {
            per_call_micro_usd: mean_cost,
            per_byte_micro_usd: 0.0,
            calls: priced.len(),
        };
        if priced.len() < 2 || spread == 0.0 {
            return Some(average);
        }
        let per_byte = together / spread;
        let per_call = mean_cost - per_byte * mean_bytes;
        // A negative part is noise read as a law: a call does not get cheaper as it grows, and
        // no call is paid less than nothing.
        if per_byte < 0.0 || per_call < 0.0 {
            return Some(average);
        }
        Some(Self {
            per_call_micro_usd: per_call,
            per_byte_micro_usd: per_byte,
            calls: priced.len(),
        })
    }
}

/// What the knowledge layer amounts to in one context.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KnowledgeTotals {
    /// Entities, facts and mentions the graph holds now, current or not yet retired.
    pub entities: usize,
    pub facts: usize,
    pub mentions: usize,
    /// Distinct spellings across all mentions: what the keys glued is this minus `entities`.
    pub spellings: usize,
    /// Revisions whose answer was checked for facts, and what that cost, summed over every
    /// result ever written — a superseded revision still cost what it cost.
    pub checked_revisions: usize,
    pub checked_chunks: i64,
    pub checked_bytes: i64,
    pub facts_kept: i64,
    pub facts_rejected_by_quote: i64,
    pub facts_rejected_unnamed: i64,
    pub facts_rejected_otherwise: i64,
    /// Tokens and price of the calls that were paid; an answer served from the cache is not
    /// paid twice.
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micro_usd: i64,
    pub paid_calls: Vec<PaidCall>,
    /// What the queue still holds: jobs, and the chunks and bytes of their revisions.
    pub queued_jobs: usize,
    pub queued_chunks: i64,
    pub queued_bytes: i64,
}

impl KnowledgeTotals {
    /// Distinct spellings the keys folded into an entity they share.
    pub fn spellings_glued(&self) -> usize {
        self.spellings.saturating_sub(self.entities)
    }

    /// What one chunk cost, in millionths of a dollar, or nothing before the first.
    pub fn cost_per_chunk_micro_usd(&self) -> Option<i64> {
        (self.checked_chunks > 0).then(|| self.cost_micro_usd / self.checked_chunks)
    }

    pub fn rate(&self) -> Option<CostRate> {
        CostRate::fitted(&self.paid_calls)
    }

    /// What the queue would cost at the rate measured so far: one call per job, and its bytes.
    pub fn forecast_micro_usd(&self) -> Option<i64> {
        let rate = self.rate()?;
        let forecast = rate.per_call_micro_usd * self.queued_jobs as f64
            + rate.per_byte_micro_usd * self.queued_bytes as f64;
        Some(forecast.round() as i64)
    }
}

pub struct KnowledgeMapService<'a, Reader> {
    reader: &'a Reader,
    is_visible: &'a dyn Fn(&NormalizedPath) -> bool,
}

impl<'a, Reader: KnowledgeReader> KnowledgeMapService<'a, Reader> {
    pub fn new(reader: &'a Reader, is_visible: &'a dyn Fn(&NormalizedPath) -> bool) -> Self {
        Self { reader, is_visible }
    }

    /// The map of this context, cut to `limit` entities.
    pub fn map(&self, limit: usize) -> Result<KnowledgeMap, RepositoryError> {
        let mentions = self.reader.current_mentions()?;
        let facts = self.reader.current_facts()?;
        let revisions: BTreeSet<String> = mentions
            .iter()
            .map(|mention| mention.revision_id.clone())
            .chain(facts.iter().map(|fact| fact.revision_id.clone()))
            .collect();
        let revisions: Vec<String> = revisions.into_iter().collect();
        let sources: HashMap<String, RevisionSource> = self
            .reader
            .revision_sources(&revisions)?
            .into_iter()
            .map(|source| (source.revision_id.clone(), source))
            .collect();
        let shown_at = |revision: &str| -> Option<(&RevisionSource, &NormalizedPath)> {
            let source = sources.get(revision)?;
            let path = source
                .present_paths
                .iter()
                .find(|path| (self.is_visible)(path))?;
            Some((source, path))
        };

        let visible_mentions: Vec<StoredMention> = mentions
            .into_iter()
            .filter(|mention| shown_at(&mention.revision_id).is_some())
            .collect();
        let shown_facts: Vec<MapFact> = facts
            .into_iter()
            .filter_map(|fact| {
                let (source, path) = shown_at(&fact.revision_id)?;
                let source = fact_source(source, path, fact.chunk_ordinal);
                Some(MapFact::new(fact, source))
            })
            .collect();
        Ok(KnowledgeMap::bounded(
            MentionedEntity::gather(&visible_mentions),
            shown_facts,
            limit,
        ))
    }
}

fn fact_source(source: &RevisionSource, path: &NormalizedPath, ordinal: i64) -> FactSource {
    let path = path.as_str().to_string();
    match &source.conversation {
        None => FactSource::File { path },
        Some(conversation) => FactSource::Conversation {
            path,
            session_id: conversation.session_id.clone(),
            name: conversation.name.clone(),
            reply: conversation.reply_of(ordinal),
        },
    }
}

/// The entities of `map` a person meant by `name`: the one whose key it reduces to, or else
/// every one whose shown name or spelling contains it, ignoring case.
pub fn entities_named<'m>(
    map: &'m KnowledgeMap,
    name: &str,
    names: &dyn EntityNames,
) -> Vec<&'m crate::domain::knowledge::MapEntity> {
    if let Some(key) = names.key_of(name) {
        let id = key.entity_id();
        if let Some(entity) = map.entity(&id) {
            return vec![entity];
        }
    }
    let wanted = name.trim().to_lowercase();
    if wanted.is_empty() {
        return Vec::new();
    }
    map.entities()
        .iter()
        .filter(|entity| {
            entity
                .spellings()
                .iter()
                .any(|spelling| spelling.to_lowercase().contains(&wanted))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{
        CostRate, KnowledgeMapService, KnowledgeReader, KnowledgeTotals, PaidCall, RevisionSource,
        SourceConversation, entities_named,
    };
    use crate::application::{EntityNames, RepositoryError};
    use crate::domain::NormalizedPath;
    use crate::domain::knowledge::{EntityKey, FactSource, StoredFact, StoredMention};

    struct Lowered;

    impl EntityNames for Lowered {
        fn key_of(&self, name: &str) -> Option<EntityKey> {
            EntityKey::from_words(name.split_whitespace().map(str::to_lowercase))
        }
    }

    fn mention(entity: &str, revision: &str) -> StoredMention {
        StoredMention {
            entity_id: format!("entity:{}", entity.to_lowercase()),
            revision_id: revision.to_string(),
            name: entity.to_string(),
            entity_type: "concept".to_string(),
            description: String::new(),
            file_id: revision.split('@').next().unwrap().to_string(),
        }
    }

    fn fact(id: &str, revision: &str, ordinal: i64) -> StoredFact {
        StoredFact {
            id: id.to_string(),
            subject_id: "entity:пакет".to_string(),
            predicate: "belongs-to".to_string(),
            object_id: "entity:контрагент".to_string(),
            description: String::new(),
            weight: 5,
            quote: "пакет принадлежит контрагенту".to_string(),
            file_id: revision.split('@').next().unwrap().to_string(),
            revision_id: revision.to_string(),
            chunk_ordinal: ordinal,
        }
    }

    fn path(text: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(text).unwrap()
    }

    struct StubReader {
        asked: RefCell<Vec<String>>,
    }

    impl KnowledgeReader for StubReader {
        fn current_mentions(&self) -> Result<Vec<StoredMention>, RepositoryError> {
            Ok(vec![
                mention("Пакет", "doc@1"),
                mention("Контрагент", "doc@1"),
                mention("Пакет", "chat@1"),
                mention("Контрагент", "chat@1"),
                mention("Пакет", "secret@1"),
                mention("Тайна", "secret@1"),
            ])
        }

        fn current_facts(&self) -> Result<Vec<StoredFact>, RepositoryError> {
            let mut hidden = fact("hidden", "secret@1", 0);
            hidden.object_id = "entity:тайна".to_string();
            Ok(vec![
                fact("doc", "doc@1", 0),
                fact("chat", "chat@1", 3),
                hidden,
            ])
        }

        fn revision_sources(
            &self,
            revision_ids: &[String],
        ) -> Result<Vec<RevisionSource>, RepositoryError> {
            self.asked.borrow_mut().extend(revision_ids.iter().cloned());
            Ok(vec![
                RevisionSource {
                    revision_id: "doc@1".to_string(),
                    present_paths: vec![path("/work/hidden/old.md"), path("/work/doc.md")],
                    conversation: None,
                },
                RevisionSource {
                    revision_id: "chat@1".to_string(),
                    present_paths: vec![path("/chats/s.jsonl")],
                    conversation: Some(SourceConversation {
                        session_id: "s".to_string(),
                        name: "Разбор реестра".to_string(),
                        named: true,
                        message_count: 4,
                        chunk_count: 5,
                    }),
                },
                RevisionSource {
                    revision_id: "secret@1".to_string(),
                    present_paths: vec![path("/work/hidden/secret.md")],
                    conversation: None,
                },
            ])
        }

        fn knowledge_totals(&self) -> Result<KnowledgeTotals, RepositoryError> {
            Ok(KnowledgeTotals::default())
        }
    }

    fn not_hidden(path: &NormalizedPath) -> bool {
        !path.as_str().contains("/hidden/")
    }

    #[test]
    fn a_fact_is_shown_through_an_admitted_path_and_names_its_source() {
        let reader = StubReader {
            asked: RefCell::new(Vec::new()),
        };

        let map = KnowledgeMapService::new(&reader, &not_hidden)
            .map(100)
            .unwrap();

        let ids: Vec<&str> = map
            .facts()
            .iter()
            .map(|one| one.fact().id.as_str())
            .collect();
        assert_eq!(ids, ["doc", "chat"]);
        assert_eq!(
            map.facts()[0].source(),
            &FactSource::File {
                path: "/work/doc.md".to_string()
            },
            "the path shown is one the policy admits"
        );
        assert_eq!(
            map.facts()[1].source(),
            &FactSource::Conversation {
                path: "/chats/s.jsonl".to_string(),
                session_id: "s".to_string(),
                name: "Разбор реестра".to_string(),
                reply: Some(2),
            }
        );
        assert!(
            map.entity("entity:тайна").is_none(),
            "an entity only a hidden file mentions is not named"
        );
        assert_eq!(map.entity("entity:пакет").unwrap().files(), 2);
        assert_eq!(*reader.asked.borrow(), ["chat@1", "doc@1", "secret@1"]);
    }

    #[test]
    fn a_reply_is_named_only_when_every_reply_took_one_chunk() {
        let conversation = SourceConversation {
            session_id: "s".to_string(),
            name: String::new(),
            named: false,
            message_count: 3,
            chunk_count: 3,
        };
        assert_eq!(conversation.reply_of(2), Some(2));
        let cut = SourceConversation {
            chunk_count: 4,
            ..conversation.clone()
        };
        assert_eq!(cut.reply_of(2), None);
        let named = SourceConversation {
            named: true,
            chunk_count: 4,
            ..conversation
        };
        assert_eq!(named.reply_of(0), None, "chunk zero is the name");
        assert_eq!(named.reply_of(1), Some(0));
    }

    #[test]
    fn a_name_finds_its_entity_by_key_and_otherwise_by_what_it_contains() {
        let reader = StubReader {
            asked: RefCell::new(Vec::new()),
        };
        let map = KnowledgeMapService::new(&reader, &not_hidden)
            .map(100)
            .unwrap();

        let by_key = entities_named(&map, "КОНТРАГЕНТ", &Lowered);
        assert_eq!(by_key.len(), 1);
        assert_eq!(by_key[0].name(), "Контрагент");
        let by_part = entities_named(&map, "агент", &Lowered);
        assert_eq!(by_part.len(), 1);
        assert!(entities_named(&map, "договор", &Lowered).is_empty());
    }

    fn paid(bytes: i64, cost_micro_usd: i64) -> PaidCall {
        PaidCall {
            bytes,
            cost_micro_usd,
        }
    }

    /// Calls of 1 000 and 3 000 bytes at 60 000 and 80 000: 50 000 a call and 10 a byte.
    #[test]
    fn a_rate_has_a_part_every_call_pays_and_a_part_per_byte() {
        let rate =
            CostRate::fitted(&[paid(1_000, 60_000), paid(3_000, 80_000), paid(9, 0)]).unwrap();

        assert!((rate.per_call_micro_usd - 50_000.0).abs() < 1e-6);
        assert!((rate.per_byte_micro_usd - 10.0).abs() < 1e-9);
        assert_eq!(
            rate.calls, 2,
            "an unpriced call says nothing about the price"
        );
    }

    #[test]
    fn calls_that_cannot_tell_the_two_parts_apart_are_averaged() {
        let same_size = CostRate::fitted(&[paid(300, 70_000), paid(300, 80_000)]).unwrap();
        assert_eq!(same_size.per_call_micro_usd, 75_000.0);
        assert_eq!(same_size.per_byte_micro_usd, 0.0);
        // Bigger and cheaper is noise, not a law.
        let noisy = CostRate::fitted(&[paid(100, 90_000), paid(5_000, 60_000)]).unwrap();
        assert_eq!(noisy.per_byte_micro_usd, 0.0);
        assert_eq!(CostRate::fitted(&[]), None);
        assert_eq!(CostRate::fitted(&[paid(100, 0)]), None);
    }

    #[test]
    fn a_forecast_is_the_queue_at_the_rate_measured_so_far() {
        let totals = KnowledgeTotals {
            entities: 10,
            spellings: 13,
            checked_chunks: 40,
            cost_micro_usd: 140_000,
            paid_calls: vec![paid(1_000, 60_000), paid(3_000, 80_000)],
            queued_jobs: 100,
            queued_bytes: 500_000,
            ..KnowledgeTotals::default()
        };

        assert_eq!(totals.spellings_glued(), 3);
        assert_eq!(totals.cost_per_chunk_micro_usd(), Some(3_500));
        assert_eq!(totals.forecast_micro_usd(), Some(100 * 50_000 + 5_000_000));
        assert_eq!(KnowledgeTotals::default().forecast_micro_usd(), None);
    }
}
