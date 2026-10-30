// `crosskit [--context <name>] index recover`.
//
// The one repair for a context a stopped writer left unopenable. It moves the write-ahead
// log the storage engine refuses out of the way, says what that costs, and says what to run
// to get it back. It never deletes anything, and on a context that opens it does nothing at
// all.
use crate::application::{GraphMaintenance, GraphRecoveryService, RecoveryReport, RepositoryError};
use crate::domain::WorkspaceName;
use crate::infrastructure::{LadybugGraphMaintenance, load_index_policy};

use super::graph_state::{BUSY, FAILED, LOG_UNREADABLE, OPENS, VERDICT_PREFIX};
use super::help;
use super::service_pause;
use super::{graph_state, workspace};

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] index recover [--check]";

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    let checking = match arguments {
        [] => false,
        [only] if only == "--check" => true,
        _ => return Err(help::misuse(USAGE)),
    };
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let workspace = workspace::select(&loaded, context)?;
    let maintenance = LadybugGraphMaintenance::for_workspace(&workspace)
        .map_err(|error| workspace::explain(&workspace, error))?;
    if checking {
        return check(&maintenance);
    }
    // Before anything is probed: the probe is a child that opens the graph, and the service's
    // lock would answer it with "busy" for as long as the service runs.
    service_pause::make_way_for(&workspace)?;

    println!("Context: {workspace}");
    println!("Database: {}", maintenance.database_path().display());
    // Said before anything moves, because it is the whole of what this costs and the
    // operator is the one who decides whether to pay it.
    println!(
        "A recovery moves the write-ahead log aside and keeps it. What the interrupted pass \
         had written since the last checkpoint is not readable from it and does not come \
         back; the graph is written again from the files on disk, which costs one pass."
    );

    let maintenance = CheckedInAChildProcess {
        moving: &maintenance,
        workspace: &workspace,
    };
    let report = GraphRecoveryService::new(&maintenance)
        .execute()
        .map_err(|error| workspace::explain(&workspace, error))?;

    match report {
        RecoveryReport::NothingToRecover => {
            println!("Nothing to recover: this context's graph opens as it is.");
            Ok(())
        }
        RecoveryReport::Recovered { logs_moved_to } => {
            println!("Recovered: the graph opens again.");
            print_moved(&logs_moved_to);
            print_next_steps(&workspace);
            Ok(())
        }
        RecoveryReport::MovedButStillClosed {
            logs_moved_to,
            failure,
        } => {
            print_moved(&logs_moved_to);
            Err(format!(
                "the graph still does not open after its log was moved aside: {failure}"
            ))
        }
    }
}

fn print_moved(logs: &[std::path::PathBuf]) {
    for log in logs {
        println!("Log moved to: {}", log.display());
    }
}

fn print_next_steps(workspace: &WorkspaceName) {
    println!(
        "Next: `crosskit --context {workspace} index reconcile <directory>` writes the graph \
         again from the files on disk, and keeps the search index in step with what it reads. \
         If `crosskit --context {workspace} doctor` still shows the index behind the graph \
         afterwards — text of files that are no longer on disk lives on in the graph — \
         `crosskit --context {workspace} index rebuild --search` writes it again in full."
    );
}

/// The maintenance a repair performs: the log is moved from here, and whether the graph
/// opens is asked of a child process.
///
/// Asking it here would mean opening a database the storage engine sometimes dies on, and a
/// repair that dies of the thing it repairs is not one. The child pays that risk; this
/// process reads what became of it and still has a voice afterwards.
struct CheckedInAChildProcess<'a> {
    moving: &'a LadybugGraphMaintenance,
    workspace: &'a WorkspaceName,
}

impl GraphMaintenance for CheckedInAChildProcess<'_> {
    fn probe(&self) -> Result<(), RepositoryError> {
        if !self.moving.log_is_present() {
            // Nothing to replay, so nothing to be killed by: the ordinary open answers.
            return self.moving.probe();
        }
        match graph_state::probe(self.workspace) {
            graph_state::GraphState::Opens => Ok(()),
            graph_state::GraphState::Busy => Err(RepositoryError::GraphBusy),
            graph_state::GraphState::LogUnreadable => Err(RepositoryError::GraphLogUnreadable {
                log: self.moving.write_ahead_log(),
            }),
            graph_state::GraphState::Unknown(reason) => Err(RepositoryError::Storage(format!(
                "this context's graph could not be checked: {reason}"
            ))),
        }
    }

    fn move_log_aside(&self) -> Result<Vec<std::path::PathBuf>, RepositoryError> {
        self.moving.move_log_aside()
    }
}

/// `--check`: open the graph, say what happened, change nothing.
///
/// This is the half that may not survive: on a log the engine refuses it usually returns,
/// and sometimes it dies where it stands. That is why it is its own command — a caller runs
/// it as a child and reads either the verdict or the signal.
fn check(maintenance: &LadybugGraphMaintenance) -> Result<(), String> {
    match maintenance.probe() {
        Ok(()) => {
            println!("{VERDICT_PREFIX}{OPENS}");
            Ok(())
        }
        Err(RepositoryError::GraphBusy) => {
            println!("{VERDICT_PREFIX}{BUSY} (another process is writing to this context)");
            Err("this context is being indexed by another process right now".to_string())
        }
        Err(RepositoryError::GraphLogUnreadable { log }) => {
            println!(
                "{VERDICT_PREFIX}{LOG_UNREADABLE} (the engine will not replay {})",
                log.display()
            );
            Err("this context's write-ahead log cannot be replayed".to_string())
        }
        Err(other) => {
            println!("{VERDICT_PREFIX}{FAILED} ({other})");
            Err(other.to_string())
        }
    }
}
