// How much evidence one context's graph holds: two counts, read outside any transaction.
use lbug::Connection;

use crate::application::{GraphInventory, RepositoryError};

use super::store_support::fetch_count;

const FILE_COUNT: &str = "MATCH (file:File) RETURN COUNT(file);";

/// Every revision the graph kept, not only the current one of each file. The two counts
/// are read separately because they answer different questions — how much of the tree is
/// known, and how much history of it was kept — and a ratio of the two is not one of them.
const REVISION_COUNT: &str = "MATCH (version:FileVersion) RETURN COUNT(version);";

pub(super) fn inventory(connection: &Connection<'_>) -> Result<GraphInventory, RepositoryError> {
    let files = fetch_count(connection, FILE_COUNT, vec![], "file count")?;
    let revisions = fetch_count(connection, REVISION_COUNT, vec![], "revision count")?;
    Ok(GraphInventory::new(files as u64, revisions as u64))
}
