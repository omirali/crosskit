// The embedded search index: one directory per context, outside the graph.
//
// The graph stays the single source of truth and this is a derivative of it — recoverable
// by rebuilding, isolated by living in its own directory, and holding nothing a reader is
// shown. Why an index outside the graph rather than the engine's full-text extension or an
// inverted index in the graph itself is measured in ADR 0011 and decided in ADR 0012.
mod analyzer;
mod context_index;
#[cfg(test)]
mod cost_tests;
mod document_collector;
/// The key of an entity, by the same analyzer the index is written with (ADR 0048).
mod entity_names;
#[cfg(test)]
mod fixture_isolation_tests;
mod language_pack;
#[cfg(test)]
mod language_pack_tests;
mod period_phrase_pack;
/// Stage 31 measurement: the terms one text gives the index. Behind the measurement
/// feature, because nothing in the product asks the analyzer for a bare list of terms.
#[cfg(all(test, feature = "insight-layer-measurement"))]
pub(crate) mod term_stream;
#[cfg(test)]
mod tests;
/// Stage 19 diagnostic: what a pass holds until it ends. Needs the allocation counter, so
/// it exists only in a build that has one.
#[cfg(all(test, feature = "allocation-profile"))]
mod writer_memory_tests;

pub use context_index::{ContextSearchIndex, search_directory};
pub use entity_names::AnalyzedEntityNames;
pub use language_pack::{
    DEFAULT_LANGUAGES, LanguagePackError, built_in_normalizations, load_normalizations,
};
pub use period_phrase_pack::built_in_period_phrases;
