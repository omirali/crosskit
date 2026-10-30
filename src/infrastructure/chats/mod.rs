// Conversations read out of AI coding tool transcripts.
//
// What is here divides in two, and the division is the one Stage 31's verdict И named:
//
//   * `transcript_reader` turns one `.jsonl` file into the lines of a conversation,
//     `application::conversation_assembly` decides what those lines add up to, and
//     `conversation_evidence` turns the result into the evidence of one revision. That is
//     production, and Stage 32 is what turned the feature off over it.
//   * everything else — the corpus walk, the term index, the association graph, the question
//     run and the report — is scaffolding for Stage 31's measurement and stays behind its
//     feature. A released Crosskit carries none of it.
pub(crate) mod conversation_evidence;
/// Stage 37's measurement of the two candidate rules for "the owner named this file". It
/// reads transcripts and nothing else — no database, no model, no feature of its own.
#[cfg(test)]
mod mention_measurement;
pub(crate) mod transcript_reader;
#[cfg(test)]
mod transcript_reader_tests;

#[cfg(all(test, feature = "insight-layer-measurement"))]
mod association_graph;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod chat_corpus;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod corpus_report;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod graph_report;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod measurement;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod question_report;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod question_set;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod ranking;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod report_format;
#[cfg(all(test, feature = "insight-layer-measurement"))]
mod term_index;
