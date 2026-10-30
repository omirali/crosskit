// The set-based batch against the real database: one batch must leave exactly the graph
// that writing the same sources one after another leaves, including the durable order of
// observations, the current state of a file observed twice, and what a replay may not
// touch.
use lbug::{Connection, Value};

use super::test_support::{
    TestDatabase, count, evidence_commit, evidence_commit_at, graph_snapshot, metadata_commit,
    path, query_int64, query_string,
};
use super::{LadybugDatabaseManager, migration};
use crate::application::{ReconcileRepository, RepositoryError, SourceBatchWriter};
use crate::domain::{
    AnalysisRequest, ChunkScript, ContentChunk, EvidenceCommit, FileKind, FileLocation,
    FileObservation, FileVersion, NormalizedPath, SourceCommit,
};

/// A batch that exercises every shape the projection has to fold: a metadata-only source,
/// sources with chunks, a hard link publishing one file under two paths, and one file
/// observed twice within the same batch.
fn mixed_batch() -> Vec<SourceCommit> {
    vec![
        metadata_commit("file:plain", "/data/docs/plain.bin"),
        evidence_commit_at(
            "file:notes",
            "/data/docs/notes.md",
            "hash:a",
            "observation:notes:1",
            100,
        ),
        // The same file under a second path: one revision, one chunk, two locations.
        evidence_commit_at(
            "file:notes",
            "/data/docs/hard-link.md",
            "hash:a",
            "observation:notes:2",
            150,
        ),
        evidence_commit_at(
            "file:other",
            "/data/docs/other.md",
            "hash:b",
            "observation:other:1",
            120,
        ),
        // The same file seen again inside one batch, with newer content.
        evidence_commit_at(
            "file:other",
            "/data/docs/other.md",
            "hash:c",
            "observation:other:2",
            220,
        ),
    ]
}

fn snapshot_of(build: impl Fn(&super::repository::LadybugGraphRepository)) -> Vec<String> {
    let database = TestDatabase::new("set-based-equivalence");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    build(&manager.repository());
    graph_snapshot(&manager)
}

#[test]
fn one_batch_leaves_exactly_what_the_same_sources_leave_one_by_one() {
    let batched = snapshot_of(|repository| {
        repository.commit_batch(mixed_batch()).unwrap();
    });
    let one_by_one = snapshot_of(|repository| {
        for commit in mixed_batch() {
            repository.commit_batch(vec![commit]).unwrap();
        }
    });

    assert_eq!(batched, one_by_one);
    // And the shape is the one the sources describe, not an empty graph that trivially
    // matches itself.
    assert!(
        batched
            .iter()
            .any(|row| row.starts_with("current_version|file:other|file:other@hash:c")),
        "{batched:#?}"
    );
    assert_eq!(
        batched
            .iter()
            .filter(|row| row.starts_with("location|"))
            .count(),
        4
    );
    assert_eq!(
        batched
            .iter()
            .filter(|row| row.starts_with("version|"))
            .count(),
        3
    );
}

#[test]
fn a_hard_link_keeps_both_locations_without_duplicating_the_revision() {
    let database = TestDatabase::new("set-based-hard-link");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    manager.repository().commit_batch(mixed_batch()).unwrap();

    assert_eq!(
        count(
            &manager,
            "MATCH (:File {id: 'file:notes'})-[:LOCATED_AT]->(:FileLocation) RETURN COUNT(*);"
        ),
        2,
        "both paths of the hard link keep their location"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileVersion {id: 'file:notes@hash:a'}) RETURN COUNT(*);"
        ),
        1
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (:FileVersion {id: 'file:notes@hash:a'})-[:HAS_CHUNK]->(:ContentChunk) RETURN COUNT(*);"
        ),
        1,
        "the shared revision keeps one chunk, not one per location"
    );
    // The later of the two sightings wins the current state.
    assert_eq!(
        query_string(
            &manager,
            "MATCH (:File {id: 'file:notes'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
            vec![],
        ),
        "observation:notes:2"
    );
}

#[test]
fn observation_sequences_are_contiguous_in_input_order_and_survive_reopen() {
    let database = TestDatabase::new("set-based-sequences");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    // Four observations share one timestamp, so only the allocated sequence orders them.
    let same_moment: Vec<SourceCommit> = (0..4)
        .map(|index| {
            evidence_commit_at(
                &format!("file:{index}"),
                &format!("/data/docs/{index}.md"),
                &format!("hash:{index}"),
                &format!("observation:{index}"),
                500,
            )
        })
        .collect();

    manager.repository().commit_batch(same_moment).unwrap();

    let sequences: Vec<i64> = (0..4)
        .map(|index| {
            query_int64(
                &manager,
                "MATCH (observation_order:ObservationOrder {id: $id}) RETURN observation_order.sequence;",
                vec![("id", Value::String(format!("observation:{index}")))],
            )
        })
        .collect();
    assert_eq!(
        sequences,
        [0, 1, 2, 3],
        "a batch hands out one contiguous range, strictly by input order"
    );
    drop(manager);

    let reopened = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    // A later batch continues from the stored maximum rather than restarting.
    reopened
        .repository()
        .commit_batch(vec![evidence_commit_at(
            "file:late",
            "/data/docs/late.md",
            "hash:late",
            "observation:late",
            600,
        )])
        .unwrap();
    assert_eq!(
        query_int64(
            &reopened,
            "MATCH (observation_order:ObservationOrder {id: 'observation:late'}) RETURN observation_order.sequence;",
            vec![],
        ),
        4
    );
}

#[test]
fn replaying_a_batch_keeps_every_sequence_and_adds_nothing() {
    let database = TestDatabase::new("set-based-replay");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository.commit_batch(mixed_batch()).unwrap();
    let before = graph_snapshot(&manager);

    for _ in 0..2 {
        repository.commit_batch(mixed_batch()).unwrap();
    }

    assert_eq!(
        graph_snapshot(&manager),
        before,
        "a replayed batch is a no-op, sequences included"
    );
}

#[test]
fn a_single_observation_can_be_retried_without_moving_its_sequence() {
    let database = TestDatabase::new("set-based-retry-observation");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository.commit_batch(mixed_batch()).unwrap();
    let sequence_before = query_int64(
        &manager,
        "MATCH (observation_order:ObservationOrder {id: 'observation:notes:1'}) RETURN observation_order.sequence;",
        vec![],
    );

    repository
        .commit_batch(vec![evidence_commit_at(
            "file:notes",
            "/data/docs/notes.md",
            "hash:a",
            "observation:notes:1",
            100,
        )])
        .unwrap();

    assert_eq!(
        query_int64(
            &manager,
            "MATCH (observation_order:ObservationOrder {id: 'observation:notes:1'}) RETURN observation_order.sequence;",
            vec![],
        ),
        sequence_before
    );
    // The retry is older than the current state, which therefore does not move back.
    assert_eq!(
        query_string(
            &manager,
            "MATCH (:File {id: 'file:notes'})-[:CURRENT_OBSERVATION]->(observation:FileObservation) RETURN observation.id;",
            vec![],
        ),
        "observation:notes:2"
    );
}

/// A source whose revision id is already stored for another file.
fn conflicting_version() -> SourceCommit {
    let version = FileVersion::new("file:notes@hash:a", "file:impostor", "hash:a", 100).unwrap();
    let observation =
        FileObservation::new("observation:impostor", "file:impostor", version.id(), 100).unwrap();
    SourceCommit::with_evidence(
        super::test_support::located("file:impostor", "/data/docs/impostor.md"),
        EvidenceCommit::new(version, observation, vec![]).unwrap(),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

/// A source whose chunk id is already stored with different text.
fn conflicting_chunk() -> SourceCommit {
    let version = FileVersion::new("file:notes@hash:z", "file:notes", "hash:z", 100).unwrap();
    let observation =
        FileObservation::new("observation:chunk-clash", "file:notes", version.id(), 100).unwrap();
    let chunk = ContentChunk::new(
        "chunk:file:notes@hash:a",
        version.id(),
        0,
        0,
        5,
        "text:other",
        "other",
        ChunkScript::Latin,
        "plain-text",
        "plain-text",
        "1",
        "config:1",
        version.content_hash(),
        100,
    )
    .unwrap();
    SourceCommit::with_evidence(
        super::test_support::located("file:notes", "/data/docs/clash.md"),
        EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

#[test]
fn a_conflict_anywhere_in_the_batch_rolls_back_every_row_of_it() {
    for conflicting in [conflicting_version(), conflicting_chunk()] {
        let database = TestDatabase::new("set-based-conflict");
        let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
        let repository = manager.repository();
        repository.commit_batch(mixed_batch()).unwrap();
        let before = graph_snapshot(&manager);

        let doomed = vec![
            evidence_commit("file:innocent", "/data/docs/innocent.md"),
            conflicting,
            evidence_commit("file:also-innocent", "/data/docs/also-innocent.md"),
        ];
        let error = repository.commit_batch(doomed).unwrap_err();

        assert!(
            matches!(error, RepositoryError::InvariantViolation(_)),
            "{error:?}"
        );
        assert_eq!(
            graph_snapshot(&manager),
            before,
            "the valid sources of a failed batch must be rolled back with it"
        );
    }
}

#[test]
fn a_claimed_insight_job_keeps_its_state_and_timestamp_through_a_batch() {
    let database = TestDatabase::new("set-based-claimed-job");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository.commit_batch(mixed_batch()).unwrap();
    let connection = Connection::new(&manager.owner.database).unwrap();
    connection
        .query("MATCH (job:InsightJob) SET job.state = 'claimed', job.revision_observed_at_unix_ms = 7;")
        .unwrap();
    let claimed = count(
        &manager,
        "MATCH (:InsightJob {state: 'claimed'}) RETURN COUNT(*);",
    );

    repository.commit_batch(mixed_batch()).unwrap();

    assert_eq!(
        count(
            &manager,
            "MATCH (:InsightJob {state: 'claimed'}) RETURN COUNT(*);"
        ),
        claimed,
        "a replayed batch must not reset the state of a job someone owns"
    );
    assert_eq!(
        count(
            &manager,
            "MATCH (job:InsightJob) WHERE job.revision_observed_at_unix_ms = 7 RETURN COUNT(*);"
        ),
        claimed
    );
}

#[test]
fn a_batch_leaves_no_revision_without_evidence_and_no_orphan_order() {
    let database = TestDatabase::new("set-based-integrity");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository.commit_batch(mixed_batch()).unwrap();
    let run = repository.begin_reconcile_run(&path("/data/docs")).unwrap();
    repository
        .commit_observed_batch(&run, mixed_batch())
        .unwrap();

    for (what, query) in [
        (
            "revision without chunks",
            "MATCH (version:FileVersion) WHERE NOT EXISTS { MATCH (version)-[:HAS_CHUNK]->(:ContentChunk) } RETURN COUNT(*);",
        ),
        (
            "revision without its outbox entry",
            "MATCH (version:FileVersion) WHERE NOT EXISTS { MATCH (version)-[:REQUESTS_INSIGHT]->(:InsightJob) } RETURN COUNT(*);",
        ),
        (
            "observation without its file",
            "MATCH (observation:FileObservation) WHERE NOT EXISTS { MATCH (:File)-[:HAS_OBSERVATION]->(observation) } RETURN COUNT(*);",
        ),
        (
            "observation without its revision",
            "MATCH (observation:FileObservation) WHERE NOT EXISTS { MATCH (observation)-[:OBSERVES]->(:FileVersion) } RETURN COUNT(*);",
        ),
        (
            "order without its observation",
            "MATCH (observation_order:ObservationOrder) WHERE NOT EXISTS { MATCH (observation_order)-[:ORDERS]->(:FileObservation) } RETURN COUNT(*);",
        ),
        (
            "chunk without its extractor",
            "MATCH (chunk:ContentChunk) WHERE NOT EXISTS { MATCH (chunk)-[:GENERATED_BY]->(:Extractor) } RETURN COUNT(*);",
        ),
    ] {
        assert_eq!(count(&manager, query), 0, "{what}");
    }
    // Both current pointers agree: the observation they name observes the named version.
    assert_eq!(
        count(
            &manager,
            "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion) RETURN COUNT(*);"
        ),
        count(
            &manager,
            "MATCH (file:File)-[:CURRENT_VERSION]->(version:FileVersion)<-[:OBSERVES]-(observation:FileObservation)<-[:CURRENT_OBSERVATION]-(file) RETURN COUNT(*);"
        )
    );
}

#[test]
fn two_locations_at_one_path_fall_back_to_writing_the_sources_one_by_one() {
    let database = TestDatabase::new("set-based-same-path");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    // Two different files claiming the same path: each retires the other, so only their
    // order decides the winner.
    let batch = vec![
        evidence_commit_at(
            "file:first",
            "/data/docs/same.md",
            "hash:1",
            "observation:1",
            100,
        ),
        evidence_commit_at(
            "file:second",
            "/data/docs/same.md",
            "hash:2",
            "observation:2",
            200,
        ),
    ];

    manager.repository().commit_batch(batch).unwrap();

    assert_eq!(
        query_string(
            &manager,
            "MATCH (location:FileLocation {file_id: 'file:second'}) RETURN location.status;",
            vec![],
        ),
        "present",
        "the last source at the path wins, as it would one by one"
    );
    assert_eq!(
        query_string(
            &manager,
            "MATCH (location:FileLocation {file_id: 'file:first'}) RETURN location.status;",
            vec![],
        ),
        "missing"
    );
}

/// One source at `path`, seen at `observed_at`, with content `content_hash`.
fn sighting(file_id: &str, path_text: &str, content_hash: &str, observed_at: i64) -> SourceCommit {
    let location = FileLocation::observed(
        file_id,
        NormalizedPath::from_absolute(path_text).unwrap(),
        FileKind::RegularFile,
        5,
        100,
        observed_at,
    )
    .unwrap();
    let version = FileVersion::new(
        format!("{file_id}@{content_hash}"),
        file_id,
        content_hash,
        observed_at,
    )
    .unwrap();
    let observation = FileObservation::new(
        format!("observation:{file_id}:{observed_at}"),
        file_id,
        version.id(),
        observed_at,
    )
    .unwrap();
    SourceCommit::with_evidence(
        location,
        EvidenceCommit::new(version, observation, vec![]).unwrap(),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

#[test]
fn the_same_location_twice_in_one_batch_keeps_the_first_sighting_time() {
    // The location does not exist yet, so whichever row is written first decides
    // `first_seen_at_unix_ms`, and only the first sighting may.
    let batch = vec![
        sighting("file:twice", "/data/docs/twice.md", "hash:a", 100),
        sighting("file:twice", "/data/docs/twice.md", "hash:b", 200),
    ];

    let batched = snapshot_of(|repository| {
        repository.commit_batch(batch.clone()).unwrap();
    });
    let one_by_one = snapshot_of(|repository| {
        for commit in batch.clone() {
            repository.commit_batch(vec![commit]).unwrap();
        }
    });

    assert_eq!(batched, one_by_one);
    let location = batched
        .iter()
        .find(|row| row.starts_with("location|"))
        .expect("the location must be stored");
    // location|<id>|<path>|<status>|<size>|<first_seen>|<last_seen>|<epoch>
    let fields: Vec<&str> = location.split('|').collect();
    assert_eq!(
        fields[5], "100",
        "the first sighting owns first_seen: {location}"
    );
    assert_eq!(
        fields[6], "200",
        "the later sighting refreshes last_seen: {location}"
    );
}

#[test]
fn a_location_conflict_names_the_file_identity_and_never_its_path() {
    const SENTINEL: &str = "/data/docs/very-secret-name.md";
    let database = TestDatabase::new("set-based-location-conflict");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_batch(vec![metadata_commit("file:sentinel", SENTINEL)])
        .unwrap();

    // The same identity claiming another kind: an immutable column would have to change.
    let conflicting = SourceCommit::metadata_only(
        FileLocation::observed(
            "file:sentinel",
            NormalizedPath::from_absolute(SENTINEL).unwrap(),
            FileKind::Symlink,
            5,
            100,
            1_000,
        )
        .unwrap(),
    );
    let error = repository.commit_batch(vec![conflicting]).unwrap_err();

    assert!(
        matches!(error, RepositoryError::InvariantViolation(_)),
        "{error:?}"
    );
    let rendered = error.to_string();
    assert!(rendered.contains("file:sentinel"), "{rendered}");
    assert!(
        !rendered.contains("very-secret-name") && !rendered.contains('/'),
        "a location error must not carry a path: {rendered}"
    );
}

/// Every link an observation must have, by kind.
fn link_counts(manager: &LadybugDatabaseManager) -> [i64; 3] {
    [
        count(
            manager,
            "MATCH (:File)-[:HAS_OBSERVATION]->(:FileObservation) RETURN COUNT(*);",
        ),
        count(
            manager,
            "MATCH (:FileObservation)-[:OBSERVES]->(:FileVersion) RETURN COUNT(*);",
        ),
        count(
            manager,
            "MATCH (:ObservationOrder)-[:ORDERS]->(:FileObservation) RETURN COUNT(*);",
        ),
    ]
}

#[test]
fn a_batch_of_new_observations_creates_exactly_one_link_of_each_kind() {
    let database = TestDatabase::new("set-based-new-links");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let batch: Vec<SourceCommit> = (0..5)
        .map(|index| {
            evidence_commit_at(
                &format!("file:{index}"),
                &format!("/data/docs/{index}.md"),
                &format!("hash:{index}"),
                &format!("observation:{index}"),
                100 + index,
            )
        })
        .collect();

    manager.repository().commit_batch(batch).unwrap();

    assert_eq!(link_counts(&manager), [5, 5, 5]);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        5
    );
}

#[test]
fn a_batch_mixing_new_observations_with_retries_links_only_what_is_new() {
    let database = TestDatabase::new("set-based-mixed-links");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let first = evidence_commit_at("file:a", "/data/docs/a.md", "hash:a", "observation:a", 100);
    repository.commit_batch(vec![first.clone()]).unwrap();
    let before = graph_snapshot(&manager);

    // The same source again, together with two the database has never seen.
    repository
        .commit_batch(vec![
            first,
            evidence_commit_at("file:b", "/data/docs/b.md", "hash:b", "observation:b", 200),
            evidence_commit_at("file:c", "/data/docs/c.md", "hash:c", "observation:c", 300),
        ])
        .unwrap();

    assert_eq!(link_counts(&manager), [3, 3, 3]);
    assert_eq!(
        count(&manager, "MATCH (:FileObservation) RETURN COUNT(*);"),
        3
    );
    // The retried row kept everything it had, including its durable sequence.
    let kept: Vec<&String> = before
        .iter()
        .filter(|row| row.contains("observation:a"))
        .collect();
    let now = graph_snapshot(&manager);
    for row in kept {
        assert!(now.contains(row), "the retried row lost {row}");
    }
}

/// The contract for a stored observation whose link was removed from outside this program:
/// a retry repairs it, which is what this store has always done.
#[test]
fn a_retry_repairs_a_link_that_was_removed_from_outside() {
    for (kind, delete) in [
        (
            "HAS_OBSERVATION",
            "MATCH (:File)-[link:HAS_OBSERVATION]->(:FileObservation) DELETE link;",
        ),
        (
            "OBSERVES",
            "MATCH (:FileObservation)-[link:OBSERVES]->(:FileVersion) DELETE link;",
        ),
        (
            "ORDERS",
            "MATCH (:ObservationOrder)-[link:ORDERS]->(:FileObservation) DELETE link;",
        ),
    ] {
        let database = TestDatabase::new("set-based-repair");
        let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
        let repository = manager.repository();
        let commit =
            evidence_commit_at("file:a", "/data/docs/a.md", "hash:a", "observation:a", 100);
        repository.commit_batch(vec![commit.clone()]).unwrap();
        let whole = graph_snapshot(&manager);

        let connection = Connection::new(&manager.owner.database).unwrap();
        connection.query(delete).unwrap();
        assert_ne!(link_counts(&manager), [1, 1, 1], "{kind} must be gone");

        repository.commit_batch(vec![commit]).unwrap();

        assert_eq!(link_counts(&manager), [1, 1, 1], "{kind} must be repaired");
        assert_eq!(
            graph_snapshot(&manager),
            whole,
            "{kind}: repairing must restore exactly what was there"
        );
    }
}

#[test]
fn a_failure_after_the_links_rolls_back_the_nodes_orders_links_state_and_job() {
    let database = TestDatabase::new("set-based-link-rollback");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    manager
        .repository()
        .commit_batch(vec![evidence_commit("file:kept", "/data/docs/kept.md")])
        .unwrap();
    let before = graph_snapshot(&manager);

    let doomed: Vec<SourceCommit> = (0..3)
        .map(|index| {
            evidence_commit_at(
                &format!("file:new{index}"),
                &format!("/data/docs/new{index}.md"),
                &format!("hash:new{index}"),
                &format!("observation:new{index}"),
                500 + index,
            )
        })
        .collect();
    let connection = Connection::new(&manager.owner.database).unwrap();
    let error = migration::in_transaction(&connection, |connection| {
        super::repository::persist_batch(connection, &doomed)?;
        Err::<(), _>(RepositoryError::Storage(
            "injected failure after the batch was written".to_string(),
        ))
    })
    .unwrap_err();

    assert!(matches!(error, RepositoryError::Storage(_)), "{error:?}");
    assert_eq!(
        graph_snapshot(&manager),
        before,
        "nothing the failed batch wrote may survive"
    );
    assert_eq!(link_counts(&manager), [1, 1, 1]);
}
