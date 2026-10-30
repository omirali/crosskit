// What the running service says about itself, in a file `crosskit service status` reads.
//
// Written whole after every change and put in place by a rename, so a reader sees one
// version or the next and never half of one. It holds the same things the journal says —
// counters, configured roots, fixed sentences — and never a path a pass came across.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use super::file_stamp::FileStamp;

/// How many of a context's latest errors are kept. The journal keeps the rest.
const ERRORS_KEPT: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub pid: u32,
    pub started_at_ms: i64,
    /// The program this process was started from, and what that file was at the start.
    pub program: String,
    pub program_build: FileStamp,
    pub config: String,
    /// The configuration's content when the service read it; absent when there was none.
    pub config_digest: Option<String>,
    /// Why the service watches nothing at all, when that is so.
    #[serde(default)]
    pub problem: Option<String>,
    /// Whether readers are answered through the socket, or why not.
    #[serde(default)]
    pub reads: Option<String>,
    #[serde(default)]
    pub stopped_at_ms: Option<i64>,
    pub contexts: Vec<ContextStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextPhase {
    /// Opening the graph, subscribing, or in the bootstrap pass.
    Starting,
    Watching,
    /// Could not start; tried again after a while. `waiting` says why.
    Waiting,
    Stopped,
}

/// What starts a pass besides the periodic one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WatchedBy {
    Events,
    Timer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedLine {
    pub at_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PassCounts {
    pub completed: usize,
    pub incomplete: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextStatus {
    pub name: String,
    pub phase: ContextPhase,
    pub since_ms: i64,
    pub watched_by: Option<WatchedBy>,
    /// Why passes follow the timer alone, in the watcher's own sentence.
    pub why_timer: Option<String>,
    pub roots: usize,
    /// Roots the event backend did not take, each with its reason.
    pub roots_without_events: Vec<String>,
    /// What the watch does with the insight queue; absent when analysis is off.
    pub analysis: Option<String>,
    pub last_pass: Option<TimedLine>,
    pub passes: PassCounts,
    /// Roots the last pass did not walk, each with its category.
    pub not_walked: Vec<String>,
    /// Of those, the roots the operating system would not let the service open.
    #[serde(default)]
    pub roots_not_permitted: usize,
    /// Paths under the walked roots the last pass could not read.
    pub paths_not_read: usize,
    pub errors: Vec<TimedLine>,
    /// Why the context is waiting, when it is.
    pub waiting: Option<String>,
}

impl ContextStatus {
    pub fn starting(name: &str, at_ms: i64) -> Self {
        Self {
            name: name.to_string(),
            phase: ContextPhase::Starting,
            since_ms: at_ms,
            watched_by: None,
            why_timer: None,
            roots: 0,
            roots_without_events: Vec::new(),
            analysis: None,
            last_pass: None,
            passes: PassCounts::default(),
            not_walked: Vec::new(),
            roots_not_permitted: 0,
            paths_not_read: 0,
            errors: Vec::new(),
            waiting: None,
        }
    }

    pub fn enter(&mut self, phase: ContextPhase, at_ms: i64) {
        if self.phase != phase {
            self.phase = phase;
            self.since_ms = at_ms;
        }
    }

    pub fn note_error(&mut self, at_ms: i64, text: String) {
        self.errors.push(TimedLine { at_ms, text });
        let excess = self.errors.len().saturating_sub(ERRORS_KEPT);
        self.errors.drain(..excess);
    }
}

/// The status of the running service, written to its file on every change.
pub struct StatusBoard {
    path: PathBuf,
    status: Mutex<ServiceStatus>,
}

impl StatusBoard {
    pub fn new(path: &Path, status: ServiceStatus) -> Self {
        Self {
            path: path.to_path_buf(),
            status: Mutex::new(status),
        }
    }

    /// Changes the status and writes it. A status that cannot be written is reported to the
    /// caller, which has a journal to say so in; the service goes on either way.
    pub fn update(&self, change: impl FnOnce(&mut ServiceStatus)) -> io::Result<()> {
        let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut status);
        write_whole(&self.path, &status)
    }

    /// Reads the status without writing it.
    pub fn read(&self, look: impl FnOnce(&ServiceStatus)) {
        look(&self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// The same for one context, which is what nearly every change is about.
    pub fn update_context(
        &self,
        context: &str,
        change: impl FnOnce(&mut ContextStatus),
    ) -> io::Result<()> {
        self.update(|status| {
            if let Some(found) = status
                .contexts
                .iter_mut()
                .find(|found| found.name == context)
            {
                change(found);
            }
        })
    }
}

fn write_whole(path: &Path, status: &ServiceStatus) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    let text = serde_json::to_string_pretty(status).map_err(io::Error::other)?;
    let staged = path.with_extension("json.partial");
    fs::write(&staged, text)?;
    fs::rename(&staged, path)
}

/// The status the service last wrote, or nothing when no service ever ran here.
pub fn read_status(path: &Path) -> io::Result<Option<ServiceStatus>> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{ContextPhase, ContextStatus, ServiceStatus, StatusBoard, read_status};
    use crate::infrastructure::service::FileStamp;

    #[test]
    fn what_the_service_writes_is_what_status_reads_and_only_the_latest_errors_stay() {
        let directory =
            std::env::temp_dir().join(format!("crosskit-status-{}", std::process::id()));
        let path = directory.join("status.json");
        let board = StatusBoard::new(
            &path,
            ServiceStatus {
                pid: 42,
                started_at_ms: 1,
                program: "/Users/alice/bin/crosskit".to_string(),
                program_build: FileStamp {
                    modified_ms: 2,
                    size: 3,
                },
                config: "/Users/alice/config.toml".to_string(),
                config_digest: None,
                problem: None,
                reads: None,
                stopped_at_ms: None,
                contexts: vec![ContextStatus::starting("work", 1)],
            },
        );

        board
            .update_context("work", |context| {
                context.enter(ContextPhase::Watching, 5);
                for error in 0..8 {
                    context.note_error(error, format!("error {error}"));
                }
            })
            .unwrap();

        let read = read_status(&path).unwrap().unwrap();
        let work = &read.contexts[0];
        assert_eq!(work.phase, ContextPhase::Watching);
        assert_eq!(work.since_ms, 5);
        assert_eq!(work.errors.len(), 5);
        assert_eq!(work.errors[0].text, "error 3");
        assert!(
            read_status(&directory.join("absent.json"))
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(&directory).unwrap();
    }
}
