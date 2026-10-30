// A pass over a context: what became of each of its roots, and what that adds up to.
//
// Until Stage 35 a pass was a walk of one directory, and a context of ten roots was ten
// separate commands with nothing checking that all ten ran. Eight of the owner's ten never
// did, for fifteen stages, and the reason nobody noticed is that no type ever held "what
// became of each root" — the one place that looped over them, `setup`, used `?` and
// returned the first failure as the answer for the whole context.
//
// So the outcome of a pass over a context is a list, not a result. The list is built here
// and nothing in it is a remainder: a root is passed, or it names no directory to walk, or
// it failed with the category of the failure that stopped it.
use std::fmt;

use super::initial_scan::IndexError;
use super::policy_scan::ScanReport;
use super::reconcile::{IncompleteReason, ReconcileCompletion, ReconcileReport};
use super::repository::RepositoryError;

/// What one root of a context did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootOutcome {
    /// The root was walked and its files published.
    Indexed { root: String, scan: ScanReport },
    /// The root was walked again and what it did not see was retired — or was not, which
    /// the report itself says.
    Reconciled {
        root: String,
        report: Box<ReconcileReport>,
    },
    /// The `include` covers files without naming a directory, so there is nowhere to start
    /// walking. Named rather than skipped: an include a pass silently ignores is exactly
    /// how a root goes unindexed without anybody being told.
    NotADirectory { include: String },
    /// The pass over this root did not happen. What the failure said is printed where it
    /// happened and written into the graph as a category; the outcome carries the category,
    /// which is the part a later reader can act on.
    Failed {
        include: String,
        category: PassFailure,
    },
}

impl RootOutcome {
    /// The `include` this outcome is about, as the configuration spells it.
    pub fn include(&self) -> &str {
        match self {
            Self::Indexed { root, .. } | Self::Reconciled { root, .. } => root,
            Self::NotADirectory { include } | Self::Failed { include, .. } => include,
        }
    }

    /// The include and why it was not walked, for the one line a caller reads when it reads
    /// nothing else. A category without an include would say nothing about where to look,
    /// and an include without a category would send the reader back to the scrollback.
    pub fn why_not_walked(&self) -> Option<String> {
        match self {
            Self::Indexed { .. } | Self::Reconciled { .. } => None,
            Self::NotADirectory { include } => Some(format!("{include} (not a directory)")),
            Self::Failed { include, category } => Some(format!("{include} ({category})")),
        }
    }

    /// Whether this root is now in the graph as far as this pass could put it there.
    ///
    /// An incomplete reconciliation counts: it walked the tree and published what it read,
    /// and what it did not do — retire what it did not see — is said in its own report. A
    /// root that names no directory does not count, because nothing was walked.
    pub fn walked(&self) -> bool {
        matches!(self, Self::Indexed { .. } | Self::Reconciled { .. })
    }

    /// Files this root published, whatever the policy allowed of each.
    pub fn published_files(&self) -> usize {
        match self {
            Self::Indexed { scan, .. } => scan.published_files(),
            Self::Reconciled { report, .. } => report.scan().published_files(),
            Self::NotADirectory { .. } | Self::Failed { .. } => 0,
        }
    }

    pub fn indexed_chunks(&self) -> usize {
        match self {
            Self::Indexed { scan, .. } => scan.indexed_chunks(),
            Self::Reconciled { report, .. } => report.scan().indexed_chunks(),
            Self::NotADirectory { .. } | Self::Failed { .. } => 0,
        }
    }
}

/// Why a pass over one root did not happen.
///
/// These are not new categories. Each one is the name of a failure the program already
/// types — `IndexError` and the `RepositoryError` variants ADR 0015 introduced — so a run
/// record and an error message cannot disagree about what went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassFailure {
    /// The tree could not be walked or read: the directory is gone, is not a directory, or
    /// an entry under it could not be described.
    Source,
    /// The operating system would not let this process open the root (`EPERM`): on macOS, a
    /// place behind privacy protection — Documents, Desktop, Downloads — that the program
    /// running the pass was not given. Apart from `Source` because the repair is a setting
    /// and not the tree: a background service started by launchd is refused where the same
    /// pass from a terminal with access is not.
    NotPermitted,
    /// The graph refused what the pass wrote, for a reason that is not one of the ones
    /// below.
    Storage,
    /// The graph did not open because the log beside it cannot be replayed.
    GraphLogUnreadable,
    /// Another process holds the graph for writing.
    GraphBusy,
    /// The graph is written in a schema this build does not read.
    SchemaNotThisBuild,
    /// The storage engine found no memory for the operation.
    StorageExhausted,
    /// The graph broke one of its own invariants.
    InvariantViolation,
}

impl PassFailure {
    /// The label a run record keeps. Stable and short, because it is stored and read back
    /// by `status` and `doctor` long after the message that produced it is gone.
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::NotPermitted => "not-permitted",
            Self::Storage => "storage",
            Self::GraphLogUnreadable => "graph-log-unreadable",
            Self::GraphBusy => "graph-busy",
            Self::SchemaNotThisBuild => "schema-not-this-build",
            Self::StorageExhausted => "storage-exhausted",
            Self::InvariantViolation => "invariant-violation",
        }
    }

    pub fn of_index_error(error: &IndexError) -> Self {
        match error {
            IndexError::Source(_) => Self::Source,
            IndexError::NotPermitted(_) => Self::NotPermitted,
            IndexError::Repository(repository) => Self::of_repository_error(repository),
        }
    }

    /// A rollback failure is reported under the category of the failure that started it:
    /// that is the one worth reading, and the type already says so.
    pub fn of_repository_error(error: &RepositoryError) -> Self {
        match error {
            RepositoryError::Storage(_) => Self::Storage,
            RepositoryError::InvariantViolation(_) => Self::InvariantViolation,
            RepositoryError::StorageExhausted(_) => Self::StorageExhausted,
            RepositoryError::RollbackFailed(inner) => Self::of_repository_error(inner),
            RepositoryError::GraphLogUnreadable { .. } => Self::GraphLogUnreadable,
            RepositoryError::GraphBusy => Self::GraphBusy,
            RepositoryError::SchemaNotThisBuild { .. } => Self::SchemaNotThisBuild,
        }
    }
}

impl fmt::Display for PassFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// What a pass over one context did, root by root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextPassReport {
    outcomes: Vec<RootOutcome>,
    /// The pass walked directories that changes named, not the context's roots.
    scoped: bool,
}

impl ContextPassReport {
    /// The report of a pass over the directories changes named. Walking none of them — every
    /// change was somewhere the policy does not index — is a pass that completed.
    pub fn within() -> Self {
        Self {
            outcomes: Vec::new(),
            scoped: true,
        }
    }

    pub fn is_scoped(&self) -> bool {
        self.scoped
    }

    pub fn record(&mut self, outcome: RootOutcome) {
        self.outcomes.push(outcome);
    }

    pub fn roots(&self) -> usize {
        self.outcomes.len()
    }

    pub fn walked(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.walked())
            .count()
    }

    /// Every include this pass did not walk, in the order the configuration declares them:
    /// the ones that failed and the ones that name no directory. The command's last line is
    /// built from this, and a caller that reads nothing else still learns which roots to
    /// look at.
    pub fn not_walked(&self) -> Vec<String> {
        self.outcomes
            .iter()
            .filter_map(RootOutcome::why_not_walked)
            .collect()
    }

    /// Roots the operating system would not let this pass open. Counted apart because their
    /// repair is a permission given to the program, which no other failure needs.
    pub fn roots_not_permitted(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| {
                matches!(
                    outcome,
                    RootOutcome::Failed {
                        category: PassFailure::NotPermitted,
                        ..
                    }
                )
            })
            .count()
    }

    /// Paths under the walked roots that the pass could not read.
    pub fn failed_paths(&self) -> usize {
        self.scans().map(ScanReport::failed_paths).sum()
    }

    pub fn published_files(&self) -> usize {
        self.outcomes.iter().map(RootOutcome::published_files).sum()
    }

    pub fn indexed_chunks(&self) -> usize {
        self.outcomes.iter().map(RootOutcome::indexed_chunks).sum()
    }

    /// Whether every root of the context was walked. This, and not the absence of a
    /// failure, is what "the context was indexed" means.
    pub fn whole_context(&self) -> bool {
        self.walked() == self.roots() && (self.roots() > 0 || self.scoped)
    }

    /// Files this pass read whole and indexed by content.
    pub fn indexed_files(&self) -> usize {
        self.scans().map(ScanReport::indexed_files).sum()
    }

    /// Files published by metadata alone, whichever of the reasons it was.
    pub fn metadata_only_files(&self) -> usize {
        self.scans()
            .map(|scan| scan.metadata_only_files() + scan.content_skipped_files())
            .sum()
    }

    pub fn excluded_paths(&self) -> usize {
        self.scans().map(ScanReport::excluded_paths).sum()
    }

    /// What one root's reconciliation reported, for the outcomes that reconciled one.
    pub fn reconciliations(&self) -> impl Iterator<Item = &ReconcileReport> {
        self.outcomes.iter().filter_map(|outcome| match outcome {
            RootOutcome::Reconciled { report, .. } => Some(report.as_ref()),
            _ => None,
        })
    }

    /// Locations this pass retired, over every root that reconciled and completed.
    pub fn marked_missing(&self) -> (usize, usize) {
        self.reconciliations()
            .map(ReconcileReport::completion)
            .fold(
                (0, 0),
                |(missing, relocated), completion| match completion {
                    ReconcileCompletion::Completed {
                        marked_missing,
                        relocated: moved,
                    } => (missing + marked_missing, relocated + moved),
                    ReconcileCompletion::Incomplete(_) => (missing, relocated),
                },
            )
    }

    /// Why this pass may not retire what it did not see, over the whole context.
    ///
    /// A pass over a context is complete only when every root was walked **and** every
    /// walk was complete. One root that could not be vouched for makes the pass incomplete,
    /// which is the same rule one root has always had, applied to the list.
    pub fn completion(&self) -> ContextCompletion {
        let mut reasons = Vec::new();
        if !self.whole_context() {
            reasons.push(ContextIncomplete::RootsNotWalked(
                self.roots() - self.walked(),
            ));
        }
        for outcome in &self.outcomes {
            if let RootOutcome::Reconciled { report, .. } = outcome
                && let ReconcileCompletion::Incomplete(root_reasons) = report.completion()
            {
                reasons.extend(
                    root_reasons
                        .iter()
                        .copied()
                        .map(ContextIncomplete::RootIncomplete),
                );
            }
        }
        match reasons.is_empty() {
            true => ContextCompletion::Completed,
            false => ContextCompletion::Incomplete(reasons),
        }
    }

    fn scans(&self) -> impl Iterator<Item = &ScanReport> {
        self.outcomes.iter().filter_map(|outcome| match outcome {
            RootOutcome::Indexed { scan, .. } => Some(scan),
            RootOutcome::Reconciled { report, .. } => Some(report.scan()),
            RootOutcome::NotADirectory { .. } | RootOutcome::Failed { .. } => None,
        })
    }
}

/// Whether a pass over a context may retire what it did not see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextCompletion {
    Completed,
    Incomplete(Vec<ContextIncomplete>),
}

impl ContextCompletion {
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// One reason a pass over a context is incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextIncomplete {
    /// This many roots were not walked at all.
    RootsNotWalked(usize),
    /// A root was walked and its walk could not be vouched for.
    RootIncomplete(IncompleteReason),
}

#[cfg(test)]
mod tests {
    use super::{ContextPassReport, PassFailure, RootOutcome};
    use crate::application::RepositoryError;
    use crate::application::initial_scan::IndexError;
    use crate::application::policy_scan::ScanReport;
    use crate::domain::NormalizedPath;

    fn indexed(root: &str) -> RootOutcome {
        RootOutcome::Indexed {
            root: root.to_string(),
            scan: ScanReport::empty(NormalizedPath::from_absolute(root).unwrap()),
        }
    }

    fn failed(include: &str) -> RootOutcome {
        RootOutcome::Failed {
            include: include.to_string(),
            category: PassFailure::Source,
        }
    }

    /// The property the whole stage exists for: one root of three failing leaves the other
    /// two walked, and the report says so instead of answering with the first failure.
    #[test]
    fn a_root_that_failed_does_not_take_the_others_with_it() {
        let mut report = ContextPassReport::default();
        report.record(indexed("/tree/one"));
        report.record(failed("/tree/two"));
        report.record(indexed("/tree/three"));

        assert_eq!(report.roots(), 3);
        assert_eq!(report.walked(), 2);
        assert_eq!(report.not_walked(), vec!["/tree/two (source)"]);
        assert!(!report.whole_context());
    }

    /// A partial success is not a success, and the type says it in one question rather than
    /// leaving every caller to compare two counts.
    #[test]
    fn a_context_is_indexed_only_when_every_root_was_walked() {
        let mut whole = ContextPassReport::default();
        whole.record(indexed("/tree/one"));
        whole.record(indexed("/tree/two"));
        assert!(whole.whole_context());

        let empty = ContextPassReport::default();
        assert!(
            !empty.whole_context(),
            "a context with no root to walk has not been indexed"
        );
    }

    /// An include that names no directory is not a failure and not a success: it is a line
    /// in the configuration that a pass cannot act on, and it has to be named.
    #[test]
    fn an_include_that_names_no_directory_is_reported_as_not_walked() {
        let mut report = ContextPassReport::default();
        report.record(indexed("/tree/one"));
        report.record(RootOutcome::NotADirectory {
            include: "/tree/*/src".to_string(),
        });

        assert_eq!(report.not_walked(), vec!["/tree/*/src (not a directory)"]);
        assert!(!report.whole_context());
    }

    /// The categories are the ones the program already types. A new name here would be a
    /// second taxonomy that could disagree with the error a person is shown.
    #[test]
    fn a_failure_keeps_the_category_of_the_error_that_caused_it() {
        assert_eq!(
            PassFailure::of_index_error(&IndexError::Source("gone".to_string())),
            PassFailure::Source
        );
        assert_eq!(
            PassFailure::of_index_error(&IndexError::Repository(RepositoryError::GraphBusy)),
            PassFailure::GraphBusy
        );
        assert_eq!(
            PassFailure::of_repository_error(&RepositoryError::RollbackFailed(Box::new(
                RepositoryError::StorageExhausted("a batch")
            ))),
            PassFailure::StorageExhausted,
            "a rollback failure is reported under the failure that started it"
        );
        assert_eq!(PassFailure::GraphBusy.label(), "graph-busy");
    }
}
