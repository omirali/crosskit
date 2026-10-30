// What the graph answers about a file identity, and what happens when a filesystem hands a
// freed inode number to a new file. The kernel's pair is not an identity over time; these
// tests hold the line that a new file never inherits the history of a deleted one.
use lbug::Value;

use super::query_plan::{primary_key_scans, relationship_scans, table_scans};
use super::test_support::{
    TestDatabase, count, create_v6_fixture, create_v7_fixture, evidence_commit,
    evidence_commit_revision, query_int64, query_string,
};
use super::{LadybugDatabaseManager, location_store};
use crate::application::{FileIdentityHistory, SourceBatchWriter};
use crate::domain::{KnownIdentity, ObservedFileIdentity};

/// The identity of the file these tests index first, and the moment `located` records as
/// its first sighting.
const DEVICE: u64 = 1;
const INODE: u64 = 2;
const FIRST_SEEN_AT_UNIX_MS: i64 = 1_000;

fn base_id() -> String {
    ObservedFileIdentity::new(DEVICE, INODE, None).base_id()
}

fn born(milliseconds: i64) -> ObservedFileIdentity {
    ObservedFileIdentity::new(DEVICE, INODE, Some(milliseconds * 1_000_000))
}

#[test]
fn an_identity_the_graph_has_never_seen_is_reported_as_unseen() {
    let database = TestDatabase::new("identity-unseen");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    assert_eq!(
        repository.known_identity(&base_id()).unwrap(),
        KnownIdentity::Unseen
    );
}

#[test]
fn an_indexed_file_dates_its_identity_by_its_earliest_sighting() {
    let database = TestDatabase::new("identity-dated");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![evidence_commit(&base_id(), "/data/a.md")])
        .unwrap();

    assert_eq!(
        repository.known_identity(&base_id()).unwrap(),
        KnownIdentity::FirstSeenAt(FIRST_SEEN_AT_UNIX_MS)
    );
}

/// A database written before locations existed carries files nothing can date.
#[test]
fn a_file_no_location_dates_is_reported_as_undated() {
    let database = TestDatabase::new("identity-undated");
    create_v7_fixture(database.path());
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    assert_eq!(
        repository.known_identity("file:1").unwrap(),
        KnownIdentity::Undated
    );
    assert_eq!(
        born(2_000).resolve(repository.known_identity("file:1").unwrap()),
        born(2_000).resolve(KnownIdentity::Undated),
        "an undated file is never inherited, whatever the birth time"
    );
}

/// Upgrading must not turn every file the graph already holds into an undated one: the
/// dates are computed from the locations already stored, so an existing file keeps the
/// identity it had and only a genuinely new file branches.
#[test]
fn upgrading_dates_the_identities_the_graph_already_held() {
    let database = TestDatabase::new("identity-backfill");
    create_v6_fixture(database.path());
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();

    assert_eq!(
        repository.known_identity("file:1").unwrap(),
        KnownIdentity::FirstSeenAt(1_000),
        "the fixture's only location was first seen at 1000"
    );
}

/// The whole point: the same device and inode pair, describing a file created after the
/// graph first saw that pair, is published as a different file. The revisions of the
/// deleted file stay with it, and nothing of the new one reaches it.
#[test]
fn a_recycled_inode_number_does_not_inherit_the_history_of_the_deleted_file() {
    let database = TestDatabase::new("identity-recycled");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![evidence_commit(&base_id(), "/data/a.md")])
        .unwrap();

    // The file at /data/a.md is deleted and the next file created takes its inode number.
    let recycled = born(FIRST_SEEN_AT_UNIX_MS + 1_000);
    let resolved = recycled.resolve(repository.known_identity(&base_id()).unwrap());
    assert_ne!(resolved, base_id(), "the new file must not be the old one");

    repository
        .commit_batch(vec![evidence_commit_revision(
            &resolved,
            "/data/b.md",
            "hash:b",
            "observation:b",
            2_000,
        )])
        .unwrap();

    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 2);
    assert_eq!(versions_of(&manager, &base_id()), 1);
    assert_eq!(versions_of(&manager, &resolved), 1);
    assert_eq!(locations_of(&manager, &base_id()), 1);
    assert_eq!(locations_of(&manager, &resolved), 1);
    assert_eq!(
        current_version_of(&manager, &base_id()),
        format!("{}@hash:a", base_id()),
        "the deleted file keeps the revision it was last seen with"
    );
    assert_eq!(
        current_version_of(&manager, &resolved),
        format!("{resolved}@hash:b")
    );
}

/// The same file seen again, and the same file at a second path: one identity, one file.
/// A rename and a hard link both look like this, and neither may branch.
#[test]
fn the_same_file_at_another_path_stays_one_file() {
    let database = TestDatabase::new("identity-same-file");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![evidence_commit(&base_id(), "/data/a.md")])
        .unwrap();

    // Born before the graph first saw the identity: the file the graph already knows.
    let same = born(FIRST_SEEN_AT_UNIX_MS - 500);
    let resolved = same.resolve(repository.known_identity(&base_id()).unwrap());
    assert_eq!(resolved, base_id());

    repository
        .commit_batch(vec![evidence_commit_revision(
            &resolved,
            "/data/link.md",
            "hash:a",
            "observation:link",
            1_500,
        )])
        .unwrap();

    assert_eq!(count(&manager, "MATCH (:File) RETURN COUNT(*);"), 1);
    assert_eq!(locations_of(&manager, &base_id()), 2);
    assert_eq!(versions_of(&manager, &base_id()), 1);
}

/// The statement the scan runs for every file it meets: it must reach the file by its
/// primary key and scan no table, or a scan would be paid once per source.
#[test]
fn the_identity_lookup_reaches_the_file_by_its_primary_key() {
    let database = TestDatabase::new("identity-plan");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![evidence_commit(&base_id(), "/data/a.md")])
        .unwrap();

    let plan = query_string(
        &manager,
        &format!("EXPLAIN {}", location_store::KNOWN_IDENTITY),
        vec![("id", Value::String(base_id()))],
    );
    assert_eq!(primary_key_scans(&plan), 1, "{plan}");
    assert_eq!(table_scans(&plan), 0, "{plan}");
    assert_eq!(relationship_scans(&plan), 0, "{plan}");
}

fn versions_of(manager: &LadybugDatabaseManager, file_id: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (:File {id: $file_id})-[:HAS_VERSION]->(version:FileVersion) RETURN COUNT(version);",
        vec![("file_id", Value::String(file_id.to_string()))],
    )
}

fn locations_of(manager: &LadybugDatabaseManager, file_id: &str) -> i64 {
    query_int64(
        manager,
        "MATCH (:File {id: $file_id})-[:LOCATED_AT]->(location:FileLocation) RETURN COUNT(location);",
        vec![("file_id", Value::String(file_id.to_string()))],
    )
}

fn current_version_of(manager: &LadybugDatabaseManager, file_id: &str) -> String {
    query_string(
        manager,
        "MATCH (:File {id: $file_id})-[:CURRENT_VERSION]->(version:FileVersion) RETURN version.id;",
        vec![("file_id", Value::String(file_id.to_string()))],
    )
}

/// Where the price of the identity rule actually is. The scan asks this question once per
/// source, so the obvious suspicion is the connection it opens each time; this measures the
/// three parts separately instead of assuming. Ignored by default: it runs the lookup tens
/// of thousands of times and reports numbers a person reads.
///
///   cargo test --release --offline the_cost_of_an_identity_lookup -- --ignored --nocapture
#[test]
#[ignore = "diagnostic: cargo test --release --offline the_cost_of_an_identity_lookup -- --ignored --nocapture"]
fn the_cost_of_an_identity_lookup_is_the_query_not_the_connection() {
    use std::time::{Duration, Instant};

    const ROUNDS: usize = 10_000;
    let database = TestDatabase::new("identity-cost");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![evidence_commit(&base_id(), "/data/a.md")])
        .unwrap();
    let milliseconds_each = |elapsed: Duration| elapsed.as_secs_f64() * 1_000.0 / ROUNDS as f64;
    let timed = |work: &mut dyn FnMut()| {
        let started = Instant::now();
        for _ in 0..ROUNDS {
            work();
        }
        milliseconds_each(started.elapsed())
    };

    let whole = timed(&mut || {
        let connection = lbug::Connection::new(&manager.owner.database).unwrap();
        location_store::known_identity(&connection, &base_id()).unwrap();
    });
    let opening = timed(&mut || {
        lbug::Connection::new(&manager.owner.database).unwrap();
    });
    let connection = lbug::Connection::new(&manager.owner.database).unwrap();
    let shared = timed(&mut || {
        location_store::known_identity(&connection, &base_id()).unwrap();
    });
    let preparing = timed(&mut || {
        connection.prepare(location_store::KNOWN_IDENTITY).unwrap();
    });
    let mut statement = connection.prepare(location_store::KNOWN_IDENTITY).unwrap();
    let executing = timed(&mut || {
        let mut result = connection
            .execute(&mut statement, vec![("id", Value::String(base_id()))])
            .unwrap();
        result.next();
    });

    println!(
        "one identity lookup, {ROUNDS} rounds each, milliseconds:\n  whole lookup, a connection of its own: {whole:.3}\n  whole lookup, one shared connection:   {shared:.3}\n  opening a connection alone:            {opening:.3}\n  preparing the statement alone:         {preparing:.3}\n  executing a prepared statement alone:  {executing:.3}"
    );
    assert!(
        opening < whole / 10.0,
        "opening a connection is a rounding error next to the query it carries: {opening:.3} of {whole:.3} ms"
    );
}
