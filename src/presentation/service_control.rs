// `service stop` and `service start`: the service off and on again, with the agent left where
// it is — so the next login starts it as before.
use std::time::{Duration, Instant};

use crate::infrastructure::service::{Launchctl, ServiceLayout, instance_holder};

/// How long a stop waits for the old process to let go of its lock: the agent's own
/// `ExitTimeOut`, plus a little for launchd to deliver the kill.
const OLD_PROCESS_WAIT: Duration = Duration::from_secs(130);

/// How often `start --after` looks whether the command it waits for has ended.
const AFTER_POLL: Duration = Duration::from_millis(250);

pub(super) fn stop_command() -> Result<(), String> {
    let layout = installed_layout("stop")?;
    let launchctl = Launchctl::new();
    if !launchctl.state()?.loaded && instance_holder(&layout.instance_lock()).is_none() {
        println!("The service is already stopped. `crosskit service start` starts it.");
        return Ok(());
    }
    stop(&launchctl, &layout)?;
    println!(
        "Stopped. The agent stays installed, so the service starts again at the next login; \
         `crosskit service start` starts it now."
    );
    Ok(())
}

/// `start`, or — with `after` — the same once that process has ended, which is how a command
/// that stopped the service for its own run hands it back whatever became of the run.
pub(super) fn start_command(after: Option<i32>) -> Result<(), String> {
    if let Some(pid) = after {
        while process_is_alive(pid) {
            std::thread::sleep(AFTER_POLL);
        }
    }
    let layout = installed_layout("start")?;
    if let Some(pid) = instance_holder(&layout.instance_lock()) {
        println!("The service is already running, as process {pid}.");
        return Ok(());
    }
    start(&Launchctl::new())?;
    println!("Started. `crosskit service status` shows each context once it has opened.");
    Ok(())
}

/// Stops the service launchd runs and waits for it to let go: a new process started while
/// the old one still holds the lock would refuse to run, and launchd would wait a minute
/// before trying it again — and a command waiting to write would be refused by the lock.
///
/// A service launchd did not start — run by hand — is not stopped here: it is the person's
/// own process, and they are told which one it is.
pub(super) fn stop(launchctl: &Launchctl, layout: &ServiceLayout) -> Result<(), String> {
    if !launchctl.state()?.loaded {
        return match instance_holder(&layout.instance_lock()) {
            Some(pid) => Err(format!(
                "a service that launchd did not start runs as process {pid} — started by hand \
                 with `crosskit service run`; stop it first (`kill {pid}`)"
            )),
            None => Ok(()),
        };
    }
    launchctl.bootout()?;
    let deadline = Instant::now() + OLD_PROCESS_WAIT;
    while let Some(pid) = instance_holder(&layout.instance_lock()) {
        if Instant::now() >= deadline {
            return Err(format!(
                "the service process {pid} did not stop within {}s; it is still finishing a \
                 pass or does not answer. `kill {pid}` stops it, then run this again",
                OLD_PROCESS_WAIT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Loads the agent, which starts the service. One that launchd still has loaded without a
/// process — it stopped and launchd is holding the restart back — is unloaded first, or the
/// load would be refused as a duplicate.
pub(super) fn start(launchctl: &Launchctl) -> Result<(), String> {
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    if launchctl.state()?.loaded {
        launchctl.bootout()?;
    }
    launchctl.bootstrap(&layout.launch_agent())
}

fn installed_layout(command: &str) -> Result<ServiceLayout, String> {
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    if !layout.launch_agent().exists() {
        return Err(format!(
            "the service is not installed, so there is nothing to {command}; `crosskit service \
             install` installs it"
        ));
    }
    Ok(layout)
}

/// Whether a process of this number still exists. `EPERM` means it exists and belongs to
/// somebody else, which for a number this program was handed is still "alive".
fn process_is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 checks for existence and permission only; nothing is delivered.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
