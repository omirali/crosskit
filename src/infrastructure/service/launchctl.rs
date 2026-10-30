// launchd, through its own command: the only way a user-level program asks it to load,
// unload or describe an agent.
//
// Found on PATH rather than at `/bin/launchctl`, so a test can put a stand-in first and no
// test ever reaches the real launchd of the machine it runs on.
use std::path::Path;
use std::process::{Command, Output};

use super::launch_agent::SERVICE_LABEL;

pub struct Launchctl {
    domain: String,
}

/// What launchd says about the service.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchdState {
    /// launchd knows the agent: it was bootstrapped and not booted out since.
    pub loaded: bool,
    /// The process launchd is running for it, if one runs.
    pub pid: Option<i32>,
    /// How the last process ended, in launchd's words (`0`, `1`, `(never exited)`).
    pub last_exit: Option<String>,
}

impl Default for Launchctl {
    fn default() -> Self {
        Self::new()
    }
}

impl Launchctl {
    /// The person's GUI session, which is where an agent of theirs runs and the domain that
    /// is there at login.
    pub fn new() -> Self {
        // SAFETY: getuid takes no arguments, has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        Self {
            domain: format!("gui/{uid}"),
        }
    }

    fn target(&self) -> String {
        format!("{}/{SERVICE_LABEL}", self.domain)
    }

    /// Loads the agent and, because it says `RunAtLoad`, starts it.
    pub fn bootstrap(&self, agent: &Path) -> Result<(), String> {
        let agent = agent.to_string_lossy();
        run(&["bootstrap", &self.domain, &agent])
    }

    /// Stops the service and unloads the agent. launchd sends the process a termination
    /// signal and waits for it up to the agent's `ExitTimeOut`.
    pub fn bootout(&self) -> Result<(), String> {
        run(&["bootout", &self.target()])
    }

    pub fn state(&self) -> Result<LaunchdState, String> {
        let output = launchctl(&["print", &self.target()])?;
        if !output.status.success() {
            return Ok(LaunchdState::default());
        }
        Ok(parse_print(&String::from_utf8_lossy(&output.stdout)))
    }
}

fn launchctl(arguments: &[&str]) -> Result<Output, String> {
    Command::new("launchctl")
        .args(arguments)
        .output()
        .map_err(|error| format!("launchctl could not be run: {error}"))
}

fn run(arguments: &[&str]) -> Result<(), String> {
    let output = launchctl(arguments)?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.trim();
    Err(format!(
        "launchctl {} failed ({}){}",
        arguments[0],
        output
            .status
            .code()
            .map_or_else(|| "killed".to_string(), |code| format!("exit {code}")),
        if said.is_empty() {
            String::new()
        } else {
            format!(": {said}")
        }
    ))
}

/// `launchctl print` is meant for people and has no stable format; the three lines read here
/// have kept their spelling across the macOS versions it was checked on, and a line that is
/// missing leaves its field empty rather than failing the report.
fn parse_print(text: &str) -> LaunchdState {
    let mut state = LaunchdState {
        loaded: true,
        ..LaunchdState::default()
    };
    for line in text.lines().map(str::trim) {
        if let Some(pid) = line.strip_prefix("pid = ") {
            state.pid = pid.trim().parse().ok();
        } else if let Some(exit) = line.strip_prefix("last exit code = ") {
            state.last_exit = Some(exit.trim().to_string());
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::parse_print;

    #[test]
    fn the_process_and_the_last_exit_are_read_from_what_launchd_prints() {
        let running = parse_print(
            "gui/501/dev.crosskit.watch = {\n\tactive count = 1\n\tstate = running\n\tpid = 4242\n\tlast exit code = (never exited)\n}\n",
        );
        assert!(running.loaded);
        assert_eq!(running.pid, Some(4242));
        assert_eq!(running.last_exit.as_deref(), Some("(never exited)"));

        let stopped = parse_print(
            "gui/501/dev.crosskit.watch = {\n\tstate = not running\n\tlast exit code = 1\n}\n",
        );
        assert_eq!(stopped.pid, None);
        assert_eq!(stopped.last_exit.as_deref(), Some("1"));
    }
}
