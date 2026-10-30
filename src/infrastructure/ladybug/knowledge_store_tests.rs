// The knowledge layer against the real database: what a finished job writes, what a newer
// revision replaces, what a sweep retires and what the map reads back.
//
// The model is a fake that answers with a chosen proposal, for the reason every analysis test
// here gives: what has to be sure — that a fact rests on a chunk, that an entity is one node
// for every file that names it, that a replaced revision's facts are gone — is a property of
// this program and of the schema, and none of it needs the network.
use std::sync::Arc;

use super::test_support::{TestDatabase, evidence_commit_saying, query_int64, query_string};
use super::{LadybugDatabaseManager, LadybugInsightCache, LadybugRevisionInput, repository};
use crate::application::{
    DESCRIBED_BY, DescribedInsightHandler, InsightProvider, InsightRequest, InsightWorkerService,
    KnowledgeGraphRepository, KnowledgeMapService, KnowledgeQuestion, KnowledgeReader,
};
use crate::domain::knowledge::{
    EntityDraft, FactDraft, FactSource, KnowledgeProposal, KnowledgeVocabulary,
};
use crate::domain::{
    ContentEgress, InsightBudget, InsightFailureCategory, InsightLease, InsightProvenance,
    InsightRetryPolicy, InsightWorkReport, ModelAnswer, NormalizedPath, RevisionTopic,
};
use crate::infrastructure::search::{AnalyzedEntityNames, built_in_normalizations};

const REGISTRY: &str = "/work/registry.md";
const RULES: &str = "/work/rules.md";
const PACKAGES: &str = "Каждый пакет документов принадлежит одному контрагенту.";
const UNIQUE: &str = "Контрагент уникален по БИН, второй с тем же БИН — ошибка.";

/// Answers every revision with the same proposal: the entities named, and one fact per
/// `(subject, object, quote)`.
struct Proposing {
    entities: Vec<&'static str>,
    facts: Vec<(&'static str, &'static str, &'static str)>,
}

impl InsightProvider for Proposing {
    fn describe(
        &self,
        _request: InsightRequest<'_>,
    ) -> Result<ModelAnswer, InsightFailureCategory> {
        let entities = self
            .entities
            .iter()
            .map(|name| EntityDraft {
                name: name.to_string(),
                entity_type: "concept".to_string(),
                description: format!("{name} в реестре."),
            })
            .collect();
        let facts = self
            .facts
            .iter()
            .map(|(subject, object, quote)| FactDraft {
                subject: subject.to_string(),
                predicate: "related-to".to_string(),
                object: object.to_string(),
                description: String::new(),
                weight: 6,
                part: 1,
                quote: quote.to_string(),
            })
            .collect();
        Ok(ModelAnswer::new(
            RevisionTopic::parse("реестр", 0.8).unwrap(),
            InsightProvenance::from_model(DESCRIBED_BY, "a-model", "p", 100, 10).costing(2_000),
        )
        .with_knowledge(KnowledgeProposal::from_drafts(entities, facts)))
    }
}

fn packages() -> Proposing {
    Proposing {
        entities: vec!["Пакет", "Контрагент", "Контрагенты"],
        facts: vec![(
            "Пакет",
            "Контрагенты",
            "пакет документов принадлежит одному контрагенту",
        )],
    }
}

fn unique() -> Proposing {
    Proposing {
        entities: vec!["Контрагент", "БИН"],
        facts: vec![("Контрагент", "БИН", "Контрагент уникален по БИН")],
    }
}

fn opened(
    test_name: &str,
) -> (
    TestDatabase,
    Arc<LadybugDatabaseManager>,
    repository::LadybugGraphRepository,
) {
    let database = TestDatabase::new(test_name);
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    (database, manager, repository)
}

fn publish(
    repository: &repository::LadybugGraphRepository,
    file: &str,
    path: &str,
    hash: &str,
    at: i64,
    text: &str,
) {
    repository
        .commit_source(evidence_commit_saying(file, path, hash, at, text))
        .unwrap();
}

fn analyse(
    manager: &Arc<LadybugDatabaseManager>,
    repository: &repository::LadybugGraphRepository,
    provider: &Proposing,
    limit: usize,
) -> InsightWorkReport {
    let reader = LadybugRevisionInput::new(Arc::clone(manager));
    let cache = LadybugInsightCache::new(Arc::clone(manager));
    let vocabulary =
        KnowledgeVocabulary::new(vec!["concept".to_string()], vec!["related-to".to_string()])
            .unwrap();
    let names = AnalyzedEntityNames::new(&built_in_normalizations());
    let handler = DescribedInsightHandler::new(
        &reader,
        &cache,
        provider,
        InsightBudget::new(10_000, 100_000),
        "insight-outbox-v1",
        "a-model",
        "p",
        KnowledgeQuestion::new(&vocabulary, &names),
        ContentEgress::Allowed.permit().unwrap(),
    );
    InsightWorkerService::with_lifecycle(
        repository,
        &handler,
        InsightLease::of_milliseconds(600_000),
        InsightRetryPolicy::of_attempts(3),
    )
    .execute(limit, &|_: &NormalizedPath| true)
    .unwrap()
}

fn count(manager: &LadybugDatabaseManager, query: &str) -> i64 {
    query_int64(manager, query, vec![])
}

#[test]
fn a_proved_fact_is_written_with_its_result_and_points_at_two_entities_and_its_chunk() {
    let (_database, manager, repository) = opened("knowledge-written");
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:a",
        100,
        PACKAGES,
    );

    let report = analyse(&manager, &repository, &packages(), 10);

    assert_eq!(report.completed(), 1);
    assert_eq!(count(&manager, "MATCH (fact:Fact) RETURN COUNT(fact);"), 1);
    assert_eq!(
        query_string(
            &manager,
            "MATCH (:Fact)-[:FACT_SUBJECT]->(entity:Entity) RETURN entity.id;",
            vec![]
        ),
        "entity:пакет"
    );
    assert_eq!(
        query_string(
            &manager,
            "MATCH (:Fact)-[:FACT_OBJECT]->(entity:Entity) RETURN entity.key;",
            vec![]
        ),
        "контрагент"
    );
    assert_eq!(
        query_string(
            &manager,
            "MATCH (:Fact)-[:STATED_IN]->(chunk:ContentChunk) RETURN chunk.id;",
            vec![]
        ),
        "chunk:file:registry@hash:a"
    );
    // «Контрагент» and «Контрагенты» are one entity with two spellings.
    assert_eq!(
        count(&manager, "MATCH (entity:Entity) RETURN COUNT(entity);"),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:Entity {id: 'entity:контрагент'})-[:MENTIONED_AS]->(mention:EntityMention)-[:MENTIONED_IN]->(:FileVersion) RETURN COUNT(mention);"
        ),
        2
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (result:InsightResult) WHERE result.knowledge_checked RETURN result.facts_kept;"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (result:InsightResult) RETURN result.cost_micro_usd;"
        ),
        2_000
    );
}

#[test]
fn a_quote_the_chunk_does_not_hold_writes_no_fact_and_is_counted_on_the_result() {
    let (_database, manager, repository) = opened("knowledge-rejected");
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:a",
        100,
        PACKAGES,
    );
    let paraphrase = Proposing {
        entities: vec!["Пакет", "Контрагент"],
        facts: vec![(
            "Пакет",
            "Контрагент",
            "пакет всегда принадлежит контрагенту",
        )],
    };

    analyse(&manager, &repository, &paraphrase, 10);

    assert_eq!(count(&manager, "MATCH (fact:Fact) RETURN COUNT(fact);"), 0);
    assert_eq!(
        count(&manager, "MATCH (entity:Entity) RETURN COUNT(entity);"),
        0
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (result:InsightResult) RETURN result.facts_rejected_by_quote;"
        ),
        1
    );
}

#[test]
fn a_new_revision_replaces_every_fact_its_file_had_and_its_entities_go_with_them() {
    let (_database, manager, repository) = opened("knowledge-replaced");
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:a",
        100,
        PACKAGES,
    );
    analyse(&manager, &repository, &packages(), 10);

    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:b",
        200,
        UNIQUE,
    );
    analyse(&manager, &repository, &unique(), 10);

    assert_eq!(count(&manager, "MATCH (fact:Fact) RETURN COUNT(fact);"), 1);
    assert_eq!(
        query_string(
            &manager,
            "MATCH (fact:Fact) RETURN fact.source_revision_id;",
            vec![]
        ),
        "file:registry@hash:b"
    );
    let mut keys: Vec<String> = ["entity:контрагент", "entity:бин", "entity:пакет"]
        .into_iter()
        .filter(|id| {
            count(
                &manager,
                &format!("MATCH (entity:Entity {{id: '{id}'}}) RETURN COUNT(entity);"),
            ) == 1
        })
        .map(str::to_string)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["entity:бин", "entity:контрагент"],
        "«Пакет» was named by the old revision only"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (mention:EntityMention) RETURN COUNT(mention);"
        ),
        2
    );
}

/// The entity is one node for every file that names it, so replacing one file's facts must
/// leave it standing for the other.
#[test]
fn an_entity_another_file_still_names_outlives_the_replacement() {
    let (_database, manager, repository) = opened("knowledge-shared");
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:a",
        100,
        PACKAGES,
    );
    publish(&repository, "file:rules", RULES, "hash:r", 150, UNIQUE);
    analyse(&manager, &repository, &packages(), 1);
    analyse(&manager, &repository, &unique(), 1);
    assert_eq!(count(&manager, "MATCH (fact:Fact) RETURN COUNT(fact);"), 2);

    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:c",
        300,
        "Пусто.",
    );
    let silent = Proposing {
        entities: vec![],
        facts: vec![],
    };
    analyse(&manager, &repository, &silent, 10);

    assert_eq!(count(&manager, "MATCH (fact:Fact) RETURN COUNT(fact);"), 1);
    assert_eq!(
        count(
            &manager,
            "MATCH (entity:Entity {id: 'entity:контрагент'}) RETURN COUNT(entity);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (entity:Entity {id: 'entity:пакет'}) RETURN COUNT(entity);"
        ),
        0
    );
}

/// A newer revision was published while the model was answering about the older one: what the
/// older one states is already not what the file says, and nothing of it is written.
#[test]
fn a_revision_replaced_before_its_answer_writes_no_knowledge() {
    let (_database, manager, repository) = opened("knowledge-superseded");
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:a",
        100,
        PACKAGES,
    );
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:b",
        200,
        UNIQUE,
    );

    // The oldest revision is claimed first.
    let first = analyse(&manager, &repository, &packages(), 1);

    assert_eq!(first.completed(), 1);
    assert_eq!(count(&manager, "MATCH (fact:Fact) RETURN COUNT(fact);"), 0);
    assert_eq!(
        count(
            &manager,
            "MATCH (result:InsightResult) RETURN COUNT(result);"
        ),
        1,
        "the job is done; only its knowledge is not written"
    );
}

#[test]
fn a_sweep_retires_the_facts_of_a_revision_a_newer_one_replaced() {
    let (_database, manager, repository) = opened("knowledge-stale");
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:a",
        100,
        PACKAGES,
    );
    analyse(&manager, &repository, &packages(), 10);
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:b",
        200,
        UNIQUE,
    );

    let reader_before = repository.current_facts().unwrap();
    let retired = repository.retire_stale_knowledge().unwrap();

    assert!(
        reader_before.is_empty(),
        "a replaced revision is not read even before a sweep"
    );
    assert_eq!(retired, 1);
    assert_eq!(count(&manager, "MATCH (fact:Fact) RETURN COUNT(fact);"), 0);
    assert_eq!(
        count(
            &manager,
            "MATCH (mention:EntityMention) RETURN COUNT(mention);"
        ),
        0
    );
    assert_eq!(
        count(&manager, "MATCH (entity:Entity) RETURN COUNT(entity);"),
        0
    );
    assert_eq!(repository.retire_stale_knowledge().unwrap(), 0);
}

#[test]
fn the_map_reads_each_fact_with_the_path_it_can_be_opened_at_and_the_totals_add_up() {
    let (_database, manager, repository) = opened("knowledge-map");
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:a",
        100,
        PACKAGES,
    );
    publish(&repository, "file:rules", RULES, "hash:r", 150, UNIQUE);
    analyse(&manager, &repository, &packages(), 1);
    analyse(&manager, &repository, &unique(), 1);

    let map = KnowledgeMapService::new(&repository, &|_: &NormalizedPath| true)
        .map(100)
        .unwrap();

    assert_eq!(map.facts().len(), 2);
    assert_eq!(
        map.facts()[0].source(),
        &FactSource::File {
            path: REGISTRY.to_string()
        }
    );
    assert_eq!(map.facts()[0].fact().chunk_ordinal, 0);
    let counterparty = map.entity("entity:контрагент").unwrap();
    assert_eq!(counterparty.degree(), 2);
    assert_eq!(counterparty.files(), 2);
    assert_eq!(counterparty.name(), "Контрагент");

    let hidden =
        KnowledgeMapService::new(&repository, &|path: &NormalizedPath| path.as_str() != RULES)
            .map(100)
            .unwrap();
    assert_eq!(
        hidden.facts().len(),
        1,
        "a fact of a hidden file is not on the map"
    );
    assert!(hidden.entity("entity:бин").is_none());

    let totals = repository.knowledge_totals().unwrap();
    assert_eq!(totals.entities, 3);
    assert_eq!(totals.facts, 2);
    assert_eq!(totals.spellings_glued(), 1);
    assert_eq!(totals.checked_revisions, 2);
    assert_eq!(totals.checked_chunks, 2);
    assert_eq!(totals.facts_kept, 2);
    assert_eq!(totals.cost_micro_usd, 4_000);
    assert_eq!(totals.queued_jobs, 0);
}

#[test]
fn what_the_queue_still_holds_is_counted_for_the_forecast() {
    let (_database, manager, repository) = opened("knowledge-forecast");
    publish(
        &repository,
        "file:registry",
        REGISTRY,
        "hash:a",
        100,
        PACKAGES,
    );
    publish(&repository, "file:rules", RULES, "hash:r", 150, UNIQUE);
    analyse(&manager, &repository, &packages(), 1);

    let totals = repository.knowledge_totals().unwrap();

    assert_eq!(totals.queued_jobs, 1);
    assert_eq!(totals.queued_chunks, 1);
    assert_eq!(totals.queued_bytes, UNIQUE.len() as i64);
    assert_eq!(totals.cost_per_chunk_micro_usd(), Some(2_000));
    assert_eq!(totals.forecast_micro_usd(), Some(2_000));
}
