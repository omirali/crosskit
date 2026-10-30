// A reconciliation run: the durable unit whose complete walk may retire unseen locations.
use super::evidence::EvidenceError;
use super::source_path::NormalizedPath;

/// One `index reconcile` execution. `sequence` is the sighting epoch the run opened:
/// every location written from then on, by this run, another run or a plain scan, is
/// stamped with an epoch not lower than it. The run may retire only locations whose
/// recorded epoch is lower, so nothing seen after it started can be declared missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileRun {
    id: String,
    sequence: i64,
    root: NormalizedPath,
    started_at_unix_ms: i64,
}

impl ReconcileRun {
    pub fn new(
        id: impl Into<String>,
        sequence: i64,
        root: NormalizedPath,
        started_at_unix_ms: i64,
    ) -> Result<Self, EvidenceError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(EvidenceError::MissingField("reconcile run id"));
        }
        Ok(Self {
            id,
            sequence,
            root,
            started_at_unix_ms,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn sequence(&self) -> i64 {
        self.sequence
    }

    pub fn root(&self) -> &NormalizedPath {
        &self.root
    }

    pub fn started_at_unix_ms(&self) -> i64 {
        self.started_at_unix_ms
    }
}
