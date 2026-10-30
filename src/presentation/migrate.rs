// `crosskit migrate`: brings every configured context to this build's schema, in one run.
//
// Opening a graph for writing is what applies the migrations that are missing, so before
// this there was no command that did it — there were only commands that did it on the way
// to something else, one `--context` at a time. After v13 two of the owner's four contexts
// would not open for reading, and the repair was four separate runs of a command that
// happens to write. A build that has outrun one context's schema has almost certainly
// outrun the others, so the repair is asked for once and applies everywhere.
//
// It is the write half of `status`, and it reads the same two files first for the same
// reason: a context nobody has indexed must not gain a database because somebody asked for
// a migration, and a graph left by a stopped pass must not be opened at all — the engine
// sometimes dies on it rather than refusing. Both are reported, neither is touched.
use crate::application::KnowledgeGraphRepository;
use crate::domain::WorkspaceName;
use crate::infrastructure::{
    GraphPresence, LadybugDatabaseManager, graph_presence_of, load_index_policy,
};

use super::help;
use super::migrate_from_earlier_name;
use super::policy::policy_file_label;
use super::service_pause;
use super::workspace;

pub(super) const USAGE: &str =
    "Usage: crosskit [--context <name>] migrate\n       crosskit migrate --from-crosscode";

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    // The data directory belongs to no context, so moving it is asked for without one.
    if arguments == [migrate_from_earlier_name::FLAG] && context.is_none() {
        return migrate_from_earlier_name::run();
    }
    if !arguments.is_empty() {
        return Err(help::misuse(USAGE));
    }
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select_all(&loaded, context)?;

    println!("Policy file: {}", policy_file_label(&loaded));
    if selected.is_empty() {
        println!("Contexts: none configured.");
        println!(
            "There is no graph to migrate until a context exists. `crosskit setup \
             <directory>` proposes contexts for a directory and indexes what you accept."
        );
        return Ok(());
    }

    println!("Contexts: {}", selected.len());
    // Every context is reported whatever became of it, and the run fails afterwards. A run
    // that stopped at the first graph it could not open would hide the ones behind it, which
    // is the state this command exists to end.
    let left_behind: Vec<&WorkspaceName> = selected
        .iter()
        .filter(|workspace| {
            println!();
            println!("{workspace}");
            !migrate(workspace)
        })
        .collect();

    if left_behind.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} of {} context(s) were not brought to this build's schema: {}. Each is reported \
         above with what it is waiting on.",
        left_behind.len(),
        selected.len(),
        left_behind
            .iter()
            .map(|workspace| workspace.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// One context. `true` means its graph is now at this build's schema, or that there is no
/// graph to bring anywhere.
fn migrate(workspace: &WorkspaceName) -> bool {
    if let Err(refusal) = service_pause::make_way_for(workspace) {
        println!("  Not migrated: {refusal}");
        return false;
    }
    let presence = match graph_presence_of(workspace) {
        Ok(presence) => presence,
        Err(error) => {
            println!("  Not migrated: {}", workspace::explain(workspace, error));
            return false;
        }
    };
    match presence {
        // Opening this for writing would create the database of a context nobody has
        // indexed, which is a first index by the wrong command.
        GraphPresence::NeverIndexed => {
            println!("  Nothing to migrate: this context has no graph yet");
            println!(
                "  `crosskit --context {workspace} index init <directory>` writes it from the \
                 files on disk."
            );
            true
        }
        GraphPresence::StoppedUncleanly { log } => {
            println!("  Not migrated: a pass was stopped without closing this context");
            println!("  Log: {}", log.display());
            println!(
                "  `crosskit --context {workspace} index recover` moves that log aside and \
                 keeps it; migrating it before that would open a graph the storage engine may \
                 not survive."
            );
            false
        }
        GraphPresence::BeingWritten { pid } | GraphPresence::HeldByWriter { pid } => {
            println!("  Not migrated: another process holds this context for writing ({pid})");
            println!("  A migration is a write, and there is one writer. Ask again when it ends.");
            false
        }
        GraphPresence::Undetermined { reason } => {
            println!("  Not migrated: the state of this context could not be determined");
            println!("  {reason}");
            false
        }
        GraphPresence::Ready => open_for_writing(workspace),
    }
}

/// The migration itself: opening a graph for writing applies what is missing, and the
/// version it reports afterwards is what it is now at.
fn open_for_writing(workspace: &WorkspaceName) -> bool {
    let opened = LadybugDatabaseManager::open_workspace(workspace)
        .and_then(|manager| manager.repository().schema_version());
    match opened {
        Ok(version) => {
            println!("  Schema version: {version}");
            true
        }
        Err(error) => {
            println!("  Not migrated: {}", workspace::explain(workspace, error));
            false
        }
    }
}
