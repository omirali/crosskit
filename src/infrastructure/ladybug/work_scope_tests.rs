// The work one directory selects, against the real database: the boundary of a directory and
// the order a scoped claim keeps.
use super::LadybugDatabaseManager;
use super::LadybugRevisionInput;
use super::test_support::{
    TestDatabase, evidence_commit_saying, evidence_commit_without_range, path,
};
use crate::application::{
    InsightJobQueue, JobScope, KnowledgeGraphRepository, WorkScope, WorkScopeService,
};
use crate::application::{RevisionInputReader, WorkScopeReader};
use crate::domain::{InsightJob, InsightLease};

#[test]
fn a_directory_selects_what_lies_under_it_and_not_its_namesakes() {
    let database = TestDatabase::new("scope-boundary");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for (file, at, place) in [
        ("file:old", 100, "/work/reg/old.md"),
        ("file:new", 300, "/work/reg/deep/new.md"),
        ("file:namesake", 200, "/work/registry/other.md"),
    ] {
        repository
            .commit_source(evidence_commit_saying(file, place, "hash:a", at, "текст"))
            .unwrap();
    }

    let selected = WorkScopeService::new(&repository)
        .select(&WorkScope::Under(path("/work/reg")))
        .unwrap();

    assert_eq!(selected.revisions(), 2);
    assert_eq!(selected.queued(), 2);
    let claimed = repository
        .claim_insight_jobs(
            10,
            InsightLease::of_milliseconds(60_000),
            JobScope::Among(selected.job_ids()),
        )
        .unwrap();
    let order: Vec<&str> = claimed.iter().map(|job| job.source_revision_id()).collect();
    assert_eq!(
        order,
        ["file:old@hash:a", "file:new@hash:a"],
        "oldest revision first"
    );
    assert_eq!(
        claimed[0].id(),
        InsightJob::id_of_revision("file:old@hash:a")
    );
    // The namesake is still waiting for a run that asks for it.
    assert_eq!(
        repository
            .claim_insight_jobs(
                10,
                InsightLease::of_milliseconds(60_000),
                JobScope::Everything
            )
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn the_whole_queue_but_the_held_jobs_is_claimed_in_its_order() {
    let database = TestDatabase::new("scope-except");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    for (file, at) in [("file:a", 100), ("file:b", 200), ("file:c", 300)] {
        repository
            .commit_source(evidence_commit_saying(
                file,
                &format!("/w/{file}.md"),
                "h",
                at,
                "т",
            ))
            .unwrap();
    }
    let held = vec![InsightJob::id_of_revision("file:b@h")];

    let claimed = repository
        .claim_insight_jobs(
            10,
            InsightLease::of_milliseconds(60_000),
            JobScope::Except(&held),
        )
        .unwrap();

    let order: Vec<&str> = claimed.iter().map(|job| job.source_revision_id()).collect();
    assert_eq!(order, ["file:a@h", "file:c@h"]);
}

/// A reply has no byte range, by design; it weighs its text — for the per-job budget and for a
/// price alike, and in bytes, not characters.
#[test]
fn a_chunk_without_a_range_weighs_the_bytes_of_its_text() {
    let database = TestDatabase::new("scope-weight");
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    repository
        .commit_source(evidence_commit_without_range(
            "file:chat",
            "/w/chat.jsonl",
            "h",
            "привет",
        ))
        .unwrap();
    let revision = "file:chat@h".to_string();

    let sizes = repository
        .revision_sizes(std::slice::from_ref(&revision))
        .unwrap();
    let job = repository
        .claim_insight_jobs(
            1,
            InsightLease::of_milliseconds(60_000),
            JobScope::Everything,
        )
        .unwrap()
        .remove(0);
    let summary = LadybugRevisionInput::new(std::sync::Arc::clone(&manager))
        .summarize(&job)
        .unwrap();

    assert_eq!(sizes[0].bytes, 12);
    assert_eq!(summary.text_bytes(), 12);
}
