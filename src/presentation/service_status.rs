// `service status` and `service logs`: what the service is doing, read from what it wrote and
// from launchd — never by opening a graph, so it answers while every graph is held.
use std::path::Path;

use crate::infrastructure::service::{
    ContextPhase, ContextStatus, Journal, Launchctl, ServiceLayout, ServiceStatus, WatchedBy,
    content_digest, file_stamp, instance_holder, now_ms, read_status,
};
use crate::infrastructure::utc_moment;

use super::service_run::ANALYSIS_OFF;

/// How many lines of the journal `logs` shows: about one screen.
const TAIL_LINES: usize = 40;

/// A later build by less than this is the same build: a copy keeps its source's time to the
/// precision the filesystem gives, which is not always the millisecond.
const SAME_BUILD_SLACK_MS: i64 = 2_000;

pub(super) fn status() -> Result<(), String> {
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    let installed = layout.launch_agent().exists();
    let holder = instance_holder(&layout.instance_lock());
    let Some(pid) = holder else {
        return not_running(installed);
    };
    let recorded = read_status(&layout.status())
        .map_err(|error| format!("the status file could not be read: {error}"))?
        .filter(|status| i64::from(status.pid) == i64::from(pid));
    let Some(status) = recorded else {
        println!(
            "Service: running as process {pid}, and it has not written its status yet. Ask again \
             in a moment; `crosskit service logs` shows what it has said."
        );
        return Ok(());
    };

    let now = now_ms();
    println!(
        "Service: running since {} ({}), process {pid}{}.",
        utc_moment(status.started_at_ms),
        ago(now, status.started_at_ms),
        if installed {
            ""
        } else {
            ", started by hand rather than by launchd — `crosskit service install` has launchd \
             start it at login"
        }
    );
    describe_program(&layout, &status);
    describe_configuration(&status);
    if let Some(problem) = &status.problem {
        println!("Not watching: {problem}");
    }
    if let Some(reads) = &status.reads {
        println!("Reads: {reads}.");
    }
    println!("Journal: {}", layout.journal().display());
    for context in &status.contexts {
        println!();
        describe_context(context, &status.program, now);
    }
    Ok(())
}

fn not_running(installed: bool) -> Result<(), String> {
    if !installed {
        println!(
            "Service: not installed. `crosskit service install` installs it: it watches every \
             context from login on, with no terminal open."
        );
        return Ok(());
    }
    let launchd = Launchctl::new().state()?;
    let exit = launchd.last_exit.map_or_else(String::new, |exit| {
        format!(" Its last process ended with {exit}.")
    });
    println!(
        "Service: installed, not running.{exit} `crosskit service logs` shows what it said last; \
         `crosskit service restart` starts it again."
    );
    if !launchd.loaded {
        println!(
            "launchd does not have it loaded — it was stopped or never started in this login \
             session."
        );
    }
    Ok(())
}

/// Which build runs, and whether a newer one is waiting.
fn describe_program(layout: &ServiceLayout, status: &ServiceStatus) {
    println!(
        "Program: {}, build of {}.",
        status.program,
        utc_moment(status.program_build.modified_ms)
    );
    let this = std::env::current_exe()
        .ok()
        .and_then(|this| file_stamp(&this).ok());
    if let Some(this) = this
        && this.modified_ms > status.program_build.modified_ms + SAME_BUILD_SLACK_MS
    {
        println!(
            "  This crosskit is a newer build, of {}. `crosskit service restart` copies it and \
             restarts the service on it.",
            utc_moment(this.modified_ms)
        );
        return;
    }
    let installed = file_stamp(&layout.program()).ok();
    if installed.is_some_and(|installed| installed != status.program_build) {
        println!(
            "  The program copy was replaced after the service started; `crosskit service \
             restart` runs it."
        );
    }
}

fn describe_configuration(status: &ServiceStatus) {
    let now = content_digest(Path::new(&status.config)).ok();
    if now == status.config_digest {
        println!(
            "Configuration: {}, unchanged since the start.",
            status.config
        );
        return;
    }
    println!(
        "Configuration: {}, changed after the start. The service still watches what it read \
         then; `crosskit service restart` reads it again.",
        status.config
    );
}

fn describe_context(context: &ContextStatus, program: &str, now: i64) {
    println!("Context {}: {}", context.name, phase_line(context, now));
    println!("  Roots: {}", context.roots);
    if !context.roots_without_events.is_empty() {
        println!(
            "  No change notifications for {} of them; every pass still walks them:",
            context.roots_without_events.len()
        );
        for root in &context.roots_without_events {
            println!("    {root}");
        }
    }
    match &context.last_pass {
        Some(pass) => println!("  Last pass: {} — {}", ago(now, pass.at_ms), pass.text),
        None => println!("  Last pass: none yet."),
    }
    println!(
        "  Passes: {} completed, {} incomplete, {} failed.",
        context.passes.completed, context.passes.incomplete, context.passes.failed
    );
    if !context.not_walked.is_empty() {
        println!("  Not walked by the last pass:");
        for root in &context.not_walked {
            println!("    {root}");
        }
    }
    if context.roots_not_permitted > 0 {
        println!(
            "  macOS did not let the service open {} root(s): a process launchd starts does not \
             share the terminal's access to Documents, Desktop or Downloads. Give {program} \
             Full Disk Access (System Settings → Privacy & Security), then `crosskit service \
             restart`.",
            context.roots_not_permitted
        );
    }
    if context.paths_not_read > 0 {
        println!(
            "  Paths the last pass could not read: {}.",
            context.paths_not_read
        );
    }
    match &context.analysis {
        Some(analysis) => println!("  {analysis}"),
        None => println!("  {ANALYSIS_OFF}"),
    }
    if !context.errors.is_empty() {
        println!("  Latest errors:");
        for error in &context.errors {
            println!("    {}: {}", ago(now, error.at_ms), error.text);
        }
    }
}

fn phase_line(context: &ContextStatus, now: i64) -> String {
    let since = ago(now, context.since_ms);
    match context.phase {
        ContextPhase::Starting => {
            format!("starting — opening the graph, subscribing or in its first pass ({since})")
        }
        ContextPhase::Watching => match (context.watched_by, &context.why_timer) {
            (Some(WatchedBy::Timer), Some(why)) => {
                format!("watching on the timer alone, since {since}: {why}")
            }
            _ => format!("watching on change events, since {since}"),
        },
        ContextPhase::Waiting => format!(
            "not watching ({since}): {} It is tried again every minute.",
            context
                .waiting
                .as_deref()
                .unwrap_or("no reason was recorded.")
        ),
        ContextPhase::Stopped => format!("stopped ({since})"),
    }
}

pub(super) fn logs() -> Result<(), String> {
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    let journal = layout.journal();
    println!(
        "Journal: {} (at most two files of a megabyte: this one and the one before it, .1).",
        journal.display()
    );
    let captured = layout.launchd_output();
    if std::fs::metadata(&captured).is_ok_and(|metadata| metadata.len() > 0) {
        println!(
            "What launchd caught besides it, a crash or a refusal before the journal opened: {}",
            captured.display()
        );
    }
    let lines = match Journal::tail(&journal, TAIL_LINES) {
        Ok(lines) => lines,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("Nothing written yet: the service has not run with this data directory.");
            return Ok(());
        }
        Err(error) => return Err(format!("the journal could not be read: {error}")),
    };
    println!();
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

/// `12 s ago`, `5 min ago`, `3 h 20 min ago`, `2 d 4 h ago`.
fn ago(now: i64, then: i64) -> String {
    let seconds = (now - then).max(0) / 1000;
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3600 => format!("{minutes} min ago"),
        3600..86_400 => format!("{} h {minutes} min ago", seconds / 3600),
        _ => format!("{days} d {hours} h ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn time_since_reads_in_the_largest_two_units() {
        assert_eq!(ago(12_000, 0), "12 s ago");
        assert_eq!(ago(300_000, 0), "5 min ago");
        assert_eq!(ago(12_000_000, 0), "3 h 20 min ago");
        assert_eq!(ago(187_200_000, 0), "2 d 4 h ago");
        assert_eq!(ago(0, 5_000), "0 s ago");
    }
}
