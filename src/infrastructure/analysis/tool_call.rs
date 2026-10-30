// Running one installed tool once: what is written to it, what it wrote back, and how it is
// stopped when it does not come back.
//
// The process is put in a process group of its own and the whole group is killed on a
// timeout, because the tool is a runtime that starts children of its own and killing only
// the process we spawned would leave them. Stage 22 measured that this is not theoretical.
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Why a run of the tool produced nothing usable. Nothing the tool wrote is carried here:
/// its output is this context's content coming back, and none of it is anybody's business
/// downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolCallFailure {
    /// The binary could not be started at all.
    NotStarted,
    /// It did not answer inside the time one call is given, and was stopped.
    TimedOut,
    /// The call ended, but what the tool wrote could not be collected on this side. Nothing
    /// is known about the answer, because none of it arrived whole — which is why this is
    /// not the same as an answer that does not fit.
    AnswerUnread,
    /// What was collected is not an answer this program accepts. The tool was heard; it
    /// said something else.
    NotAnAnswer,
}

/// What one finished run left behind.
pub(super) struct ToolOutput {
    pub(super) stdout: String,
    pub(super) status_success: bool,
}

/// Runs `command`, writes `input` to its standard input and reads its standard output,
/// stopping it if `timeout` passes first.
///
/// Standard output is read on a thread of its own: a tool that writes more than a pipe
/// holds would block on the write while this side waited on the exit, and neither would
/// move again.
pub(super) fn run(
    mut command: Command,
    input: String,
    timeout: Duration,
) -> Result<ToolOutput, ToolCallFailure> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: `setpgid` is async-signal-safe and touches nothing this side owns. It runs in
    // the child between fork and exec, and makes the child the leader of its own group so
    // the group can be killed whole without the killer being in it.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| ToolCallFailure::NotStarted)?;

    let mut stdin = child.stdin.take().ok_or(ToolCallFailure::NotStarted)?;
    let writer = thread::spawn(move || {
        // A tool that answered before reading everything closes the pipe; that is its
        // answer, not a failure of this side.
        let _ = stdin.write_all(input.as_bytes());
        drop(stdin);
    });
    let mut stdout = child.stdout.take().ok_or(ToolCallFailure::NotStarted)?;
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let read = stdout.read_to_string(&mut text);
        let _ = sender.send(());
        (text, read.is_ok())
    });

    // The reader ends when the tool closes its output, which every exit does.
    if receiver.recv_timeout(timeout).is_err() {
        kill_group(&child);
        let _ = child.wait();
        let _ = writer.join();
        let _ = reader.join();
        return Err(ToolCallFailure::TimedOut);
    }
    let status = child.wait().map_err(|_| ToolCallFailure::AnswerUnread)?;
    let _ = writer.join();
    let (stdout, read_whole) = reader.join().map_err(|_| ToolCallFailure::AnswerUnread)?;
    if !read_whole {
        return Err(ToolCallFailure::AnswerUnread);
    }
    Ok(ToolOutput {
        stdout,
        status_success: status.success(),
    })
}

/// Kills the child and everything it started, by the group it leads.
///
/// `SIGKILL` rather than `SIGTERM`: this path is only reached because the tool already
/// failed to answer in the time it was given, and a runtime that is wedged is not going to
/// honour a polite request either.
fn kill_group(child: &Child) {
    let pid = child.id();
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: the pid is the child's, it leads its own group, and this process is not in
    // that group. A group that has already gone leaves `ESRCH`, which is not an error here.
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use super::{ToolCallFailure, run};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A script on disk, executable, standing in for the installed tool.
    fn script(body: &str) -> (PathBuf, PathBuf) {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-tool-call-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("fake-tool");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        (root, path)
    }

    #[test]
    fn what_the_tool_wrote_comes_back_whole() {
        let (root, path) = script("cat >/dev/null; printf '%s' '{\"ok\":true}'");
        let output = run(
            Command::new(&path),
            "sent".to_string(),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(output.stdout, "{\"ok\":true}");
        assert!(output.status_success);
        fs::remove_dir_all(root).unwrap();
    }

    /// The input reaches the tool even when it is larger than a pipe holds, because the
    /// writing happens on a thread of its own. Without that, a tool that reads all of its
    /// input before answering and this side waiting for the answer would each wait for the
    /// other.
    #[test]
    fn an_input_larger_than_a_pipe_still_arrives_whole() {
        let (root, path) = script("wc -c");
        let sent = "x".repeat(1_000_000);
        let output = run(Command::new(&path), sent, Duration::from_secs(30)).unwrap();
        assert_eq!(output.stdout.trim(), "1000000");
        fs::remove_dir_all(root).unwrap();
    }

    /// A tool that does not answer is stopped, and the wait is bounded by what it was given
    /// rather than by how long it felt like taking.
    #[test]
    fn a_tool_that_never_answers_is_stopped_at_the_timeout() {
        let (root, path) = script("sleep 60");
        let started = Instant::now();

        let failure = run(
            Command::new(&path),
            String::new(),
            Duration::from_millis(300),
        );

        assert_eq!(failure.err(), Some(ToolCallFailure::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the call must end at its timeout, not at the tool's: {:?}",
            started.elapsed()
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// And nothing it started outlives it.
    ///
    /// This is the case that makes the process group worth its complication: the tool is a
    /// runtime that starts children, and killing only the process this program spawned
    /// leaves them running with the content they were given. The child here writes a file
    /// after the parent is already dead, so a survivor leaves proof of itself.
    #[test]
    fn nothing_the_tool_started_outlives_the_timeout() {
        let (root, path) = script("(sleep 2; echo survived > \"$1\") & sleep 60");
        let witness = root.join("orphan-witness");

        let failure = run(
            {
                let mut command = Command::new(&path);
                command.arg(&witness);
                command
            },
            String::new(),
            Duration::from_millis(300),
        );

        assert_eq!(failure.err(), Some(ToolCallFailure::TimedOut));
        // Long enough that a survivor would have written by now.
        std::thread::sleep(Duration::from_secs(4));
        assert!(
            !witness.exists(),
            "a process the tool started outlived the call that was stopped"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Output that is not text at all never becomes an answer on this side, and that is a
    /// failure to read it rather than a verdict on it: the tool may well have answered
    /// correctly around the byte that could not be decoded. This is the one producer of
    /// `AnswerUnread` a test can reach — the other two need `wait` or a thread to fail —
    /// and it is the one a real tool can reach too, with a single stray byte.
    #[test]
    fn output_that_is_not_text_is_a_failure_to_read_it() {
        let (root, path) = script("cat >/dev/null; printf '\\377\\376'");

        let failure = run(Command::new(&path), String::new(), Duration::from_secs(30));

        assert_eq!(failure.err(), Some(ToolCallFailure::AnswerUnread));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_binary_that_is_not_there_is_told_apart_from_one_that_answered_badly() {
        let missing = std::env::temp_dir().join("crosskit-no-such-tool-ever");
        let failure = run(
            Command::new(missing),
            String::new(),
            Duration::from_secs(30),
        );
        assert_eq!(failure.err(), Some(ToolCallFailure::NotStarted));
    }
}
