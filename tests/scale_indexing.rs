// Scale benchmark and recovery proof for the indexing pipeline: walker → SourceCommit →
// one transaction per source → evidence, current state and InsightJob. Ignored by default;
// run explicitly with
//
//   cargo test --release --offline --test scale_indexing -- --ignored --nocapture
//
// The corpus is synthetic and deterministic, the database lives in its own
// CROSSKIT_DATA_DIR, nothing outside the benchmark directory is touched, and the
// directory is kept afterwards for inspection. Timings are printed as a baseline, never
// asserted: they depend on the machine. Correctness invariants are asserted.
//
// The benchmark is Unix-only: it exercises the descriptor-relative walker and makes a
// directory unreadable to interrupt a pass. On another platform the whole benchmark is
// compiled out, so an ordinary `cargo test` still builds, and running it explicitly fails
// with a clear reason instead of pretending to measure anything.
#[cfg(unix)]
mod scale_indexing {
    pub mod benchmark;
    pub mod cli;
    pub mod corpus;
    #[path = "../memory_diagnostic/measure.rs"]
    pub mod measure;
    pub mod search;
}

#[cfg(unix)]
#[test]
#[ignore = "scale benchmark: cargo test --release --offline --test scale_indexing -- --ignored --nocapture"]
fn indexing_stays_correct_idempotent_and_recoverable_at_scale() {
    scale_indexing::benchmark::run();
}

#[cfg(not(unix))]
#[test]
#[ignore = "scale benchmark: Unix only"]
fn indexing_stays_correct_idempotent_and_recoverable_at_scale() {
    panic!(
        "scale benchmark requires the Unix descriptor-relative walker; it is not available on this platform"
    );
}
