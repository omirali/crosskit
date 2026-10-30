// A pass over every root of one context.
//
// A context is a list of `include` directories and a pass is a walk of one of them. Until
// Stage 35 nothing joined the two except `setup`, which looped over the includes with `?` —
// so the first root that failed ended the context and the eight behind it were never walked.
// That is how eight of the owner's ten roots in one context went unindexed for fifteen
// stages (`docs/coverage-audit.md`).
//
// The loop therefore lives here, once, and `index init`, `index reconcile` and `setup` all
// go through it. Its three rules:
//
//   * a root that fails does not end the others;
//   * every root is named in the report, whatever became of it;
//   * a pass that did not happen is written into the graph, so "why is this root empty" has
//     an answer after the terminal is closed.
use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

use crate::application::{
    ContextPassReport, IndexError, InitialScanService, PassAttemptLog, PassFailure,
    ReconcileCompletion, ReconcileService, RootOutcome, ScanReport, SearchError, SearchIndexUpdate,
    SourceWalker,
};
use crate::domain::{ContextRoot, NormalizedPath, WorkspaceName};
use crate::infrastructure::ladybug::LadybugGraphRepository;
use crate::infrastructure::{DescriptorRelativeWalker, LadybugDatabaseManager, LoadedIndexPolicy};

use super::context_wording::counted;
use super::index::describe_reasons;
use super::policy::policy_file_label;
use super::search_index::{self, ContextSearch, report_search_failures};
use super::{format_number, workspace};

/// Which pass a run is making over each root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pass {
    /// Publishes what it reads and retires nothing.
    Initial,
    /// Walks again and retires what a complete walk did not see.
    Reconcile,
}

/// One context, opened and ready to walk.
///
/// Preparing and walking are two calls because everything that can refuse the whole run
/// happens in the first: a run that is refused must print nothing at all, and the caller
/// prints its own header between the two. While that was one call with the header inside
/// it, the header was either printed before a refusal or printed once per context by
/// `setup`, which says the same path five times on the owner's machine.
pub(super) struct ContextPass<'a> {
    loaded: &'a LoadedIndexPolicy,
    workspace: &'a WorkspaceName,
    roots: Vec<ContextRoot>,
    manager: Arc<LadybugDatabaseManager>,
    search: ContextSearch,
    /// What stopped the search index, kept across passes. A watcher runs for hours and
    /// reports these once when it stops; a single pass reports them as it ends. Either way
    /// the list belongs to the context, not to a pass.
    search_failures: RefCell<Vec<SearchError>>,
}

/// Whether a pass says what each root did, or only hands back the report.
///
/// A single `index init` prints the whole scan of every root, which is what ten separate
/// runs printed before. A watcher may not: it runs a pass every few seconds for hours, and
/// fifteen lines per root would bury the one line per pass its output exists to be. And
/// `setup` indexes every root of every context at once — twenty-three on the owner's machine —
/// so it says one line per root and leaves the full scan to `index init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Narration {
    PerRoot,
    OneLinePerRoot,
    Silent,
}

/// Resolves what this run will walk and opens what it will write to, without printing.
///
/// The graph and the search index are opened once, because they belong to the context and
/// not to a root: if either refuses, no root is walked and that refusal is the answer for
/// all of them. That is the one case the old `?` in `setup` was right about, and it is now
/// the only case where one failure speaks for the whole context.
pub(super) fn prepare<'a>(
    loaded: &'a LoadedIndexPolicy,
    workspace: &'a WorkspaceName,
    named: Option<&Path>,
) -> Result<ContextPass<'a>, String> {
    let roots = roots_to_walk(loaded, workspace, named, &DescriptorRelativeWalker)?;
    let manager = workspace::open(workspace)?;
    let search = search_index::open(loaded, workspace, &manager)?;
    Ok(ContextPass {
        loaded,
        workspace,
        roots,
        manager,
        search,
        search_failures: RefCell::new(Vec::new()),
    })
}

impl ContextPass<'_> {
    /// The directories a watcher can subscribe to: the includes that name one.
    ///
    /// An include that is a mask names no directory, so there is nothing to register. It is
    /// still walked by every pass, which is why leaving it out here costs coverage nothing.
    pub(super) fn roots_to_watch(&self) -> Vec<NormalizedPath> {
        self.roots
            .iter()
            .filter_map(ContextRoot::directory)
            .filter_map(|directory| NormalizedPath::from_absolute(directory).ok())
            .collect()
    }

    /// The directories the roots resolve to — the spelling the filesystem's events use.
    pub(super) fn resolved_roots(&self) -> Vec<NormalizedPath> {
        self.roots_to_watch()
            .iter()
            .map(|root| {
                DescriptorRelativeWalker
                    .resolve_root(Path::new(root.as_str()))
                    .unwrap_or_else(|_| root.clone())
            })
            .collect()
    }

    /// A reconcile of these directories only, each as if it were a root: what the walk does
    /// not see under one is retired under that one alone.
    ///
    /// Quiet, like every pass a watcher makes; the failures are written into the graph as for
    /// a root.
    pub(super) fn walk_within(&self, directories: &[NormalizedPath]) -> ContextPassReport {
        let walker = DescriptorRelativeWalker;
        let update = self.search.update();
        let repository = self.manager.repository();
        let mut report = ContextPassReport::within();
        for directory in directories {
            let root = ContextRoot::Directory(directory.as_str().to_string());
            report.record(self.walk_root(
                Pass::Reconcile,
                &walker,
                &repository,
                &update,
                &root,
                Narration::Silent,
            ));
        }
        *self.search_failures.borrow_mut() = update.failures();
        report
    }

    /// The graph this pass writes to, for the work a watcher does beside its passes.
    pub(super) fn manager(&self) -> &Arc<LadybugDatabaseManager> {
        &self.manager
    }

    /// Everything that stopped the search index since this context was opened.
    pub(super) fn search_failures(&self) -> Vec<SearchError> {
        self.search_failures.borrow().clone()
    }

    /// Walks every root, one after another, and reports each one whatever became of it.
    pub(super) fn walk(&self, pass: Pass, narration: Narration) -> ContextPassReport {
        let walker = DescriptorRelativeWalker;
        let update = self.search.update();
        let repository = self.manager.repository();

        if narration == Narration::PerRoot {
            println!("Context: {}", self.workspace);
            println!("Policy file: {}", policy_file_label(self.loaded));
            println!("Roots: {}", self.roots.len());
        }

        let mut report = ContextPassReport::default();
        let mut reported_failures = 0;
        for root in &self.roots {
            if narration == Narration::PerRoot {
                println!();
                println!("{}", root.as_str());
            }
            let outcome = self.walk_root(pass, &walker, &repository, &update, root, narration);
            match narration {
                // Whatever stopped the index while this root walked — a root that never
                // started leaves no report of its own to carry it.
                Narration::PerRoot => reported_failures = render_root(&outcome, reported_failures),
                Narration::OneLinePerRoot => {
                    if let Some(scan) = scan_of(&outcome) {
                        println!(
                            "{}",
                            root_line(
                                outcome.include(),
                                scan.published_files(),
                                scan.indexed_chunks(),
                                scan.failed_paths()
                            )
                        );
                        reported_failures = report_new_search_failures(
                            scan.search_index_failures(),
                            reported_failures,
                        );
                    }
                }
                Narration::Silent => {}
            }
            report.record(outcome);
        }

        if narration != Narration::Silent {
            report_new_search_failures(&update.failures(), reported_failures);
            render_totals(&report);
        }
        *self.search_failures.borrow_mut() = update.failures();
        report
    }
}

/// Which directories this run walks: the one it was given, or every include of the context.
///
/// A directory named on the command line is still checked against the context's coverage.
/// That check is what keeps a run from writing another context's files into this graph, and
/// narrowing to one root does not weaken it.
fn roots_to_walk(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    named: Option<&Path>,
    walker: &DescriptorRelativeWalker,
) -> Result<Vec<ContextRoot>, String> {
    let Some(named) = named else {
        let roots = loaded.workspaces().roots_of(workspace);
        if roots.is_empty() {
            return Err(format!(
                "context {workspace} covers nothing, so there is nothing to walk; give it an \
                 `include` pattern in the policy file"
            ));
        }
        return Ok(roots);
    };
    let root = walker
        .resolve_root(named)
        .map_err(|error| error.to_string())?;
    workspace::require_root_in_context(loaded, workspace, &root)?;
    Ok(vec![ContextRoot::Directory(root.as_str().to_string())])
}

impl ContextPass<'_> {
    /// One root, with every failure caught: the loop above always reaches the next one.
    fn walk_root(
        &self,
        pass: Pass,
        walker: &DescriptorRelativeWalker,
        repository: &LadybugGraphRepository,
        update: &SearchIndexUpdate<'_>,
        root: &ContextRoot,
        narration: Narration,
    ) -> RootOutcome {
        let Some(directory) = root.directory() else {
            match narration {
                Narration::PerRoot => println!(
                    "  Not walked: this include is a pattern and not a directory, so there is \
                     nowhere to start. Name the directories it stands for, or index each of \
                     them by itself."
                ),
                Narration::OneLinePerRoot => println!(
                    "  {}: not walked, a pattern and not a directory",
                    root.as_str()
                ),
                Narration::Silent => {}
            }
            return RootOutcome::NotADirectory {
                include: root.as_str().to_string(),
            };
        };

        let resolved = match resolve_inside_context(self.loaded, self.workspace, walker, directory)
        {
            Ok(resolved) => resolved,
            Err(error) => return failed(repository, root.as_str(), None, error, narration),
        };
        let policy = self.loaded.policy();
        let rules = self.loaded.analysis_rules();
        let walked = match pass {
            Pass::Initial => InitialScanService::new(walker, repository, policy, rules)
                .keeping_search_index(update)
                .execute(&resolved)
                .map(|scan| RootOutcome::Indexed {
                    root: resolved.as_str().to_string(),
                    scan,
                }),
            Pass::Reconcile => ReconcileService::new(walker, repository, policy, rules)
                .keeping_search_index(update)
                .execute(&resolved)
                .map(|report| RootOutcome::Reconciled {
                    root: resolved.as_str().to_string(),
                    report: Box::new(report),
                }),
        };

        walked.unwrap_or_else(|error| {
            failed(repository, root.as_str(), Some(&resolved), error, narration)
        })
    }
}

fn resolve_inside_context(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    walker: &DescriptorRelativeWalker,
    directory: &str,
) -> Result<NormalizedPath, IndexError> {
    let root = walker.resolve_root(Path::new(directory))?;
    workspace::require_root_in_context(loaded, workspace, &root).map_err(IndexError::Source)?;
    Ok(root)
}

/// Says what stopped this root and writes the attempt into the graph.
fn failed(
    repository: &LadybugGraphRepository,
    include: &str,
    resolved: Option<&NormalizedPath>,
    error: IndexError,
    narration: Narration,
) -> RootOutcome {
    let category = PassFailure::of_index_error(&error);
    // The attempt is written down whether or not anybody is being told about it: a watcher
    // says nothing per root, and the question "why did this root stop" is asked afterwards.
    let recorded = resolved.map(|root| repository.record_failed_pass(root, category));
    if narration == Narration::OneLinePerRoot {
        println!("  {include}: not walked, {category}: {error}");
    }
    if narration == Narration::PerRoot {
        println!("  Not walked: {error}");
        println!("  Category: {category}");
        match recorded {
            Some(Ok(id)) => println!("  Attempt recorded in the graph: {id}"),
            Some(Err(refusal)) => println!("  The attempt could not be recorded either: {refusal}"),
            // No resolved path, so there is nothing to record the attempt under. A record
            // naming a path this run never reached would be worse than no record at all.
            None => println!("  Not recorded: this include did not resolve to a directory"),
        }
    }
    RootOutcome::Failed {
        include: include.to_string(),
        category,
    }
}

/// What one root's pass did, in full and indented under the root it belongs to.
///
/// The whole scan report and not a summary: these counters are how a person finds out that
/// a tree was read but nothing came of it — excluded paths, generated directories, sources
/// with no creation time. A pass over ten roots prints ten of these, which is exactly what
/// ten separate `index init` runs printed before, in one place and with the roots named.
///
/// Excluded entries are only counted: the report never names a path the policy hid.
fn render_root(outcome: &RootOutcome, reported_failures: usize) -> usize {
    let Some(scan) = scan_of(outcome) else {
        return reported_failures;
    };
    // Said only when it differs, and then it matters: a root reached through a symbolic link
    // is walked at the directory the link resolves to, and that is the path everything below
    // is about.
    if scan.root().as_str() != outcome.include() {
        println!("  Resolved to: {}", scan.root());
    }
    if let RootOutcome::Reconciled { report, .. } = outcome {
        println!("  Run: {}", report.run_id());
    }
    println!(
        "  Indexed files (metadata and content): {}",
        format_number(scan.indexed_files() as u64)
    );
    println!(
        "  Indexed chunks: {}",
        format_number(scan.indexed_chunks() as u64)
    );
    println!(
        "  Metadata-only files (policy): {}",
        scan.metadata_only_files()
    );
    println!(
        "  Metadata-only files (content skipped): {}",
        scan.content_skipped_files()
    );
    println!(
        "  Metadata-only files (larger than the content limit): {}",
        scan.oversize_files()
    );
    println!(
        "  Files with no text in them (published, nothing to index): {}",
        scan.no_text_files()
    );
    println!(
        "  Unstable sources (changed during the scan, not published): {}",
        scan.unstable_sources()
    );
    println!(
        "  Sources without a creation time (a reused inode number is indistinguishable): {}",
        scan.undated_sources()
    );
    println!(
        "  Excluded paths: {}",
        format_number(scan.excluded_paths() as u64)
    );
    println!(
        "  Generated directories skipped whole (marker): {}",
        scan.generated_directories()
    );
    println!(
        "  Skipped entries with non-UTF-8 names: {}",
        scan.unrepresentable_names()
    );
    println!("  Failed paths: {}", scan.failed_paths());
    for failure in scan.failures() {
        eprintln!("Index failure: {failure}");
    }
    if let RootOutcome::Reconciled { report, .. } = outcome {
        match report.completion() {
            ReconcileCompletion::Completed {
                marked_missing,
                relocated,
            } => println!(
                "  Locations marked missing: {marked_missing} ({relocated} relocated, the same \
                 file identity remains present)"
            ),
            ReconcileCompletion::Incomplete(reasons) => println!(
                "  Incomplete, so nothing here was retired: {}",
                describe_reasons(reasons)
            ),
        }
    }
    report_new_search_failures(scan.search_index_failures(), reported_failures)
}

/// One root in one line: what it published, and the paths it could not read only when there
/// were any. The full scan of the root is what `index init` prints.
fn root_line(include: &str, files: usize, chunks: usize, failed: usize) -> String {
    let mut line = format!(
        "  {include}: {}, {}",
        counted(files as u64, "file"),
        counted(chunks as u64, "chunk")
    );
    if failed > 0 {
        line.push_str(&format!(", {} not read", counted(failed as u64, "path")));
    }
    line
}

/// What one root's walk counted, for the outcomes that walked one.
fn scan_of(outcome: &RootOutcome) -> Option<&ScanReport> {
    match outcome {
        RootOutcome::Indexed { scan, .. } => Some(scan),
        RootOutcome::Reconciled { report, .. } => Some(report.scan()),
        RootOutcome::NotADirectory { .. } | RootOutcome::Failed { .. } => None,
    }
}

/// The search index failures this pass has not printed yet, and how many have been printed
/// once it returns.
///
/// One update writes the index of the whole context, and its list of failures only grows, so
/// every root's report carries everything that went wrong since the pass began. Printing the
/// whole list under each root would say the same thing ten times; printing the tail says
/// which root was being walked when the index fell behind.
fn report_new_search_failures(failures: &[SearchError], already_reported: usize) -> usize {
    report_search_failures(&failures[already_reported.min(failures.len())..]);
    failures.len()
}

fn render_totals(report: &ContextPassReport) {
    println!();
    println!("Roots walked: {} of {}", report.walked(), report.roots());
    println!(
        "Files indexed: {}",
        format_number(report.published_files() as u64)
    );
    println!(
        "Chunks indexed: {}",
        format_number(report.indexed_chunks() as u64)
    );
}

/// How the run ends when part of a context was walked and part was not.
///
/// A pass that walked some of a context did not do what was asked, so the run fails — that
/// much `Context X was not indexed` had right. What it had wrong is everything else: it
/// named no root, and it said the context was not indexed while the context was indexed
/// halfway, which sent whoever read it to look at a database that was working.
pub(super) fn outcome_of(
    report: &ContextPassReport,
    workspace: &WorkspaceName,
) -> Result<(), String> {
    if report.whole_context() {
        return Ok(());
    }
    let not_walked = report.not_walked();
    Err(format!(
        "context {workspace}: {} of {} root(s) not walked — {}. The other {} were walked and \
         what they published is in the graph; each root above says what stopped it.",
        not_walked.len(),
        report.roots(),
        not_walked.join(", "),
        report.walked()
    ))
}

#[cfg(test)]
mod tests {
    use super::root_line;

    /// One line a root, and the paths that could not be read are said only when there are some.
    #[test]
    fn a_root_is_one_line_and_names_failures_only_when_there_are_any() {
        let quiet = root_line("/Users/alice/Projects/alpha", 412, 1308, 0);
        assert_eq!(
            quiet,
            "  /Users/alice/Projects/alpha: 412 files, 1,308 chunks"
        );
        assert_eq!(quiet.lines().count(), 1);

        assert_eq!(
            root_line("/Users/alice/Documents", 1, 1, 2),
            "  /Users/alice/Documents: 1 file, 1 chunk, 2 paths not read"
        );
    }
}
