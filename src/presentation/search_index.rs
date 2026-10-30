// Opening the two halves of search for one context: its index and the graph that index is
// derived from.
//
// Every command that touches search opens them the same way, so the directory a context's
// index lives in is decided in one place and no command can reach another context's.
use std::sync::Arc;

use super::refusal::Refusal;
use crate::application::{SearchError, SearchIndexReader, SearchIndexUpdate};
use crate::domain::{NormalizedPath, PolicyClass, WorkspaceName};
use crate::infrastructure::{
    ContextSearchIndex, LadybugDatabaseManager, LadybugSearchGraph, LoadedIndexPolicy,
    data_directory, search_directory,
};

/// What a reader is told when the index was not written by the rules this build applies.
///
/// One sentence in one place, because two commands reach the same state by different roads:
/// `search` is handed it with the page it prints, and `ask` has to look before it spends a
/// model call on an answer drawn from part of a corpus.
pub(super) const WRITTEN_BY_OTHER_RULES: &str = "This index was written by other rules than this build applies, so part of what it holds is in a form this query cannot reach. `crosskit index rebuild --search` writes it again.";

/// The search index of one context and the graph beside it.
pub(super) struct ContextSearch {
    pub(super) index: ContextSearchIndex,
    pub(super) graph: LadybugSearchGraph,
}

pub(super) fn open(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    manager: &Arc<LadybugDatabaseManager>,
) -> Result<ContextSearch, String> {
    Ok(open_or_refuse(loaded, workspace, manager)?)
}

/// The same, with the refusal still typed for a command that answers a program.
pub(super) fn open_or_refuse(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    manager: &Arc<LadybugDatabaseManager>,
) -> Result<ContextSearch, Refusal> {
    let directory = directory_of(workspace).map_err(Refusal::failed)?;
    Ok(ContextSearch {
        index: ContextSearchIndex::open(&directory, loaded.normalizations())
            .map_err(Refusal::of_index)?,
        graph: LadybugSearchGraph::new(Arc::clone(manager)),
    })
}

/// The same, for a command that may not create one.
///
/// `None` means this context has no index directory at all, which is what a context that
/// was never indexed looks like — and is a different answer from an index that is there and
/// empty. Opening would create one, so the question is asked of the filesystem first.
pub(super) fn open_existing(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    manager: &Arc<LadybugDatabaseManager>,
) -> Result<Option<ContextSearch>, String> {
    let directory = directory_of(workspace)?;
    if !directory.exists() {
        return Ok(None);
    }
    Ok(Some(ContextSearch {
        index: ContextSearchIndex::open_existing(&directory, loaded.normalizations())
            .map_err(|error| error.to_string())?,
        graph: LadybugSearchGraph::new(Arc::clone(manager)),
    }))
}

/// The same, for a rebuild, which may replace an index it cannot open.
pub(super) fn open_for_rebuild(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    manager: &Arc<LadybugDatabaseManager>,
) -> Result<ContextSearch, String> {
    let directory = directory_of(workspace)?;
    Ok(ContextSearch {
        index: ContextSearchIndex::open_for_rebuild(&directory, loaded.normalizations())
            .map_err(|error| error.to_string())?,
        graph: LadybugSearchGraph::new(Arc::clone(manager)),
    })
}

fn directory_of(workspace: &WorkspaceName) -> Result<std::path::PathBuf, String> {
    let data = data_directory().map_err(|error| error.to_string())?;
    Ok(search_directory(&data, workspace))
}

impl ContextSearch {
    /// Says so when this index was not written by the rules this build applies.
    ///
    /// An index that cannot say what it was written by is not reported on here: every
    /// command that opens one has already had that failure handed to it, and a second
    /// sentence about it would be this layer saying one thing twice.
    pub(super) fn warn_if_written_by_other_rules(&self) {
        if self
            .index
            .state()
            .is_ok_and(|state| !state.matches_this_build())
        {
            eprintln!("{WRITTEN_BY_OTHER_RULES}");
        }
    }

    /// Keeping this context's index up to date while a scan publishes to its graph.
    ///
    /// A run over an index that is already behind the graph publishes everything it reads,
    /// which is what repairs it without anybody asking for a rebuild. A run over an index in
    /// step publishes only the files whose current version changed, because republishing the
    /// rest would delete and re-add text the index already holds.
    pub(super) fn update(&self) -> SearchIndexUpdate<'_> {
        SearchIndexUpdate::for_run(&self.index, &self.graph)
    }
}

/// Which paths a result of this run may name: the ones this context covers, minus the ones
/// a rule of its policy hides.
///
/// Both halves are asked now rather than when the document was written. A path that has
/// since been excluded, or that a later configuration gave to another context, is not a
/// path this run may print, whatever the index remembers about it.
///
/// Hidden means a rule says so. A path no rule matches is not hidden: the scan that
/// indexed it was granted its own root, and a policy file does not have to carry a
/// standing `include` for every directory anybody ever indexed. That is the difference
/// between this question and the one a scan asks, where an unmatched path is refused
/// because nothing has granted it anything yet.
pub(super) fn visible_paths<'a>(
    loaded: &'a LoadedIndexPolicy,
    workspace: &'a WorkspaceName,
) -> impl Fn(&NormalizedPath) -> bool + 'a {
    move |path: &NormalizedPath| {
        if loaded.workspaces().workspace_of(path) != Some(workspace) {
            return false;
        }
        let decision = loaded.policy().decide(path);
        decision.class() != PolicyClass::Exclude || decision.rule().is_none()
    }
}

/// Which revisions the queue may still spend anything on: the paths this run may name, and
/// of those, the ones an analysis rule covers.
///
/// Two questions, asked as one, because the worker acts on one answer. A job whose file the
/// policy no longer admits and a job whose revision nobody asked to analyse are both work
/// this installation is not asking for, and stage 21 already gave that one outcome —
/// `skipped`, said once, with no model called. Stage 23 adds the second reason to the same
/// question rather than a second state to the queue.
pub(super) fn wanted_paths<'a>(
    loaded: &'a LoadedIndexPolicy,
    workspace: &'a WorkspaceName,
) -> impl Fn(&NormalizedPath) -> bool + 'a {
    let visible = visible_paths(loaded, workspace);
    move |path: &NormalizedPath| visible(path) && loaded.analysis_rules().cover(path)
}

/// Reports, on stderr, that the search index was not kept up to date, and what to do.
///
/// The scan itself succeeded — the graph holds everything it read — so this never changes
/// the exit status. What it costs is an index behind the graph, which is exactly what
/// `doctor` counts and a rebuild repairs.
pub(super) fn report_search_failures(failures: &[SearchError]) {
    for line in search_failure_lines(failures) {
        eprintln!("{line}");
    }
}

/// The same report as lines, for a watcher whose lines go into a journal.
pub(super) fn search_failure_lines(failures: &[SearchError]) -> Vec<String> {
    if failures.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = failures
        .iter()
        .map(|failure| format!("Search index not updated: {failure}"))
        .collect();
    lines.push(
        "The graph holds everything this pass read. The search index is behind it: `crosskit doctor` shows by how much and `crosskit index rebuild --search` repairs it."
            .to_string(),
    );
    lines
}
