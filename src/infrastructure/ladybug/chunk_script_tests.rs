// What v12 does to a database written before it. The old column said `en` when a file was
// ASCII and `und` when it was not, decided once for the whole file; the new one says which
// scripts each chunk's own text actually holds. The values are recomputed from the text the
// graph already stores — nothing is read from disk — so a database that migrates and a
// database indexed today have to agree, chunk for chunk.
use lbug::{Connection, Value};

use super::test_support::{TestDatabase, count, open_fixture_database, query_string};
use super::{LadybugDatabaseManager, migration};

/// Three chunks whose stored `language` was wrong in all three ways the old rule could be
/// wrong: Russian prose called undetermined, English text called English only because it
/// happened to be ASCII, and a mixed chunk called undetermined for its Russian half while
/// its English half was invisible.
const LEGACY_CHUNKS: &[(&str, &str, &str, &str)] = &[
    (
        "chunk:russian",
        "Сверщик обходит индекс и сообщает об удалённых локациях.",
        "und",
        "cyrillic",
    ),
    (
        "chunk:english",
        "The reconciler walks the index and reports deleted locations.",
        "en",
        "latin",
    ),
    (
        "chunk:mixed",
        "the walker обходит tree и пишет chunk в базу",
        "und",
        "latin+cyrillic",
    ),
    ("chunk:digits", "1234 5678 90", "en", "neither"),
];

/// A database at v11 holding chunks with the field as it was then.
fn create_v11_fixture(path: &std::path::Path) {
    let database = open_fixture_database(path);
    let connection = Connection::new(&database).unwrap();
    migration::apply_through(&connection, 11).unwrap();
    for (id, text, legacy_language, _) in LEGACY_CHUNKS {
        let text = text.replace('\'', "''");
        connection
            .query(&format!(
                "CREATE (:ContentChunk {{id: '{id}', source_revision_id: 'file:a@hash:a', ordinal: 0, start_byte: 0, end_byte: {}, text_hash: 'hash:{id}', text: '{text}', language: '{legacy_language}', extraction_method: 'plain-text', extractor_id: 'plain-text', extractor_version: '1', config_hash: 'config:1', input_content_hash: 'hash:a', created_at_unix_ms: 100}});",
                text.len()
            ))
            .unwrap();
    }
}

fn script_of(manager: &LadybugDatabaseManager, id: &str) -> String {
    query_string(
        manager,
        "MATCH (chunk:ContentChunk {id: $id}) RETURN chunk.script;",
        vec![("id", Value::String(id.to_string()))],
    )
}

#[test]
fn a_v11_database_gains_a_script_recomputed_from_the_text_it_already_holds() {
    let database = TestDatabase::new("v11-chunk-script");
    create_v11_fixture(database.path());

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    for (id, _, legacy_language, expected) in LEGACY_CHUNKS {
        assert_eq!(
            script_of(&manager, id),
            *expected,
            "{id} was stored as {legacy_language:?} and must now say what its text holds"
        );
    }
}

/// The old name is gone, not merely unused: a column called `language` sitting next to one
/// called `script` would be the same claim again, one release later.
#[test]
fn no_column_still_claims_to_hold_a_language() {
    let database = TestDatabase::new("v11-chunk-script-columns");
    create_v11_fixture(database.path());
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();

    let columns: Vec<String> = connection
        .query("CALL TABLE_INFO('ContentChunk') RETURN name;")
        .unwrap()
        .filter_map(|row| match row.as_slice() {
            [Value::String(name)] => Some(name.clone()),
            _ => None,
        })
        .collect();

    assert!(
        columns.iter().any(|name| name == "script"),
        "the table must carry the honest column, found {columns:?}"
    );
    assert!(
        !columns.iter().any(|name| name == "language"),
        "the column that claimed a language must be gone, found {columns:?}"
    );
}

/// The migration must not depend on how many chunks there are: the backfill reads keyset
/// pages, and a graph larger than one page has to come out the same as a graph smaller than
/// one. This writes more chunks than `CHUNK_SCRIPT_BACKFILL_PAGE_SIZE` allows in a page.
#[test]
fn the_backfill_crosses_its_page_boundary_without_missing_a_chunk() {
    let database = TestDatabase::new("v11-chunk-script-paged");
    {
        let fixture = open_fixture_database(database.path());
        let connection = Connection::new(&fixture).unwrap();
        migration::apply_through(&connection, 11).unwrap();
        for ordinal in 0..(migration::CHUNK_SCRIPT_BACKFILL_PAGE_SIZE * 2 + 7) {
            let text = if ordinal % 2 == 0 {
                "страница"
            } else {
                "page"
            };
            connection
                .query(&format!(
                    "CREATE (:ContentChunk {{id: 'chunk:{ordinal:05}', source_revision_id: 'file:a@hash:a', ordinal: 0, start_byte: 0, end_byte: 8, text_hash: 'hash:{ordinal}', text: '{text}', language: 'und', extraction_method: 'plain-text', extractor_id: 'plain-text', extractor_version: '1', config_hash: 'config:1', input_content_hash: 'hash:a', created_at_unix_ms: 100}});"
                ))
                .unwrap();
        }
    }

    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    assert_eq!(
        count(
            &manager,
            "MATCH (chunk:ContentChunk) WHERE chunk.script = '' RETURN COUNT(chunk);"
        ),
        0,
        "every chunk must have been recomputed, on every page"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (chunk:ContentChunk) WHERE chunk.script = 'cyrillic' RETURN COUNT(chunk);"
        ),
        (migration::CHUNK_SCRIPT_BACKFILL_PAGE_SIZE * 2 + 7).div_ceil(2) as i64
    );
}
