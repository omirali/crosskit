// Use case: explicit, operator-driven retention of closed reconciliation runs.
//
// Retention is a maintenance action a person asks for, never a side effect of `index
// reconcile`: the reconcile path only appends runs and advances the epoch.
use crate::domain::{RetentionMode, RunRetention, RunRetentionReport};

use super::repository::RepositoryError;

/// Maintenance of the `ReconcileRun` records. Only closed runs (`completed` or
/// `incomplete`) without historical `OBSERVED` evidence may be removed; `running` runs and
/// legacy runs are protected. Retention counts per root, so activity under one root
/// never erases the history of another. Removing runs touches nothing else: not the epoch,
/// not locations, not evidence.
pub trait ReconcileRunMaintenance {
    /// Counts what `apply_run_retention` would remove, without writing anything.
    fn preview_run_retention(
        &self,
        retention: RunRetention,
    ) -> Result<RunRetentionReport, RepositoryError>;

    /// Removes every eligible closed run beyond the newest `retention.keep()` per root, all
    /// in one transaction: a failure anywhere leaves every run in place.
    fn apply_run_retention(
        &self,
        retention: RunRetention,
    ) -> Result<RunRetentionReport, RepositoryError>;
}

pub struct RunRetentionService<'a, Repository> {
    repository: &'a Repository,
}

impl<'a, Repository: ReconcileRunMaintenance> RunRetentionService<'a, Repository> {
    pub fn new(repository: &'a Repository) -> Self {
        Self { repository }
    }

    pub fn execute(
        &self,
        retention: RunRetention,
        mode: RetentionMode,
    ) -> Result<RunRetentionReport, RepositoryError> {
        match mode {
            RetentionMode::Preview => self.repository.preview_run_retention(retention),
            RetentionMode::Apply => self.repository.apply_run_retention(retention),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{ReconcileRunMaintenance, RunRetentionService};
    use crate::application::RepositoryError;
    use crate::domain::{RetentionMode, RunRetention, RunRetentionReport};

    #[derive(Default)]
    struct RecordingMaintenance {
        calls: RefCell<Vec<(RetentionMode, usize)>>,
    }

    impl ReconcileRunMaintenance for RecordingMaintenance {
        fn preview_run_retention(
            &self,
            retention: RunRetention,
        ) -> Result<RunRetentionReport, RepositoryError> {
            self.calls
                .borrow_mut()
                .push((RetentionMode::Preview, retention.keep()));
            Ok(RunRetentionReport::empty(RetentionMode::Preview))
        }

        fn apply_run_retention(
            &self,
            retention: RunRetention,
        ) -> Result<RunRetentionReport, RepositoryError> {
            self.calls
                .borrow_mut()
                .push((RetentionMode::Apply, retention.keep()));
            Ok(RunRetentionReport::empty(RetentionMode::Apply))
        }
    }

    #[test]
    fn preview_never_reaches_the_applying_port_method() {
        let repository = RecordingMaintenance::default();
        let report = RunRetentionService::new(&repository)
            .execute(RunRetention::keep_newest(2), RetentionMode::Preview)
            .unwrap();

        assert_eq!(report.mode(), RetentionMode::Preview);
        assert_eq!(*repository.calls.borrow(), [(RetentionMode::Preview, 2)]);
    }

    #[test]
    fn apply_reaches_only_the_applying_port_method() {
        let repository = RecordingMaintenance::default();
        let report = RunRetentionService::new(&repository)
            .execute(RunRetention::keep_newest(0), RetentionMode::Apply)
            .unwrap();

        assert_eq!(report.mode(), RetentionMode::Apply);
        assert_eq!(*repository.calls.borrow(), [(RetentionMode::Apply, 0)]);
    }
}
