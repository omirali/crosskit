// What the database does with the four statements that persist the chunks of a batch.
// Stage 2K established that with `UNWIND` rows a comma-separated pattern joins two table
// scans and `MERGE` scans the relationship table even when both ends are known; these
// tests hold that knowledge against the statements `chunk_store` actually runs.
use lbug::{Connection, LogicalType, Value};

use super::query_plan::{primary_key_lookups, relationship_scans, table_scans};
use super::test_support::{TestDatabase, evidence_commit};
use super::{LadybugDatabaseManager, chunk_store};
use crate::application::SourceBatchWriter;

fn seeded() -> (TestDatabase, std::sync::Arc<LadybugDatabaseManager>) {
    let database = TestDatabase::new("chunk-link-capability");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    manager
        .repository()
        .commit_batch(vec![evidence_commit("file:a", "/data/docs/a.md")])
        .unwrap();
    (database, manager)
}

/// One row carrying every field any of the four statements reads.
fn rows() -> Value {
    let row = Value::Struct(vec![
        ("id".to_string(), Value::String("chunk:a".to_string())),
        (
            "source_revision_id".to_string(),
            Value::String("file:a@hash:a".to_string()),
        ),
        (
            "extractor_key".to_string(),
            Value::String("10plain-text1".to_string()),
        ),
        (
            "extractor_id".to_string(),
            Value::String("plain-text".to_string()),
        ),
        (
            "extractor_version".to_string(),
            Value::String("1".to_string()),
        ),
        ("ordinal".to_string(), Value::Int64(0)),
        ("start_byte".to_string(), Value::Int64(0)),
        ("end_byte".to_string(), Value::Int64(4096)),
        ("text_hash".to_string(), Value::String("h".to_string())),
        ("text".to_string(), Value::String("text".to_string())),
        ("script".to_string(), Value::String("latin".to_string())),
        (
            "extraction_method".to_string(),
            Value::String("plain-text".to_string()),
        ),
        (
            "config_hash".to_string(),
            Value::String("config".to_string()),
        ),
        (
            "input_content_hash".to_string(),
            Value::String("hash:a".to_string()),
        ),
        ("created_at_unix_ms".to_string(), Value::Int64(1)),
    ]);
    Value::List(LogicalType::from(&row), vec![row])
}

fn plan(connection: &Connection<'_>, query: &str) -> String {
    let mut statement = connection.prepare(&format!("EXPLAIN {query}")).unwrap();
    let mut result = connection
        .execute(&mut statement, vec![("rows", rows())])
        .unwrap();
    match result.next().as_deref() {
        Some([Value::String(plan)]) => plan.clone(),
        other => panic!("a plan was expected, received {other:?}"),
    }
}

/// What Stage 2R rests on: every statement that writes a chunk new to the database
/// reaches what it needs by key and scans nothing. The insert has no endpoints to find,
/// so it is held to scanning nothing rather than to a lookup count.
#[test]
fn writing_a_new_chunk_never_scans_a_table() {
    let (_database, manager) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in chunk_store::NEW_CHUNK_STATEMENTS {
        let plan = plan(&connection, query);
        assert_eq!(
            table_scans(&plan),
            0,
            "{name}: no node table may be scanned"
        );
        assert_eq!(
            relationship_scans(&plan),
            0,
            "{name}: the relationship table may not be scanned"
        );
        if *name != "chunk insert" {
            assert_eq!(
                primary_key_lookups(&plan),
                2,
                "{name}: both endpoints must be found by primary key"
            );
        }
    }
}

/// Why the new statements are written the way they are: the shapes they replaced use
/// neither key and scan both the node tables and the relationship table. Kept pinned so
/// the difference stays demonstrable on this build.
#[test]
fn the_link_shapes_that_were_replaced_still_scan() {
    let (_database, manager) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in chunk_store::REPLACED_LINK_SHAPES {
        let plan = plan(&connection, query);
        assert!(
            table_scans(&plan) > 0 && relationship_scans(&plan) > 0,
            "{name}: recorded as scanning nodes and relationships, but the plan no longer does"
        );
        assert_eq!(
            primary_key_lookups(&plan),
            0,
            "{name}: the replaced shape now uses a key, so the comparison it is kept for no longer holds"
        );
    }
}

/// The extractor upsert scans, and is recorded as scanning: what it scans is a table of one
/// row per extractor version, which does not grow with the corpus.
#[test]
fn the_extractor_upsert_is_recorded_as_scanning() {
    let (_database, manager) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in chunk_store::SCANNING_UPSERT {
        let plan = plan(&connection, query);
        assert!(
            table_scans(&plan) > 0,
            "{name}: recorded as scanning, but the plan no longer does"
        );
    }
}

/// Why the repair of an already stored chunk is no longer on the write path: every chunk of
/// an unchanged tree took it on every pass, and each one scans.
#[test]
fn the_repair_shapes_that_were_removed_still_scan() {
    let (_database, manager) = seeded();
    let connection = Connection::new(&manager.owner.database).unwrap();

    for (name, query) in chunk_store::REMOVED_REPAIR_STATEMENTS {
        let plan = plan(&connection, query);
        assert!(
            table_scans(&plan) > 0 || relationship_scans(&plan) > 0,
            "{name}: recorded as scanning, but the plan no longer does"
        );
    }
}
