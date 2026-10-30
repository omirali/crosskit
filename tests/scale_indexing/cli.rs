//! Shared by more than one benchmark, each of which uses a part of it; the parts a
//! given benchmark does not use are not dead code.
#![allow(dead_code)]

// Driving the release binary against an isolated data directory, parsing its counters,
// timing each phase and reading the graph directly for invariants the CLI does not print.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use lbug::{Connection, Database, SystemConfig, Value};

use super::measure::Measurement;

/// The context every benchmark works in. A benchmark run needs one like any other run, so
/// it declares one in the policy file it writes and names it on every command rather than
/// reaching the storage around the rules.
pub const BENCH_CONTEXT: &str = "bench";

pub struct Bench {
    pub directory: PathBuf,
    pub data_directory: PathBuf,
    config_path: PathBuf,
}

impl Bench {
    /// `CROSSKIT_SCALE_DIR` names the benchmark directory; otherwise one is created under
    /// the temp directory. It must not exist yet, and it is kept after the run.
    pub fn create(profile: &str) -> Self {
        let directory = match std::env::var_os("CROSSKIT_SCALE_DIR") {
            Some(path) => PathBuf::from(path),
            None => std::env::temp_dir()
                .join(format!("crosskit-scale-{profile}-{}", std::process::id())),
        };
        Self::create_at(directory)
    }

    /// A benchmark of its own named `name` inside `parent`. A curve needs one per point,
    /// because each point indexes its own corpus into its own graph, and then it is the
    /// directory holding all of them that `CROSSKIT_SCALE_DIR` names.
    pub fn create_in(parent: &Path, name: &str) -> Self {
        Self::create_at(parent.join(name))
    }

    fn create_at(directory: PathBuf) -> Self {
        fs::create_dir(&directory).unwrap_or_else(|error| {
            panic!(
                "benchmark directory {} must be new: {error}",
                directory.display()
            )
        });
        let data_directory = directory.join("data");
        fs::create_dir(&data_directory).unwrap();
        Self {
            config_path: data_directory.join("config.toml"),
            directory,
            data_directory,
        }
    }

    /// `workspace_root` is the tree the benchmark indexes; it is canonicalized because the
    /// scan resolves its root and a macOS temp path resolves through `/private`, so a
    /// pattern written from the unresolved path would cover nothing.
    pub fn write_config(
        &self,
        workspace_root: &Path,
        metadata_directory: &Path,
        max_content_mb: u64,
    ) {
        let covered = fs::canonicalize(workspace_root)
            .unwrap_or_else(|error| panic!("canonicalize {}: {error}", workspace_root.display()));
        fs::write(
            &self.config_path,
            format!(
                "[index]\nmetadata_only = [\"{}\"]\nmax_content_size_mb = {max_content_mb}\n\n{}[workspace.{BENCH_CONTEXT}]\ninclude = [\"{}\"]\n",
                metadata_directory.display(),
                declared_languages(),
                covered.display()
            ),
        )
        .unwrap();
    }

    /// A `[search]` section when the run declares one, so the same benchmark can be run
    /// with another set of languages and the two compared.
    ///
    ///   CROSSKIT_SCALE_LANGUAGES=ru,en,kk
    ///
    /// Nothing is guessed from the corpus: without the variable the configuration says
    /// nothing about languages and the defaults apply, exactly as in every run before this
    /// one.
    pub fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_crosskit"));
        command
            .args(Self::in_context(arguments))
            .env("CROSSKIT_DATA_DIR", &self.data_directory)
            .env("CROSSKIT_CONFIG", &self.config_path)
            .env("NO_COLOR", "1");
        command
    }

    pub fn run(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }

    pub fn run_ok(&self, arguments: &[&str]) -> String {
        let output = self.run(arguments);
        assert!(
            output.status.success(),
            "{arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    pub fn timed(&self, arguments: &[&str]) -> (Output, Duration) {
        let started = Instant::now();
        let output = self.run(arguments);
        (output, started.elapsed())
    }

    /// Every command names the context it works in, so a benchmark exercises the same path
    /// an operator does.
    fn in_context(arguments: &[&str]) -> Vec<String> {
        let mut named = vec!["--context".to_string(), BENCH_CONTEXT.to_string()];
        named.extend(arguments.iter().map(|argument| (*argument).to_string()));
        named
    }

    /// Runs one command of a benchmark in its own process, timed and with the peak
    /// resident set size that process reached. Measuring each command separately is what
    /// lets a report say which phase holds the peak instead of giving one number for a
    /// whole run.
    pub fn measured(&self, label: &str, arguments: &[&str]) -> Measurement {
        let named = Self::in_context(arguments);
        super::measure::measure(
            label,
            Path::new(env!("CARGO_BIN_EXE_crosskit")),
            &named.iter().map(String::as_str).collect::<Vec<_>>(),
            &[
                ("CROSSKIT_DATA_DIR", self.data_directory.as_path()),
                ("CROSSKIT_CONFIG", self.config_path()),
            ],
        )
    }

    pub fn queued_jobs(&self) -> usize {
        counter(&self.run_ok(&["index", "jobs"]), "Insight jobs queued:")
    }

    pub fn schema_version(&self) -> u64 {
        counter(&self.run_ok(&["doctor"]), "Schema version:") as u64
    }

    /// The policy file this benchmark wrote, for a runner that spawns the binary itself.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_directory.join(Self::database_file_name())
    }

    fn database_file_name() -> String {
        format!("graph-{BENCH_CONTEXT}.lbug")
    }

    /// The database file plus its write-ahead log.
    pub fn database_bytes(&self) -> u64 {
        fs::read_dir(&self.data_directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&Self::database_file_name())
            })
            .map(|entry| entry.metadata().unwrap().len())
            .sum()
    }

    pub fn graph(&self) -> GraphCounts {
        GraphCounts::read(&self.database_path())
    }

    /// The search index of this context, on disk.
    pub fn search_index_bytes(&self) -> u64 {
        directory_bytes(&self.data_directory.join(format!("search-{BENCH_CONTEXT}")))
    }

    /// What `doctor` says about the search index against the graph.
    pub fn search_status(&self) -> SearchStatus {
        SearchStatus::parse(&self.run_ok(&["doctor"]))
    }
}

fn directory_bytes(directory: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                directory_bytes(&path)
            } else {
                path.metadata().map(|meta| meta.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// The two lines `doctor` prints about search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchStatus {
    pub documents: u64,
    pub chunks: u64,
    pub in_step: bool,
    pub matches_this_build: bool,
}

impl SearchStatus {
    fn parse(stdout: &str) -> Self {
        let line = stdout
            .lines()
            .find_map(|line| line.strip_prefix("Search index: "))
            .unwrap_or_else(|| panic!("no search index line in:\n{stdout}"));
        let numbers: Vec<u64> = line
            .split_whitespace()
            .filter_map(|word| word.parse().ok())
            .collect();
        assert_eq!(numbers.len(), 2, "unexpected search index line: {line}");
        Self {
            documents: numbers[0],
            chunks: numbers[1],
            in_step: line.contains("in step with the graph"),
            matches_this_build: !stdout.contains("the index was written by"),
        }
    }
}

/// The counters `index init` and `index reconcile` print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanCounters {
    pub indexed_files: usize,
    pub indexed_chunks: usize,
    pub metadata_only_files: usize,
    pub content_skipped_files: usize,
    pub unstable_sources: usize,
    pub excluded_paths: usize,
    pub unrepresentable_names: usize,
    pub failed_paths: usize,
    pub marked_missing: Option<usize>,
    pub relocated: Option<usize>,
}

impl ScanCounters {
    pub fn parse(stdout: &str) -> Self {
        Self {
            indexed_files: counter(stdout, "Indexed files (metadata and content):"),
            indexed_chunks: counter(stdout, "Indexed chunks:"),
            metadata_only_files: counter(stdout, "Metadata-only files (policy):"),
            content_skipped_files: counter(stdout, "Metadata-only files (content skipped):"),
            unstable_sources: counter(
                stdout,
                "Unstable sources (changed during the scan, not published):",
            ),
            excluded_paths: counter(stdout, "Excluded paths:"),
            unrepresentable_names: counter(stdout, "Skipped entries with non-UTF-8 names:"),
            failed_paths: counter(stdout, "Failed paths:"),
            marked_missing: optional_counter(stdout, "Locations marked missing:"),
            relocated: optional_counter(
                stdout,
                "Relocated locations (same file identity remains present):",
            ),
        }
    }

    pub fn describe(&self) -> String {
        let mut text = format!(
            "indexed {} files / {} chunks, metadata-only {}, content-skipped {}, excluded {}, unstable {}, non-UTF-8 {}, failed {}",
            self.indexed_files,
            self.indexed_chunks,
            self.metadata_only_files,
            self.content_skipped_files,
            self.excluded_paths,
            self.unstable_sources,
            self.unrepresentable_names,
            self.failed_paths
        );
        if let (Some(missing), Some(relocated)) = (self.marked_missing, self.relocated) {
            text.push_str(&format!(
                ", marked missing {missing}, relocated {relocated}"
            ));
        }
        text
    }
}

pub fn counter(stdout: &str, prefix: &str) -> usize {
    optional_counter(stdout, prefix)
        .unwrap_or_else(|| panic!("no line starting with {prefix:?} in:\n{stdout}"))
}

fn optional_counter(stdout: &str, prefix: &str) -> Option<usize> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(|rest| rest.trim().parse().unwrap())
}

/// Invariants read straight from the graph once no crosskit process holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCounts {
    pub locations: usize,
    pub present: usize,
    pub missing: usize,
    pub file_versions: usize,
    pub chunks: usize,
    pub insight_jobs: usize,
    pub queued_jobs: usize,
    pub versions_without_job: usize,
    pub versions_without_chunks: usize,
    pub runs_running: usize,
    pub runs_completed: usize,
    pub runs_incomplete: usize,
}

impl GraphCounts {
    fn read(database_path: &Path) -> Self {
        let database = Database::new(database_path, SystemConfig::default()).unwrap();
        let connection = Connection::new(&database).unwrap();
        let count = |query: &str| -> usize {
            let mut result = connection.query(query).unwrap();
            match result.next().unwrap().as_slice() {
                [Value::Int64(value)] => usize::try_from(*value).unwrap(),
                values => panic!("expected one count, received {values:?}"),
            }
        };
        Self {
            locations: count("MATCH (:FileLocation) RETURN COUNT(*);"),
            present: count("MATCH (:FileLocation {status: 'present'}) RETURN COUNT(*);"),
            missing: count("MATCH (:FileLocation {status: 'missing'}) RETURN COUNT(*);"),
            file_versions: count("MATCH (:FileVersion) RETURN COUNT(*);"),
            chunks: count("MATCH (:ContentChunk) RETURN COUNT(*);"),
            insight_jobs: count("MATCH (:InsightJob) RETURN COUNT(*);"),
            queued_jobs: count("MATCH (:InsightJob {state: 'queued'}) RETURN COUNT(*);"),
            versions_without_job: count(
                "MATCH (version:FileVersion) WHERE NOT EXISTS { MATCH (version)-[:REQUESTS_INSIGHT]->(:InsightJob) } RETURN COUNT(*);",
            ),
            versions_without_chunks: count(
                "MATCH (version:FileVersion) WHERE NOT EXISTS { MATCH (version)-[:HAS_CHUNK]->(:ContentChunk) } RETURN COUNT(*);",
            ),
            runs_running: count("MATCH (:ReconcileRun {status: 'running'}) RETURN COUNT(*);"),
            runs_completed: count("MATCH (:ReconcileRun {status: 'completed'}) RETURN COUNT(*);"),
            runs_incomplete: count("MATCH (:ReconcileRun {status: 'incomplete'}) RETURN COUNT(*);"),
        }
    }

    /// The part of the graph a pass may only change deliberately: runs are bookkeeping and
    /// grow by one per pass, everything else must stay put when nothing changed on disk.
    pub fn evidence(&self) -> EvidenceState {
        EvidenceState {
            locations: self.locations,
            present: self.present,
            missing: self.missing,
            file_versions: self.file_versions,
            chunks: self.chunks,
            insight_jobs: self.insight_jobs,
            queued_jobs: self.queued_jobs,
        }
    }

    /// Every revision has exactly its evidence and its outbox entry; nothing half written.
    pub fn assert_consistent(&self) {
        assert_eq!(self.versions_without_job, 0, "{self:?}");
        assert_eq!(self.versions_without_chunks, 0, "{self:?}");
        assert_eq!(self.insight_jobs, self.file_versions, "{self:?}");
        assert_eq!(self.present + self.missing, self.locations, "{self:?}");
    }

    pub fn chunk_text_contains(database_path: &Path, needle: &str) -> bool {
        let database = Database::new(database_path, SystemConfig::default()).unwrap();
        let connection = Connection::new(&database).unwrap();
        let mut statement = connection
            .prepare(
                "MATCH (chunk:ContentChunk) WHERE contains(chunk.text, $needle) RETURN COUNT(*);",
            )
            .unwrap();
        let mut result = connection
            .execute(
                &mut statement,
                vec![("needle", Value::String(needle.to_string()))],
            )
            .unwrap();
        matches!(result.next().unwrap().as_slice(), [Value::Int64(count)] if *count > 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceState {
    pub locations: usize,
    pub present: usize,
    pub missing: usize,
    pub file_versions: usize,
    pub chunks: usize,
    pub insight_jobs: usize,
    pub queued_jobs: usize,
}

/// One measured phase for the report.
pub struct PhaseResult {
    pub name: String,
    pub duration: Duration,
    /// Zero when the platform offers no external measurement.
    pub peak_rss_bytes: u64,
    /// Absent unless the binary was built with `allocation-profile`.
    pub allocation_peak_bytes: Option<u64>,
    /// What the command returned. The recovery pass is the one phase that must not be zero.
    pub exit_code: Option<i32>,
    pub database_bytes: u64,
    pub counters: String,
    pub queued_jobs: usize,
}

pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// The resident peak a phase reached, or a plain statement that nobody measured it.
pub fn peak_rss_label(measurement: &Measurement) -> String {
    if measurement.peak_rss_bytes == 0 {
        return "peak RSS not measured on this platform".to_string();
    }
    format!("peak RSS {}", megabytes(measurement.peak_rss_bytes))
}

/// What Crosskit's own code allocated in this phase, when the binary counts it. An
/// ordinary build says nothing here instead of printing a zero that would read like a
/// measurement. The exact byte count is printed beside the rounded one: this number is a
/// baseline two runs are compared against, and a tenth of a mebibyte is too coarse to tell
/// a repeat from a regression.
pub fn allocated_peak_label(measurement: &Measurement) -> String {
    measurement
        .allocation_peak_bytes
        .map(|bytes| format!(", allocated peak {} ({bytes} bytes)", megabytes(bytes)))
        .unwrap_or_default()
}

fn declared_languages() -> String {
    let Ok(declared) = std::env::var("CROSSKIT_SCALE_LANGUAGES") else {
        return String::new();
    };
    let names: Vec<String> = declared
        .split(',')
        .map(|name| format!("\"{}\"", name.trim()))
        .collect();
    format!("[search]\nlanguages = [{}]\n\n", names.join(", "))
}
