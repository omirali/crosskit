// What one posting costs to store, and whether reading one term stays bounded as the
// posting table grows. Stage 7 was a spike: measure the price of our own inverted index
// before building one. These are the measurements the price came from.
//
// Ignored by default; each says its own command. They write a few hundred megabytes and
// take minutes, and their timings are a baseline, never an assertion: they depend on the
// machine. What is asserted is the one thing that is a property rather than a speed — that
// a term lookup does not grow with the table.
//
// Both live in one binary, so `--test-threads=1` is part of the command: run together they
// compete for the machine and each reports the other's contention as its own cost.
//
//   cargo test --release --offline --bins posting_cost -- --ignored --nocapture --test-threads=1
use std::time::{Duration, Instant};

use lbug::{Connection, LogicalType, Value};

use super::test_support::TestDatabase;

/// Chunks, distinct terms and postings per chunk. The postings per chunk is the number that
/// matters and it is measured, not chosen: real text in this repository yields 157 distinct
/// terms per 4 KiB chunk. The synthetic scale corpus cannot stand in for it — its small
/// notes are built from a fifteen-word vocabulary and average 16.5 distinct terms a chunk,
/// which understates the work by about eight times.
const CHUNKS: i64 = 20_000;
const TERMS: i64 = 20_000;
const POSTINGS_PER_CHUNK: i64 = 80;
const PAGE: usize = 4_096;

/// The statements the writer would run, as `posting_capability_tests` proved them free of
/// scans. `LookupsOnly` is the same two key lookups with nothing created, which is how the
/// cost is attributed to the lookups rather than to the write.
#[derive(Clone, Copy)]
enum Shape {
    Linked,
    Bare,
    LookupsOnly,
}

impl Shape {
    fn name(self) -> &'static str {
        match self {
            Self::Linked => "posting with term frequency",
            Self::Bare => "posting without a property",
            Self::LookupsOnly => "the two key lookups alone",
        }
    }

    fn query(self) -> &'static str {
        match self {
            Self::Linked => {
                "UNWIND $rows AS row MATCH (term:Term {id: row.term_id}) MATCH (chunk:Chunk {id: row.chunk_id}) CREATE (term)-[:MENTIONS {term_frequency: row.term_frequency}]->(chunk) RETURN COUNT(*);"
            }
            Self::Bare => {
                "UNWIND $rows AS row MATCH (term:Term {id: row.term_id}) MATCH (chunk:Chunk {id: row.chunk_id}) CREATE (term)-[:MENTIONS_BARE]->(chunk) RETURN COUNT(*);"
            }
            Self::LookupsOnly => {
                "UNWIND $rows AS row MATCH (term:Term {id: row.term_id}) MATCH (chunk:Chunk {id: row.chunk_id}) RETURN COUNT(*);"
            }
        }
    }
}

fn list(rows: Vec<Value>) -> Value {
    let logical = LogicalType::from(&rows[0]);
    Value::List(logical, rows)
}

fn node_rows(prefix: &str, from: i64, to: i64) -> Vec<Value> {
    (from..to)
        .map(|index| {
            Value::Struct(vec![(
                "id".to_string(),
                Value::String(format!("{prefix}:{index}")),
            )])
        })
        .collect()
}

/// A database holding the chunks and terms, with nothing linked yet.
fn seeded(name: &str) -> (TestDatabase, lbug::Database) {
    let directory = TestDatabase::new(name);
    let database = lbug::Database::new(directory.path(), lbug::SystemConfig::default()).unwrap();
    {
        let connection = Connection::new(&database).unwrap();
        for statement in [
            "CREATE NODE TABLE Chunk(id STRING PRIMARY KEY);",
            "CREATE NODE TABLE Term(id STRING PRIMARY KEY, document_frequency INT64);",
            "CREATE REL TABLE MENTIONS(FROM Term TO Chunk, term_frequency INT64, MANY_MANY);",
            "CREATE REL TABLE MENTIONS_BARE(FROM Term TO Chunk, MANY_MANY);",
        ] {
            connection.query(statement).unwrap();
        }
        for (prefix, count, query) in [
            (
                "chunk",
                CHUNKS,
                "UNWIND $rows AS row CREATE (:Chunk {id: row.id}) RETURN COUNT(*);",
            ),
            (
                "term",
                TERMS,
                "UNWIND $rows AS row CREATE (:Term {id: row.id, document_frequency: 0}) RETURN COUNT(*);",
            ),
        ] {
            let mut statement = connection.prepare(query).unwrap();
            let mut written = 0;
            while written < count {
                let until = (written + PAGE as i64).min(count);
                connection
                    .execute(
                        &mut statement,
                        vec![("rows", list(node_rows(prefix, written, until)))],
                    )
                    .unwrap();
                written = until;
            }
        }
    }
    (directory, database)
}

/// Which term the posting for chunk `chunk`, slot `slot` points at. Deterministic, so two
/// runs write the same postings.
fn term_of(chunk: i64, slot: i64) -> i64 {
    (chunk * 7 + slot * 13) % TERMS
}

fn write_all(connection: &Connection<'_>, shape: Shape) -> (i64, Duration) {
    let mut statement = connection.prepare(shape.query()).unwrap();
    let mut page: Vec<Value> = Vec::with_capacity(PAGE);
    let mut postings = 0;
    let started = Instant::now();
    for chunk in 0..CHUNKS {
        for slot in 0..POSTINGS_PER_CHUNK {
            let mut fields = vec![
                (
                    "term_id".to_string(),
                    Value::String(format!("term:{}", term_of(chunk, slot))),
                ),
                (
                    "chunk_id".to_string(),
                    Value::String(format!("chunk:{chunk}")),
                ),
            ];
            if matches!(shape, Shape::Linked | Shape::LookupsOnly) {
                fields.push(("term_frequency".to_string(), Value::Int64(1 + slot % 3)));
            }
            page.push(Value::Struct(fields));
            postings += 1;
            if page.len() == PAGE {
                connection
                    .execute(
                        &mut statement,
                        vec![("rows", list(std::mem::take(&mut page)))],
                    )
                    .unwrap();
            }
        }
    }
    if !page.is_empty() {
        connection
            .execute(&mut statement, vec![("rows", list(page))])
            .unwrap();
    }
    (postings, started.elapsed())
}

/// The headline number of the spike, and where it goes. Every shape here is one the
/// capability tests proved free of scans, so what is left is the irreducible per-row work.
#[test]
#[ignore = "posting cost diagnostic: cargo test --release --offline --bins the_price_of_one_posting -- --ignored --nocapture --test-threads=1"]
fn the_price_of_one_posting() {
    for shape in [Shape::Linked, Shape::Bare, Shape::LookupsOnly] {
        let (_directory, database) = seeded("posting-price");
        let connection = Connection::new(&database).unwrap();
        let (postings, elapsed) = write_all(&connection, shape);
        println!(
            "{:<30} {postings} postings in {:.2}s — {:.0} postings/s, {:.1} us each",
            shape.name(),
            elapsed.as_secs_f64(),
            postings as f64 / elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1e6 / postings as f64,
        );
    }
}

/// Reading one term. Its plan names `SCAN_REL_TABLE`, and the name is what makes this
/// measurement necessary: a name cannot say whether the work is bounded by the term or by
/// the size of the table. The needle term is given the same few postings at every size, so
/// a lookup that grew with the table would show it.
///
/// This is the one assertion here, and it is deliberately loose: the claim is that the cost
/// does not follow the table, not that it is constant.
#[test]
#[ignore = "posting cost diagnostic: cargo test --release --offline --bins a_term_lookup_stays_flat -- --ignored --nocapture --test-threads=1"]
fn a_term_lookup_stays_flat_as_the_posting_table_grows() {
    const NEEDLE: &str = "term:needle";
    const NEEDLE_POSTINGS: i64 = 5;

    let mut measured = Vec::new();
    for divisor in [8, 1] {
        let (_directory, database) = seeded("posting-lookup");
        let connection = Connection::new(&database).unwrap();

        let mut statement = connection.prepare(Shape::Linked.query()).unwrap();
        let mut page: Vec<Value> = Vec::with_capacity(PAGE);
        let mut postings = 0i64;
        for chunk in 0..(CHUNKS / divisor) {
            for slot in 0..POSTINGS_PER_CHUNK {
                page.push(Value::Struct(vec![
                    (
                        "term_id".to_string(),
                        Value::String(format!("term:{}", term_of(chunk, slot))),
                    ),
                    (
                        "chunk_id".to_string(),
                        Value::String(format!("chunk:{chunk}")),
                    ),
                    ("term_frequency".to_string(), Value::Int64(1)),
                ]));
                postings += 1;
                if page.len() == PAGE {
                    connection
                        .execute(
                            &mut statement,
                            vec![("rows", list(std::mem::take(&mut page)))],
                        )
                        .unwrap();
                }
            }
        }
        if !page.is_empty() {
            connection
                .execute(&mut statement, vec![("rows", list(page))])
                .unwrap();
        }

        connection
            .query(&format!(
                "CREATE (:Term {{id: '{NEEDLE}', document_frequency: {NEEDLE_POSTINGS}}});"
            ))
            .unwrap();
        for chunk in 0..NEEDLE_POSTINGS {
            connection
                .query(&format!(
                    "MATCH (term:Term {{id: '{NEEDLE}'}}) MATCH (chunk:Chunk {{id: 'chunk:{chunk}'}}) CREATE (term)-[:MENTIONS {{term_frequency: 1}}]->(chunk);"
                ))
                .unwrap();
        }

        let mut read = connection
            .prepare("MATCH (term:Term {id: $term_id})-[mention:MENTIONS]->(chunk) RETURN chunk.id AS id, CAST(mention.term_frequency AS DOUBLE) AS score ORDER BY score DESC, id ASC LIMIT 20;")
            .unwrap();
        let mut samples = Vec::new();
        for _ in 0..40 {
            let started = Instant::now();
            let mut result = connection
                .execute(
                    &mut read,
                    vec![("term_id", Value::String(NEEDLE.to_string()))],
                )
                .unwrap();
            let mut rows = 0;
            while result.next().is_some() {
                rows += 1;
            }
            assert_eq!(rows, NEEDLE_POSTINGS, "the needle must keep its postings");
            samples.push(started.elapsed().as_micros() as u64);
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        println!("postings in table: {postings:>9}   needle lookup median: {median} us");
        measured.push((postings, median));
    }

    let (small_table, small_median) = measured[0];
    let (large_table, large_median) = measured[1];
    let table_growth = large_table as f64 / small_table as f64;
    let cost_growth = large_median as f64 / small_median as f64;
    println!("table grew {table_growth:.1}x, the lookup grew {cost_growth:.2}x");
    assert!(
        cost_growth < table_growth / 2.0,
        "a lookup that followed the table would have grown about {table_growth:.1}x; it grew {cost_growth:.2}x, \
         so the traversal is bounded by the term — but this run no longer shows that"
    );
}
