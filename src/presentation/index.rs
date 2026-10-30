use std::path::Path;

use crate::application::{IncompleteReason, SearchIndexService};
use crate::infrastructure::load_index_policy;

use super::help;
use super::index_context::{self, Narration, Pass};
use super::index_review;
use super::search_index;
use super::{index_jobs, index_recover, index_runs, index_watch, workspace};

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] index <init|reconcile> [<directory>]\n       crosskit [--context <name>] index watch [<directory>]\n       crosskit [--context <name>] index review [approve <mask>]\n       crosskit [--context <name>] index rebuild --search\n       crosskit [--context <name>] index recover\n       crosskit [--context <name>] index runs prune --keep <N> [--apply]\n       crosskit [--context <name>] index jobs\n       crosskit [--context <name>] index jobs run --limit <N>\n       crosskit [--context <name>] index jobs withdraw";

/// Only the search index is rebuilt. The graph is the source of truth and there is nothing
/// to rebuild it from; the index is a derivative of it and can always be written again.
const REBUILD_USAGE: &str = "Usage: crosskit [--context <name>] index rebuild --search\n       The search index is the only thing a rebuild writes: the graph is the source of truth it is written from.";

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    match arguments {
        [command] if command == "init" => run_pass(Pass::Initial, None, context),
        [command, root] if command == "init" => {
            run_pass(Pass::Initial, Some(Path::new(root)), context)
        }
        [command] if command == "reconcile" => run_pass(Pass::Reconcile, None, context),
        [command, root] if command == "reconcile" => {
            run_pass(Pass::Reconcile, Some(Path::new(root)), context)
        }
        [command, rest @ ..] if command == "review" => index_review::run(rest, context),
        [command] if command == "watch" => index_watch::run(None, context),
        [command, root] if command == "watch" => index_watch::run(Some(Path::new(root)), context),
        [command, rest @ ..] if command == "rebuild" => run_rebuild(rest, context),
        [command, rest @ ..] if command == "recover" => index_recover::run(rest, context),
        [command, rest @ ..] if command == "runs" => index_runs::run(rest, context),
        [command, rest @ ..] if command == "jobs" => index_jobs::run(rest, context),
        _ => Err(help::misuse(USAGE)),
    }
}

/// One pass over a context, or over one directory of it.
///
/// With no directory it walks every root the context declares; a directory narrows it to
/// that one. The narrowing is the option and the whole context is the default, because a
/// context is what the configuration declares and a directory is what somebody happened to
/// type — and for fifteen stages the second was the only thing this command could be given.
fn run_pass(pass: Pass, named: Option<&Path>, context: Option<&str>) -> Result<(), String> {
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let workspace = workspace::select(&loaded, context)?;
    let prepared = index_context::prepare(&loaded, &workspace, named)?;
    index_context::outcome_of(&prepared.walk(pass, Narration::PerRoot), &workspace)
}

/// Writes the search index of this context again from the graph.
///
/// It reads the graph and writes nothing to it, so it is safe to run at any time; what it
/// costs is one pass over the chunks of the current versions.
fn run_rebuild(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    if arguments != ["--search"] {
        return Err(help::misuse(REBUILD_USAGE));
    }
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let workspace = workspace::select(&loaded, context)?;
    let manager = workspace::open(&workspace)?;
    let search = search_index::open_for_rebuild(&loaded, &workspace, &manager)?;
    let report = SearchIndexService::new(&search.index, &search.graph)
        .rebuild()
        .map_err(|error| error.to_string())?;

    println!("Search index rebuilt.");
    println!("Context: {workspace}");
    println!("Indexed documents: {}", report.documents());
    println!("Chunks of current versions: {}", report.chunks());
    println!("Written by: {}", report.state().writes_with());
    Ok(())
}

pub(super) fn describe_reasons(reasons: &[IncompleteReason]) -> String {
    reasons
        .iter()
        .map(|reason| match reason {
            IncompleteReason::FailedPaths(count) => format!("{count} failed path(s)"),
            IncompleteReason::UnstableSources(count) => format!("{count} unstable source(s)"),
            IncompleteReason::UnrepresentableNames(count) => {
                format!("{count} entry(ies) with non-UTF-8 names")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
