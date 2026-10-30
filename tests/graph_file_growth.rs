// Stage 34, point 5: does a context's graph file grow every time the context is built
// again?
//
// The question comes from `chats`: Stage 32 cut its chunks by 86% and its database stayed
// at 681 MB. The engine does not hand pages back to the filesystem, so the open question is
// whether the file merely keeps the space it once needed or gains more at each rebuild. For
// a product that runs a watcher those are two different futures.
//
//   cargo test --release --offline --test graph_file_growth -- --ignored --nocapture
//
// Both measurements are ignored: the first reads the owner's real configuration and their
// real tree, and the second writes a few hundred files. Neither touches the owner's data
// directory — the first copies the one database it measures into a directory of its own,
// and rebuilds the copy.
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::Sandbox;

/// The context Stage 34 was told to measure: the smallest one the owner has.
const CONTEXT: &str = "github-com";
/// How many files the in-place measurement writes, and how many times it rewrites them.
const SANDBOX_FILES: usize = 300;
const REWRITES: usize = 8;

#[test]
#[ignore = "замер роста файла графа: cargo test --release --offline --test graph_file_growth -- --ignored --nocapture"]
fn what_two_rebuilds_do_to_the_size_of_one_graph() {
    let Some(data_directory) = owners_data_directory() else {
        println!("CROSSKIT_DATA_DIR/HOME не заданы — ничего не измерено.");
        return;
    };
    let live = data_directory.join(format!("graph-{CONTEXT}.lbug"));
    if !live.exists() {
        println!("{} нет — ничего не измерено.", live.display());
        return;
    }
    let roots = match roots_of_context(&data_directory, CONTEXT) {
        roots if roots.is_empty() => {
            println!("у контекста {CONTEXT} нет корней в конфигурации — ничего не измерено.");
            return;
        }
        roots => roots,
    };

    let copy = WorkingCopy::of(&data_directory, CONTEXT);
    println!("Копия: {}", copy.directory.display());
    println!("корней: {}", roots.len());
    println!(
        "1. живой файл контекста, как он лежит у владельца: {}",
        megabytes(file_size(&copy.graph))
    );

    for round in 1..=2 {
        copy.empty_the_context();
        for root in &roots {
            copy.run(&["--context", CONTEXT, "index", "init", root]);
        }
        println!(
            "{}. после полной перестройки №{round}: {}",
            round + 1,
            megabytes(file_size(&copy.graph))
        );
    }

    for pass in 1..=6 {
        for root in &roots {
            copy.run(&["--context", CONTEXT, "index", "reconcile", root]);
        }
        println!(
            "   холостая сверка №{pass} поверх перестройки: {}",
            megabytes(file_size(&copy.graph))
        );
    }
}

/// The same question asked where the answer can actually be seen: a tree whose every file
/// is rewritten between passes, so each pass writes a new revision and new chunks into the
/// database that already holds the old ones.
///
/// A rebuild into a fresh file cannot show a leak; only writing again into a file that has
/// already been written can. The owner's tree may not be rewritten, so this uses a tree of
/// its own of the same order of size.
#[test]
#[ignore = "замер роста файла графа: см. команду выше"]
fn what_rewriting_every_file_costs_the_same_graph_file() {
    let sandbox = Sandbox::new("graph-growth");
    for index in 0..SANDBOX_FILES {
        write_document(&sandbox.documents, index, 0);
    }
    let graph = sandbox
        .data_directory
        .join(format!("graph-{}.lbug", Sandbox::CONTEXT));

    let output = sandbox.crosskit(&["index", "init", &sandbox.documents_argument()]);
    assert!(output.status.success(), "первый проход не прошёл");
    println!(
        "0 перезаписей ({SANDBOX_FILES} файлов): {}",
        megabytes(file_size(&graph))
    );

    for generation in 1..=REWRITES {
        for index in 0..SANDBOX_FILES {
            write_document(&sandbox.documents, index, generation);
        }
        sandbox.reconcile();
        println!(
            "{generation} перезапись(и) всех файлов: {}",
            megabytes(file_size(&graph))
        );
    }
}

/// One document, written differently in each generation so that every pass sees a file
/// whose content hash changed and therefore publishes a new revision of it.
fn write_document(documents: &Path, index: usize, generation: usize) {
    let text = format!(
        "Document {index}, generation {generation}.\n{}\n",
        "word ".repeat(200 + generation)
    );
    fs::write(documents.join(format!("note-{index:04}.md")), text).unwrap();
}

/// A copy of one context's graph and search index in a directory of its own, with the
/// owner's configuration beside it. The owner's own files are read once and never written.
struct WorkingCopy {
    directory: PathBuf,
    graph: PathBuf,
    search: PathBuf,
}

impl WorkingCopy {
    fn of(data_directory: &Path, context: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "crosskit-graph-growth-{}-{context}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();

        let graph = directory.join(format!("graph-{context}.lbug"));
        fs::copy(data_directory.join(format!("graph-{context}.lbug")), &graph).unwrap();
        fs::copy(
            data_directory.join("config.toml"),
            directory.join("config.toml"),
        )
        .unwrap();
        let search = directory.join(format!("search-{context}"));
        copy_directory(&data_directory.join(format!("search-{context}")), &search);

        Self {
            directory,
            graph,
            search,
        }
    }

    /// What a rebuild of a context starts from: no graph and no search index, and the same
    /// configuration. This is the only cure the documentation offers for a graph holding
    /// what no pass can reach any more.
    fn empty_the_context(&self) {
        let _ = fs::remove_file(&self.graph);
        let _ = fs::remove_file(self.graph.with_extension("lbug.wal"));
        let _ = fs::remove_dir_all(&self.search);
    }

    fn run(&self, arguments: &[&str]) {
        let output = Command::new(env!("CARGO_BIN_EXE_crosskit"))
            .args(arguments)
            .env("CROSSKIT_DATA_DIR", &self.directory)
            .env("CROSSKIT_CONFIG", self.directory.join("config.toml"))
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?} не прошла: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for WorkingCopy {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!("не удалось удалить {}: {error}", self.directory.display());
        }
    }
}

fn copy_directory(from: &Path, to: &Path) {
    let Ok(entries) = fs::read_dir(from) else {
        return;
    };
    fs::create_dir_all(to).unwrap();
    for entry in entries.filter_map(Result::ok) {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn owners_data_directory() -> Option<PathBuf> {
    if let Some(directory) = std::env::var_os("CROSSKIT_DATA_DIR") {
        return Some(PathBuf::from(directory));
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Crosskit"),
    )
}

/// The include list of one context, taken from the configuration file as written. The
/// measurement runs the real commands over the real roots, so it needs the roots and
/// nothing else out of that file.
fn roots_of_context(data_directory: &Path, context: &str) -> Vec<String> {
    let Ok(text) = fs::read_to_string(data_directory.join("config.toml")) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == format!("[workspace.{context}]");
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(quoted) = trimmed.strip_prefix('"')
            && let Some((path, _)) = quoted.split_once('"')
        {
            roots.push(path.to_string());
        }
    }
    roots
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} МБ ({bytes} байт)", bytes as f64 / (1024.0 * 1024.0))
}
