// `index watch [<directory>]`: the foreground watcher, and the one composition of a watch over
// a context — the background service runs exactly this, once per context.
//
// Output names the root the operator passed and counters, never a changed or excluded
// path: a filesystem event is a hint, and only the reconcile pass decides anything.
use std::path::Path;
use std::sync::Arc;

use crate::application::{
    ContextPassReport, IndexError, InsightProvider, PassScope, ReconcilePass, SearchError,
    WatchError, WatchEventSource, WatchObserver, WatchReport, WatchSchedule, WatchService,
    directories_to_walk,
};
use crate::domain::{NormalizedPath, WorkspaceName};
use crate::infrastructure::analysis::EngineUnavailable;
use crate::infrastructure::insight_run::{self, InsightRun};
use crate::infrastructure::{
    DiskScopeProbe, LadybugDatabaseManager, LoadedIndexPolicy, NotifyWatchSource, SystemClock,
    default_probe_service_directory, load_index_policy, stop_on_ctrl_c,
};

use super::index_context::{self, ContextPass, Narration, Pass};
use super::policy::policy_file_label;
use super::search_index::{report_search_failures, wanted_paths};
use super::watch_narration::{Console, NarratedWatch, render_summary};
use super::workspace;

/// Watches a whole context, or one directory of it when a directory is named.
///
/// Everything that can refuse the run happens before anything opens, and the pass the
/// watcher runs is the same pass `index reconcile` runs: over every root of the context,
/// with one root's failure not ending the others. That is what makes a root the backend
/// refuses a latency problem instead of a coverage one — every pass walks it anyway.
///
/// The policy is loaded once: changing the file requires a restart.
pub(super) fn run(named: Option<&Path>, context: Option<&str>) -> Result<(), String> {
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select(&loaded, context)?;
    let prepared = prepare(&loaded, &selected, named)?;
    let (events, stop) = NotifyWatchSource::new(Some(default_probe_service_directory()?));
    stop_on_ctrl_c(stop)?;

    let mut console = Console;
    let mut narration = prepared.narration(&mut console);
    let outcome = prepared.run(&events, &mut narration);
    // Once, when the watcher stops, rather than after every pass: a watcher runs for hours
    // and a lock held by another process would otherwise print the same line all day.
    report_search_failures(&prepared.search_failures());
    match outcome {
        Ok(report) => {
            render_summary(&mut Console, &report, prepared.analysed());
            Ok(())
        }
        Err(WatchError::FinalPassFailed(report)) => {
            render_summary(&mut Console, &report, prepared.analysed());
            Err(WatchError::FinalPassFailed(report).to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// One context, opened, with its analysis decided, ready to be watched.
pub(super) struct PreparedWatch<'a> {
    loaded: &'a LoadedIndexPolicy,
    workspace: &'a WorkspaceName,
    pass: ContextPass<'a>,
    jobs_per_pass: Option<usize>,
    analysis: Option<InsightRun<Box<dyn InsightProvider>>>,
    /// The one sentence that says what this watch does with the queue, or nothing when the
    /// configuration did not turn analysis on.
    announcement: Option<String>,
    schedule: WatchSchedule,
}

/// Opens the context and decides, once, whether its watch works the insight queue.
///
/// Decided here rather than per pass: a watcher that re-read its permission every pass could
/// change what it sends halfway through a session, and one with no key would repeat the same
/// refusal until the lifecycle it exists to report was unreadable.
pub(super) fn prepare<'a>(
    loaded: &'a LoadedIndexPolicy,
    workspace: &'a WorkspaceName,
    named: Option<&Path>,
) -> Result<PreparedWatch<'a>, String> {
    let pass = index_context::prepare(loaded, workspace, named)?;
    let jobs_per_pass = loaded.watch_analysis_limit();
    let decided = jobs_per_pass.map(|_| {
        insight_run::decide(
            pass.manager(),
            loaded.workspaces().content_egress(workspace),
            loaded.engine(),
            loaded.insight_budget(),
            insight_run::Question::new(loaded.vocabulary(), loaded.normalizations()),
        )
    });
    let announcement = analysis_line(jobs_per_pass, decided.as_ref());
    Ok(PreparedWatch {
        loaded,
        workspace,
        pass,
        jobs_per_pass,
        analysis: decided.and_then(Result::ok),
        announcement,
        schedule: WatchSchedule::default(),
    })
}

impl PreparedWatch<'_> {
    /// The same watch on another schedule — what the background service runs (ADR 0050).
    pub(super) fn scheduled(mut self, schedule: WatchSchedule) -> Self {
        self.schedule = schedule;
        self
    }

    /// The graph this watch writes to, which the service also reads through.
    pub(super) fn manager(&self) -> &Arc<LadybugDatabaseManager> {
        self.pass.manager()
    }

    /// The directories a watcher can subscribe to.
    pub(super) fn roots_to_watch(&self) -> Vec<NormalizedPath> {
        self.pass.roots_to_watch()
    }

    /// Whether this watch was told to work the queue at all — even if the engine then could
    /// not be reached, which `announcement` says.
    pub(super) fn analysed(&self) -> bool {
        self.jobs_per_pass.is_some()
    }

    pub(super) fn announcement(&self) -> Option<&str> {
        self.announcement.as_deref()
    }

    /// Everything that stopped the search index since this context was opened.
    pub(super) fn search_failures(&self) -> Vec<SearchError> {
        self.pass.search_failures()
    }

    /// The watch's own lines, told to `lines`.
    pub(super) fn narration<'l>(
        &self,
        lines: &'l mut dyn super::watch_narration::WatchLines,
    ) -> NarratedWatch<'l> {
        NarratedWatch::new(
            lines,
            self.workspace.to_string(),
            self.roots_to_watch().len(),
            policy_file_label(self.loaded),
            self.schedule,
            self.announcement.clone(),
        )
    }

    /// Watches until a stop arrives on `events`.
    pub(super) fn run(
        &self,
        events: &dyn WatchEventSource,
        observer: &mut dyn WatchObserver,
    ) -> Result<WatchReport, WatchError> {
        let manager: &Arc<LadybugDatabaseManager> = self.pass.manager();
        let repository = manager.repository();
        // The watcher asks the same question of the queue that a search asks of a result: a
        // path this context no longer indexes is not worth spending a model call on.
        let is_wanted = wanted_paths(self.loaded, self.workspace);
        let sweeps = self
            .analysis
            .as_ref()
            .map(|analysis| analysis.sweeps(&repository, &is_wanted));
        let reconcile = ContextReconcile {
            pass: &self.pass,
            loaded: self.loaded,
        };
        let mut service = WatchService::new(
            &reconcile,
            &repository,
            events,
            &SystemClock,
            observer,
            self.schedule,
        );
        if let (Some(sweeps), Some(limit)) = (&sweeps, self.jobs_per_pass) {
            service = service.analyzing(sweeps, limit);
        }
        service.run(&self.roots_to_watch())
    }
}

/// The one sentence that says what this watcher will do with the queue, or nothing at all
/// when the configuration did not turn analysis on. A watcher that was not asked to analyse
/// prints exactly what it printed before analysis existed.
fn analysis_line(
    jobs_per_pass: Option<usize>,
    decided: Option<&Result<InsightRun<Box<dyn InsightProvider>>, EngineUnavailable>>,
) -> Option<String> {
    let jobs_per_pass = jobs_per_pass?;
    Some(match decided? {
        Ok(run) => format!(
            "Analysis: {}; at most {jobs_per_pass} job(s) per pass.",
            run.label()
        ),
        Err(unavailable) => format!(
            "Analysis: not run — {}",
            unavailable.reason("No job is claimed while this watcher runs.")
        ),
    })
}

/// The watcher's pass: the same pass `index reconcile` makes over the context, run quietly.
///
/// The watcher owns no other way to change the graph, and it is deliberately the very same
/// code path — a watcher with a pass of its own would be a second way to index, and the two
/// would drift about which roots a pass covers.
struct ContextReconcile<'a> {
    pass: &'a ContextPass<'a>,
    loaded: &'a LoadedIndexPolicy,
}

impl ReconcilePass for ContextReconcile<'_> {
    /// A pass after named changes walks the directories they lie in; one that cannot be
    /// scoped — not knowing where, too many places, a path under no root — walks the context.
    fn execute(&self, scope: &PassScope) -> Result<ContextPassReport, IndexError> {
        if let PassScope::Changes(changes) = scope
            && let Some(directories) = directories_to_walk(
                changes,
                &self.pass.resolved_roots(),
                self.loaded.policy(),
                &DiskScopeProbe,
            )
        {
            return Ok(self.pass.walk_within(&directories));
        }
        Ok(self.pass.walk(Pass::Reconcile, Narration::Silent))
    }
}
