// What the database does with an inverted index held as terms and postings. Stage 7 asked
// what our own index would cost before building one, and the first gate was the write: a
// posting is a relationship, and Stage 2K and 2R recorded that a relationship written the
// wrong way brings back a cost that grows with the stored history.
//
// These tests hold the answer to the gate. The write shapes reach both endpoints by key and
// scan nothing; the two shapes those replaced still scan, for the reasons they always did.
// The read shape is here for a different reason: its plan *names* two scans and is not one,
// and a plan read by operator name would have condemned it. What settles that is behaviour,
// and it is measured in `posting_cost_tests`, not claimed here.
//
// Nothing in this module is part of the production schema. The tables are created by the
// test, on the test's own database, and no migration knows about them: the spike measured
// the price and the price is recorded in ADR 0011, which decided against paying it.
use lbug::{Connection, LogicalType, Value};

use super::LadybugDatabaseManager;
use super::query_plan::{primary_key_lookups, primary_key_scans, relationship_scans, table_scans};
use super::test_support::{TestDatabase, evidence_commit};
use crate::application::SourceBatchWriter;

/// The chunk `evidence_commit` stores, which the postings point at.
const CHUNK: &str = "chunk:file:a@hash:a";
const TERM: &str = "обход";

/// The tables an inverted index would add: the term, and the posting that says a term occurs
/// in a chunk. `MANY_MANY`, because a term occurs in many chunks and a chunk holds many
/// terms — which is exactly why the posting is the row that multiplies.
const SCHEMA: &[&str] = &[
    "CREATE NODE TABLE Term(id STRING PRIMARY KEY, document_frequency INT64);",
    "CREATE REL TABLE MENTIONS(FROM Term TO ContentChunk, term_frequency INT64, MANY_MANY);",
];

/// One page of postings, written the way Stage 2K and 2R established: a separate `MATCH` per
/// key, `CREATE` rather than `MERGE` where the row is known to be new, and rows in pages.
/// The number is how many endpoints the statement must reach by primary key.
struct WriteStatement {
    name: &'static str,
    query: &'static str,
    primary_key_lookups: usize,
}

const WRITE_STATEMENTS: &[WriteStatement] = &[
    WriteStatement {
        name: "which terms of this page are already stored",
        query: "UNWIND $rows AS row MATCH (term:Term {id: row.term_id}) RETURN term.id;",
        primary_key_lookups: 1,
    },
    WriteStatement {
        name: "create the terms that are new",
        query: "UNWIND $rows AS row CREATE (:Term {id: row.term_id, document_frequency: 0}) RETURN COUNT(*);",
        primary_key_lookups: 0,
    },
    WriteStatement {
        name: "link a term to a chunk",
        query: "UNWIND $rows AS row MATCH (term:Term {id: row.term_id}) MATCH (chunk:ContentChunk {id: row.chunk_id}) CREATE (term)-[:MENTIONS {term_frequency: row.term_frequency}]->(chunk) RETURN COUNT(*);",
        primary_key_lookups: 2,
    },
    WriteStatement {
        name: "raise the document frequency of a term",
        query: "UNWIND $rows AS row MATCH (term:Term {id: row.term_id}) SET term.document_frequency = term.document_frequency + row.term_frequency RETURN COUNT(*);",
        primary_key_lookups: 1,
    },
];

/// The shapes measured to scan, kept so the reason the statements above are written that way
/// cannot quietly stop being true.
struct ScanningShape {
    name: &'static str,
    query: &'static str,
    table_scans: usize,
    relationship_scans: usize,
}

const SCANNING_SHAPES: &[ScanningShape] = &[
    ScanningShape {
        name: "merge instead of create",
        query: "UNWIND $rows AS row MATCH (term:Term {id: row.term_id}) MATCH (chunk:ContentChunk {id: row.chunk_id}) MERGE (term)-[:MENTIONS]->(chunk) RETURN COUNT(*);",
        table_scans: 1,
        relationship_scans: 1,
    },
    ScanningShape {
        name: "one comma-separated pattern",
        query: "UNWIND $rows AS row MATCH (term:Term {id: row.term_id}), (chunk:ContentChunk {id: row.chunk_id}) CREATE (term)-[:MENTIONS {term_frequency: row.term_frequency}]->(chunk) RETURN COUNT(*);",
        table_scans: 2,
        relationship_scans: 0,
    },
];

fn seeded() -> (TestDatabase, std::sync::Arc<LadybugDatabaseManager>) {
    let database = TestDatabase::new("posting-capability");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    manager
        .repository()
        .commit_batch(vec![evidence_commit("file:a", "/data/docs/a.md")])
        .unwrap();
    {
        let connection = Connection::new(&manager.owner.database).unwrap();
        for statement in SCHEMA {
            connection.query(statement).unwrap();
        }
    }
    (database, manager)
}

/// One posting: a term, the chunk it occurs in, and how often. Every statement above reads
/// its fields from this one row.
fn rows() -> Value {
    let row = Value::Struct(vec![
        ("term_id".to_string(), Value::String(TERM.to_string())),
        ("chunk_id".to_string(), Value::String(CHUNK.to_string())),
        ("term_frequency".to_string(), Value::Int64(3)),
    ]);
    Value::List(LogicalType::from(&row), vec![row])
}

fn plan(connection: &Connection<'_>, query: &str, parameters: Vec<(&str, Value)>) -> String {
    let mut statement = connection.prepare(&format!("EXPLAIN {query}")).unwrap();
    let mut result = connection.execute(&mut statement, parameters).unwrap();
    match result.next().as_deref() {
        Some([Value::String(plan)]) => plan.clone(),
        other => panic!("a plan was expected, received {other:?}"),
    }
}

#[test]
fn every_statement_that_writes_a_posting_reaches_its_rows_by_primary_key() {
    let (_database, manager) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for statement in WRITE_STATEMENTS {
        let plan = plan(&connection, statement.query, vec![("rows", rows())]);
        assert_eq!(
            primary_key_lookups(&plan),
            statement.primary_key_lookups,
            "{}: how many rows this statement reaches by key changed",
            statement.name
        );
        assert_eq!(
            table_scans(&plan),
            0,
            "{}: no node table may be scanned",
            statement.name
        );
        assert_eq!(
            relationship_scans(&plan),
            0,
            "{}: the posting table may not be scanned",
            statement.name
        );
    }
}

/// The same two shapes Stage 2K rejected, on the posting table this time: `MERGE` looks the
/// relationship up by scanning it even with both endpoints in hand, and one comma-separated
/// pattern uses neither key.
#[test]
fn the_shapes_those_statements_replaced_still_scan() {
    let (_database, manager) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for shape in SCANNING_SHAPES {
        let plan = plan(&connection, shape.query, vec![("rows", rows())]);
        assert_eq!(
            table_scans(&plan),
            shape.table_scans,
            "{}: the node scans this shape was replaced for changed",
            shape.name
        );
        assert_eq!(
            relationship_scans(&plan),
            shape.relationship_scans,
            "{}: the relationship scan this shape was replaced for changed",
            shape.name
        );
    }
}

/// Reading one term's postings. The term itself is reached by primary key, which is what
/// this test proves. What it deliberately does not assert is the absence of a relationship
/// scan: the plan contains the operator names `SCAN_REL_TABLE` and `SCAN_NODE_TABLE` for the
/// traversal, and those names do not say whether the work is bounded by the term or by the
/// size of the table. Asserting either way from the plan would be a claim about a word.
/// `posting_cost_tests::a_term_lookup_stays_flat_as_the_posting_table_grows` settles it by
/// behaviour instead, which is the only thing that can.
#[test]
fn reading_one_terms_postings_reaches_the_term_by_primary_key() {
    let (_database, manager) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let plan = plan(
        &connection,
        "MATCH (term:Term {id: $term_id})-[mention:MENTIONS]->(chunk) RETURN chunk.id AS id, CAST(mention.term_frequency AS DOUBLE) AS score ORDER BY score DESC, id ASC LIMIT 20;",
        vec![("term_id", Value::String(TERM.to_string()))],
    );

    assert_eq!(
        primary_key_scans(&plan),
        1,
        "the term the query names must be found by its key, not by a scan"
    );
}

/// Where the posting table differs from everything else this program stores, and the reason
/// it matters more than the price. A job cannot have two results because `HAS_INSIGHT_RESULT`
/// is `ONE_ONE`: the schema holds idempotency and no code checks for it. A posting has to be
/// `MANY_MANY` — a term occurs in many chunks and a chunk holds many terms — and `MANY_MANY`
/// accepts the same pair again. So a page written twice is not refused and not deduplicated:
/// it is stored twice, and the second copy is counted again in every score.
///
/// The consequence is the shape of the writer, not a detail. Making a retry safe would need
/// a probe for postings that already exist, one lookup per posting, on top of the two per
/// posting the write already spends — and those lookups are what
/// `posting_cost_tests::the_price_of_one_posting` measures as the dominant cost.
#[test]
fn the_same_posting_can_be_stored_twice_and_is_then_counted_twice() {
    let (_database, manager) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();
    let link = WRITE_STATEMENTS[2].query;

    connection
        .query(&format!(
            "CREATE (:Term {{id: '{TERM}', document_frequency: 0}});"
        ))
        .unwrap();
    for _ in 0..2 {
        let mut statement = connection.prepare(link).unwrap();
        connection
            .execute(&mut statement, vec![("rows", rows())])
            .unwrap();
    }

    let mut result = connection
        .query("MATCH ()-[mention:MENTIONS]->() RETURN COUNT(mention);")
        .unwrap();
    assert_eq!(
        result.next().as_deref(),
        Some([Value::Int64(2)].as_slice()),
        "the repeated page must be visible as two postings, because nothing refused it"
    );
}

/// The spike never touched the production schema, and this is what says so: the tables the
/// postings would live in do not exist until a test creates them.
#[test]
fn the_production_schema_carries_no_term_or_posting_table() {
    let database = TestDatabase::new("posting-schema");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    manager
        .repository()
        .commit_batch(vec![evidence_commit("file:a", "/data/docs/a.md")])
        .unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();

    // The catalog, not a query that happens to fail: `MATCH (n:MENTIONS)` is refused for a
    // relationship table whether or not it exists, so it would have proved nothing.
    let migrated: Vec<String> = connection
        .query("CALL SHOW_TABLES() RETURN name;")
        .unwrap()
        .filter_map(|row| match row.as_slice() {
            [Value::String(name)] => Some(name.clone()),
            _ => None,
        })
        .collect();

    assert!(
        migrated.iter().any(|name| name == "ContentChunk"),
        "the catalog was read wrongly: the migrated schema must contain ContentChunk, found {migrated:?}"
    );
    for absent in ["Term", "MENTIONS"] {
        assert!(
            !migrated.iter().any(|name| name == absent),
            "{absent} must not be part of the migrated schema, found {migrated:?}"
        );
    }
}
