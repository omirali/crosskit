// The one question a scan asks storage before it describes a file: what does the graph
// already hold under this identity? The answer plus the birth time the filesystem reported
// decide, by the domain rule, which identity the observation is published under.
use crate::domain::KnownIdentity;

use super::repository::RepositoryError;

pub trait FileIdentityHistory {
    /// What the graph holds under the kernel's device-and-inode identity. Read outside any
    /// transaction of the scan: it only reports what is already durable, and a file the
    /// scan itself writes later cannot contradict its own observation.
    fn known_identity(&self, base_id: &str) -> Result<KnownIdentity, RepositoryError>;
}
