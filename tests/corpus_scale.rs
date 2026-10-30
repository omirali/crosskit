// Stage 18: how large a tree does Crosskit index, and what does the memory of a pass grow
// with?
//
// Every memory number the product stands on was taken on one corpus of 10 407 files. Stage
// 17 found in passing that a corpus of 40 000 does not index under 1 GiB and named it a
// property of reconciliation rather than of the worker. This benchmark takes the curve that
// says where the limit actually is: four corpus sizes, each generated fresh, each indexed
// into a graph of its own, with cold `index init` and an idle `index reconcile` measured in
// processes of their own — peak resident set size, wall clock, what Crosskit's own code
// allocated, the database on disk and the search index on disk.
//
//   cargo test --release --offline --test corpus_scale -- --ignored --nocapture
//
// Add `--features allocation-profile` to the same command to get Crosskit's own allocation
// peak beside the resident one. That pair is the attribution Stage 17 used: allocations
// that stay flat while resident memory climbs say the growth is not ours.
//
// Under a container memory limit a point the kernel kills is a result, not a failure: the
// report says which signal ended it and how far it got, and the curve stops there rather
// than measuring recovery on the next point. The benchmark asserts only what it can prove
// on any machine — that every pass which finished indexed every file it was given, and that
// an idle reconcile after it changed nothing. The shape of the curve belongs to the report,
// which a person reads.
//
// The layer attribution is a separate test, because it answers a different question on one
// corpus size rather than four.
#[cfg(unix)]
#[path = "scale_indexing/cli.rs"]
mod cli;
#[cfg(unix)]
#[path = "scale_indexing/corpus.rs"]
mod corpus;
#[cfg(unix)]
#[path = "memory_diagnostic/measure.rs"]
mod measure;
#[cfg(unix)]
mod corpus_scale {
    pub mod curve;
    pub mod parts;
    pub mod report;
}

#[cfg(unix)]
#[test]
#[ignore = "corpus size curve: cargo test --release --offline --test corpus_scale -- --ignored --nocapture --exact indexing_a_tree_of_every_size_stays_within_the_memory_of_the_machine"]
fn indexing_a_tree_of_every_size_stays_within_the_memory_of_the_machine() {
    corpus_scale::curve::run();
}

#[cfg(unix)]
#[test]
#[ignore = "where a pass spends its memory: cargo test --release --offline --test corpus_scale -- --ignored --nocapture --exact where_the_memory_of_a_pass_belongs"]
fn where_the_memory_of_a_pass_belongs() {
    corpus_scale::curve::attribute();
}

/// The other half of the attribution, and the one that decides what a fix could even be:
/// equal passes into one growing graph. Stage 17 asked the same question of the worker by
/// splitting one queue over ten processes.
#[cfg(unix)]
#[test]
#[ignore = "the tree or the graph: cargo test --release --offline --test corpus_scale -- --ignored --nocapture --exact a_pass_costs_the_tree_it_walks_or_the_graph_it_writes_into"]
fn a_pass_costs_the_tree_it_walks_or_the_graph_it_writes_into() {
    corpus_scale::parts::run();
}

#[cfg(not(unix))]
#[test]
#[ignore = "corpus size curve: Unix only"]
fn indexing_a_tree_of_every_size_stays_within_the_memory_of_the_machine() {
    panic!("the corpus size curve drives the descriptor-relative walker and needs Unix");
}
