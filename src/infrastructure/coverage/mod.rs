// Stage 34's coverage audit: how much of the tree each context actually holds, and what
// every loss between the disk and the search index is called.
//
// Nothing here is compiled into a released Crosskit. Every module is declared
// `cfg(all(test, feature = "coverage-audit"))`, so even a build that turns the feature on
// carries none of it unless it is a test build — the shape Stage 31 established for a
// measurement that reads the owner's own machine.
//
// The map:
//
//   * `funnel` — the arithmetic: one root's entries sorted into named classes, and the
//     claim that no entry is ever lost between them.
//   * `tree_census` — one root walked by the product's walker under the product's policy,
//     counting instead of writing.
//   * `path_diff` — the two lists of one root reconciled: what the policy admits on disk
//     now against what the graph holds, so a gap is a list of files with a reason each.
//   * `configured_roots` — which directories each context names.
//   * `report` — how the audit prints itself.
//   * `audit` — the funnel itself: every root walked and put beside its graph.
//   * `file_probes` — where one named file stops, and what an unindexed context says.
//   * `graph_probes` — what the graphs say without walking: which roots a pass reached,
//     which files the gap is made of, and where a database's size comes from.
//
// The graph half is not here: a Cypher statement belongs to the storage adapter, so it
// lives in `ladybug::coverage_queries` under the same feature.
mod audit;
mod configured_roots;
mod file_probes;
mod funnel;
mod graph_probes;
mod path_diff;
mod report;
mod tree_census;
