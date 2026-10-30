// What v14 does to a database the previous release wrote, and what it must not do.
//
// Two files in the fixture and they are there for opposite reasons. The transcript is what the
// migration is for: its chunks were produced by a parser that read a conversation as a log,
// and they go, so the next pass can publish what the conversation profile makes of the same
// revision. The document is the control: every working context of this installation is made
// of files like it, and the migration must pass over them without a row changing.
use std::path::Path;

use lbug::Connection;

use super::migration;
use super::test_support::{TestDatabase, open_fixture_database};

/// A database written by the release before Stage 32: schema v13, one transcript and one
/// document, both read by `plain-text`.
fn create_v13_fixture(path: &Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).expect("fixture connection");
    migration::apply_through(&connection, 13).expect("migrate the fixture to v13");
    for statement in [
        // The transcript.
        "CREATE (:File {id: 'file:1'});",
        "CREATE (:FileVersion {id: 'file:1@sha256:a', file_id: 'file:1', content_hash: 'a', created_at_unix_ms: 100});",
        "MATCH (file:File {id: 'file:1'}), (version:FileVersion {id: 'file:1@sha256:a'}) CREATE (file)-[:HAS_VERSION]->(version);",
        "CREATE (:FileLocation {id: 'file:1@/chats/session.jsonl', file_id: 'file:1', normalized_path: '/chats/session.jsonl', display_name: 'session.jsonl', kind: 'file', size_bytes: 9, modified_at_unix_ms: 100, first_seen_at_unix_ms: 100, last_seen_at_unix_ms: 100, status: 'present'});",
        "MATCH (file:File {id: 'file:1'}), (location:FileLocation {id: 'file:1@/chats/session.jsonl'}) CREATE (file)-[:LOCATED_AT]->(location);",
        "CREATE (:Extractor {id: 'plain-text:1', extractor_id: 'plain-text', extractor_version: '1'});",
        "CREATE (:ContentChunk {id: 'chunk:transcript:0', source_revision_id: 'file:1@sha256:a', ordinal: 0, start_byte: 0, end_byte: 9, text_hash: 'h', text: 'вывод инструмента', script: 'cyrillic', extraction_method: 'plain-text', extractor_id: 'plain-text', extractor_version: '1', config_hash: 'c', input_content_hash: 'a', created_at_unix_ms: 100});",
        "MATCH (version:FileVersion {id: 'file:1@sha256:a'}), (chunk:ContentChunk {id: 'chunk:transcript:0'}) CREATE (version)-[:HAS_CHUNK]->(chunk);",
        "MATCH (chunk:ContentChunk {id: 'chunk:transcript:0'}), (extractor:Extractor {id: 'plain-text:1'}) CREATE (chunk)-[:GENERATED_BY]->(extractor);",
        // The document, which must survive untouched.
        "CREATE (:File {id: 'file:2'});",
        "CREATE (:FileVersion {id: 'file:2@sha256:b', file_id: 'file:2', content_hash: 'b', created_at_unix_ms: 200});",
        "MATCH (file:File {id: 'file:2'}), (version:FileVersion {id: 'file:2@sha256:b'}) CREATE (file)-[:HAS_VERSION]->(version);",
        "CREATE (:FileLocation {id: 'file:2@/work/notes.md', file_id: 'file:2', normalized_path: '/work/notes.md', display_name: 'notes.md', kind: 'file', size_bytes: 4, modified_at_unix_ms: 200, first_seen_at_unix_ms: 200, last_seen_at_unix_ms: 200, status: 'present'});",
        "MATCH (file:File {id: 'file:2'}), (location:FileLocation {id: 'file:2@/work/notes.md'}) CREATE (file)-[:LOCATED_AT]->(location);",
        "CREATE (:ContentChunk {id: 'chunk:notes:0', source_revision_id: 'file:2@sha256:b', ordinal: 0, start_byte: 0, end_byte: 4, text_hash: 'h2', text: 'заметка', script: 'cyrillic', extraction_method: 'plain-text', extractor_id: 'plain-text', extractor_version: '1', config_hash: 'c', input_content_hash: 'b', created_at_unix_ms: 200});",
        "MATCH (version:FileVersion {id: 'file:2@sha256:b'}), (chunk:ContentChunk {id: 'chunk:notes:0'}) CREATE (version)-[:HAS_CHUNK]->(chunk);",
        "MATCH (chunk:ContentChunk {id: 'chunk:notes:0'}), (extractor:Extractor {id: 'plain-text:1'}) CREATE (chunk)-[:GENERATED_BY]->(extractor);",
    ] {
        connection.query(statement).expect("fixture data");
    }
}

fn count(connection: &Connection<'_>, query: &str) -> i64 {
    let mut result = connection.query(query).expect("count query");
    let row = result.next().expect("one row");
    match row.first() {
        Some(lbug::Value::Int64(value)) => *value,
        other => panic!("expected a count, got {other:?}"),
    }
}

#[test]
fn a_v13_database_upgrades_to_v14_and_only_the_transcript_loses_its_chunks() {
    let database_path = TestDatabase::new("conversation-migration");
    create_v13_fixture(database_path.path());

    let database = open_fixture_database(database_path.path());
    let connection = Connection::new(&database).expect("connection");
    let version = migration::apply(&connection).expect("migrate to the latest schema");
    assert_eq!(version, migration::LATEST_SCHEMA_VERSION);

    // What the migration was for: the transcript's chunks are gone, so the next pass can
    // publish what the conversation profile makes of the same revision.
    assert_eq!(
        count(
            &connection,
            "MATCH (chunk:ContentChunk {id: 'chunk:transcript:0'}) RETURN COUNT(chunk);"
        ),
        0,
        "чанки транскрипта, прочитанного как лог, должны исчезнуть"
    );
    // The control: an ordinary document is untouched, which is what every working context of
    // this installation is made of.
    assert_eq!(
        count(
            &connection,
            "MATCH (chunk:ContentChunk {id: 'chunk:notes:0'}) RETURN COUNT(chunk);"
        ),
        1,
        "обычный документ миграция трогать не должна"
    );

    // Nothing else went with them. The revision, its location and its identity stay, so
    // history, observations and anything queued against that revision survive.
    assert_eq!(
        count(
            &connection,
            "MATCH (version:FileVersion) RETURN COUNT(version);"
        ),
        2
    );
    assert_eq!(
        count(&connection, "MATCH (file:File) RETURN COUNT(file);"),
        2
    );
    assert_eq!(
        count(
            &connection,
            "MATCH (location:FileLocation) RETURN COUNT(location);"
        ),
        2
    );
    assert_eq!(
        count(
            &connection,
            "MATCH (extractor:Extractor) RETURN COUNT(extractor);"
        ),
        1
    );

    // The three tables the stage adds exist and are empty: a migration adds room, a pass
    // fills it.
    for table in ["Conversation", "Message", "FileTouch"] {
        assert_eq!(
            count(
                &connection,
                &format!("MATCH (node:{table}) RETURN COUNT(node);")
            ),
            0,
            "таблица {table} должна существовать и быть пустой"
        );
    }

    // Applying it again changes nothing, which is what lets `migrate` be run twice.
    assert_eq!(
        migration::apply(&connection).expect("re-apply"),
        migration::LATEST_SCHEMA_VERSION
    );
    assert_eq!(
        count(
            &connection,
            "MATCH (chunk:ContentChunk) RETURN COUNT(chunk);"
        ),
        1
    );
}

/// A context with no transcript in it at all — every working context of this installation —
/// passes the migration without a row changing.
#[test]
fn a_context_without_transcripts_migrates_with_nothing_changing() {
    let database_path = TestDatabase::new("conversation-migration-idle");
    {
        let database = open_fixture_database(database_path.path());
        let connection = Connection::new(&database).expect("fixture connection");
        migration::apply_through(&connection, 13).expect("v13");
        for statement in [
            "CREATE (:File {id: 'file:2'});",
            "CREATE (:FileVersion {id: 'file:2@sha256:b', file_id: 'file:2', content_hash: 'b', created_at_unix_ms: 200});",
            "MATCH (file:File {id: 'file:2'}), (version:FileVersion {id: 'file:2@sha256:b'}) CREATE (file)-[:HAS_VERSION]->(version);",
            "CREATE (:FileLocation {id: 'file:2@/work/main.rs', file_id: 'file:2', normalized_path: '/work/main.rs', display_name: 'main.rs', kind: 'file', size_bytes: 4, modified_at_unix_ms: 200, first_seen_at_unix_ms: 200, last_seen_at_unix_ms: 200, status: 'present'});",
            "MATCH (file:File {id: 'file:2'}), (location:FileLocation {id: 'file:2@/work/main.rs'}) CREATE (file)-[:LOCATED_AT]->(location);",
            "CREATE (:ContentChunk {id: 'chunk:main:0', source_revision_id: 'file:2@sha256:b', ordinal: 0, start_byte: 0, end_byte: 4, text_hash: 'h', text: 'fn main', script: 'latin', extraction_method: 'plain-text', extractor_id: 'plain-text', extractor_version: '1', config_hash: 'c', input_content_hash: 'b', created_at_unix_ms: 200});",
            "MATCH (version:FileVersion {id: 'file:2@sha256:b'}), (chunk:ContentChunk {id: 'chunk:main:0'}) CREATE (version)-[:HAS_CHUNK]->(chunk);",
        ] {
            connection.query(statement).expect("fixture data");
        }
    }

    let database = open_fixture_database(database_path.path());
    let connection = Connection::new(&database).expect("connection");
    assert_eq!(
        migration::apply(&connection).expect("migrate"),
        migration::LATEST_SCHEMA_VERSION
    );
    assert_eq!(
        count(
            &connection,
            "MATCH (chunk:ContentChunk) RETURN COUNT(chunk);"
        ),
        1,
        "в контексте без транскриптов миграция не должна тронуть ни одной строки"
    );
}
