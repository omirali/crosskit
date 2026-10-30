// Retention of closed reconciliation runs: how many to keep per root and what one
// maintenance pass protected, kept or removed.

/// Keep the newest `keep` removable closed runs of every root; remove the older ones.
/// Runs that are still `running` and historical runs that carry `OBSERVED` evidence are
/// never removable and never count against `keep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunRetention {
    keep: usize,
}

impl RunRetention {
    pub fn keep_newest(keep: usize) -> Self {
        Self { keep }
    }

    pub fn keep(&self) -> usize {
        self.keep
    }
}

/// Whether a maintenance pass only reports what it would remove or actually removes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionMode {
    Preview,
    Apply,
}

/// Counters of one retention pass over every root. `eligible_closed_runs` is derived, so
/// the report can never claim more retained and removed runs than were eligible. The
/// report carries no run ids and no roots: it is safe to print as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunRetentionReport {
    mode: RetentionMode,
    protected_running_runs: usize,
    protected_legacy_runs: usize,
    retained_runs: usize,
    removed_runs: usize,
}

impl RunRetentionReport {
    pub fn empty(mode: RetentionMode) -> Self {
        Self {
            mode,
            protected_running_runs: 0,
            protected_legacy_runs: 0,
            retained_runs: 0,
            removed_runs: 0,
        }
    }

    pub fn mode(&self) -> RetentionMode {
        self.mode
    }

    /// Closed runs that retention may remove: those it kept plus those it removed.
    pub fn eligible_closed_runs(&self) -> usize {
        self.retained_runs + self.removed_runs
    }

    pub fn protected_running_runs(&self) -> usize {
        self.protected_running_runs
    }

    /// Closed runs from the historical v6 model that still hold `OBSERVED` evidence.
    pub fn protected_legacy_runs(&self) -> usize {
        self.protected_legacy_runs
    }

    pub fn retained_runs(&self) -> usize {
        self.retained_runs
    }

    /// Removed by an `Apply` pass, or what a `Preview` pass would remove.
    pub fn removed_runs(&self) -> usize {
        self.removed_runs
    }

    pub fn protect_running(&mut self, count: usize) {
        self.protected_running_runs += count;
    }

    pub fn protect_legacy(&mut self, count: usize) {
        self.protected_legacy_runs += count;
    }

    pub fn retain_run(&mut self) {
        self.retained_runs += 1;
    }

    pub fn remove_run(&mut self) {
        self.removed_runs += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{RetentionMode, RunRetention, RunRetentionReport};

    #[test]
    fn eligible_runs_are_the_retained_plus_the_removed_ones() {
        let mut report = RunRetentionReport::empty(RetentionMode::Preview);
        report.protect_running(1);
        report.protect_legacy(2);
        report.retain_run();
        report.retain_run();
        report.remove_run();

        assert_eq!(report.mode(), RetentionMode::Preview);
        assert_eq!(report.eligible_closed_runs(), 3);
        assert_eq!(report.protected_running_runs(), 1);
        assert_eq!(report.protected_legacy_runs(), 2);
        assert_eq!(report.retained_runs(), 2);
        assert_eq!(report.removed_runs(), 1);
    }

    #[test]
    fn retention_keeps_the_requested_number_of_runs_including_zero() {
        assert_eq!(RunRetention::keep_newest(0).keep(), 0);
        assert_eq!(RunRetention::keep_newest(3).keep(), 3);
    }
}
