//! The measurement vocabulary the Stage 18 benchmarks share: where a run puts its
//! directories, what counts as a process that finished, and how one measured process is
//! written into a table.
//!
//! One place, because two benchmarks that printed "killed by signal 9" and "not measured"
//! by two different rules would be two benchmarks nobody could compare.
use std::fs;
use std::path::PathBuf;

use crate::cli::{ScanCounters, megabytes};
use crate::measure::Measurement;

/// `CROSSKIT_SCALE_DIR` names the directory holding every point of a run; otherwise one is
/// created under the temp directory. It must not exist yet.
pub fn run_directory(stem: &str) -> PathBuf {
    let directory = match std::env::var_os("CROSSKIT_SCALE_DIR") {
        Some(path) => PathBuf::from(path),
        None => std::env::temp_dir().join(format!("crosskit-{stem}-{}", std::process::id())),
    };
    fs::create_dir(&directory).unwrap_or_else(|error| {
        panic!("run directory {} must be new: {error}", directory.display())
    });
    directory
}

/// A process that exited zero. A process the kernel killed leaves no exit code at all, and
/// its output is whatever it had printed by then — not counters anything may be asserted
/// against.
pub fn finished(measurement: &Measurement) -> bool {
    measurement.signal.is_none() && measurement.exit_code == Some(0)
}

pub fn finished_counters(measurement: &Measurement) -> Option<ScanCounters> {
    finished(measurement).then(|| ScanCounters::parse(&measurement.stdout))
}

/// One measured process as one line of a table: how it ended, how long it took, how much
/// memory it held and how much of that was ours.
pub fn cell(measurement: Option<&Measurement>) -> String {
    let Some(measurement) = measurement else {
        return "not run | - | - | -".to_string();
    };
    format!(
        "{} | {:.1}s | {} | {}",
        measurement.outcome(),
        measurement.duration.as_secs_f64(),
        peak_rss(measurement),
        allocated(measurement),
    )
}

pub fn peak_rss(measurement: &Measurement) -> String {
    if measurement.peak_rss_bytes == 0 {
        return "not measured".to_string();
    }
    megabytes(measurement.peak_rss_bytes)
}

/// What Crosskit's own code allocated, in bytes rather than mebibytes: this number is
/// compared between points down to a handful of bytes, and a tenth of a mebibyte would hide
/// the whole result.
pub fn allocated(measurement: &Measurement) -> String {
    match measurement.allocation_peak_bytes {
        Some(bytes) => format!("{bytes} B"),
        None => "not built with allocation-profile".to_string(),
    }
}

/// How much bigger than the first point, or a plain dash where nothing was measured.
pub fn ratio(value: f64, first: f64) -> String {
    if first <= 0.0 || value <= 0.0 {
        return "-".to_string();
    }
    format!("{value_over_first:.2}x", value_over_first = value / first)
}

/// A list of numbers from the environment, for a run that needs other points than the ones
/// a benchmark is built around.
pub fn numbers_from_env(variable: &str) -> Option<Vec<usize>> {
    let listed = std::env::var(variable).ok()?;
    Some(
        listed
            .split(',')
            .map(|point| {
                point
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("{variable}: {point:?} is not a number"))
            })
            .collect(),
    )
}
