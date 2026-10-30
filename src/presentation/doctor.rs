use std::sync::Arc;

use crate::application::{
    DoctorService, RootCoverageReader, SearchIndexReport, SearchIndexService,
};
use crate::domain::WorkspaceName;
use crate::infrastructure::{
    GraphPresence, LadybugDatabaseManager, LoadedIndexPolicy, graph_presence_of, load_index_policy,
};

use super::root_report::{self, RootState};
use super::search_index;
use super::workspace;

/// A binary built for the batch limit benchmark states the limits it was compiled with,
/// so the benchmark reports what actually ran instead of what it was asked to print. A
/// released Crosskit prints nothing here.
#[cfg(feature = "batch-limit-benchmark")]
fn print_batch_limits() {
    let limits = crate::application::SourceBatchLimits::production();
    println!(
        "Batch limits: {} sources, {} payload bytes",
        limits.sources(),
        limits.payload_bytes()
    );
}

#[cfg(not(feature = "batch-limit-benchmark"))]
fn print_batch_limits() {}

/// The LadybugDB buffer pool size and thread count this binary was built with. A released
/// Crosskit names both as `auto` because it leaves both to the engine; a binary built for
/// the engine tuning benchmark names what it compiled in, so a measured run cannot be
/// labelled with a configuration it did not use.
fn print_engine_tuning() {
    println!(
        "Engine tuning: {}",
        crate::infrastructure::ladybug::EngineTuning::production().describe()
    );
}

/// Whether a new file that takes a deleted file's inode number can be told apart from it
/// here. Where the platform gives no creation time the protection is simply off, and a
/// reader is told so rather than left to assume it is on. How many sources of a given scan
/// were affected is a counter of that scan's report.
fn print_file_identity() {
    if crate::infrastructure::reads_creation_times() {
        println!(
            "File identity: creation times are read, so a file that takes a deleted file's inode number is published as a different file; each scan counts the sources whose filesystem recorded none."
        );
        return;
    }
    println!(
        "File identity: creation times are not read on this platform, so a new file that takes a deleted file's inode number inherits its history."
    );
}

pub fn run(context: Option<&str>) -> Result<(), String> {
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select(&loaded, context)?;
    let manager = workspace::open_for_reading(&selected)?;
    let repository = manager.repository();
    let report = DoctorService
        .execute(&repository)
        .map_err(|error| error.to_string())?;

    println!("Crosskit storage is ready.");
    println!("Context: {selected}");
    println!(
        "Database: {}{}",
        manager.database_path().display(),
        writer_note(&selected)
    );
    println!("Schema version: {}", report.schema_version());
    println!("LadybugDB crate: {}", lbug::VERSION);
    println!("LadybugDB storage: {}", lbug::get_storage_version());
    print_batch_limits();
    print_engine_tuning();
    print_file_identity();
    print_coverage(&loaded, &selected, &repository);
    print_search_index(&loaded, &selected, &manager);
    Ok(())
}

/// Which roots of this context hold nothing, and what the last attempt on them was.
///
/// `status` prints every root of every context, because coverage is something to see every
/// day. `doctor` prints only the roots that are a problem, because it is read when something
/// is already wrong — and a root that was never walked is the answer to "why does search say
/// nothing about this repository", which is the question Stage 34 could not answer from any
/// command the program had.
fn print_coverage(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    repository: &impl RootCoverageReader,
) {
    let roots = loaded.workspaces().roots_of(workspace);
    let mut unwalked = Vec::new();
    for root in &roots {
        match root_report::state_of(root, repository) {
            RootState::Covered(_, coverage) if coverage.ever_walked() => {}
            RootState::NotADirectory => {
                unwalked.push(format!("{}: a pattern, not a directory", root.as_str()));
            }
            RootState::Unanswered(reason) => {
                unwalked.push(format!("{}: {reason}", root.as_str()));
            }
            RootState::Covered(path, coverage) => unwalked.push(match coverage.last_failure() {
                Some(failure) => format!(
                    "{path}: never walked; the last attempt failed {}",
                    root_report::describe_failure(failure)
                ),
                None => format!("{path}: never walked"),
            }),
        }
    }

    if unwalked.is_empty() {
        println!(
            "Coverage: all {} root(s) of this context have been walked",
            roots.len()
        );
        return;
    }
    println!(
        "Coverage: {} of {} root(s) of this context hold nothing",
        unwalked.len(),
        roots.len()
    );
    for line in &unwalked {
        println!("  {line}");
    }
    println!(
        "  `crosskit --context {workspace} index init` walks every root of this context; a \
         root that fails no longer stops the others."
    );
}

/// Whether somebody else is writing to this database. Said on the line that names it,
/// because every count below is then a snapshot of what that writer had committed when this
/// command opened it, not of what the context holds now.
///
/// Asked of the files, the way `status` asks it. It used to be read off the door this
/// command came in by — and since `open_for_reading` is read-only *always* (ADR 0020, 0034),
/// that door said "another process is writing to this context" on every single run, whether
/// or not one was. A report whose subject is whether things are in order may not open with a
/// sentence that is usually false.
fn writer_note(workspace: &WorkspaceName) -> String {
    match graph_presence_of(workspace) {
        Ok(GraphPresence::BeingWritten { pid } | GraphPresence::HeldByWriter { pid }) => format!(
            " (another process is writing to this context, {pid}; the counts below are what it \
             had folded in when this command opened the database)"
        ),
        _ => String::new(),
    }
}

/// What the search index holds against what the graph holds, and by which rules it was
/// written.
///
/// Both are reported, because they are two different ways for search to answer wrongly and
/// they have one repair. An index behind the graph is missing recent text; an index written
/// by other rules holds what it has in a form this build's queries do not reach — terms
/// another analyzer produced, or a field a query filters on that its documents were written
/// without. Neither is visible from the results themselves, which is the whole reason these
/// lines exist.
fn print_search_index(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    manager: &Arc<LadybugDatabaseManager>,
) {
    let report = search_index::open(loaded, workspace, manager).and_then(|search| {
        SearchIndexService::new(&search.index, &search.graph)
            .status()
            .map_err(|error| error.to_string())
    });
    match report {
        Ok(report) => print_search_status(&report),
        // Doctor reports; it does not fail because one of the things it reports on is
        // unreachable. The failure is printed as it was worded, without a second word of
        // this layer's own in front of it: an index written with other fields is there and
        // readable by the build that wrote it, and calling that "unavailable" would send a
        // reader looking for a directory that is not missing.
        Err(error) => println!("Search index: {error}"),
    }
}

fn print_search_status(report: &SearchIndexReport) {
    println!(
        "Search index: {} documents against {} chunks of current versions ({})",
        report.documents(),
        report.chunks(),
        if report.counts_agree() {
            "in step with the graph"
        } else {
            "behind the graph; rebuild with `crosskit index rebuild --search`"
        }
    );
    let state = report.state();
    match state.written_by() {
        Some(written_by) if !report.matches_this_build() => println!(
            "Search index written by: this build writes {}, the index was written by {}; rebuild with `crosskit index rebuild --search`",
            state.writes_with(),
            written_by
        ),
        Some(written_by) => println!("Search index written by: {written_by}"),
        None => println!(
            "Search index written by: {} (nothing written yet)",
            state.writes_with()
        ),
    }
}
