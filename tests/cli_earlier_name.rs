// CLI smoke: the product was called Crosscode until 2026-10-02, and an owner's data is still
// under that name.
//
// What has to hold is that nothing is lost and nothing happens on its own: a build that finds
// only the earlier directory refuses instead of starting empty, the move is asked for and is
// one rename, the index and the graph written before the rename open after it without a
// rebuild, and a variable still set under the earlier name is never ignored in silence.
mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use common::{Sandbox, WatchProcess, snapshot};

/// The root the fixture's context covered when the build before the rename indexed it. The
/// file is not there any more; the graph and the index remember it, which is what is tested.
const FIXTURE_ROOT: &str = "/private/tmp/before-rename-fixture/documents";
const FIXTURE_CONTEXT: &str = "sandbox";
const MOVE: &[&str] = &["migrate", "--from-crosscode"];

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

/// A home whose data directories are the platform defaults, with no variable naming them.
struct DefaultHome {
    root: PathBuf,
    home: PathBuf,
}

impl DefaultHome {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-earlier-name-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        Self { root, home }
    }

    #[cfg(target_os = "macos")]
    fn data_parent(&self) -> PathBuf {
        self.home.join("Library").join("Application Support")
    }

    #[cfg(not(target_os = "macos"))]
    fn data_parent(&self) -> PathBuf {
        self.home.join(".local").join("share")
    }

    #[cfg(target_os = "macos")]
    fn earlier(&self) -> PathBuf {
        self.data_parent().join("Crosscode")
    }

    #[cfg(not(target_os = "macos"))]
    fn earlier(&self) -> PathBuf {
        self.data_parent().join("crosscode")
    }

    #[cfg(target_os = "macos")]
    fn current(&self) -> PathBuf {
        self.data_parent().join("Crosskit")
    }

    #[cfg(not(target_os = "macos"))]
    fn current(&self) -> PathBuf {
        self.data_parent().join("crosskit")
    }

    /// The fixture written by the build before the rename, put where that build kept it.
    fn with_fixture_in(&self, directory: &Path) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/before-rename");
        copy_directory(&fixture, directory);
        fs::write(
            directory.join("config.toml"),
            format!("[workspace.{FIXTURE_CONTEXT}]\ninclude = [\"{FIXTURE_ROOT}\"]\n"),
        )
        .unwrap();
    }

    /// The binary with this home and nothing else naming where its data is: no data or
    /// config variable under either name is inherited from the machine running the tests.
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_crosskit"));
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if name.starts_with("CROSSKIT_") || name.starts_with("CROSSCODE_") {
                command.env_remove(name.as_ref());
            }
        }
        command
            .args(arguments)
            .env_remove("XDG_DATA_HOME")
            .env("HOME", &self.home)
            .env("NO_COLOR", "1");
        command
    }

    fn crosskit(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }
}

impl Drop for DefaultHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn copy_directory(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// What a directory holds, named relative to it, so the same contents under two names
/// compare equal.
fn contents(directory: &Path) -> Vec<String> {
    let prefix = directory.display().to_string();
    snapshot(directory)
        .into_iter()
        .map(|entry| entry.replacen(&prefix, "", 1))
        .collect()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn succeeds(output: Output, what: &str) -> String {
    assert!(
        output.status.success(),
        "{what} failed:\n{}{}",
        stdout(&output),
        stderr(&output)
    );
    stdout(&output)
}

/// A build that finds only the earlier directory neither starts empty nor moves it: it stops
/// with the command that moves it, and the disk is as it was.
#[test]
fn data_left_under_the_earlier_name_is_refused_with_the_command_that_moves_it() {
    let home = DefaultHome::new("refused");
    home.with_fixture_in(&home.earlier());
    let before = snapshot(&home.earlier());

    let status = home.crosskit(&["status"]);

    assert!(!status.status.success(), "{}", stdout(&status));
    assert!(
        stderr(&status).contains("`crosskit migrate --from-crosscode`"),
        "{}",
        stderr(&status)
    );
    assert!(
        !home.current().exists(),
        "a refusal must not create the new directory"
    );
    assert_eq!(snapshot(&home.earlier()), before);
}

/// The move is a rename — the directory keeps its inode, so nothing was copied — and what the
/// build before the rename wrote is read by this one as it is: the index is in step with the
/// graph, written by the rules this build applies, and answers the same query.
#[test]
fn the_move_is_one_rename_and_what_the_earlier_build_wrote_opens_without_a_rebuild() {
    let home = DefaultHome::new("moved");
    home.with_fixture_in(&home.earlier());
    let before = contents(&home.earlier());
    let inode = fs::metadata(home.earlier()).unwrap().ino();

    let moved = succeeds(home.crosskit(MOVE), "the move");

    assert!(moved.contains("Moved:"), "{moved}");
    assert!(!home.earlier().exists());
    assert_eq!(fs::metadata(home.current()).unwrap().ino(), inode);
    assert_eq!(
        contents(&home.current()),
        before,
        "the move rewrote nothing inside"
    );

    // The fixture's graph was written at schema 16. A later schema is brought in by the one
    // command that writes it, and it changes no id and nothing of the search index: the index
    // below still opens as written by this build's rules, without a rebuild.
    succeeds(
        home.crosskit(&["migrate"]),
        "the migration to this build's schema",
    );
    let doctor = succeeds(home.crosskit(&["doctor"]), "doctor");
    assert!(doctor.contains("(in step with the graph)"), "{doctor}");
    assert!(
        doctor.contains("Search index written by: fields=") && !doctor.contains("rebuild"),
        "the index must read as written by this build's rules:\n{doctor}"
    );
    let search = succeeds(
        home.crosskit(&["search", "контрагент", "--include-missing"]),
        "search",
    );
    assert!(search.contains("Results: 1"), "{search}");
    assert!(
        search.contains("before-rename-fixture/documents/notes.md"),
        "{search}"
    );

    let again = succeeds(home.crosskit(MOVE), "the second move");
    assert!(again.contains("Nothing to move"), "{again}");
}

/// Two directories is a question about which one is real, and the answer is not guessed.
#[test]
fn with_both_directories_present_nothing_is_moved() {
    let home = DefaultHome::new("both");
    home.with_fixture_in(&home.earlier());
    fs::create_dir_all(home.current()).unwrap();
    let before = snapshot(&home.earlier());

    let refused = home.crosskit(MOVE);

    assert!(!refused.status.success(), "{}", stdout(&refused));
    assert!(stderr(&refused).contains("both"), "{}", stderr(&refused));
    assert_eq!(snapshot(&home.earlier()), before);
    assert_eq!(fs::read_dir(home.current()).unwrap().count(), 0);
}

/// A watcher of the earlier build holding a graph in the earlier directory is named by its
/// process, and the directory stays where the watcher expects it.
#[test]
fn a_graph_held_by_a_watcher_is_named_and_nothing_is_moved() {
    let home = DefaultHome::new("held");
    let mut sandbox = Sandbox::new("earlier-name-held");
    fs::create_dir_all(home.earlier()).unwrap();
    sandbox.data_directory = home.earlier();
    sandbox.write_config("");
    let mut watch = WatchProcess::spawn(&sandbox);
    watch.wait_for_line("Pass 1 (bootstrap) completed");

    let refused = home.crosskit(MOVE);

    watch.interrupt();
    let _ = watch.wait_for_exit();
    assert!(!refused.status.success(), "{}", stdout(&refused));
    assert!(
        stderr(&refused).contains("holds") && stderr(&refused).contains("graph-sandbox.lbug"),
        "{}",
        stderr(&refused)
    );
    assert!(home.earlier().exists() && !home.current().exists());
}

/// A variable under the earlier name alone stops the run with the name to set; beside its
/// new name it loses, and says so.
#[test]
fn a_variable_under_the_earlier_name_is_refused_alone_and_overridden_beside_the_new_one() {
    let home = DefaultHome::new("variables");
    let data = home.root.join("data");
    home.with_fixture_in(&data);

    let alone = home
        .command(&["status"])
        .env("CROSSCODE_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(!alone.status.success(), "{}", stdout(&alone));
    assert!(
        stderr(&alone).contains("CROSSKIT_DATA_DIR (not CROSSCODE_DATA_DIR)"),
        "{}",
        stderr(&alone)
    );

    let both = home
        .command(&["status"])
        .env("CROSSCODE_DATA_DIR", home.root.join("elsewhere"))
        .env("CROSSKIT_DATA_DIR", &data)
        .output()
        .unwrap();
    let status = succeeds(both.clone(), "status with both variables");
    assert!(
        stderr(&both).contains("CROSSCODE_DATA_DIR is set and ignored"),
        "{}",
        stderr(&both)
    );
    assert!(status.contains(FIXTURE_CONTEXT), "{status}");
}
