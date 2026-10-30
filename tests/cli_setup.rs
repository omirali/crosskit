// `crosskit setup` end to end: the first run on a machine that has nothing configured.
//
// The command is a conversation now — the person names the contexts, says where the remainder
// goes, answers about each group — and then a chain: show, ask, write, index. Every test here is
// about one link of it holding: that each context holds what its groups hold and its own
// conversations and no one else's, that nothing reaches a context without an answer, that a
// run nobody finished answering leaves the disk exactly as it found it, that `--yes` has
// nobody to name contexts and says so, and that a machine which already has contexts is
// refused rather than rewritten.
mod common;

use std::fs;

use common::{HomeSandbox, snapshot};

/// The unique words the fixture's conversations say, one per conversation, so a search for one
/// can only find the conversation that said it.
const ALPHA_WORD: &str = "взаимозачёты";
const GAMMA_WORD: &str = "гаммаплан";
const SUBAGENT_WORD: &str = "подагентслово";
const SCRATCH_WORD: &str = "черновикпробы";
const MIXED_WORD: &str = "смешанныйразговор";

/// The three contexts every full dialog here names — three, so no number of contexts is built
/// into what passes, and one in Cyrillic, which TOML cannot read as a bare key.
const THREE_CONTEXTS: &str = "client-a\nclient-b\nличное\n\n";
/// Remainder to `личное`; group 1 to `client-a`, group 2 to `client-b`, group 3 left without an
/// answer; then yes.
const ANSWERS: &str = "3\n1\n2\n\n";

/// A home with two organisations on one host, a project without a remote, a folder of
/// documents, the transcript folders of two projects, one of a throwaway directory and one of the
/// directory that holds the projects, a subagent transcript, and a Codex session.
fn two_organizations(name: &str) -> HomeSandbox {
    let sandbox = HomeSandbox::new(name);
    sandbox.file(
        ".gitconfig",
        "[user]\n\tname = Alice\n\temail = alice@acme.example\n",
    );
    sandbox
        .repository(
            "Projects/alpha",
            "git@git.example.org:acme.example/alpha.git",
        )
        .file("Projects/alpha/main.rs", "fn main() {}\n")
        .file("Projects/alpha/notes.md", "альфа ведёт расчёты");
    sandbox
        .repository(
            "Projects/beta",
            "https://git.example.org/acme.example/platform/beta.git",
        )
        .file("Projects/beta/readme.md", "бета");
    sandbox
        .repository("Projects/gamma", "git@git.example.org:org-two/gamma.git")
        .file("Projects/gamma/plan.md", "гамма");
    sandbox
        .repository("Projects/local", "")
        .file("Projects/local/todo.md", "локальный список");
    sandbox.file("Documents/letters/letter.txt", "личное письмо");

    let alpha = sandbox.resolved("Projects/alpha");
    let main = format!("{alpha}/main.rs");
    sandbox.conversation(
        &alpha,
        "a1",
        &[
            title("a1", "Беседа про расчёты"),
            said("a1", &alpha, &format!("обсуждаем {ALPHA_WORD} альфы")),
            read("a1", &alpha, &main),
        ],
    );
    sandbox.file(
        &format!(
            ".claude/projects/{}/a1/subagents/agent-1.jsonl",
            common::transcript_folder_name(&alpha)
        ),
        &format!("{}\n", said("a1", &alpha, SUBAGENT_WORD)),
    );
    let gamma = sandbox.resolved("Projects/gamma");
    sandbox.conversation(&gamma, "g1", &[said("g1", &gamma, GAMMA_WORD)]);
    let scratch = "/private/tmp/crosskit-scratch-probe";
    sandbox.conversation(scratch, "t1", &[said("t1", scratch, SCRATCH_WORD)]);
    let projects = sandbox.resolved("Projects");
    sandbox.conversation(&projects, "m1", &[said("m1", &projects, MIXED_WORD)]);
    sandbox.file(".codex/sessions/2026/09/30/rollout-1.jsonl", "{}\n");
    sandbox
}

fn title(session: &str, name: &str) -> String {
    format!(r#"{{"type":"custom-title","sessionId":"{session}","customTitle":"{name}"}}"#)
}

fn said(session: &str, cwd: &str, text: &str) -> String {
    format!(
        r#"{{"type":"user","sessionId":"{session}","cwd":"{cwd}","timestamp":"2026-09-30T07:00:00.000Z","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

fn read(session: &str, cwd: &str, file: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{session}","cwd":"{cwd}","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Write","input":{{"file_path":"{file}"}}}}]}}}}"#
    )
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// One section of the written configuration: from its header to the next.
fn section<'a>(config: &'a str, header: &str) -> &'a str {
    let start = config
        .find(header)
        .unwrap_or_else(|| panic!("no {header} in:\n{config}"));
    let rest = &config[start + header.len()..];
    let end = rest.find("\n[").map_or(rest.len(), |end| end + 1);
    &rest[..end]
}

fn entries(directory: &std::path::Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn search(sandbox: &HomeSandbox, context: &str, word: &str) -> String {
    let output = sandbox.crosskit(&["--context", context, "search", word]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    stdout_of(&output)
}

/// The point of the stage. The person names three contexts, answers three questions about four
/// projects, and gets three contexts — each holding its groups' projects and their own
/// conversations and nothing of anybody else's.
#[test]
fn the_person_names_the_contexts_and_each_holds_its_own_conversations() {
    let sandbox = two_organizations("three-contexts");

    let output = sandbox.setup_answering(&format!("{THREE_CONTEXTS}{ANSWERS}y\n"));

    assert!(
        output.status.success(),
        "setup failed: {}",
        stderr_of(&output)
    );
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("Group 3 of 3") && !stdout.contains("Group 4"),
        "four projects, three questions:\n{stdout}"
    );
    assert!(
        stdout.contains(
            "Group 1 of 3: `origin` under group acme.example on git.example.org — looks like \
             your organization\n"
        ) && stdout.contains("Group 2 of 3: `origin` under group org-two on git.example.org\n"),
        "the group carrying the mail's domain is marked and the other is not:\n{stdout}"
    );
    assert!(
        stdout.contains("Codex: 1 session found; they are outside every context"),
        "{stdout}"
    );

    let config = fs::read_to_string(sandbox.config_path()).unwrap();
    let alpha = sandbox.resolved("Projects/alpha");
    let alpha_folder = format!(
        "{}/{}",
        sandbox.resolved(".claude/projects"),
        common::transcript_folder_name(&alpha)
    );
    let client_a = section(&config, "[workspace.client-a]");
    for root in [&alpha, &sandbox.resolved("Projects/beta"), &alpha_folder] {
        assert!(
            client_a.contains(&format!("\"{root}\"")),
            "{root} in:\n{client_a}"
        );
    }
    let client_b = section(&config, "[workspace.client-b]");
    assert!(
        client_b.contains(&sandbox.resolved("Projects/gamma")),
        "{client_b}"
    );
    let own = section(&config, "[workspace.\"личное\"]");
    assert!(own.contains(&sandbox.resolved("Projects/local")), "{own}");
    assert!(
        own.contains(&sandbox.resolved("Documents/letters")),
        "{own}"
    );
    assert!(
        !config.contains("crosskit-scratch-probe")
            && !config.contains(&format!(
                "\"{}/{}\"",
                sandbox.resolved(".claude/projects"),
                common::transcript_folder_name(&sandbox.resolved("Projects"))
            )),
        "a throwaway folder and a mixed one are in no context:\n{config}"
    );
    assert!(
        !config
            .lines()
            .any(|line| line.trim_start().starts_with("send_content_outside")),
        "no context lets its content leave:\n{config}"
    );

    // Each context finds its own conversation and nobody else's.
    assert!(search(&sandbox, "client-a", ALPHA_WORD).contains("a1.jsonl"));
    assert!(!search(&sandbox, "client-b", ALPHA_WORD).contains("a1.jsonl"));
    assert!(!search(&sandbox, "личное", ALPHA_WORD).contains("a1.jsonl"));
    assert!(search(&sandbox, "client-b", GAMMA_WORD).contains("g1.jsonl"));
    assert!(!search(&sandbox, "client-a", GAMMA_WORD).contains("g1.jsonl"));
    for context in ["client-a", "client-b", "личное"] {
        for word in [SUBAGENT_WORD, SCRATCH_WORD, MIXED_WORD] {
            assert!(
                search(&sandbox, context, word).contains("Results: 0"),
                "{word} is in no conversation of {context}"
            );
        }
    }
    let touched = sandbox.crosskit(&["--context", "client-a", "touches", "main.rs"]);
    assert!(
        stdout_of(&touched).contains("Беседа про расчёты"),
        "{}{}",
        stdout_of(&touched),
        stderr_of(&touched)
    );
    let elsewhere = sandbox.crosskit(&["--context", "client-b", "touches", "main.rs"]);
    assert!(
        !stdout_of(&elsewhere).contains("Беседа про расчёты"),
        "{}",
        stdout_of(&elsewhere)
    );

    // And the run said what each context holds and what stayed outside.
    for line in [
        "client-a: 2 projects, 1 conversation",
        "client-b: 1 project, 1 conversation",
        "личное: 1 project, 1 folder without git, 0 conversations",
        "temporary places: 1 place, 1 conversation",
        "mixed folders: 1 place, 1 conversation",
        "Contexts indexed whole: 3",
        "client-a 1 conversation, client-b 1 conversation, личное 0 conversations",
    ] {
        assert!(stdout.contains(line), "{line:?} missing from:\n{stdout}");
    }
}

/// Shown and written are one string. What the person agreed to is what landed on disk,
/// character for character.
#[test]
fn the_file_written_is_the_file_that_was_shown() {
    let sandbox = two_organizations("shown-is-written");

    let stdout = stdout_of(&sandbox.setup_answering(&format!("{THREE_CONTEXTS}{ANSWERS}y\n")));
    let written = fs::read_to_string(sandbox.config_path()).unwrap();

    assert!(
        stdout.contains(&written),
        "the file was shown whole before it was written:\nshown:\n{stdout}\nwritten:\n{written}"
    );
}

/// Every question answered and the last one answered no: nothing is written, and the disk is
/// asked rather than the wording.
#[test]
fn a_run_that_was_not_confirmed_writes_nothing_anywhere() {
    let sandbox = two_organizations("declined");
    let before = snapshot(&sandbox.home);

    let output = sandbox.setup_answering(&format!("{THREE_CONTEXTS}{ANSWERS}n\n"));

    assert!(
        output.status.success(),
        "declining is an answer, not a fault"
    );
    assert!(stdout_of(&output).contains("Nothing was written."));
    assert_eq!(entries(&sandbox.data_directory), Vec::<String>::new());
    assert_eq!(snapshot(&sandbox.home), before, "and the tree is untouched");
}

/// A closed input is not a yes to any question. Wherever the answers stop — before a name,
/// after the names, in the middle of the groups, before the last yes — nothing happens.
#[test]
fn an_input_that_ends_before_the_last_answer_writes_nothing() {
    let sandbox = two_organizations("no-answer");
    for typed in [
        String::new(),
        THREE_CONTEXTS.to_string(),
        format!("{THREE_CONTEXTS}3\n1\n"),
        format!("{THREE_CONTEXTS}{ANSWERS}"),
    ] {
        let output = sandbox.setup_answering(&typed);

        assert!(output.status.success(), "{typed:?}: {}", stderr_of(&output));
        assert!(
            stdout_of(&output).contains("Nothing was written"),
            "{typed:?}"
        );
        assert_eq!(
            entries(&sandbox.data_directory),
            Vec::<String>::new(),
            "{typed:?} wrote something"
        );
    }
}

/// `--yes` used to write the proposal unasked. Now the contexts are named by a person, and a
/// run nobody watches has nobody to name them: it is refused before anything is read.
#[test]
fn yes_is_refused_because_nobody_is_there_to_name_the_contexts() {
    let sandbox = two_organizations("yes");

    let refused = sandbox.crosskit(&["setup", "--yes", &sandbox.home_argument()]);

    assert_eq!(refused.status.code(), Some(1));
    let stderr = stderr_of(&refused);
    assert!(
        stderr.contains("contexts are named by a person"),
        "{stderr}"
    );
    assert!(stderr.contains("no guess"), "{stderr}");
    assert_eq!(entries(&sandbox.data_directory), Vec::<String>::new());
}

/// A machine that already declares contexts is refused, with or without `--yes`, and the
/// refusal says what to do: move the file aside and run again.
#[test]
fn a_machine_that_already_has_contexts_is_refused_and_its_configuration_is_untouched() {
    let sandbox = two_organizations("already-configured");
    let existing = format!(
        "[workspace.mine]\ninclude = [\"{}\"]\n",
        sandbox.resolved("")
    );
    fs::write(sandbox.config_path(), &existing).unwrap();

    for arguments in [
        vec!["setup".to_string(), sandbox.home_argument()],
        vec![
            "setup".to_string(),
            "--yes".to_string(),
            sandbox.home_argument(),
        ],
    ] {
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let refused = sandbox.crosskit(&arguments);

        assert_eq!(refused.status.code(), Some(1));
        let stderr = stderr_of(&refused);
        assert!(stderr.contains("already has the context mine"), "{stderr}");
        assert!(stderr.contains("move that file aside"), "{stderr}");
        assert_eq!(fs::read_to_string(sandbox.config_path()).unwrap(), existing);
        assert_eq!(
            entries(&sandbox.data_directory),
            vec!["config.toml".to_string()]
        );
    }
}

/// A configuration that declares no context is replaced rather than refused — but the file
/// that was there is kept beside the new one, under a name carrying the moment.
#[test]
fn a_configuration_without_contexts_is_replaced_and_the_old_one_is_kept() {
    let sandbox = two_organizations("kept");
    fs::write(sandbox.config_path(), "[index]\nmax_content_size_mb = 4\n").unwrap();

    let output = sandbox.setup_answering(&format!("{THREE_CONTEXTS}{ANSWERS}y\n"));

    assert!(output.status.success(), "{}", stderr_of(&output));
    let kept: Vec<String> = entries(&sandbox.data_directory)
        .into_iter()
        .filter(|name| name.contains("before-setup-"))
        .collect();
    assert_eq!(kept.len(), 1, "exactly one file kept: {kept:?}");
    assert_eq!(
        fs::read_to_string(sandbox.data_directory.join(&kept[0])).unwrap(),
        "[index]\nmax_content_size_mb = 4\n"
    );
    assert!(stdout_of(&output).contains("is kept at:"));
}

/// One context that cannot be indexed does not take the others with it.
#[test]
fn a_context_that_fails_leaves_the_others_indexed_and_the_run_names_it() {
    let sandbox = two_organizations("partial-failure");
    fs::write(
        sandbox.data_directory.join("graph-client-b.lbug"),
        "this is not a database",
    )
    .unwrap();

    let output = sandbox.setup_answering(&format!("{THREE_CONTEXTS}{ANSWERS}y\n"));

    assert_eq!(output.status.code(), Some(1));
    let stdout = stdout_of(&output);
    assert!(stdout.contains("Contexts configured: 3"), "{stdout}");
    assert!(stdout.contains("Contexts indexed whole: 2"), "{stdout}");
    assert!(
        stderr_of(&output).contains("client-b"),
        "{}",
        stderr_of(&output)
    );
    assert!(search(&sandbox, "client-a", ALPHA_WORD).contains("a1.jsonl"));
}

/// A directory with nothing in it is answered plainly, before any question, and writes nothing.
#[test]
fn a_directory_with_nothing_in_it_is_answered_rather_than_configured() {
    let sandbox = HomeSandbox::new("empty");

    let output = sandbox.setup_answering("");

    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout_of(&output).contains("Nothing to propose"),
        "{}",
        stdout_of(&output)
    );
    assert_eq!(entries(&sandbox.data_directory), Vec::<String>::new());
}

/// Files and no project, transcript folder or folder of documents: nothing to ask about, so
/// nothing is asked and nothing is written.
#[test]
fn a_tree_that_holds_nothing_to_ask_about_is_explained_rather_than_configured() {
    let sandbox = HomeSandbox::new("no-signal");
    sandbox.file("stuff/a.txt", "просто файл");

    let output = sandbox.setup_answering("");

    assert_eq!(output.status.code(), Some(0));
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("Nothing was written: nothing under"),
        "{stdout}"
    );
    assert!(!stdout.contains("Name the contexts"), "{stdout}");
    assert_eq!(entries(&sandbox.data_directory), Vec::<String>::new());
}

/// Setup writes the contexts, so it cannot be told to run inside one.
#[test]
fn a_named_context_is_refused_rather_than_ignored() {
    let sandbox = two_organizations("named-context");

    let refused = sandbox.crosskit(&["--context", "mine", "setup", &sandbox.home_argument()]);

    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stderr_of(&refused).contains("--context mine"),
        "{}",
        stderr_of(&refused)
    );
    assert!(!sandbox.config_path().exists());
}

/// `n` to the remainder used to mean "none of it", and two plainly work folders went with the
/// messenger's. Now each folder is asked about after the groups: here two go to two contexts and
/// one is skipped, and the file holds exactly those two, each in its own context.
#[test]
fn a_remainder_answered_no_is_asked_about_folder_by_folder() {
    let sandbox = two_organizations("remainder-by-folder");
    sandbox
        .file("Documents/drafts/plan.txt", "черновик плана")
        .file("Documents/reports/q3.txt", "квартальный отчёт");

    // Folders are asked in path order: drafts, letters, reports.
    let output = sandbox.setup_answering(&format!("{THREE_CONTEXTS}n\n1\n2\n\n1\n2\ns\ny\n"));

    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = stdout_of(&output);
    assert!(stdout.contains("Folder 3 of 3"), "{stdout}");
    let config = fs::read_to_string(sandbox.config_path()).unwrap();
    let by_answer: Vec<&str> = config
        .lines()
        .filter(|line| line.contains("this folder by your answer"))
        .collect();
    assert_eq!(by_answer.len(), 2, "{config}");
    assert!(
        section(&config, "[workspace.client-a]").contains(&sandbox.resolved("Documents/drafts")),
        "{config}"
    );
    assert!(
        section(&config, "[workspace.client-b]").contains(&sandbox.resolved("Documents/letters")),
        "{config}"
    );
    assert!(
        !config.contains(&sandbox.resolved("Documents/reports")),
        "{config}"
    );
    assert!(
        !config.contains(&sandbox.resolved("Projects/local")),
        "a group left without an answer and no remainder context goes nowhere:\n{config}"
    );
}

/// Setup indexes every root of every context, so it says one line a root and the totals; the
/// root that could not read a path says so on its line, and the others say nothing of failure.
#[test]
fn setup_says_one_line_a_root_and_a_failure_only_where_there_is_one() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = two_organizations("one-line");
    let closed = sandbox.home.join("Projects/gamma/closed");
    fs::create_dir_all(&closed).unwrap();
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();

    let output = sandbox.setup_answering(&format!("{THREE_CONTEXTS}{ANSWERS}y\n"));
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = stdout_of(&output);
    let block = |context: &str| -> Vec<String> {
        let start = stdout
            .find(&format!("Indexing context {context}..."))
            .unwrap_or_else(|| panic!("{stdout}"));
        stdout[start..]
            .lines()
            .skip(1)
            .take_while(|line| !line.starts_with("Roots walked"))
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect()
    };
    let client_a = block("client-a");
    assert_eq!(client_a.len(), 3, "three roots, three lines: {client_a:#?}");
    assert!(
        client_a.iter().all(|line| !line.contains("not read")),
        "{client_a:#?}"
    );
    let client_b = block("client-b");
    assert_eq!(client_b.len(), 2, "{client_b:#?}");
    assert!(
        client_b[0].contains(&sandbox.resolved("Projects/gamma"))
            && client_b[0].ends_with(", 1 path not read"),
        "{client_b:#?}"
    );
    assert!(!client_b[1].contains("not read"), "{client_b:#?}");
    assert_eq!(
        stdout
            .matches("index init <directory>` prints for it")
            .count(),
        1
    );
    assert!(!stdout.contains("Metadata-only files"), "{stdout}");
}
