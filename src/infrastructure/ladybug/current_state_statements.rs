// Static current-state statements shared by the store and its capability diagnostics.
//
// Both write paths are here: the batch statements the set-based writer runs over `$rows`,
// and the per-file statements the sequential fallback runs when a batch repeats a location.
//
// How the two keys of a pointer write are named matters only in the batch statements.
// There the keys arrive as `UNWIND` rows, and one comma-separated pattern makes the
// planner join two full table scans, while a separate `MATCH` per key gives two
// `QUERY_PRIMARY_KEY_LOOKUP` (Stage 2N). With the scalar parameters of the per-file
// statements the planner reaches both keys by primary key whichever way they are written,
// so those keep their comma-separated form. Every claim in this paragraph is held by a
// capability test; the batch texts that were replaced, and the form that never entered
// production, stay below under `#[cfg(test)]`.

/// Reads the order of the current observation for every file in a batch.
pub(super) const CURRENT_ORDER_LOOKUP: &str = "UNWIND $ids AS id MATCH (file:File {id: id})-[:CURRENT_OBSERVATION]->(:FileObservation)<-[:ORDERS]-(observation_order:ObservationOrder) RETURN file.id, observation_order.observed_at_unix_ms, observation_order.sequence;";

/// Removes the version pointer for every winning file in a batch.
pub(super) const REMOVE_CURRENT_VERSION: &str = "UNWIND $rows AS row MATCH (:File {id: row.file_id})-[current:CURRENT_VERSION]->(:FileVersion) DELETE current;";

/// Removes the observation pointer for every winning file in a batch.
pub(super) const REMOVE_CURRENT_OBSERVATION: &str = "UNWIND $rows AS row MATCH (:File {id: row.file_id})-[current:CURRENT_OBSERVATION]->(:FileObservation) DELETE current;";

/// Creates the version pointer for every winning file in a batch. The schema's ONE_ONE
/// cardinality remains the only guard against a second pointer; the separate `MATCH`
/// clauses change how the two endpoints are found, not what may be written.
pub(super) const SET_CURRENT_VERSION: &str = "UNWIND $rows AS row MATCH (file:File {id: row.file_id}) MATCH (version:FileVersion {id: row.version_id}) CREATE (file)-[:CURRENT_VERSION]->(version) RETURN COUNT(*);";

/// Creates the observation pointer for every winning file in a batch.
pub(super) const SET_CURRENT_OBSERVATION: &str = "UNWIND $rows AS row MATCH (file:File {id: row.file_id}) MATCH (observation:FileObservation {id: row.observation_id}) CREATE (file)-[:CURRENT_OBSERVATION]->(observation) RETURN COUNT(*);";

/// Removes the version pointer of one file, for the sequential fallback.
pub(super) const REMOVE_CURRENT_VERSION_FOR_FILE: &str =
    "MATCH (:File {id: $file_id})-[current:CURRENT_VERSION]->(:FileVersion) DELETE current;";

/// Removes the observation pointer of one file, for the sequential fallback.
pub(super) const REMOVE_CURRENT_OBSERVATION_FOR_FILE: &str = "MATCH (:File {id: $file_id})-[current:CURRENT_OBSERVATION]->(:FileObservation) DELETE current;";

/// Creates the version pointer of one file, for the sequential fallback. The two keys are
/// named in one comma-separated pattern, which here costs nothing: with scalar parameters
/// the planner reaches both by primary key either way, and separate `MATCH` clauses give
/// the identical plan. The capability test holds that equivalence.
pub(super) const SET_CURRENT_VERSION_FOR_FILE: &str = "MATCH (file:File {id: $file_id}), (version:FileVersion {id: $version_id}) CREATE (file)-[:CURRENT_VERSION]->(version);";

/// Creates the observation pointer of one file, for the sequential fallback.
pub(super) const SET_CURRENT_OBSERVATION_FOR_FILE: &str = "MATCH (file:File {id: $file_id}), (observation:FileObservation {id: $observation_id}) CREATE (file)-[:CURRENT_OBSERVATION]->(observation);";

/// Replaced in Stage 2N: one comma-separated pattern for the two primary keys. It is
/// semantically the same write under the same ONE_ONE checks, and it scans.
#[cfg(test)]
pub(super) const REPLACED_SET_CURRENT_VERSION: &str = "UNWIND $rows AS row MATCH (file:File {id: row.file_id}), (version:FileVersion {id: row.version_id}) CREATE (file)-[:CURRENT_VERSION]->(version) RETURN COUNT(*);";

/// Replaced in Stage 2N, the observation half of the same comma-separated form.
#[cfg(test)]
pub(super) const REPLACED_SET_CURRENT_OBSERVATION: &str = "UNWIND $rows AS row MATCH (file:File {id: row.file_id}), (observation:FileObservation {id: row.observation_id}) CREATE (file)-[:CURRENT_OBSERVATION]->(observation) RETURN COUNT(*);";

/// Candidate only: obtains an order by the identity shared with its observation instead
/// of traversing `ORDERS`. It is not semantically interchangeable with
/// `CURRENT_ORDER_LOOKUP`: a missing or mismatched `ORDERS` relationship would be hidden.
/// A future use must preserve that invariant with an explicit check.
#[cfg(test)]
pub(super) const CANDIDATE_CURRENT_ORDER_BY_OBSERVATION_ID: &str = "UNWIND $ids AS id MATCH (file:File {id: id})-[:CURRENT_OBSERVATION]->(observation:FileObservation) MATCH (observation_order:ObservationOrder {id: observation.id}) RETURN file.id, observation_order.observed_at_unix_ms, observation_order.sequence;";
