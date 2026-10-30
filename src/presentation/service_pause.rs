// A command that writes, run while the background service holds the graph it needs.
//
// The service is the one writer of every context for as long as it runs (ADR 0014, ADR 0049),
// so `index reconcile`, `index jobs run`, `migrate` and `index recover` would be refused by
// its lock all day. Instead they ask, once per run, whether to stop the service while they
// work — and the service comes back when the command ends, however it ends.
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::domain::WorkspaceName;
use crate::infrastructure::service::{Launchctl, ServiceLayout, instance_holder};
use crate::infrastructure::{GraphPresence, graph_presence_of};

use super::confirmation::confirmed;
use super::service_control;

/// Set once this run has stopped the service: every later context it opens is already free,
/// and the question is not asked again.
static SERVICE_STOPPED: AtomicBool = AtomicBool::new(false);

/// Set by the service itself. It opens every context through the same door as any command,
/// and it must never look at its own lock file from inside: closing any descriptor of a file
/// drops every POSIX lock the process holds on it, so a look would hand its lock away —
/// measured, the second context to open made the service invisible to `status`.
static INSIDE_THE_SERVICE: AtomicBool = AtomicBool::new(false);

/// Called once by `service run` before any context opens.
pub(super) fn inside_the_service() {
    INSIDE_THE_SERVICE.store(true, Ordering::SeqCst);
}

/// Makes way for a write to `workspace` when the background service is what holds it.
///
/// Anything else — no writer, a terminal's `index watch`, a stopped pass — is left to the
/// opening that follows and to its own words. Only the service is asked about, because only
/// the service is the person's own process that this program knows how to stop and start.
///
/// A closed input is a no: a command run by a script is refused as before, with the two
/// commands that do by hand what the question offers.
pub(super) fn make_way_for(workspace: &WorkspaceName) -> Result<(), String> {
    if SERVICE_STOPPED.load(Ordering::SeqCst) || INSIDE_THE_SERVICE.load(Ordering::SeqCst) {
        return Ok(());
    }
    let Some(service) = service_holding(workspace) else {
        return Ok(());
    };
    println!(
        "Context {workspace} is held by the background service (process {service}), which \
         keeps every context current for as long as it runs."
    );
    if !confirmed(
        "Stop the service while this command runs? It starts again when the command ends.",
    )? {
        return Err(held_by_service(workspace, service));
    }
    pause_for_this_run()?;
    SERVICE_STOPPED.store(true, Ordering::SeqCst);
    println!("The service is stopped; it starts again when this command ends.");
    Ok(())
}

/// The refusal for a context the service holds: the same commands the question offered.
pub(super) fn held_by_service(workspace: &WorkspaceName, service: i32) -> String {
    format!(
        "context {workspace} is held by the background service (process {service}), and a \
         write needs it to itself. `crosskit service stop` stops the service, `crosskit \
         service start` starts it again; run this between the two"
    )
}

/// The process of the background service, when that is the writer holding `workspace`.
pub(super) fn service_holding(workspace: &WorkspaceName) -> Option<i32> {
    let writer = match graph_presence_of(workspace).ok()? {
        GraphPresence::HeldByWriter { pid } | GraphPresence::BeingWritten { pid } => pid,
        _ => return None,
    };
    let layout = ServiceLayout::resolve().ok()?;
    (instance_holder(&layout.instance_lock()) == Some(writer)).then_some(writer)
}

/// Stops the service, with its start already arranged.
///
/// The start is a process of its own, spawned before the stop and outside this terminal's
/// process group: it waits for this process to end and then starts the service. Ctrl-C, a
/// panic or a kill of this command ends this process and not that one, so "when the command
/// ends" holds for every way a command can end. Spawned first, so that no moment exists when
/// the service is stopped and nothing is going to start it.
fn pause_for_this_run() -> Result<(), String> {
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    let program = std::env::current_exe()
        .map_err(|error| format!("this program's own file could not be found: {error}"))?;
    Command::new(program)
        .args(["service", "start", "--after", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| {
            format!("the service was left running: what would start it again could not be started: {error}")
        })?;
    println!("Stopping the service: it ends the pass in flight first.");
    service_control::stop(&Launchctl::new(), &layout)
}
