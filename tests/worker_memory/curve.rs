//! The four points of the curve, each in its own graph and its own process.
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{BENCH_CONTEXT, Bench};
use crate::measure::{Measurement, counter, measure_command};

/// How many jobs one process is asked to work. One job per revision, so this is also the
/// size of the corpus each point indexes.
///
/// `CROSSKIT_WORKER_MEMORY_POINTS` replaces them, as a comma-separated list, for a run that
/// needs to see further along the curve than the four points a report is built from.
const POINTS: [usize; 4] = [1_000, 5_000, 10_000, 20_000];

fn points() -> Vec<usize> {
    match std::env::var("CROSSKIT_WORKER_MEMORY_POINTS") {
        Ok(listed) => listed
            .split(',')
            .map(|point| {
                point
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("{point:?} is not a number of jobs"))
            })
            .collect(),
        Err(_) => POINTS.to_vec(),
    }
}

/// Each note is a few lines of prose: large enough to be indexed as content and cut into
/// one chunk, small enough that the whole corpus of twenty thousand is a few megabytes.
/// The growth being looked for is per job, not per byte, so the bytes are kept out of the
/// way on purpose.
const NOTE_BYTES: usize = 220;

/// The second shape: the same ten thousand jobs, worked a thousand at a time by ten
/// processes over one graph. It separates the two things the first shape cannot: memory
/// that belongs to how much the graph holds, and memory that belongs to how much one
/// process has done. If every one of the ten costs what a thousand-job process costs, the
/// growth is inside the process and not in the data.
const SPLIT_TOTAL: usize = 10_000;
const SPLIT_BATCH: usize = 1_000;

pub fn run() {
    let bench = Bench::create("worker-memory");
    println!("benchmark directory: {}", bench.directory.display());
    let mut rows = Vec::new();
    for jobs in points() {
        rows.push(measure_point(&bench, jobs));
    }
    report(&rows);
    if std::env::var_os("CROSSKIT_WORKER_MEMORY_POINTS").is_none() {
        report_split(&measure_split(&bench));
    }
}

/// The attribution, on its own: what one transaction of each shape costs, against the
/// engine directly. Separate from the curve because it answers a different question and
/// takes minutes rather than a quarter of an hour.
pub fn attribute() {
    let bench = Bench::create("transaction-shapes");
    println!("benchmark directory: {}", bench.directory.display());
    report_shapes(&measure_transaction_shapes(&bench));
}

/// The same number of equally small transactions, written two ways, each in its own
/// process. Against the engine directly, with no Crosskit above it, so what it shows
/// belongs to the engine and to nothing this program decides.
fn measure_transaction_shapes(bench: &Bench) -> Vec<Measurement> {
    use crate::worker_memory::transactions::{
        CHECKPOINTED, FRESH_CONNECTION, ONE_CONNECTION, SHAPE_VARIABLE, TRANSACTIONS,
        WITH_RELATIONSHIP,
    };

    let worker = std::env::current_exe().expect("the benchmark knows its own binary");
    [
        FRESH_CONNECTION,
        ONE_CONNECTION,
        WITH_RELATIONSHIP,
        CHECKPOINTED,
    ]
    .into_iter()
    .map(|shape| {
        let database = bench
            .directory
            .join(format!("transactions-{}.lbug", shape.replace(' ', "-")));
        let mut command = std::process::Command::new(&worker);
        command
            .arg("--ignored")
            .arg("--exact")
            .arg("--nocapture")
            .arg("worker_memory_transaction_worker")
            .env("CROSSKIT_TRANSACTION_DB", &database)
            .env(SHAPE_VARIABLE, shape);
        measure_command(&format!("{TRANSACTIONS} transactions, {shape}"), command)
    })
    .collect()
}

/// One graph of `SPLIT_TOTAL` jobs, worked `SPLIT_BATCH` at a time, each batch in a process
/// of its own.
fn measure_split(bench: &Bench) -> Vec<Measurement> {
    let root = bench.directory.join("corpus-split");
    let data = bench.directory.join("data-split");
    write_corpus(&root, SPLIT_TOTAL);
    fs::create_dir(&data).unwrap();
    let point = Point::new(bench, &root, &data);
    point.run_ok(&["index", "reconcile", root.to_str().unwrap()]);

    let limit = SPLIT_BATCH.to_string();
    let mut batches = Vec::new();
    let mut worked = 0;
    while worked < SPLIT_TOTAL {
        let measurement = point.measured(
            &format!("jobs {}..{}", worked + 1, worked + SPLIT_BATCH),
            &["index", "jobs", "run", "--limit", &limit],
        );
        if measurement.signal.is_some() {
            batches.push(measurement);
            break;
        }
        worked += SPLIT_BATCH;
        batches.push(measurement);
    }
    batches
}

/// One point: a fresh corpus and a fresh graph, indexed once so the queue is full, then one
/// measured process that works the whole queue.
///
/// Fresh rather than cumulative because a graph that already held the results of the point
/// before would make every later point carry its predecessor, and the curve would rise for
/// a reason that has nothing to do with the worker.
fn measure_point(bench: &Bench, jobs: usize) -> Row {
    let root = bench.directory.join(format!("corpus-{jobs}"));
    let data = bench.directory.join(format!("data-{jobs}"));
    write_corpus(&root, jobs);
    fs::create_dir(&data).unwrap();
    let point = Point::new(bench, &root, &data);

    let indexed = point.run_ok(&["index", "reconcile", root.to_str().unwrap()]);
    assert_eq!(
        counter(&indexed, "Indexed files (metadata and content):"),
        Some(jobs),
        "the corpus of {jobs} must be indexed as content:\n{indexed}"
    );
    let queued = counter(&point.run_ok(&["index", "jobs"]), "Insight jobs queued:");
    assert_eq!(queued, Some(jobs), "one job per revision");

    let limit = jobs.to_string();
    let measurement = point.measured(
        &format!("{jobs} jobs"),
        &["index", "jobs", "run", "--limit", &limit],
    );
    let done = counter(&measurement.stdout, "Completed jobs:");
    if measurement.signal.is_none() {
        assert_eq!(
            done,
            Some(jobs),
            "a run that was not killed must work every job it was given: {}",
            measurement.outcome()
        );
    }
    Row {
        jobs,
        database_bytes: file_bytes(&data.join(format!("graph-{}.lbug", BENCH_CONTEXT))),
        log_bytes: file_bytes(&data.join(format!("graph-{}.lbug.wal", BENCH_CONTEXT))),
        measurement,
    }
}

/// One point's own data directory and policy file. `Bench` owns the benchmark directory;
/// each point needs a graph of its own beside it, and a policy that covers only its corpus.
struct Point {
    data_directory: PathBuf,
    config_path: PathBuf,
}

impl Point {
    fn new(_bench: &Bench, root: &Path, data_directory: &Path) -> Self {
        let config_path = data_directory.join("config.toml");
        let covered = fs::canonicalize(root).unwrap();
        fs::write(
            &config_path,
            format!(
                "[workspace.{}]\ninclude = [\"{}\"]\n",
                BENCH_CONTEXT,
                covered.display()
            ),
        )
        .unwrap();
        Self {
            data_directory: data_directory.to_path_buf(),
            config_path,
        }
    }

    fn command(&self, arguments: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_crosskit"));
        command
            .arg("--context")
            .arg(BENCH_CONTEXT)
            .args(arguments)
            .env("CROSSKIT_DATA_DIR", &self.data_directory)
            .env("CROSSKIT_CONFIG", &self.config_path)
            .env("NO_COLOR", "1");
        command
    }

    fn run_ok(&self, arguments: &[&str]) -> String {
        let output = self.command(arguments).output().unwrap();
        assert!(
            output.status.success(),
            "{arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn measured(&self, label: &str, arguments: &[&str]) -> Measurement {
        measure_command(label, self.command(arguments))
    }
}

struct Row {
    jobs: usize,
    database_bytes: u64,
    log_bytes: u64,
    measurement: Measurement,
}

fn write_corpus(root: &Path, notes: usize) {
    fs::create_dir(root).unwrap();
    for note in 0..notes {
        let mut text = format!("# note {note}\n\n");
        let subject = note % 97;
        while text.len() < NOTE_BYTES {
            text.push_str(&format!(
                "This note is about subject {subject} and was written for the memory curve. "
            ));
        }
        text.push('\n');
        fs::write(root.join(format!("note-{note:06}.md")), text).unwrap();
    }
}

fn file_bytes(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

fn report(rows: &[Row]) {
    println!();
    println!("jobs | outcome | wall | peak RSS | our allocations | database | log");
    for row in rows {
        println!(
            "{:>6} | {} | {:.2}s | {} | {} | {} | {}",
            row.jobs,
            row.measurement.outcome(),
            row.measurement.duration.as_secs_f64(),
            if row.measurement.peak_rss_bytes == 0 {
                "not measured".to_string()
            } else {
                megabytes(row.measurement.peak_rss_bytes)
            },
            match row.measurement.allocation_peak_bytes {
                Some(bytes) => megabytes(bytes),
                None => "not built with allocation-profile".to_string(),
            },
            megabytes(row.database_bytes),
            megabytes(row.log_bytes),
        );
    }
    println!();
    for row in rows {
        let stderr = row.measurement.program_stderr();
        if !stderr.is_empty() {
            println!("{} stderr: {stderr}", row.jobs);
        }
    }
}

fn report_shapes(shapes: &[Measurement]) {
    println!();
    println!("against the engine directly, one process per shape:");
    println!("shape | outcome | wall | peak RSS");
    for measurement in shapes {
        println!(
            "{} | {} | {:.2}s | {}",
            measurement.label,
            measurement.outcome(),
            measurement.duration.as_secs_f64(),
            if measurement.peak_rss_bytes == 0 {
                "not measured".to_string()
            } else {
                megabytes(measurement.peak_rss_bytes)
            },
        );
    }
}

fn report_split(batches: &[Measurement]) {
    println!();
    println!("the same {SPLIT_TOTAL} jobs over one graph, {SPLIT_BATCH} to a process:");
    println!("batch | outcome | wall | peak RSS | done");
    for measurement in batches {
        println!(
            "{} | {} | {:.2}s | {} | {}",
            measurement.label,
            measurement.outcome(),
            measurement.duration.as_secs_f64(),
            if measurement.peak_rss_bytes == 0 {
                "not measured".to_string()
            } else {
                megabytes(measurement.peak_rss_bytes)
            },
            counter(&measurement.stdout, "Completed jobs:")
                .map(|done| done.to_string())
                .unwrap_or_else(|| "-".to_string()),
        );
    }
}
