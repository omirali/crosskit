// Where the memory of one large source goes. Stage 2P measured 2.16 GiB for a single file
// at the policy's 100 MiB content limit and could not say which layer holds it; this walks
// the same production path and stops at each layer in turn, in its own process, so
// `/usr/bin/time -l` attributes the peak to the layer rather than to the run.
//
// macOS-only, because that is where the peak resident set size comes from. Ignored by
// default: it writes hundreds of megabytes and takes minutes.
//
//   cargo test --release --offline --bins source_cost -- --ignored --nocapture
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use lbug::Value;

use super::batch_projection::{BatchProjection, Projected};
use super::chunk_store::chunk_value;
use crate::application::{
    FileIdentityHistory, InitialScanService, RepositoryError, SourceBatchWriter, SourceWalker,
};
use crate::domain::{AnalysisRules, IndexPolicy, KnownIdentity, PathPattern, SourceCommit};
use crate::infrastructure::DescriptorRelativeWalker;

/// How far down the write path one measured process goes before it stops and lets the
/// operating system report what it held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    /// The source as the scan produced it: file read, text extracted, chunks cut.
    Read,
    /// Plus the typed rows a batch is projected into.
    Project,
    /// Plus the parameter values the `UNWIND` statements are given.
    Marshal,
    /// Plus everything the engine does inside the transaction: the whole production write.
    Engine,
    /// The prototype: the same transaction, but the chunks of one source are written in
    /// pages instead of one statement. It exists to answer whether paging inside a single
    /// transaction lowers the peak at all, which is the cheapest change that would keep a
    /// source atomic.
    EnginePaged(usize),
}

impl Layer {
    fn parse(name: &str) -> Self {
        match name {
            "read" => Self::Read,
            "project" => Self::Project,
            "marshal" => Self::Marshal,
            "engine" => Self::Engine,
            paged => match paged.strip_prefix("engine-paged-").map(str::parse) {
                Some(Ok(page)) => Self::EnginePaged(page),
                _ => panic!("unknown source cost layer {name:?}"),
            },
        }
    }

    fn name(self) -> String {
        match self {
            Self::Read => "read".to_string(),
            Self::Project => "project".to_string(),
            Self::Marshal => "marshal".to_string(),
            Self::Engine => "engine".to_string(),
            Self::EnginePaged(page) => format!("engine-paged-{page}"),
        }
    }
}

/// Runs the write path down to `layer` and holds what it built, so the process is still
/// holding it when it ends. Everything below the chosen layer is the real production code.
struct LayerWriter<'a> {
    layer: Layer,
    repository: Option<&'a super::repository::LadybugGraphRepository>,
    manager: Option<&'a super::LadybugDatabaseManager>,
}

/// The diagnostic writes into a database of its own, so every identity it meets is new.
impl FileIdentityHistory for LayerWriter<'_> {
    fn known_identity(&self, _base_id: &str) -> Result<KnownIdentity, RepositoryError> {
        Ok(KnownIdentity::Unseen)
    }
}

impl SourceBatchWriter for LayerWriter<'_> {
    fn commit_observed_batch(
        &self,
        _run: &crate::domain::ReconcileRun,
        commits: Vec<SourceCommit>,
    ) -> Result<(), RepositoryError> {
        self.commit_batch(commits)
    }

    fn commit_batch(&self, commits: Vec<SourceCommit>) -> Result<(), RepositoryError> {
        let chunks: usize = commits
            .iter()
            .map(|commit| {
                commit
                    .evidence()
                    .map_or(0, |evidence| evidence.chunks().len())
            })
            .sum();
        println!("layer-batch|{}|{chunks}", commits.len());
        if self.layer == Layer::Read {
            // Held until the end of the call, which is where the peak is taken.
            return hold(&commits);
        }

        let projection = match BatchProjection::of(&commits)? {
            Projected::SetBased(projection) => projection,
            Projected::RequiresSequential => {
                panic!("the diagnostic corpus must never need the sequential fallback")
            }
        };
        if self.layer == Layer::Project {
            return hold(&projection);
        }

        let values: Vec<Value> = projection.chunks.iter().map(chunk_value).collect();
        if self.layer == Layer::Marshal {
            return hold(&values);
        }
        drop(values);

        if let Layer::EnginePaged(page) = self.layer {
            return self.write_in_pages(&projection, page);
        }
        drop(projection);

        self.repository
            .expect("the engine layer needs a repository")
            .commit_batch(commits)
    }
}

impl LayerWriter<'_> {
    /// The prototype write: one transaction holding the part of the production path the
    /// chunks depend on — the files and locations they hang from, their revisions, then the
    /// chunks. A `page` of zero writes them the way production does, in one statement, so
    /// the two shapes differ in nothing but the size of the slice. Observations, current
    /// state and jobs are left out of both alike: they carry no chunk text, and what is
    /// compared here is the cost of the chunks.
    fn write_in_pages(
        &self,
        projection: &BatchProjection<'_>,
        page: usize,
    ) -> Result<(), RepositoryError> {
        let connection = lbug::Connection::new(
            &self
                .manager
                .expect("the paged prototype needs a database")
                .owner
                .database,
        )
        .map_err(|error| RepositoryError::Storage(format!("open connection: {error}")))?;
        super::migration::in_transaction(&connection, |connection| {
            let epoch = super::reconcile_store::current_epoch(connection)?;
            super::location_store::persist_location_batch(
                connection,
                &projection.files,
                &projection.locations,
                epoch,
            )?;
            super::file_version_store::persist_file_version_batch(
                connection,
                &projection.versions,
            )?;
            let rows_per_page = if page == 0 {
                projection.chunks.len().max(1)
            } else {
                page
            };
            for rows in projection.chunks.chunks(rows_per_page) {
                super::chunk_store::persist_chunk_batch(connection, rows, &projection.extractors)?;
            }
            Ok(())
        })
    }
}

/// Keeps `held` alive across the point where the peak is observed without letting the
/// optimizer decide it was never needed.
fn hold<T>(held: &T) -> Result<(), RepositoryError> {
    if std::hint::black_box(std::ptr::from_ref(held)).is_null() {
        return Err(RepositoryError::Storage("unreachable".to_string()));
    }
    Ok(())
}

/// The worker half: one layer, one source, one process. The parent runs this same test
/// binary again under `/usr/bin/time -l`.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "source cost diagnostic worker"]
fn source_cost_worker() {
    let Ok(layer) = std::env::var("CROSSKIT_SOURCE_COST_LAYER") else {
        return;
    };
    let layer = Layer::parse(&layer);
    let corpus = PathBuf::from(
        std::env::var("CROSSKIT_SOURCE_COST_CORPUS").expect("the worker needs its corpus"),
    );
    let policy = IndexPolicy::new(Vec::new(), 100 * 1024 * 1024);

    let database = super::test_support::TestDatabase::new("source-cost");
    let manager = super::LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let repository = manager.repository();
    let writer = LayerWriter {
        layer,
        repository: (layer == Layer::Engine).then_some(&repository),
        manager: matches!(layer, Layer::EnginePaged(_)).then_some(manager.as_ref()),
    };

    let report = InitialScanService::new(
        &DescriptorRelativeWalker,
        &writer,
        &policy,
        &AnalysisRules::new(vec![PathPattern::parse("**/*").unwrap()], Vec::new()),
    )
    .execute(
        &DescriptorRelativeWalker
            .resolve_root(&corpus)
            .expect("the diagnostic corpus is a directory"),
    )
    .unwrap();
    println!(
        "layer-done|{}|{}|{}",
        layer.name(),
        report.indexed_files(),
        report.indexed_chunks()
    );
}

/// A file of `megabytes` of plain ASCII text, written without holding it in memory.
fn write_source_of(path: &Path, megabytes: u64) {
    let line = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod\n";
    let mut file = std::io::BufWriter::new(fs::File::create(path).unwrap());
    let mut written = 0;
    while written < megabytes * 1024 * 1024 - line.len() as u64 {
        file.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
    }
    file.flush().unwrap();
}

/// One measured process.
#[cfg(target_os = "macos")]
struct Sample {
    peak_rss_bytes: u64,
    seconds: f64,
    chunks: usize,
}

#[cfg(target_os = "macos")]
fn measure_layer(corpus: &Path, layer: Layer, chunk_bytes: Option<usize>) -> Sample {
    let mut command = Command::new("/usr/bin/time");
    command
        .arg("-l")
        .arg(std::env::current_exe().unwrap())
        .arg("--ignored")
        .arg("--exact")
        .arg("--nocapture")
        .arg("infrastructure::ladybug::source_cost_tests::source_cost_worker")
        .env("CROSSKIT_SOURCE_COST_LAYER", layer.name())
        .env("CROSSKIT_SOURCE_COST_CORPUS", corpus);
    if let Some(bytes) = chunk_bytes {
        command.env("CROSSKIT_DIAGNOSTIC_CHUNK_BYTES", bytes.to_string());
    }
    let started = std::time::Instant::now();
    let output = command.output().unwrap();
    let seconds = started.elapsed().as_secs_f64();
    let report = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{} layer failed: {report}",
        layer.name()
    );
    Sample {
        peak_rss_bytes: peak_rss(&report),
        seconds,
        chunks: stdout
            .lines()
            .find_map(|line| line.strip_prefix("layer-done|"))
            .and_then(|rest| rest.rsplit('|').next()?.parse().ok())
            .unwrap_or_default(),
    }
}

#[cfg(target_os = "macos")]
fn peak_rss(report: &str) -> u64 {
    report
        .lines()
        .find_map(|line| {
            let line = line.trim();
            line.strip_suffix("maximum resident set size")?
                .trim()
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("no peak resident set size in:\n{report}"))
}

#[cfg(target_os = "macos")]
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The cost curve of one source, and the attribution of the largest one by layer.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "source cost diagnostic: cargo test --release --offline --bins source_cost -- --ignored --nocapture"]
fn the_cost_of_one_large_source_is_attributed() {
    let root = std::env::temp_dir().join(format!("crosskit-source-cost-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();

    println!("megabytes | chunks | wall s | peak RSS MiB | MiB per content MiB");
    for megabytes in [1u64, 4, 8, 16, 32, 64, 100] {
        let corpus = root.join(format!("curve-{megabytes}"));
        fs::create_dir_all(&corpus).unwrap();
        write_source_of(&corpus.join("source.txt"), megabytes);
        let sample = measure_layer(&corpus, Layer::Engine, None);
        println!(
            "{megabytes} | {} | {:.1} | {:.0} | {:.1}",
            sample.chunks,
            sample.seconds,
            mib(sample.peak_rss_bytes),
            mib(sample.peak_rss_bytes) / megabytes as f64
        );
        fs::remove_dir_all(&corpus).unwrap();
    }

    let corpus = root.join("attribution");
    fs::create_dir_all(&corpus).unwrap();
    write_source_of(&corpus.join("source.txt"), 100);
    println!();
    println!("layer | wall s | peak RSS MiB | added by this layer MiB");
    let mut previous = 0.0;
    for layer in [Layer::Read, Layer::Project, Layer::Marshal, Layer::Engine] {
        let sample = measure_layer(&corpus, layer, None);
        let peak = mib(sample.peak_rss_bytes);
        println!(
            "{} | {:.1} | {:.0} | {:+.0}",
            layer.name(),
            sample.seconds,
            peak,
            peak - previous
        );
        previous = peak;
    }

    println!();
    println!("write shape | wall s | peak RSS MiB | one 100 MiB source");
    for layer in [
        Layer::Engine,
        Layer::EnginePaged(0),
        Layer::EnginePaged(2000),
        Layer::EnginePaged(500),
        Layer::EnginePaged(100),
    ] {
        let sample = measure_layer(&corpus, layer, None);
        println!(
            "{} | {:.1} | {:.0} |",
            layer.name(),
            sample.seconds,
            mib(sample.peak_rss_bytes)
        );
    }

    println!();
    println!("chunk bytes | chunks | peak RSS MiB | same 100 MiB of content");
    for chunk_bytes in [1024usize, 4096, 16384, 65536] {
        let sample = measure_layer(&corpus, Layer::Engine, Some(chunk_bytes));
        println!(
            "{chunk_bytes} | {} | {:.0} |",
            sample.chunks,
            mib(sample.peak_rss_bytes)
        );
    }

    fs::remove_dir_all(&root).unwrap();
}
