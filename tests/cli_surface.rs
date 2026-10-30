// The surface a person meets in the first ten minutes: `--help`, `--version`, what a run
// ends with, and whether the commands the documentation names are the commands that exist.
//
// None of it touches a database. The sandbox here has no configuration at all, which is on
// purpose: everything below has to answer on a machine where nothing has been set up yet,
// because that is the machine a person has when they type `crosskit --help` for the first
// time.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// The commands this program has, taken from its own help rather than from a list written
/// here: a second list is a second thing to keep in step.
fn commands(sandbox: &Sandbox) -> Vec<String> {
    let overview = sandbox.succeeding(&["--help"]);
    overview
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect()
}

struct Sandbox {
    root: PathBuf,
    data_directory: PathBuf,
}

impl Sandbox {
    fn new(test_name: &str) -> Self {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "crosskit-surface-{test_name}-{}-{sequence}",
            std::process::id()
        ));
        let data_directory = root.join("data");
        fs::create_dir_all(&data_directory).unwrap();
        Self {
            root,
            data_directory,
        }
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_crosskit"));
        command
            .args(arguments)
            .env("CROSSKIT_DATA_DIR", &self.data_directory)
            .env("CROSSKIT_CONFIG", self.data_directory.join("config.toml"))
            .env("HOME", &self.root)
            .env("NO_COLOR", "1");
        command
    }

    fn crosskit(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }

    fn succeeding(&self, arguments: &[&str]) -> String {
        let output = self.crosskit(arguments);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`crosskit {}` did not succeed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Runs the command with its output going into a pipe that is closed at once, which is
    /// what `| head` does to it, and returns how the run ended and what it said on stderr.
    fn with_the_reader_leaving(&self, arguments: &[&str]) -> (Option<i32>, String) {
        let mut child = self
            .command(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Dropped before the child can finish printing: from here on every write it makes
        // goes into a pipe with nobody at the other end.
        drop(child.stdout.take());
        let output = child.wait_with_output().unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("failed to remove {}: {error}", self.root.display());
        }
    }
}

/// The first thing a person types. It has to work before anything is configured, name every
/// command, and end with zero.
#[test]
fn help_answers_on_a_machine_where_nothing_is_set_up() {
    let sandbox = Sandbox::new("help");

    let overview = sandbox.succeeding(&["--help"]);

    assert!(overview.contains("Usage: crosskit"), "{overview}");
    assert!(
        overview.contains("crosskit setup"),
        "the overview says where to start: {overview}"
    );
    for option in ["--context", "--help", "--version"] {
        assert!(overview.contains(option), "{option} is missing: {overview}");
    }
    let listed = commands(&sandbox);
    assert!(
        listed.len() >= 8,
        "every command belongs in the overview: {listed:?}"
    );
}

/// Every command answers `--help` with its own grammar, and the answer is the same text the
/// command refuses with — one source, not a copy that can drift.
#[test]
fn every_command_explains_itself_and_says_the_same_thing_when_it_refuses() {
    let sandbox = Sandbox::new("per-command");

    for command in commands(&sandbox) {
        let helped = sandbox.succeeding(&[&command, "--help"]);
        assert!(
            helped.contains(&format!("crosskit {command} —")),
            "`crosskit {command} --help` does not name the command: {helped}"
        );
        let usage = helped
            .lines()
            .find(|line| line.starts_with("Usage: crosskit"))
            .unwrap_or_else(|| panic!("no usage line for {command}: {helped}"));
        assert!(
            usage.contains(&command),
            "the usage of {command} does not spell it: {usage}"
        );

        // A refusal that quotes a usage line has to quote this same one. That is what
        // makes the help and the parser one text: a second copy would drift, and the person
        // reading the refusal would be told a grammar the parser does not enforce.
        let refused = sandbox.crosskit(&[&command]);
        let said = String::from_utf8_lossy(&refused.stderr);
        if said.contains("Usage: crosskit") {
            assert!(
                said.contains(usage),
                "`crosskit {command}` refuses with a usage line that is not the one \
                 `--help` prints.\nrefusal: {said}\nhelp:    {usage}"
            );
        }
    }
}

/// `--help` works wherever it appears, and a word that is not a command is refused by name
/// rather than answered with a general text that does not mention it.
#[test]
fn help_is_asked_for_by_the_same_word_everywhere_and_a_mistyped_command_is_named() {
    let sandbox = Sandbox::new("help-placement");

    assert!(sandbox.succeeding(&["--help"]).contains("Commands:"));
    assert!(
        sandbox
            .succeeding(&["--context", "work", "status", "--help"])
            .contains("Usage: crosskit"),
        "naming a context must not hide the question"
    );

    let refused = sandbox.crosskit(&["statuss", "--help"]);
    assert_eq!(refused.status.code(), Some(1));
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("statuss"), "{said}");
    assert!(
        said.contains("status") && said.contains("--help"),
        "a mistyped command is told what there is instead: {said}"
    );
}

/// The version comes from the package, so there is no second place to remember.
#[test]
fn the_version_is_the_one_the_package_declares() {
    let sandbox = Sandbox::new("version");

    let printed = sandbox.succeeding(&["--version"]);

    assert_eq!(
        printed.trim(),
        format!("crosskit {}", env!("CARGO_PKG_VERSION"))
    );
}

/// Every row of the exit code table, checked against the program.
///
/// Stated as a table here too, because the point is the rule and not any one case: a change
/// that made one command end differently from the others would show up as one row.
#[test]
fn a_run_ends_with_the_code_the_table_says() {
    let sandbox = Sandbox::new("exit-codes");

    let rows: &[(&[&str], i32, &str)] = &[
        (&["--help"], 0, "a question about the program"),
        (&["--version"], 0, "a question about the program"),
        (&["status", "--help"], 0, "a question about one command"),
        (
            &["status"],
            0,
            "did what was asked: there is no context yet, and that is an answer",
        ),
        (&["statuss"], 1, "there is no such command"),
        (&["policy"], 1, "a request that does not parse"),
        (&["policy", "explain"], 1, "a request that does not parse"),
        (
            &["search", "word"],
            1,
            "no context is configured, so nothing can be searched",
        ),
        (
            &["doctor"],
            1,
            "no context is configured, so there is nothing to report on",
        ),
    ];

    for (arguments, expected, why) in rows {
        let output = sandbox.crosskit(arguments);
        assert_eq!(
            output.status.code(),
            Some(*expected),
            "`crosskit {}` ({why}) ended with {:?}, stderr: {}",
            arguments.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A run whose output nobody is reading ends quietly and with zero. It did what was asked;
/// the person who asked stopped listening.
#[test]
fn a_run_whose_reader_left_ends_quietly_with_zero() {
    let sandbox = Sandbox::new("broken-pipe");

    for arguments in [
        vec!["--help"],
        vec!["status"],
        vec!["status", "--help"],
        vec!["--version"],
    ] {
        let (code, said) = sandbox.with_the_reader_leaving(&arguments);
        assert_eq!(
            code,
            Some(0),
            "`crosskit {}` ended with {code:?} when its reader left; stderr: {said}",
            arguments.join(" ")
        );
        assert!(
            said.is_empty(),
            "`crosskit {}` complained to a stderr nobody asked about: {said}",
            arguments.join(" ")
        );
    }
}

/// A command that fails still fails when its output is not being read: the reader leaving is
/// not a way to turn a refusal into a success.
#[test]
fn a_reader_leaving_does_not_turn_a_refusal_into_a_success() {
    let sandbox = Sandbox::new("broken-pipe-failure");

    let (code, _) = sandbox.with_the_reader_leaving(&["statuss"]);

    assert_eq!(
        code,
        Some(1),
        "a command that was refused was still refused"
    );
}

/// Every refusal is written in one voice.
///
/// What that means here is narrow and checkable: a refusal continues the sentence
/// `crosskit failed: `, so it begins in lower case, and it is long enough to say something
/// a person can act on. Whether each one says the right thing is a reading, not an
/// assertion; what this catches is the next message written in a voice of its own.
///
/// It checks each message on its own, which is why it could not see a message assembled out
/// of two — a lower layer's sentence with the command layer's paragraph after it, each half
/// right and the two together saying one thing twice. That property needs the halves, not
/// the process, so it is checked where they are composed: `presentation::one_voice_tests`
/// walks every message this program builds that way.
#[test]
fn every_refusal_continues_the_same_sentence() {
    let sandbox = Sandbox::new("voice");

    let refusals: &[&[&str]] = &[
        &["statuss"],
        &["policy"],
        &["policy", "explain"],
        &["policy", "clarify", "/tmp"],
        &["contexts"],
        &["contexts", "invent", "/tmp"],
        &["search"],
        &["search", "word", "--limit"],
        &["search", "word", "--limit", "many"],
        &["search", "word", "--after"],
        &["search", "word", "--nonsense"],
        &["ask"],
        &["index"],
        &["index", "rebuild"],
        &["index", "runs"],
        &["index", "runs", "prune"],
        &["index", "runs", "prune", "--keep"],
        &["index", "runs", "prune", "--keep", "many"],
        &["index", "jobs", "run"],
        &["index", "jobs", "run", "--limit"],
        &["index", "jobs", "run", "--limit", "many"],
        &["index", "jobs", "withdraw", "--yes"],
        &["migrate", "--all"],
        &["migrate", "now"],
        &["index", "recover", "--wrong"],
        &["setup"],
        &["setup", "/tmp", "--force"],
        &["status", "extra"],
        &["--context", "a", "--context", "b", "status"],
        &["--context", "work", "setup", "/tmp"],
        &["--context", "work", "contexts", "propose", "/tmp"],
    ];

    for arguments in refusals {
        let output = sandbox.crosskit(arguments);
        assert_eq!(
            output.status.code(),
            Some(1),
            "`crosskit {}` was expected to be refused",
            arguments.join(" ")
        );
        let said = String::from_utf8_lossy(&output.stderr);
        let Some(message) = said.strip_prefix("crosskit failed: ") else {
            panic!(
                "`crosskit {}` did not report through the one place that reports: {said}",
                arguments.join(" ")
            );
        };
        let first = message.chars().next().unwrap_or(' ');
        assert!(
            !first.is_uppercase(),
            "`crosskit {}` starts a new sentence where it continues one: {message:?}",
            arguments.join(" ")
        );
        assert!(
            message.trim().len() > 20,
            "`crosskit {}` says too little to act on: {message:?}",
            arguments.join(" ")
        );
    }
}

/// The documentation and the program name the same commands.
///
/// A README that promises a command the program does not have is a person following
/// instructions into an error, and this has already cost a run once: the hint in an
/// `#[ignore]` drifted from the README and somebody ran the wrong thing.
#[test]
fn every_command_the_readme_spells_is_a_command_that_exists() {
    let sandbox = Sandbox::new("readme");
    let known = commands(&sandbox);
    // Read rather than skipped when absent: a check of the documentation that quietly
    // passes when it cannot find the documentation is worse than no check.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "this test reads {}, so it needs the checkout it was built from: {error}",
            path.display()
        )
    });

    // Every word that follows `crosskit` in the README, with the options that may stand
    // between them skipped the way the dispatcher skips them.
    let mut spelled: Vec<String> = Vec::new();
    for mention in readme.split("crosskit ").skip(1) {
        let mut words = mention.split_whitespace();
        let mut word = words.next();
        while let Some(current) = word {
            if current == "--context" {
                words.next();
                word = words.next();
                continue;
            }
            break;
        }
        let Some(word) = word else { continue };
        let word = word.trim_end_matches(['`', '.', ',', ':', ';', ')', '"']);
        if word.is_empty() || word.starts_with('-') || word.starts_with('<') {
            continue;
        }
        if !spelled.contains(&word.to_string()) {
            spelled.push(word.to_string());
        }
    }

    let promised_but_absent: Vec<&String> = spelled
        .iter()
        .filter(|word| !known.contains(word))
        .collect();
    assert!(
        promised_but_absent.is_empty(),
        "the README spells commands this program does not have: {promised_but_absent:?}\n\
         it has: {known:?}"
    );

    let real_but_undocumented: Vec<&String> = known
        .iter()
        .filter(|name| !spelled.contains(name))
        .collect();
    assert!(
        real_but_undocumented.is_empty(),
        "these commands exist and the README never spells them: {real_but_undocumented:?}"
    );
}

/// Every command in the list is one the dispatcher takes, and a word that is not in it is
/// not taken. "Taken" means it was understood as a command — refused for what it was given,
/// never refused for its name.
#[test]
fn the_listed_commands_are_the_ones_that_parse_and_no_others_do() {
    let sandbox = Sandbox::new("dispatch");

    for command in commands(&sandbox) {
        let output = sandbox.crosskit(&[&command]);
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(
            !said.contains("there is no command"),
            "`crosskit {command}` is listed but the dispatcher does not know it: {said}"
        );
    }

    for absent in ["statuss", "reindex", "explain", "init", "help"] {
        let refused = sandbox.crosskit(&[absent, "--help"]);
        assert_eq!(
            refused.status.code(),
            Some(1),
            "`crosskit {absent} --help` must not be answered as a command"
        );
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("there is no command"),
            "`crosskit {absent}` was not refused by name"
        );
    }
}
