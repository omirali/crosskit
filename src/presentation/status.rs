// `crosskit status`: every context at once, and nothing written anywhere.
//
// It reports, so it may not repair, migrate or create — and on this program those are not
// choices made here but consequences of how a database is opened. Opening one for writing
// creates the database of a context nobody has indexed and migrates one an older build
// wrote; so this command never opens one for writing, and a context whose schema is not
// this build's is a line rather than a migration.
//
// The state of each context is read from its files before anything is opened at all, which
// is what keeps the risky openings from happening: a graph left by a stopped pass is named
// from the log lying beside it and the absence of the process that was writing it, not by
// asking the engine — which on that database sometimes dies instead of answering.
use std::sync::Arc;

use crate::application::{
    ContextStatusReport, ContextStatusService, InsightQueueSummary, RootCoverageReader,
    SearchIndexReport, SearchIndexService,
};
use crate::domain::{ContextRoot, WorkspaceName};
use crate::infrastructure::{
    GraphPresence, LadybugDatabaseManager, LoadedIndexPolicy, context_free_graph,
    context_free_graph_reason, graph_presence_of, load_index_policy, utc_stamp,
};

use super::help;
use super::policy::policy_file_label;
use super::root_report::{self, RootState};
use super::search_index;
use super::{format_number, workspace};

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] status";

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    if !arguments.is_empty() {
        return Err(help::misuse(USAGE));
    }
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select_all(&loaded, context)?;

    println!("Policy file: {}", policy_file_label(&loaded));
    if selected.is_empty() {
        println!("Contexts: none configured.");
        println!(
            "A path no context covers is not indexed. `crosskit setup <directory>` proposes \
             contexts for a directory, shows the configuration it would write and indexes \
             what you accept."
        );
        return Ok(());
    }

    println!("Contexts: {}", selected.len());
    if report_context_free_graph(&selected) {
        return Ok(());
    }
    for workspace in &selected {
        println!();
        report(&loaded, workspace);
    }
    Ok(())
}

/// A graph from before contexts, said once, before the list. `true` means it was there and
/// there is nothing further to report.
///
/// This condition belongs to the data directory and not to any context: every context
/// derives its path from that directory, so one such file stops all of them for the same
/// reason, and none of them is the thing that is wrong. Asked per context it printed the
/// same paragraph once per context — four of them on the owner's machine — and left no room
/// for a word about the four contexts themselves, which were in order.
///
/// The rule that every context is reported however broken it is holds: it is about
/// contexts, and this is not about a context. Nothing below can be read while the file is
/// there, so what is left to say is which contexts are configured, and that is said.
fn report_context_free_graph(selected: &[WorkspaceName]) -> bool {
    // A data directory this command cannot even name is the ordinary path's problem, and it
    // says so per context with the rest of what it knows.
    let Ok(Some(legacy)) = context_free_graph() else {
        return false;
    };
    println!();
    println!("{}", context_free_graph_reason(&legacy));
    println!("Until then nothing below opens, and nothing below has been touched.");
    println!();
    for workspace in selected {
        println!("{workspace}");
    }
    true
}

/// One context, whatever state it is in.
///
/// Nothing here fails the command. A context that cannot be read is the finding, not an
/// accident of the reporting, and a report over several of them that stopped at the first
/// broken one would hide the others — which is the state this command exists to show.
fn report(loaded: &LoadedIndexPolicy, workspace: &WorkspaceName) {
    println!("{workspace}");
    let presence = match graph_presence_of(workspace) {
        Ok(presence) => presence,
        Err(error) => {
            println!(
                "  State: unknown ({})",
                workspace::explain(workspace, error)
            );
            return;
        }
    };
    match presence {
        GraphPresence::NeverIndexed => {
            println!("  State: not indexed yet");
            println!("  Next: {}.", workspace::index_init_repair(workspace));
        }
        GraphPresence::StoppedUncleanly { log } => {
            println!("  State: a pass was stopped without closing this context");
            println!(
                "  Its write-ahead log is still beside its graph, and the storage engine may \
                 refuse to replay it, which would keep every command out of this context. \
                 What the interrupted pass wrote since the last checkpoint does not come back."
            );
            println!("  Log: {}", log.display());
            println!(
                "  Next: `crosskit --context {workspace} index recover --check` opens it and \
                 says whether it still opens; `crosskit --context {workspace} index recover` \
                 moves that log aside and keeps it."
            );
        }
        GraphPresence::BeingWritten { pid } => {
            println!("  State: being indexed right now (process {pid})");
            println!(
                "  What that pass has written is in a log only it can replay, so nothing of \
                 this context is readable until the pass ends. Nothing is wrong: ask again \
                 when it is done."
            );
        }
        GraphPresence::Undetermined { reason } => {
            println!("  State: could not be determined ({reason})");
            println!(
                "  Nothing was opened, because which of the states this is decides whether \
                 opening it is safe."
            );
        }
        GraphPresence::Ready => describe_graph(loaded, workspace, None),
        GraphPresence::HeldByWriter { pid } => describe_graph(loaded, workspace, Some(pid)),
    }
}

/// What the graph holds, read through an opening that may not write to it.
fn describe_graph(loaded: &LoadedIndexPolicy, workspace: &WorkspaceName, writer: Option<i32>) {
    let manager = match LadybugDatabaseManager::open_workspace_read_only(workspace) {
        Ok(manager) => manager,
        Err(error) => {
            println!("  State: did not open");
            println!("  {}", workspace::explain(workspace, error));
            return;
        }
    };
    match writer {
        None => println!("  State: indexed"),
        Some(pid) => println!(
            "  State: indexed, and held between passes by a writer (process {pid}); the counts \
             below are what that writer has folded into the database"
        ),
    }
    println!("  Database: {}", manager.database_path().display());

    let repository = manager.repository();
    match ContextStatusService::new(&repository).execute() {
        Ok(report) => print_report(&report),
        Err(error) => println!("  Contents: unavailable ({error})"),
    }
    print_roots(loaded, workspace, &repository, writer);
    print_search_index(loaded, workspace, &manager);
}

fn print_report(report: &ContextStatusReport) {
    println!("  Schema version: {}", report.schema_version());
    println!(
        "  Files: {} ({} revisions kept)",
        format_number(report.inventory().files()),
        format_number(report.inventory().revisions())
    );
    println!("  Insight jobs: {}", describe_jobs(&report.jobs()));
}

/// Every root of this context, and what the graph holds under it.
///
/// This is the line that would have made Stage 34 impossible. A context is a list of roots
/// and a pass walks one of them, so a context of ten roots with one indexed and a context
/// indexed whole both printed `Files: 3,998` and nothing else — for fifteen stages, while
/// eight of those ten roots had never been walked at all.
///
/// Read from the locations rather than from the run records: `index init` writes no run, so
/// a root it indexed would otherwise look like a root nobody had walked.
fn print_roots(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    repository: &impl RootCoverageReader,
    writer: Option<i32>,
) {
    let roots = loaded.workspaces().roots_of(workspace);
    if roots.is_empty() {
        println!("  Roots: none — this context covers nothing, so nothing is indexed into it");
        return;
    }
    println!("  Roots: {}", roots.len());
    let never_walked = roots
        .iter()
        .filter(|root| print_root(root, repository))
        .count();
    // Said where the roots are, because it is a statement about all of them at once: after
    // Stage 36 a watcher watches its whole context, so a root under a running watcher and a
    // root beside it are covered by the same passes. Which of them the backend took hints
    // for is the watcher's own opening line — it changes the latency of a change, not
    // whether the root is in the index.
    if let Some(pid) = writer {
        println!(
            "    All {} of these roots are covered by the pass a watcher runs (process \
             {pid}); what each root last recorded is above.",
            roots.len()
        );
    }
    if never_walked == 0 {
        return;
    }
    // Said once for the context and not under each root: eight roots of one of the owner's
    // contexts hold nothing, and eight copies of the same sentence would bury the eight
    // lines that matter.
    println!(
        "  {never_walked} of these root(s) hold nothing. `crosskit --context {workspace} index \
         init` walks every root of this context, and a root that fails no longer stops the \
         others."
    );
}

/// One root. Returns whether it has never been walked, so the context can say once how many
/// of them there are and what to run about it.
fn print_root(root: &ContextRoot, repository: &impl RootCoverageReader) -> bool {
    match root_report::state_of(root, repository) {
        RootState::NotADirectory => {
            println!(
                "    {}: a pattern, not a directory — no pass can start here",
                root.as_str()
            );
            true
        }
        RootState::Unanswered(reason) => {
            println!("    {}: {reason}", root.as_str());
            false
        }
        RootState::Covered(path, coverage) if !coverage.ever_walked() => {
            println!("    {path}: NEVER WALKED — holds nothing");
            if let Some(failure) = coverage.last_failure() {
                println!(
                    "      The last attempt failed {}",
                    root_report::describe_failure(failure)
                );
            }
            true
        }
        RootState::Covered(path, coverage) => {
            println!(
                "    {path}: {} file(s){}",
                format_number(coverage.files()),
                match coverage.last_seen_at_unix_ms() {
                    Some(at) => format!(", last seen {}", utc_stamp(at)),
                    None => String::new(),
                }
            );
            if let Some(failure) = coverage.last_failure() {
                println!(
                    "      A later attempt failed {}, so what is above may be older than the \
                     tree",
                    root_report::describe_failure(failure)
                );
            }
            false
        }
    }
}

/// Every state with its count, on one line and in the order the queue declares them. A
/// state with no job is printed as zero rather than left out, so two contexts are read
/// against each other without counting the words.
fn describe_jobs(summary: &InsightQueueSummary) -> String {
    summary
        .counts()
        .map(|(state, count)| format!("{} {}", state.label(), format_number(count as u64)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The search index of this context against the graph it is derived from: the same two
/// failures `doctor` reports, because they are invisible from the results themselves.
fn print_search_index(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    manager: &Arc<LadybugDatabaseManager>,
) {
    let report = search_index::open_existing(loaded, workspace, manager).and_then(|opened| {
        opened
            .map(|search| {
                SearchIndexService::new(&search.index, &search.graph)
                    .status()
                    .map_err(|error| error.to_string())
            })
            .transpose()
    });
    match report {
        Ok(Some(report)) => print_search_status(&report, workspace),
        Ok(None) => {
            println!("  Search index: none built yet");
            println!("  Next: {}", rebuild(workspace));
        }
        // As in `doctor`: the failure keeps the words it was raised with.
        Err(error) => println!("  Search index: {error}"),
    }
}

/// Both answers are printed whichever way they come out: "in step" and "the analyzer this
/// build reads with" are findings too, and a report that only speaks up when something is
/// wrong leaves a reader unable to tell a healthy context from one it forgot to check.
///
/// Which fingerprint the index carries is `doctor`'s line, not this one. Over several
/// contexts the verdict is what an operator acts on; the text of it is one command away.
fn print_search_status(report: &SearchIndexReport, workspace: &WorkspaceName) {
    println!(
        "  Search index: {} documents against {} chunks of current versions ({})",
        format_number(report.documents()),
        format_number(report.chunks()),
        if report.counts_agree() {
            "in step with the graph".to_string()
        } else {
            format!("behind the graph — {}", rebuild(workspace))
        }
    );
    match report.state().written_by() {
        None => println!("  Search index written by: nothing written yet"),
        Some(_) if report.matches_this_build() => {
            println!("  Search index written by: the rules this build reads with");
        }
        Some(_) => println!(
            "  Search index written by: not the rules this build reads with — {}",
            rebuild(workspace)
        ),
    }
}

/// The one repair for both halves of search being out of step, spelled once.
fn rebuild(workspace: &WorkspaceName) -> String {
    format!("`crosskit --context {workspace} index rebuild --search` writes it from the graph")
}
