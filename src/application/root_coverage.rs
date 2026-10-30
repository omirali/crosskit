// What one root of a context has in the graph, and what the last attempt on it was.
//
// A context is a list of roots and a pass is a walk of one of them, so "is this context
// indexed" is not a question with one answer. Stage 34 measured what that cost: eight of ten
// roots of one context had never been walked, and every report the program printed said
// `indexed`, because every report counted the context's files and none of them asked about a
// root.
use super::repository::RepositoryError;
use crate::domain::NormalizedPath;

/// What the graph holds under one root.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RootCoverage {
    files: u64,
    last_seen_at_unix_ms: Option<i64>,
    last_failure: Option<FailedPass>,
}

impl RootCoverage {
    pub fn new(
        files: u64,
        last_seen_at_unix_ms: Option<i64>,
        last_failure: Option<FailedPass>,
    ) -> Self {
        Self {
            files,
            last_seen_at_unix_ms,
            last_failure,
        }
    }

    /// Files the graph holds at a path under this root, present or missing.
    pub fn files(&self) -> u64 {
        self.files
    }

    /// When a pass last saw anything here. It comes from the locations rather than from a
    /// run record, because `index init` writes no run and would otherwise look like a root
    /// nobody ever walked.
    pub fn last_seen_at_unix_ms(&self) -> Option<i64> {
        self.last_seen_at_unix_ms
    }

    /// The last attempt on this root that did not happen, if there was one.
    pub fn last_failure(&self) -> Option<&FailedPass> {
        self.last_failure.as_ref()
    }

    /// Whether any pass ever walked this root.
    ///
    /// Read from what the graph holds, not from what a command once printed: a root with no
    /// file and no sighting was never walked, whatever any run said at the time.
    pub fn ever_walked(&self) -> bool {
        self.files > 0 || self.last_seen_at_unix_ms.is_some()
    }
}

/// A pass that was attempted and did not happen, kept so the question "why is this root
/// empty" has an answer later and not only in the output of the run that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedPass {
    category: String,
    at_unix_ms: i64,
}

impl FailedPass {
    pub fn new(category: impl Into<String>, at_unix_ms: i64) -> Self {
        Self {
            category: category.into(),
            at_unix_ms,
        }
    }

    /// The category the failure was typed as when it happened.
    pub fn category(&self) -> &str {
        &self.category
    }

    pub fn at_unix_ms(&self) -> i64 {
        self.at_unix_ms
    }
}

/// Where a report asks what one root holds. Read-only: every caller of this is a command
/// that may not write.
pub trait RootCoverageReader {
    fn coverage_of_root(&self, root: &NormalizedPath) -> Result<RootCoverage, RepositoryError>;
}

#[cfg(test)]
mod tests {
    use super::{FailedPass, RootCoverage};

    /// The question the whole stage turns on, and it is answered from the graph rather than
    /// from anything a command printed.
    #[test]
    fn a_root_with_no_file_and_no_sighting_was_never_walked() {
        assert!(!RootCoverage::default().ever_walked());
        assert!(RootCoverage::new(1, None, None).ever_walked());
        assert!(RootCoverage::new(0, Some(1_700_000_000_000), None).ever_walked());
    }

    /// A failed attempt does not make a root walked. It explains why it is not.
    #[test]
    fn a_failed_attempt_is_not_a_walk() {
        let coverage = RootCoverage::new(0, None, Some(FailedPass::new("source", 1_000)));

        assert!(!coverage.ever_walked());
        assert_eq!(
            coverage.last_failure().map(FailedPass::category),
            Some("source")
        );
    }
}
