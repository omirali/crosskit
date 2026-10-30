// Shared sandbox for CLI smoke tests: an isolated data directory and a small directory
// of documents, both removed on drop.
//
// Every integration test binary compiles this module separately and uses the part of it it
// needs, so what one binary does not call is not dead code — it is a helper another binary
// calls.
#![allow(
    dead_code,
    unused_imports,
    reason = "each test binary compiles this module and uses part of it"
)]
mod home_sandbox;
pub mod service;
mod task_conversations;
mod watch_process;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

pub use home_sandbox::{HomeSandbox, transcript_folder_name};
pub use task_conversations::{fake_map_engine, write_task_conversations};
pub use watch_process::WatchProcess;

/// Every file under `root` with its size, the moment it was last written, and a digest of
/// what is actually in it.
///
/// What a command did to the disk, asked of the disk. Two of these taken around a run and
/// compared is the only way to tell a command that reports from one that repairs, migrates
/// or checkpoints on its way out — all three of which are what opening a database for
/// writing does, whatever the command then prints.
///
/// The digest is not belt and braces. Measured on a graph of one file, a write opening
/// rewrote the database and left its size and its modification time exactly as they were:
/// size and time alone watched that happen and reported nothing.
pub fn snapshot(root: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(read) = fs::read_dir(&directory) else {
            // `root` may be one file rather than a directory, which is how a test asks
            // about one database and not about everything beside it.
            if directory == root {
                entries.push(describe(root));
            }
            continue;
        };
        for entry in read.filter_map(Result::ok) {
            let path = entry.path();
            entries.push(describe(&path));
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                pending.push(path);
            }
        }
    }
    entries.sort();
    entries
}

fn describe(path: &Path) -> String {
    let metadata = fs::symlink_metadata(path).unwrap();
    format!(
        "{} {} {:?} {:016x}",
        path.display(),
        metadata.len(),
        metadata.modified().ok(),
        fs::read(path).map(|bytes| digest(&bytes)).unwrap_or(0)
    )
}

/// FNV-1a over the bytes. Nothing here needs a cryptographic hash — this answers "is this
/// the same file as a moment ago", and the only writer either answer could come from is the
/// command under test.
fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Writes a script called `claude` and returns its directory, to be put on PATH.
///
/// The script sets a PATH of its own: the one this program hands it holds nothing but the
/// script, which is the point. The real adapter runs against it — only the thing at the far
/// end is scripted, because that is the only part a test must not reach over the network.
pub fn fake_engine(body: &str) -> (PathBuf, String) {
    let sequence = NEXT_SANDBOX.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "crosskit-fake-tool-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("claude");
    fs::write(&path, format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let directory = root.to_string_lossy().into_owned();
    (root, directory)
}

static NEXT_SANDBOX: AtomicU64 = AtomicU64::new(0);

pub struct Sandbox {
    root: PathBuf,
    pub data_directory: PathBuf,
    pub documents: PathBuf,
}

impl Sandbox {
    pub fn new(test_name: &str) -> Self {
        let sequence = NEXT_SANDBOX.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-cli-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        let data_directory = root.join("data");
        let documents = root.join("documents");
        fs::create_dir_all(&data_directory).unwrap();
        fs::create_dir_all(&documents).unwrap();
        fs::write(documents.join("notes.md"), "Crosskit keeps this note.").unwrap();
        let sandbox = Self {
            root,
            data_directory,
            documents,
        };
        sandbox.write_config("");
        sandbox
    }

    /// The name of the one context these tests work in. One configured context is taken
    /// without being named, which is what a single-context installation looks like.
    pub const CONTEXT: &'static str = "sandbox";

    /// Writes the policy file with `index_section` on top of the context that covers the
    /// documents directory. Every command needs a context, so the sandbox always declares
    /// one instead of letting a test reach the database without one. The context keeps its
    /// content to itself, which is what a context that says nothing means.
    pub fn write_config(&self, index_section: &str) {
        self.write_config_sections(index_section, "");
    }

    /// The same with extra lines inside the context's own section, for a test about what a
    /// context permits.
    pub fn write_config_sections(&self, index_section: &str, workspace_lines: &str) {
        fs::write(
            self.data_directory.join("config.toml"),
            format!(
                "{index_section}[workspace.{}]\ninclude = [\"{}\"]\n{workspace_lines}",
                Self::CONTEXT,
                // The scan resolves its root, and on macOS a temp path resolves through
                // /private; a pattern written from the unresolved path would cover nothing.
                fs::canonicalize(&self.documents).unwrap().display()
            ),
        )
        .unwrap();
    }

    /// The binary under test with its storage and policy pointed at the sandbox.
    pub fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_crosskit"));
        command
            .args(arguments)
            .env("CROSSKIT_DATA_DIR", &self.data_directory)
            .env("CROSSKIT_CONFIG", self.data_directory.join("config.toml"))
            .env("NO_COLOR", "1");
        command
    }

    pub fn crosskit(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }

    pub fn documents_argument(&self) -> String {
        self.documents.to_string_lossy().into_owned()
    }

    pub fn reconcile(&self) {
        let output = self.crosskit(&["index", "reconcile", &self.documents_argument()]);
        assert!(
            output.status.success(),
            "reconcile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The rows of one read-only query against this sandbox's graph, each row's columns
    /// joined by `|`.
    ///
    /// A helper and not a copy in every binary: three test binaries now look into the graph
    /// to check what a command wrote, and three spellings of "read a row" would drift.
    /// Read-only and outside any transaction — the command under test has already finished.
    pub fn graph_rows(&self, query: &str) -> Vec<String> {
        let path = self
            .data_directory
            .join(format!("graph-{}.lbug", Self::CONTEXT));
        let database = lbug::Database::new(&path, lbug::SystemConfig::default())
            .unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
        let connection = lbug::Connection::new(&database).unwrap();
        let result = connection
            .query(query)
            .unwrap_or_else(|error| panic!("query {query:?}: {error}"));
        let mut rows = Vec::new();
        for values in result {
            rows.push(
                values
                    .iter()
                    .map(|value| match value {
                        lbug::Value::String(text) => text.clone(),
                        lbug::Value::Int64(number) => number.to_string(),
                        other => format!("{other:?}"),
                    })
                    .collect::<Vec<_>>()
                    .join("|"),
            );
        }
        rows
    }

    pub fn database_exists(&self) -> bool {
        self.data_directory
            .join(format!("graph-{}.lbug", Self::CONTEXT))
            .exists()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("failed to remove {}: {error}", self.root.display());
        }
    }
}

/// Asserts that every expected fragment appears somewhere in `stdout`.
pub fn assert_counters(stdout: &str, expected: &[&str]) {
    for line in expected {
        assert!(stdout.contains(line), "missing {line:?} in:\n{stdout}");
    }
}
