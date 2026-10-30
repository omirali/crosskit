// The foreground watcher as a child process: started, followed through its stdout and
// stopped with SIGINT exactly like Ctrl-C.
//
// Shared because two test binaries need a running watcher — one to check the watch
// lifecycle itself, one to check what another command may do while a watcher holds the
// context.
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use super::Sandbox;

/// Long enough for FSEvents and the production debounce on a loaded machine.
const LINE_WAIT: Duration = Duration::from_secs(30);
const EXIT_WAIT: Duration = Duration::from_secs(30);

pub struct WatchProcess {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl WatchProcess {
    pub fn spawn(sandbox: &Sandbox) -> Self {
        Self::watching(sandbox.command(&["index", "watch", &sandbox.documents_argument()]))
    }

    /// A watcher over the whole context, which is what `index watch` with no directory is.
    pub fn spawn_context(sandbox: &Sandbox) -> Self {
        Self::watching(sandbox.command(&["index", "watch"]))
    }

    /// A watcher over one named context, for a sandbox that declares several of them.
    pub fn spawn_for(sandbox: &Sandbox, context: &str, directory: &str) -> Self {
        Self::watching(sandbox.command(&["--context", context, "index", "watch", directory]))
    }

    /// A watcher started from a command the caller prepared, for a scenario about the
    /// environment the watcher runs in rather than the arguments it was given.
    pub fn spawn_command(command: Command) -> Self {
        Self::watching(command)
    }

    fn watching(mut command: Command) -> Self {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    /// Waits, within a bound, for a stdout line containing `needle`.
    pub fn wait_for_line(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + LINE_WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if line.contains(needle) {
                        return line;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => panic!(
                    "no line containing {needle:?} within {LINE_WAIT:?}; seen so far:\n{}",
                    self.seen.join("\n")
                ),
            }
        }
    }

    /// The same signal a terminal sends for Ctrl-C.
    pub fn interrupt(&self) {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success(), "kill -INT failed");
    }

    /// Kills the watcher outright, the way an OOM kill or a lost battery does: no signal
    /// it can handle, no chance to close the database.
    pub fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }

    /// Waits for the child to exit within a bound and returns its status, the rest of
    /// stdout and all of stderr.
    pub fn wait_for_exit(mut self) -> (ExitStatus, Vec<String>, String) {
        let deadline = Instant::now() + EXIT_WAIT;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                self.child.kill().unwrap();
                panic!(
                    "the watcher did not exit within {EXIT_WAIT:?}; stdout so far:\n{}",
                    self.seen.join("\n")
                );
            }
            thread::sleep(Duration::from_millis(50));
        };
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        let mut lines = std::mem::take(&mut self.seen);
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(2)) {
            lines.push(line);
        }
        (status, lines, stderr)
    }
}
