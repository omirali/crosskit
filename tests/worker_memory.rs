// Stage 17: does the memory of one worker process grow with the number of jobs it works?
//
// `index jobs run --limit 20000` over a queue of ten thousand jobs was killed by the kernel
// under a 1 GiB container in Stage 16, at about seven thousand jobs. This benchmark takes
// the curve that says what grows: four points, each a fresh graph and its own process, with
// the peak resident set size of that process, what Crosskit's own code allocated in it, and
// what the run left on disk.
//
// Ignored by default; run explicitly with
//
//   cargo test --release --offline --test worker_memory -- --ignored --nocapture
//
// Add `--features allocation-profile` to the same command to get Crosskit's own allocation
// peak beside the resident one. That pair is the whole point: allocations that stay flat
// while resident memory climbs say the growth is not ours.
//
// Under a container memory limit the run that is killed is a result, not a failure: the
// report says which signal ended it and how far it got. The benchmark asserts only what it
// can prove on any machine — that every point that finished worked every job it was given.
// The shape of the curve belongs to the report, which a person reads.
#[cfg(unix)]
#[path = "scale_indexing/cli.rs"]
mod cli;
#[cfg(unix)]
#[path = "memory_diagnostic/measure.rs"]
mod measure;
#[cfg(unix)]
mod worker_memory {
    pub mod curve;
    pub mod transactions;
}

#[cfg(unix)]
#[test]
#[ignore = "worker memory curve: cargo test --release --offline --test worker_memory -- --ignored --nocapture"]
fn worker_memory_does_not_grow_with_the_number_of_jobs() {
    worker_memory::curve::run();
}

#[cfg(unix)]
#[test]
#[ignore = "what a transaction costs: cargo test --release --offline --test worker_memory -- --ignored --nocapture --exact what_one_transaction_costs_is_attributed"]
fn what_one_transaction_costs_is_attributed() {
    worker_memory::curve::attribute();
}

/// The worker half of the attribution: the benchmark runs this same binary again, once per
/// shape, so each shape gets a process and a resident peak of its own.
#[cfg(unix)]
#[test]
#[ignore = "internal worker of the worker memory curve; the benchmark runs it"]
fn worker_memory_transaction_worker() {
    let Ok(database) = std::env::var("CROSSKIT_TRANSACTION_DB") else {
        // `--ignored` also selects this internal test at the top level. Only a child the
        // benchmark configured is a worker.
        return;
    };
    let shape = std::env::var(worker_memory::transactions::SHAPE_VARIABLE)
        .expect("a transaction worker is told which shape to run");
    worker_memory::transactions::run_worker(std::path::Path::new(&database), &shape);
}

#[cfg(not(unix))]
#[test]
#[ignore = "worker memory curve: Unix only"]
fn worker_memory_does_not_grow_with_the_number_of_jobs() {
    panic!("the worker memory curve drives the descriptor-relative walker and needs Unix");
}
