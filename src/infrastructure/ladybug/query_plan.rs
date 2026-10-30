// Reading a physical plan LadybugDB printed. The capability tests rest on telling a point
// access from a table scan, and the engine's names make that easy to get wrong:
// `PRIMARY_KEY_SCAN_NODE_TABLE` contains `SCAN_NODE_TABLE`, so a substring test counts a
// point access as a scan and confirms a conclusion that may not be there. Everything here
// counts whole operators instead, and both capability test modules use it.

/// A full scan of a node table, with the primary-key point access excluded.
pub(super) fn table_scans(plan: &str) -> usize {
    plan.matches("SCAN_NODE_TABLE").count() - primary_key_scans(plan)
}

/// A scan of a relationship table.
pub(super) fn relationship_scans(plan: &str) -> usize {
    plan.matches("SCAN_REL_TABLE").count()
}

/// Reaching a node by its primary key from a scalar parameter.
pub(super) fn primary_key_scans(plan: &str) -> usize {
    plan.matches("PRIMARY_KEY_SCAN_NODE_TABLE").count()
}

/// Reaching a node by its primary key from a value of an `UNWIND` row.
pub(super) fn primary_key_lookups(plan: &str) -> usize {
    plan.matches("QUERY_PRIMARY_KEY_LOOKUP").count()
}

/// Whether the plan scans any table, as opposed to reaching rows by key.
pub(super) fn has_scan(plan: &str) -> bool {
    table_scans(plan) > 0 || relationship_scans(plan) > 0
}

/// The operator names of a plan, in the order the engine printed them, so two plans can be
/// compared without depending on how the boxes are drawn.
pub(super) fn operators(plan: &str) -> Vec<&str> {
    plan.split_whitespace()
        .filter(|word| {
            word.ends_with(']') && word.contains('[') && word.starts_with(char::is_uppercase)
        })
        .collect()
}
