// What v16 does to a database the previous release wrote.
//
// The migration adds two marks to `FileTouch` and the whole question is what it says about
// rows that were there before them. It must not say "the owner never named this file" and it
// must not say "this happened once": nobody looked, and a graph that answers a question
// nobody asked is worse than one that says it does not know.
use std::path::Path;

use lbug::Connection;

use super::migration;
use super::test_support::{TestDatabase, open_fixture_database};
use crate::application::{Direction, TouchQuery, TouchReader, TouchService};
use crate::domain::OwnerNaming;
use crate::domain::conversation::TouchVerb;

/// A database written by the release before Stage 37: schema v15, one conversation that read
/// one file.
fn create_v15_fixture(path: &Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).expect("fixture connection");
    migration::apply_through(&connection, 15).expect("migrate the fixture to v15");
    for statement in [
        "CREATE (:Conversation {id: 'conversation:1', source_revision_id: 'file:1@sha256:a', session_id: 's1', name: 'Беседа Б', named_by: 'model', first_message_at_unix_ms: 100, last_message_at_unix_ms: 200, message_count: 2, created_at_unix_ms: 100});",
        "CREATE (:FileTouch {id: 'conversation:1:t0', conversation_id: 'conversation:1', path: '/work/draft_service.py', verb: 'read', source: 'tool_input', at_unix_ms: 150});",
        "MATCH (conversation:Conversation {id: 'conversation:1'}), (touch:FileTouch {id: 'conversation:1:t0'}) CREATE (conversation)-[:TOUCHED_FILE]->(touch);",
    ] {
        connection.query(statement).expect("fixture data");
    }
}

fn text_of(connection: &Connection<'_>, query: &str) -> String {
    let mut result = connection.query(query).expect("query");
    let row = result.next().expect("one row");
    match row.first() {
        Some(lbug::Value::String(value)) => value.clone(),
        Some(lbug::Value::Int64(value)) => value.to_string(),
        other => panic!("expected a value, got {other:?}"),
    }
}

#[test]
fn a_v15_touch_comes_out_of_the_migration_saying_nobody_looked() {
    let database_path = TestDatabase::new("touch-migration");
    create_v15_fixture(database_path.path());

    let database = open_fixture_database(database_path.path());
    let connection = Connection::new(&database).expect("connection");
    let version = migration::apply(&connection).expect("migrate to the latest schema");
    assert_eq!(version, migration::LATEST_SCHEMA_VERSION);

    assert_eq!(
        text_of(
            &connection,
            "MATCH (touch:FileTouch {id: 'conversation:1:t0'}) RETURN touch.naming;"
        ),
        OwnerNaming::NotComputed.as_str(),
        "у строки, написанной до отметок, никто ничего не мерил"
    );
    assert_eq!(
        text_of(
            &connection,
            "MATCH (touch:FileTouch {id: 'conversation:1:t0'}) RETURN touch.times;"
        ),
        "0",
        "ноль — это «не считали», а не «один раз»"
    );
    // What the migration must not disturb: the touch itself.
    assert_eq!(
        text_of(
            &connection,
            "MATCH (touch:FileTouch {id: 'conversation:1:t0'}) RETURN touch.path;"
        ),
        "/work/draft_service.py"
    );
    assert_eq!(
        text_of(
            &connection,
            "MATCH (touch:FileTouch {id: 'conversation:1:t0'}) RETURN touch.verb;"
        ),
        TouchVerb::Read.as_str()
    );
}

/// The reason the third state exists, seen from the command: an old row is left out of a
/// strict answer, and the answer says how many were left out for want of a measurement rather
/// than counting them as files nobody asked about.
#[test]
fn an_unmarked_touch_is_counted_apart_when_the_answer_is_strict() {
    let database_path = TestDatabase::new("touch-migration-answer");
    create_v15_fixture(database_path.path());
    let manager = super::LadybugDatabaseManager::open(database_path.path().to_path_buf())
        .expect("open the migrated database");
    let repository = manager.repository();

    let touches = repository
        .touches_ending_with("draft_service.py")
        .expect("read the touches");
    assert_eq!(touches.len(), 1, "{touches:?}");

    let query = TouchQuery::new("draft_service.py", Direction::WhichConversations, false)
        .expect("непустой запрос");
    let report = TouchService::new(&repository)
        .execute(&query)
        .expect("ответ");
    assert_eq!(report.in_passing(), 1);
    assert_eq!(report.unmarked_in_passing(), 1);
    assert!(report.groups()[0].pairs().is_empty());
}
