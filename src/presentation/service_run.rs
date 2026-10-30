// `crosskit service run`: the process launchd starts. One watcher per configured context, each
// on a thread of its own, each the same composition `index watch` runs.
//
// What a context's watcher says goes to the journal under the context's name; what state it
// is in goes to the status file `crosskit service status` reads. A context that cannot start
// — its graph held by a terminal's `index watch`, a schema this build has not migrated — is
// tried again a minute later and never takes the others with it.
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::application::{
    AnalysisPass, EventDelivery, PassOutcome, PassRecord, RepositoryError, RootWatch, WatchError,
    WatchMode, WatchObserver, WatchSchedule,
};
use crate::domain::WorkspaceName;
use crate::infrastructure::service::{
    ContextPhase, ContextStatus, InstanceLock, InstanceLockError, Journal, PlacedSocket,
    ServerLimits, ServiceLayout, ServiceStatus, StatusBoard, StopBroadcast, TimedLine, WatchedBy,
    content_digest, file_stamp, now_ms, place_socket, serve, trim_launchd_output,
};
use crate::infrastructure::{
    LoadedIndexPolicy, NotifyWatchSource, config_file, default_probe_service_directory,
    load_index_policy,
};

use super::index_watch::{self, PreparedWatch};
use super::search_index::search_failure_lines;
use super::served_reads::HeldGraphs;
use super::service_pause;
use super::watch_narration::{
    WatchLines, degraded_label, describe_pass, render_summary, unconfirmed_label,
};

/// How long a context that could not start waits before it tries again. Long enough that a
/// refusal is a line a minute in the journal rather than a stream of them; short enough that
/// a terminal's `index watch` closed by the owner hands its context back within a minute.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// How the service's watchers are timed (ADR 0050).
///
/// A pass after changes walks only the directories they lie in, so the minute of rest ADR 0049
/// needed against whole passes back to back is three seconds now: enough that a transcript a
/// model session appends to every second costs one small pass every three, short enough that
/// an edit is in the index within seconds. The whole pass is the safety net and no longer the
/// way changes arrive — every sign of lost events and every start still run one at once — so
/// the periodic one is half an hour apart: on the owner's `work` it walks 22 roots, minutes of
/// work, and every five minutes was half of all the time.
fn service_schedule() -> WatchSchedule {
    WatchSchedule::default()
        .with_rest(Duration::from_secs(3))
        .with_periodic_interval(Duration::from_secs(30 * 60))
}

/// What a service with nothing to watch says once, and then it waits to be stopped: exiting
/// would have launchd start it again a minute later only to say the same.
const IDLE_WAIT: Duration = Duration::from_secs(3600);

pub(super) fn run() -> Result<(), String> {
    service_pause::inside_the_service();
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    let _instance =
        InstanceLock::acquire(&layout.instance_lock()).map_err(|error| match error {
            InstanceLockError::HeldBy(pid) => format!(
                "a crosskit service already runs for this data directory, as process {pid}; \
             `crosskit service status` describes it"
            ),
            InstanceLockError::Io(error) => format!("the service lock could not be taken: {error}"),
        })?;
    trim_launchd_output(&layout.launchd_output());
    let journal = Journal::open(&layout.journal())
        .map_err(|error| format!("the service journal could not be opened: {error}"))?;
    journal.write(None, &open_files_line());
    let stop = StopBroadcast::default();
    stop.stop_on_termination()?;

    let loaded = load_index_policy();
    let status = opening_status(loaded.as_ref().ok());
    journal.write(None, &opening_line(&status));
    let board = StatusBoard::new(&layout.status(), status);
    // Written at once, so `status` sees this process from its first second and not from the
    // end of its first pass.
    write_status(&board, &journal, |_| {});

    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            let problem = format!(
                "the configuration does not load: {error}. Nothing is watched; fix it and run \
                 `crosskit service restart`."
            );
            return idle(&board, &journal, &stop, problem);
        }
    };
    if loaded.workspaces().names().is_empty() {
        let problem = "the configuration declares no context, so there is nothing to watch; \
                       `crosskit setup` proposes them, then `crosskit service restart`."
            .to_string();
        return idle(&board, &journal, &stop, problem);
    }

    let contexts = loaded.workspaces().names().to_vec();
    let held = HeldGraphs::new(&loaded, board_config_digest(&board));
    // Placed before any thread of the service's own exists: it sets the working directory
    // for the moment it binds (`socket_place`).
    let socket = place_reads(&layout, &board, &journal);
    thread::scope(|scope| {
        if let Some(socket) = socket {
            let (held, stop, journal) = (&held, &stop, &journal);
            let spawned = thread::Builder::new()
                .name("crosskit-reads".to_string())
                .spawn_scoped(scope, move || {
                    serve(socket, held, ServerLimits::default(), stop, &|line| {
                        journal.write(None, line);
                    });
                });
            if let Err(error) = spawned {
                journal.write(
                    None,
                    &format!("Reads are not served: their thread could not start: {error}"),
                );
            }
        }
        for context in &contexts {
            let worker = ContextWorker {
                loaded: &loaded,
                context,
                board: &board,
                journal: &journal,
                stop: &stop,
                held: &held,
            };
            let spawned = thread::Builder::new()
                .name(format!("crosskit-watch-{context}"))
                .spawn_scoped(scope, move || worker.run());
            if let Err(error) = spawned {
                journal.write(
                    Some(context.as_str()),
                    &format!("Not watched: its thread could not start: {error}"),
                );
            }
        }
        while !stop.wait(IDLE_WAIT) {}
    });
    finish(&board, &journal);
    Ok(())
}

/// The digest of the configuration the service read, which every reader's question is checked
/// against.
fn board_config_digest(board: &StatusBoard) -> Option<String> {
    let mut digest = None;
    board.read(|status| digest = status.config_digest.clone());
    digest
}

/// Puts the socket in place, or says why reads are answered without the service.
fn place_reads(
    layout: &ServiceLayout,
    board: &StatusBoard,
    journal: &Journal,
) -> Option<PlacedSocket> {
    let (socket, reads) = match place_socket(&layout.service_directory()) {
        Ok(socket) => {
            let line = format!("answered through {}", layout.socket().display());
            (Some(socket), line)
        }
        Err(refusal) => (
            None,
            format!("not served — {refusal}; readers open the graphs themselves"),
        ),
    };
    journal.write(None, &format!("Reads: {reads}."));
    write_status(board, journal, |status| status.reads = Some(reads));
    socket
}

/// The status a service starts with: who it is, what it runs and what it read.
fn opening_status(loaded: Option<&LoadedIndexPolicy>) -> ServiceStatus {
    let program = std::env::current_exe().ok();
    let config = loaded
        .map(|loaded| loaded.path().to_path_buf())
        .or_else(|| config_file().ok());
    let started = now_ms();
    ServiceStatus {
        pid: std::process::id(),
        started_at_ms: started,
        program: program
            .as_ref()
            .map_or_else(String::new, |program| program.display().to_string()),
        program_build: program
            .as_deref()
            .and_then(|program| file_stamp(program).ok())
            .unwrap_or(crate::infrastructure::service::FileStamp {
                modified_ms: 0,
                size: 0,
            }),
        config: config
            .as_ref()
            .map_or_else(String::new, |config| config.display().to_string()),
        config_digest: config
            .as_deref()
            .and_then(|config| content_digest(config).ok()),
        problem: None,
        reads: None,
        stopped_at_ms: None,
        contexts: loaded
            .map(|loaded| {
                loaded
                    .workspaces()
                    .names()
                    .iter()
                    .map(|name| ContextStatus::starting(name.as_str(), started))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn opening_line(status: &ServiceStatus) -> String {
    let contexts: Vec<&str> = status
        .contexts
        .iter()
        .map(|context| context.name.as_str())
        .collect();
    format!(
        "Service started: process {}, program {}, configuration {}, contexts: {}.",
        status.pid,
        status.program,
        status.config,
        if contexts.is_empty() {
            "none".to_string()
        } else {
            contexts.join(", ")
        }
    )
}

fn idle(
    board: &StatusBoard,
    journal: &Journal,
    stop: &StopBroadcast,
    problem: String,
) -> Result<(), String> {
    journal.write(None, &format!("Not watching: {problem}"));
    write_status(board, journal, |status| status.problem = Some(problem));
    while !stop.wait(IDLE_WAIT) {}
    finish(board, journal);
    Ok(())
}

fn finish(board: &StatusBoard, journal: &Journal) {
    write_status(board, journal, |status| {
        status.stopped_at_ms = Some(now_ms())
    });
    journal.write(None, "Service stopped.");
}

/// A status that cannot be written is a line in the journal, never the end of the service:
/// the watching goes on whether or not anybody can read about it.
fn write_status(board: &StatusBoard, journal: &Journal, change: impl FnOnce(&mut ServiceStatus)) {
    if let Err(error) = board.update(change) {
        journal.write(
            None,
            &format!("The status file could not be written: {error}"),
        );
    }
}

fn write_context(
    board: &StatusBoard,
    journal: &Journal,
    context: &str,
    change: impl FnOnce(&mut ContextStatus),
) {
    if let Err(error) = board.update_context(context, change) {
        journal.write(
            Some(context),
            &format!("The status file could not be written: {error}"),
        );
    }
}

/// One context's watcher, for as long as the service runs.
struct ContextWorker<'a> {
    loaded: &'a LoadedIndexPolicy,
    context: &'a WorkspaceName,
    board: &'a StatusBoard,
    journal: &'a Journal,
    stop: &'a StopBroadcast,
    held: &'a HeldGraphs<'a>,
}

/// A context's graph, readable through the service for as long as this lives — released on
/// every way out of a watch, a panic included.
struct Holding<'a> {
    held: &'a HeldGraphs<'a>,
    context: &'a WorkspaceName,
}

impl Drop for Holding<'_> {
    fn drop(&mut self) {
        self.held.release(self.context);
    }
}

impl ContextWorker<'_> {
    fn run(&self) {
        let name = self.context.as_str();
        while !self.stop.wait(Duration::ZERO) {
            // A panic in one context is that context's failure: caught here, it is a line in
            // the journal and a retry, and the other contexts never notice. Uncaught, it would
            // end the service and every watcher in it.
            let attempt = panic::catch_unwind(AssertUnwindSafe(|| self.watch_once()))
                .unwrap_or_else(|_| Err("the watcher stopped on an internal error".to_string()));
            let Err(reason) = attempt else {
                break;
            };
            self.journal.write(
                Some(name),
                &format!(
                    "Not watching: {reason} Trying again in {}s.",
                    RETRY_AFTER.as_secs()
                ),
            );
            let at = now_ms();
            write_context(self.board, self.journal, name, |context| {
                context.enter(ContextPhase::Waiting, at);
                context.waiting = Some(reason.clone());
                context.note_error(at, reason);
            });
            if self.stop.wait(RETRY_AFTER) {
                break;
            }
        }
        let at = now_ms();
        write_context(self.board, self.journal, name, |context| {
            context.enter(ContextPhase::Stopped, at);
        });
    }

    /// One watch, from opening the context to the stop. `Ok` once it was stopped; `Err` with
    /// the reason when it could not start or could not go on.
    fn watch_once(&self) -> Result<(), String> {
        let name = self.context.as_str();
        let at = now_ms();
        write_context(self.board, self.journal, name, |context| {
            context.enter(ContextPhase::Starting, at);
            context.waiting = None;
        });
        let prepared =
            index_watch::prepare(self.loaded, self.context, None)?.scheduled(service_schedule());
        self.held.hold(self.context, Arc::clone(prepared.manager()));
        let _holding = Holding {
            held: self.held,
            context: self.context,
        };
        let (events, handle) = NotifyWatchSource::new(Some(default_probe_service_directory()?));
        self.stop.register(name, handle);
        self.announce(&prepared);

        let mut lines = JournalLines {
            journal: self.journal,
            context: name,
        };
        let mut narration = prepared.narration(&mut lines).without_stop_hint();
        let mut status = StatusObserver {
            board: self.board,
            journal: self.journal,
            context: name,
        };
        let outcome = prepared.run(
            &events,
            &mut Both {
                first: &mut narration,
                second: &mut status,
            },
        );
        for line in search_failure_lines(&prepared.search_failures()) {
            lines.warn(&line);
        }
        match outcome {
            Ok(report) | Err(WatchError::FinalPassFailed(report)) => {
                render_summary(&mut lines, &report, prepared.analysed());
                Ok(())
            }
            Err(error) => Err(format!("{error}.")),
        }
    }

    /// Says once, when the context starts, how many roots it has and whether its queue is
    /// worked — and that no model is called when it is not.
    fn announce(&self, prepared: &PreparedWatch<'_>) {
        let name = self.context.as_str();
        let analysis = prepared.announcement().map(str::to_string);
        if analysis.is_none() {
            self.journal.write(Some(name), ANALYSIS_OFF);
        }
        let roots = prepared.roots_to_watch().len();
        write_context(self.board, self.journal, name, |context| {
            context.roots = roots;
            context.analysis = analysis;
        });
    }
}

/// The sentence for a context whose queue the service leaves alone.
pub(super) const ANALYSIS_OFF: &str = "Analysis: off — `[watch] analyze` is not set, so the \
     queue is not worked and no model is called.";

/// The watcher's lines, into the journal under the context's name.
struct JournalLines<'a> {
    journal: &'a Journal,
    context: &'a str,
}

impl WatchLines for JournalLines<'_> {
    fn say(&mut self, line: &str) {
        self.journal.write(Some(self.context), line);
    }

    fn warn(&mut self, line: &str) {
        self.journal.write(Some(self.context), line);
    }
}

/// Two observers told the same thing: the narration and the status.
struct Both<'a, 'b> {
    first: &'a mut dyn WatchObserver,
    second: &'b mut dyn WatchObserver,
}

impl WatchObserver for Both<'_, '_> {
    fn started(&mut self, mode: WatchMode, delivery: EventDelivery) {
        self.first.started(mode, delivery);
        self.second.started(mode, delivery);
    }

    fn pass_finished(&mut self, record: &PassRecord) {
        self.first.pass_finished(record);
        self.second.pass_finished(record);
    }

    fn retry_scheduled(&mut self, delay: Duration) {
        self.first.retry_scheduled(delay);
        self.second.retry_scheduled(delay);
    }

    fn degraded(&mut self, periodic_interval: Duration) {
        self.first.degraded(periodic_interval);
        self.second.degraded(periodic_interval);
    }

    fn checkpoint_failed(&mut self, error: &RepositoryError) {
        self.first.checkpoint_failed(error);
        self.second.checkpoint_failed(error);
    }

    fn analysis_finished(&mut self, pass_number: usize, pass: &AnalysisPass) {
        self.first.analysis_finished(pass_number, pass);
        self.second.analysis_finished(pass_number, pass);
    }

    fn analysis_failed(&mut self, pass_number: usize, error: &RepositoryError) {
        self.first.analysis_failed(pass_number, error);
        self.second.analysis_failed(pass_number, error);
    }

    fn watching(&mut self, roots: &[RootWatch]) {
        self.first.watching(roots);
        self.second.watching(roots);
    }

    fn stopping(&mut self, final_pass: bool) {
        self.first.stopping(final_pass);
        self.second.stopping(final_pass);
    }
}

/// Keeps the context's entry of the status file in step with its watcher.
struct StatusObserver<'a> {
    board: &'a StatusBoard,
    journal: &'a Journal,
    context: &'a str,
}

impl StatusObserver<'_> {
    fn change(&self, change: impl FnOnce(&mut ContextStatus)) {
        write_context(self.board, self.journal, self.context, change);
    }
}

impl WatchObserver for StatusObserver<'_> {
    fn started(&mut self, _mode: WatchMode, delivery: EventDelivery) {
        let at = now_ms();
        self.change(|context| {
            context.enter(ContextPhase::Watching, at);
            match delivery {
                EventDelivery::Confirmed => {
                    context.watched_by = Some(WatchedBy::Events);
                    context.why_timer = None;
                }
                EventDelivery::Unconfirmed(reason) => {
                    context.watched_by = Some(WatchedBy::Timer);
                    context.why_timer = Some(unconfirmed_label(reason).to_string());
                }
            }
        });
    }

    fn watching(&mut self, roots: &[RootWatch]) {
        let refused: Vec<String> = roots
            .iter()
            .filter_map(|root| {
                root.failure()
                    .map(|failure| format!("{} — {failure}", root.root()))
            })
            .collect();
        self.change(|context| context.roots_without_events = refused);
    }

    fn pass_finished(&mut self, record: &PassRecord) {
        let at = now_ms();
        let line = describe_pass(record);
        self.change(|context| {
            context.last_pass = Some(TimedLine {
                at_ms: at,
                text: line.clone(),
            });
            match record.outcome() {
                PassOutcome::Completed(report) | PassOutcome::Incomplete(report) => {
                    context.not_walked = report.not_walked();
                    context.roots_not_permitted = report.roots_not_permitted();
                    context.paths_not_read = report.failed_paths();
                }
                PassOutcome::Failed(_) => context.note_error(at, line),
            }
            match record.outcome() {
                PassOutcome::Completed(_) => context.passes.completed += 1,
                PassOutcome::Incomplete(_) => context.passes.incomplete += 1,
                PassOutcome::Failed(_) => context.passes.failed += 1,
            }
        });
    }

    fn retry_scheduled(&mut self, _delay: Duration) {}

    fn degraded(&mut self, _periodic_interval: Duration) {
        let at = now_ms();
        self.change(|context| {
            context.watched_by = Some(WatchedBy::Timer);
            context.why_timer = Some(degraded_label().to_string());
            context.note_error(at, degraded_label().to_string());
        });
    }

    fn checkpoint_failed(&mut self, error: &RepositoryError) {
        let at = now_ms();
        let text = format!("folding a pass into the database file failed: {error}");
        self.change(|context| context.note_error(at, text));
    }

    fn analysis_finished(&mut self, _pass_number: usize, _pass: &AnalysisPass) {}

    fn analysis_failed(&mut self, pass_number: usize, error: &RepositoryError) {
        let at = now_ms();
        let text = format!("pass {pass_number} analysis did not run: {error}");
        self.change(|context| context.note_error(at, text));
    }

    fn stopping(&mut self, _final_pass: bool) {}
}

/// Raises the limit on open files launchd started this process with, and says so: the
/// service ran out of them once, and a checkpoint that cannot open its log leaves the graph
/// unopenable (ADR 0060).
fn open_files_line() -> String {
    match crate::infrastructure::raise_open_file_limit() {
        Ok(limit) if limit.now > limit.before => {
            format!(
                "Open files: up to {} (started with {}).",
                limit.now, limit.before
            )
        }
        Ok(limit) => format!("Open files: up to {}.", limit.now),
        Err(error) => format!("Open files: the limit could not be raised: {error}."),
    }
}
