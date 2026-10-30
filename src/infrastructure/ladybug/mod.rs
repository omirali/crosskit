/// How many bytes of text one chunk named `chunk` is, as a Cypher expression for `concat!`.
///
/// The byte range when the chunk has one — a file's chunk, measured without reading its text —
/// and otherwise the bytes of its text. A reply of a conversation is extracted out of JSON and
/// has no honest range (its range is zero by design), and reading it as zero bytes let every
/// transcript pass the per-job budget and priced it at nothing. One expression wherever a
/// size is summed, so a budget and a price cannot disagree about what a chunk weighs.
macro_rules! chunk_bytes {
    () => {
        "CASE WHEN chunk.end_byte > chunk.start_byte THEN chunk.end_byte - chunk.start_byte ELSE octet_length(encode(chunk.text)) END"
    };
}

mod batch_projection;
#[cfg(test)]
mod batch_tests;
#[cfg(test)]
mod checkpoint_cost_tests;
#[cfg(test)]
mod chunk_link_capability_tests;
#[cfg(test)]
mod chunk_script_tests;
mod chunk_store;
#[cfg(test)]
mod conversation_migration_tests;
mod conversation_store;
/// Stage 34's coverage audit reads the graph through these; nothing else may.
#[cfg(all(test, feature = "coverage-audit"))]
pub(super) mod coverage_queries;
mod current_state_statements;
mod current_state_store;
#[cfg(test)]
mod current_state_tests;
#[cfg(test)]
mod current_version_question_tests;
mod engine_tuning;
#[cfg(test)]
mod explain_capability_tests;
#[cfg(test)]
mod file_identity_tests;
mod file_version_store;
mod graph_maintenance;
mod graph_presence;
/// UI-4: the reads behind `crosskit graph`.
mod graph_view_store;
#[cfg(test)]
mod in_process_read_capability_tests;
#[cfg(test)]
mod insight_analysis_tests;
mod insight_job_store;
#[cfg(test)]
mod insight_job_tests;
mod insight_work_store;
#[cfg(test)]
mod insight_work_tests;
mod inventory_store;
/// The knowledge layer: entities, facts and what each fact rests on (ADR 0048).
mod knowledge_store;
#[cfg(test)]
mod knowledge_store_tests;
#[cfg(test)]
mod link_capability_tests;
#[cfg(test)]
mod location_link_capability_tests;
mod location_store;
#[cfg(test)]
mod log_failure_capability_tests;
mod manager;
mod migration;
mod observation_store;
mod period_store;
#[cfg(test)]
mod posting_capability_tests;
#[cfg(test)]
mod posting_cost_tests;
mod process_memory_limit;
#[cfg(test)]
mod query_plan;
#[cfg(test)]
mod read_only_access_tests;
mod reconcile_store;
#[cfg(test)]
mod reconcile_tests;
mod repository;
mod revision_input_store;
mod root_coverage_store;
mod run_retention_store;
#[cfg(test)]
mod run_retention_tests;
mod search_store;
#[cfg(test)]
mod set_based_tests;
#[cfg(test)]
mod source_cost_tests;
#[cfg(test)]
mod storage_failure_tests;
mod store_support;
/// Every current conversation read back for stitching into tasks (ADR 0052).
mod task_store;
#[cfg(test)]
pub(super) mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod touch_migration_tests;
mod touch_store;
#[cfg(test)]
mod unwind_capability_tests;
/// The work one directory selects for `index jobs run`.
mod work_scope_store;
#[cfg(test)]
mod work_scope_tests;

pub use engine_tuning::EngineTuning;
pub use graph_maintenance::LadybugGraphMaintenance;
pub use graph_presence::{
    GraphPresence, presence_at as graph_presence_at, presence_of as graph_presence_of,
};
pub use manager::{LadybugDatabaseManager, context_free_graph, context_free_graph_reason};
pub use repository::LadybugGraphRepository;
pub use revision_input_store::{LadybugInsightCache, LadybugRevisionInput};
pub use search_store::LadybugSearchGraph;
