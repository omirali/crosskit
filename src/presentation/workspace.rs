// How a command is told which context it works in, and what that context allows it to do.
// Everything here happens before a database is opened: a run that cannot say which context
// it belongs to must not touch one.
use std::sync::Arc;

use crate::application::RepositoryError;
use crate::domain::{NormalizedPath, WorkspaceName};
use crate::infrastructure::{
    GraphPresence, LadybugDatabaseManager, LadybugGraphMaintenance, LoadedIndexPolicy,
    graph_presence_of,
};

use super::graph_state::{self, GraphState};
use super::machine_answer::FailureCategory;
use super::refusal::Refusal;
use super::service_pause;

const OPTION: &str = "--context";

/// Removes `--context <name>` or `--context=<name>` from the arguments, wherever it
/// appears, so every command shares one spelling of it without repeating the parsing.
pub(super) fn take_context(
    arguments: Vec<String>,
) -> Result<(Option<String>, Vec<String>), String> {
    let mut context: Option<String> = None;
    let mut rest = Vec::with_capacity(arguments.len());
    let mut remaining = arguments.into_iter();
    while let Some(argument) = remaining.next() {
        let named = if argument == OPTION {
            Some(
                remaining
                    .next()
                    .ok_or_else(|| format!("{OPTION} needs the name of a context"))?,
            )
        } else {
            argument
                .strip_prefix(&format!("{OPTION}="))
                .map(str::to_string)
        };
        match named {
            Some(name) if context.is_some() => {
                return Err(format!(
                    "{OPTION} is given twice ({}, {name}); a run works in exactly one context",
                    context.unwrap_or_default()
                ));
            }
            Some(name) => context = Some(name),
            None => rest.push(argument),
        }
    }
    Ok((context, rest))
}

/// The context this run works in: the one named, or the only one configured.
pub(super) fn select(
    loaded: &LoadedIndexPolicy,
    requested: Option<&str>,
) -> Result<WorkspaceName, String> {
    Ok(choose(loaded, requested)?)
}

/// The same, with the refusal still typed for a command that answers a program.
pub(super) fn choose(
    loaded: &LoadedIndexPolicy,
    requested: Option<&str>,
) -> Result<WorkspaceName, Refusal> {
    loaded
        .workspaces()
        .select(requested)
        .cloned()
        .map_err(Refusal::of_workspace)
}

/// The contexts a command that works over all of them runs in: the one named, or every
/// configured one when none is.
///
/// The rule that `--context` narrows rather than being required lives here once, because
/// two commands hold it — `status` and `migrate` — and a second copy is a second answer to
/// "what does a run with no context mean". Each context is still opened on its own, one
/// after another: this decides how many are visited, not how many are open at a time.
///
/// An empty answer is not a failure. A machine with nothing configured has no context to
/// report on and none to migrate, and that is a line for the caller to print rather than a
/// refusal — which is why this does not fall back to `select`, whose refusal is about a run
/// that must name one.
pub(super) fn select_all(
    loaded: &LoadedIndexPolicy,
    requested: Option<&str>,
) -> Result<Vec<WorkspaceName>, String> {
    match requested {
        Some(named) => Ok(vec![select(loaded, Some(named))?]),
        None => Ok(loaded.workspaces().names().to_vec()),
    }
}

/// The graph of one context.
///
/// When the background service holds it, the person is asked first whether to stop the
/// service for the length of this run (`service_pause`).
pub(super) fn open(workspace: &WorkspaceName) -> Result<Arc<LadybugDatabaseManager>, String> {
    service_pause::make_way_for(workspace)?;
    refuse_a_graph_that_kills_the_process(workspace)?;
    LadybugDatabaseManager::open_workspace(workspace).map_err(|error| explain(workspace, error))
}

/// The graph of one context for a command that writes nothing to it.
///
/// Read-only always, and never the write opening — not even when nothing holds the
/// database. A write opening is not a preference here, it is three writes: it creates the
/// database of a context nobody has indexed, it applies every migration this build has that
/// the graph has not, and it folds the log into the database when it closes. A command that
/// only answers may do none of those, and until this line these did all three. Measured: an
/// ordinary `search` against an idle context already at this build's schema rewrote its
/// graph file every single run, and on a graph from an older build it migrated it silently
/// and irreversibly — there are no down migrations, and v12 and v13 rename columns because
/// dropping one kills the engine (ADR 0012).
///
/// This is the rule Stage 14 made for `status` (ADR 0020), now held by the whole class
/// rather than by one command. What it costs is what `status` pays for it: a graph whose
/// schema is not this build's is refused with the repair rather than migrated under the
/// reader, a context nobody has indexed is named rather than created, and what a writer has
/// committed but not yet checkpointed is not read.
///
/// Reading beside a live writer is unchanged (ADR 0014). That case never used the write
/// opening: the engine holds the file locked for the writer, the write attempt failed, and
/// this is the opening it fell back to. Only the free database changes doors.
pub(super) fn open_for_reading(
    workspace: &WorkspaceName,
) -> Result<Arc<LadybugDatabaseManager>, String> {
    Ok(open_for_reading_or_refuse(workspace)?)
}

/// The same, with the refusal still typed for a command that answers a program.
pub(super) fn open_for_reading_or_refuse(
    workspace: &WorkspaceName,
) -> Result<Arc<LadybugDatabaseManager>, Refusal> {
    refuse_a_context_that_was_never_indexed(workspace)?;
    refuse_a_graph_that_kills_the_process(workspace)?;
    LadybugDatabaseManager::open_workspace_read_only(workspace)
        .map_err(|error| refused(workspace, why_a_reading_did_not_open(workspace, error)))
}

/// Why a read-only opening failed, decided from the files rather than from what the engine
/// happened to see.
///
/// A reader cannot tell a graph this build has outrun from one a pass is building right
/// now: measured mid-way through a first index, the database honestly records schema 5 of
/// 13, and answering "run `crosskit migrate`" would send someone to repair a context that
/// is being written correctly while they read. The write opening used to answer this by
/// failing on the lock, and that lock is still the fact — it is read from the file here
/// instead of being taken (ADR 0020), which is the whole of what changed.
fn why_a_reading_did_not_open(
    workspace: &WorkspaceName,
    error: RepositoryError,
) -> RepositoryError {
    match graph_presence_of(workspace) {
        Ok(GraphPresence::BeingWritten { .. } | GraphPresence::HeldByWriter { .. }) => {
            RepositoryError::GraphBusy
        }
        _ => error,
    }
}

/// A context nobody has indexed, named before anything is opened.
///
/// The write opening used to answer this by creating the database, which is the very thing
/// a reading command may not do. A read-only opening refuses it instead — but in the
/// engine's own words, about read-only mode, which say nothing to someone who has simply
/// not indexed anything yet. So the state is recognised here, from the files, exactly as
/// `status` recognises it (ADR 0020), and named in this program's words.
///
/// Only this one state is answered here. Every other one is either readable or refused
/// further down with its own repair, and a presence that could not be determined is left to
/// the opening, which is the thing that actually knows.
fn refuse_a_context_that_was_never_indexed(workspace: &WorkspaceName) -> Result<(), Refusal> {
    match graph_presence_of(workspace) {
        Ok(GraphPresence::NeverIndexed) => Err(Refusal::new(
            FailureCategory::NotIndexed,
            format!(
                "context {workspace} has not been indexed yet, so it has no graph to read. {}",
                index_init_repair(workspace)
            ),
        )),
        _ => Ok(()),
    }
}

/// Refuses, with the repair, a graph that opening here would not survive.
///
/// The storage engine sometimes dies on a log left by a stopped writer instead of refusing
/// it — measured on Linux, three databases out of six — and a process that dies prints
/// nothing at all. So before opening a graph that has a log beside it, a child process
/// opens it first and this one reads what became of the child.
///
/// Only that one state stops the command. A graph the child opened is a graph this process
/// can open too, and a busy one is not broken: the ordinary path handles it, and for a
/// reader it is the difference between an answer and a refusal. A log exists only while a
/// pass is writing or after a stop that was not a close, so on a quiet healthy context this
/// costs nothing and runs nothing.
fn refuse_a_graph_that_kills_the_process(workspace: &WorkspaceName) -> Result<(), Refusal> {
    let maintenance = LadybugGraphMaintenance::for_workspace(workspace)
        .map_err(|error| Refusal::failed(error.to_string()))?;
    if !maintenance.log_is_present() {
        return Ok(());
    }
    match graph_state::probe(workspace) {
        GraphState::LogUnreadable => Err(refused(
            workspace,
            RepositoryError::GraphLogUnreadable {
                log: maintenance.write_ahead_log(),
            },
        )),
        GraphState::Opens | GraphState::Busy | GraphState::Unknown(_) => Ok(()),
    }
}

/// A failure to open a context's graph, worded by `explain` and typed by what it was.
fn refused(workspace: &WorkspaceName, error: RepositoryError) -> Refusal {
    let typed = error.clone();
    Refusal::of_graph(&typed, explain(workspace, error))
}

/// What a failure to open a context's graph means to the person who ran the command, and
/// what to run about it.
///
/// The categories arrive from infrastructure without a recipe in them, because which
/// command repairs what is this layer's knowledge and nothing below it should have to
/// know the spelling of a command. Every command that opens a graph answers the same
/// failure with the same words, which is why this is one function and not a paragraph in
/// each of them.
pub(super) fn explain(workspace: &WorkspaceName, error: RepositoryError) -> String {
    match error {
        RepositoryError::GraphLogUnreadable { log } => format!(
            "context {workspace} did not open: its write-ahead log ends where the process \
             writing it was stopped, and the storage engine will not replay it. Nothing that \
             the interrupted pass had written since the last checkpoint survives, and the \
             graph is written again from the files on disk. Run `crosskit --context \
             {workspace} index recover`, which moves that log aside and keeps it, and then \
             index the directory again.\nLog: {}",
            log.display()
        ),
        RepositoryError::GraphBusy => format!(
            "context {workspace} is being indexed by another process right now, which holds \
             its database. Try again when that pass is done; `crosskit --context {workspace} \
             search` and `crosskit --context {workspace} doctor` answer between passes. If \
             that process is the background service (`crosskit service status`), it holds the \
             database for as long as it runs: `crosskit service stop` stops it, and `crosskit \
             service start` starts it again."
        ),
        // The error already says what the two versions are and that this opening does not
        // migrate. All this layer adds is the command, because that is all it knows that the
        // error does not — saying the rule again here is how one paragraph came to state it
        // twice.
        schema @ RepositoryError::SchemaNotThisBuild { .. } => {
            format!("{schema}. {}", migrate_repair())
        }
        other => other.to_string(),
    }
}

/// The one command that writes a context's graph for the first time, spelled in one place:
/// a refusal and a report both name it, and two spellings of it would drift apart.
pub(super) fn index_init_repair(workspace: &WorkspaceName) -> String {
    format!(
        "`crosskit --context {workspace} index init <directory>` writes its graph from the \
         files on disk"
    )
}

/// The one command that brings a graph to this build's schema, spelled in one place.
///
/// Every context is named rather than this one: a build that outran one context's schema
/// has almost certainly outrun the others, and repairing them one `--context` at a time is
/// what this sentence exists to stop.
pub(super) fn migrate_repair() -> String {
    "`crosskit migrate` brings every configured context to this build's schema, in one run"
        .to_string()
}

/// A run may only walk a directory its own context covers. A root that belongs to another
/// context, or to none, is refused before anything opens: a scan whose root spans contexts
/// would have to decide per path which graph a file goes into, and that decision is exactly
/// what a boundary must not make at scan time.
pub(super) fn require_root_in_context(
    loaded: &LoadedIndexPolicy,
    workspace: &WorkspaceName,
    root: &NormalizedPath,
) -> Result<(), String> {
    match loaded.workspaces().workspace_of(root) {
        Some(owner) if owner == workspace => Ok(()),
        Some(owner) => Err(format!(
            "the directory belongs to context {owner}, not to {workspace}; run it in its own context"
        )),
        None => Err(format!(
            "the directory belongs to no context, so it is not indexed; give context {workspace} an `include` pattern that covers it"
        )),
    }
}
