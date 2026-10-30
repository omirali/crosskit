// Asking whether a context's graph opens, from a process that can afford the answer.
//
// The storage engine does not always refuse a log it cannot replay. On Linux it died of a
// signal on three of six databases left by a killed pass, and a process that dies takes its
// diagnosis with it: the operator sees `Segmentation fault` and nothing about what to do.
//
// So the question is asked by a child process running this same program, and the answer is
// its verdict line or the fact that it did not live to print one. The child pays whatever
// the open costs; the command that asked stays alive to say what it means.
//
// This runs only when a log is lying beside the database, which is the only state in which
// the engine has a log to choke on. Every other command, on every healthy context, spawns
// nothing.
use std::process::Command;

use crate::domain::WorkspaceName;

/// The verdict line the child prints, which is also what a person reading it sees.
pub(super) const VERDICT_PREFIX: &str = "Graph state: ";
pub(super) const OPENS: &str = "opens";
pub(super) const BUSY: &str = "busy";
pub(super) const LOG_UNREADABLE: &str = "log-unreadable";
pub(super) const FAILED: &str = "failed";

/// What a child process saw when it opened this context's graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GraphState {
    /// It opened. The child closed it again, which also folded its log into it.
    Opens,
    /// Another process holds it for writing.
    Busy,
    /// The engine will not replay the log beside it — or died trying, which says the same
    /// thing about this database and nothing about which of the two it was.
    LogUnreadable,
    /// Anything else, including being unable to ask.
    Unknown(String),
}

/// Runs the child and reads its verdict.
pub(super) fn probe(workspace: &WorkspaceName) -> GraphState {
    let Ok(program) = std::env::current_exe() else {
        return GraphState::Unknown("this program cannot find its own binary".to_string());
    };
    let output = Command::new(program)
        .args([
            "--context",
            workspace.as_str(),
            "index",
            "recover",
            "--check",
        ])
        .output();
    let Ok(output) = output else {
        return GraphState::Unknown("this program could not run itself".to_string());
    };
    // A child that died of a signal has no exit code. Nothing else about this program dies
    // that way, and the one thing the child does is open the graph.
    if output.status.code().is_none() {
        return GraphState::LogUnreadable;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(verdict) = stdout
        .lines()
        .find_map(|line| line.strip_prefix(VERDICT_PREFIX))
    else {
        return GraphState::Unknown(format!(
            "the check said nothing this program understands: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    };
    match verdict.split_whitespace().next().unwrap_or_default() {
        OPENS => GraphState::Opens,
        BUSY => GraphState::Busy,
        LOG_UNREADABLE => GraphState::LogUnreadable,
        _ => GraphState::Unknown(verdict.to_string()),
    }
}
