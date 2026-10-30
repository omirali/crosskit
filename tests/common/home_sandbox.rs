// A home directory to set up, beside a data directory that starts empty.
//
// `setup` is the one command that must run before anything is configured, and it reads the
// person's home — the standard layout, `~/.claude/projects`, `~/.gitconfig`, `~/.codex` — so its
// sandbox has a home of its own and a `HOME` pointing at it, where the ordinary `Sandbox` has a
// configured context instead.
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

pub struct HomeSandbox {
    root: PathBuf,
    pub data_directory: PathBuf,
    pub home: PathBuf,
}

impl HomeSandbox {
    pub fn new(test_name: &str) -> Self {
        let sequence = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-home-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        let sandbox = Self {
            data_directory: root.join("data"),
            home: root.join("home"),
            root,
        };
        fs::create_dir_all(&sandbox.data_directory).unwrap();
        fs::create_dir_all(&sandbox.home).unwrap();
        sandbox
    }

    pub fn file(&self, path: &str, contents: &str) -> &Self {
        let path = self.home.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    /// A repository whose `origin` is `url`; with an empty `url`, one without a remote.
    pub fn repository(&self, path: &str, url: &str) -> &Self {
        let config = match url {
            "" => "[core]\n\tbare = false\n".to_string(),
            url => format!("[remote \"origin\"]\n\turl = {url}\n"),
        };
        self.file(&format!("{path}/.git/config"), &config)
    }

    /// The spelling the walk reports for a path under the home, which on macOS resolves
    /// through `/private`.
    pub fn resolved(&self, relative: &str) -> String {
        let home = fs::canonicalize(&self.home).unwrap();
        match relative {
            "" => home.to_string_lossy().into_owned(),
            relative => home.join(relative).to_string_lossy().into_owned(),
        }
    }

    /// One conversation of Claude Code, filed the way the tool files it: in the folder named
    /// after `started_in`, as `<session>.jsonl`, each of `lines` a line of it.
    pub fn conversation(&self, started_in: &str, session: &str, lines: &[String]) -> &Self {
        self.file(
            &format!(
                ".claude/projects/{}/{session}.jsonl",
                transcript_folder_name(started_in)
            ),
            &format!("{}\n", lines.join("\n")),
        )
    }

    pub fn config_path(&self) -> PathBuf {
        self.data_directory.join("config.toml")
    }

    /// The binary with its storage, its policy file and its home all inside the sandbox.
    pub fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_crosskit"));
        command
            .args(arguments)
            .env("CROSSKIT_DATA_DIR", &self.data_directory)
            .env("CROSSKIT_CONFIG", self.config_path())
            .env("HOME", &self.home)
            .env("NO_COLOR", "1");
        command
    }

    pub fn crosskit(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }

    pub fn home_argument(&self) -> String {
        self.home.to_string_lossy().into_owned()
    }

    /// `setup` with `typed` on its standard input, closed after it: how the questions are
    /// answered when nobody is at a terminal.
    pub fn setup_answering(&self, typed: &str) -> Output {
        let mut child = self
            .command(&["setup", &self.home_argument()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(typed.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for HomeSandbox {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("failed to remove {}: {error}", self.root.display());
        }
    }
}

/// The folder Claude Code names after a directory: every character outside ASCII letters and
/// digits becomes a dash. Written out here rather than borrowed from the program, so the test
/// holds the program to the tool's rule instead of to itself; the sandbox's paths are ASCII,
/// which is all this spelling needs.
pub fn transcript_folder_name(directory: &str) -> String {
    directory
        .chars()
        .map(|character| match character.is_ascii_alphanumeric() {
            true => character,
            false => '-',
        })
        .collect()
}
