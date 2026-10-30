//! Shared by more than one benchmark, each of which uses a part of it; the parts a
//! given benchmark does not use are not dead code.
#![allow(dead_code)]

// Running one command in its own process and recording what it cost. Peak resident set
// size comes from the operating system, never from the process measuring itself, so the
// benchmark runner's own memory can never be mistaken for the indexer's.
//
// It is read with `wait4`, which reports the rusage of exactly the child that was waited
// for, on every Unix. The earlier `/usr/bin/time -l` reported the same `getrusage` field
// but exists only on macOS, so every run under a Linux container printed "peak RSS not
// measured on this platform" — precisely where a memory limit makes the number matter.
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// What one measured process cost.
#[derive(Debug, Clone)]
pub struct Measurement {
    pub label: String,
    pub duration: Duration,
    pub peak_rss_bytes: u64,
    /// The peak sum of live Rust allocations the process reported, when it was built with
    /// the `allocation-profile` feature. An ordinary build reports nothing and this is
    /// `None`, which is what a released Crosskit always is.
    pub allocation_peak_bytes: Option<u64>,
    pub exit_code: Option<i32>,
    /// The signal that ended the process, when one did. A run the system killed and a run
    /// that failed and said why are different results, and a benchmark that cannot tell
    /// them apart reports neither honestly.
    pub signal: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Measurement {
    pub fn peak_rss_mib(&self) -> f64 {
        self.peak_rss_bytes as f64 / (1024.0 * 1024.0)
    }

    /// `exit 0`, or how the run ended when it did not exit on its own. Under a hard memory
    /// limit the kernel ends a process with a signal and leaves no exit code at all, and a
    /// report that shows only an absent code hides exactly what such a run measures.
    pub fn outcome(&self) -> String {
        match (self.exit_code, self.signal) {
            (_, Some(signal)) => format!("killed by signal {signal}"),
            (Some(code), None) => format!("exit {code}"),
            (None, None) => "no exit status".to_string(),
        }
    }

    /// What the measured program itself wrote to stderr, on one line. Nothing else writes
    /// there any more: the child is spawned directly, so its stderr is its own.
    pub fn program_stderr(&self) -> String {
        self.stderr
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Runs `program` with `arguments` in its own process and records what it cost.
pub fn measure(
    label: &str,
    program: &Path,
    arguments: &[&str],
    environment: &[(&str, &Path)],
) -> Measurement {
    let mut command = Command::new(program);
    command.args(arguments);
    for (name, value) in environment {
        command.env(name, value);
    }
    measure_command(label, command)
}

/// The same for a command the caller has already configured, which is the one thing a
/// benchmark that spawns its own worker needs.
///
/// The child is spawned directly and waited for with `wait4`, so `ru_maxrss` belongs to
/// that child and to nothing else: `getrusage(RUSAGE_CHILDREN)` would carry the high-water
/// mark of every child reaped before it, and a curve taken that way could never come down.
///
/// Its output goes to files rather than pipes. A pipe fills at 64 KiB and the writer then
/// blocks, and this function waits before it reads, so a talkative child under `--nocapture`
/// would deadlock instead of being measured.
pub fn measure_command(label: &str, mut command: Command) -> Measurement {
    let (out_path, err_path) = scratch_paths();
    command
        .stdin(Stdio::null())
        .stdout(File::create(&out_path).unwrap())
        .stderr(File::create(&err_path).unwrap());

    let started = Instant::now();
    let child = command.spawn().unwrap();
    let pid = child.id() as libc::pid_t;
    let mut status: libc::c_int = 0;
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let waited = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
    assert_eq!(waited, pid, "wait4 must reap the child it was given");
    // Already reaped here, so `Child` must never wait again: a pid the kernel has recycled
    // would belong to somebody else by then.
    std::mem::forget(child);
    let duration = started.elapsed();

    Measurement {
        label: label.to_string(),
        duration,
        peak_rss_bytes: max_resident_bytes(&usage),
        allocation_peak_bytes: None,
        exit_code: exited(status),
        signal: terminated_by(status),
        stdout: read_and_remove(&out_path),
        stderr: read_and_remove(&err_path),
    }
    .with_allocation_peak()
}

impl Measurement {
    /// The allocation profile a binary built with `allocation-profile` prints as it ends.
    fn with_allocation_peak(mut self) -> Self {
        self.allocation_peak_bytes = allocation_peak(&self.stderr);
        self
    }
}

/// `ru_maxrss` is in bytes on macOS and in kilobytes on Linux. One place knows that, so a
/// report cannot mix the two and call the difference a measurement.
fn max_resident_bytes(usage: &libc::rusage) -> u64 {
    let max_resident = usage.ru_maxrss.max(0) as u64;
    if cfg!(target_os = "macos") {
        max_resident
    } else {
        max_resident * 1024
    }
}

fn exited(status: libc::c_int) -> Option<i32> {
    libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status))
}

/// The signal that ended the child, when one did. Under a hard memory limit this is the
/// whole result: the kernel leaves no exit code at all.
fn terminated_by(status: libc::c_int) -> Option<i32> {
    libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status))
}

/// A pair of file names nothing else in this process uses.
fn scratch_paths() -> (PathBuf, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let stem = format!("crosskit-measure-{}-{sequence}", std::process::id());
    let directory = std::env::temp_dir();
    (
        directory.join(format!("{stem}.out")),
        directory.join(format!("{stem}.err")),
    )
}

fn read_and_remove(path: &Path) -> String {
    let mut text = String::new();
    File::open(path)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let _ = std::fs::remove_file(path);
    text
}

/// The peak sum of live Rust allocations a binary built with `allocation-profile` prints
/// when it ends. An ordinary build prints no such line, and this is then `None` rather than
/// a zero that would read like a measurement.
pub fn allocation_peak(report: &str) -> Option<u64> {
    report.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Allocation profile: peak ")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

/// A counter the measured command printed, such as `Indexed files (…): 12`.
pub fn counter(stdout: &str, prefix: &str) -> Option<usize> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .and_then(|rest| rest.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{allocation_peak, max_resident_bytes};

    #[test]
    fn an_ordinary_build_reports_no_allocation_peak() {
        assert_eq!(allocation_peak("Indexed files: 3\n"), None);
        assert_eq!(
            allocation_peak(
                "Allocation profile: peak 4096 bytes, live at exit 0 bytes, allocations 7\n"
            ),
            Some(4096)
        );
    }

    #[test]
    fn the_resident_peak_is_read_in_the_unit_of_this_platform() {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        usage.ru_maxrss = 504_332;
        let expected = if cfg!(target_os = "macos") {
            504_332
        } else {
            504_332 * 1024
        };
        assert_eq!(max_resident_bytes(&usage), expected);
        // A kernel that reported nothing is zero, never a negative turned into a huge
        // unsigned number.
        usage.ru_maxrss = -1;
        assert_eq!(max_resident_bytes(&usage), 0);
    }
}
