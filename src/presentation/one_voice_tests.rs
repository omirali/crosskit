// One face, checked on messages this layer assembles out of two.
//
// Stage 15 gave every refusal one voice and checked each message on its own: it continues
// the sentence `crosskit failed: `, and it says enough to act on. That check cannot see
// the defect this module exists for, because the defect needs two halves to appear. A
// lower layer's message said "only a command that opens it for writing migrates it", the
// command layer appended "Any command that opens this context for writing migrates it —",
// and each half was right; together they were one paragraph saying one thing twice.
//
// So the property here is about the whole: no message this layer composes says the same
// run of words twice. Prose only — a command spelling and a usage grammar repeat by nature,
// and repeating them is how a person is told which command to run.
use crate::application::RepositoryError;
use crate::domain::WorkspaceName;
use crate::infrastructure::analysis::EngineUnavailable;

use super::{help, workspace};

/// How many words in a row have to come back before it is a repetition rather than a
/// coincidence. Three catches ordinary English ("that opens this", "and this build");
/// four is the shortest run that only appears when a sentence is being said again, and it
/// is what the seam above tripped on — "for writing migrates it".
const RUN: usize = 4;

/// The first run of `RUN` words this message says twice, when it says one at all.
///
/// Two occurrences never overlap: the search for the second one starts where the first
/// ends, so a phrase is not found repeating inside itself.
fn phrase_said_twice(message: &str) -> Option<String> {
    let words = prose_words(message);
    if words.len() < RUN {
        return None;
    }
    for start in 0..=words.len() - RUN {
        let run = &words[start..start + RUN];
        if words[start + RUN..].windows(RUN).any(|later| later == run) {
            return Some(run.join(" "));
        }
    }
    None
}

/// The words of a message that are prose, lowercased and stripped of punctuation.
///
/// What is left out is left out because repeating it is the point: a command inside
/// backticks is an instruction to type it, and a usage grammar lists one line per form of
/// the command, every line beginning with the same words.
fn prose_words(message: &str) -> Vec<String> {
    let mut prose = String::new();
    let mut inside_backticks = false;
    for part in message.split('`') {
        if !inside_backticks {
            prose.push_str(part);
            prose.push(' ');
        }
        inside_backticks = !inside_backticks;
    }
    prose
        .lines()
        .flat_map(|line| {
            let end = ["Usage:", "crosskit "]
                .iter()
                .filter_map(|marker| line.find(marker))
                .min()
                .unwrap_or(line.len());
            line[..end].split(|character: char| !character.is_alphanumeric())
        })
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn workspace() -> WorkspaceName {
    WorkspaceName::parse("work").expect("a context name this program accepts")
}

/// Every message this layer builds by putting its own words after a lower layer's, in one
/// list, so the next one that is added is checked by being added here.
fn composed_messages() -> Vec<(String, String)> {
    let named = workspace();
    let mut messages: Vec<(String, String)> = vec![
        (
            "explain: the graph is written in another build's schema".to_string(),
            workspace::explain(
                &named,
                RepositoryError::SchemaNotThisBuild {
                    recorded: 12,
                    expected: 13,
                },
            ),
        ),
        (
            "explain: a pass was stopped without closing the graph".to_string(),
            workspace::explain(
                &named,
                RepositoryError::GraphLogUnreadable {
                    log: std::path::PathBuf::from("/data/graph-work.lbug.wal"),
                },
            ),
        ),
        (
            "explain: another process holds the graph".to_string(),
            workspace::explain(&named, RepositoryError::GraphBusy),
        ),
        (
            "explain: anything else the storage says".to_string(),
            workspace::explain(
                &named,
                RepositoryError::Storage("the database file is not one".to_string()),
            ),
        ),
        (
            "unknown command".to_string(),
            help::unknown_command("statuss"),
        ),
        // The other shape this layer composes in, and the one the seam should have used:
        // the lower layer takes the caller's clause as an argument instead of the caller
        // appending a paragraph to a finished sentence.
        (
            "the engine is not installed".to_string(),
            format!(
                "Analysis: not run — {}",
                EngineUnavailable::NotInstalled.reason("No job was claimed.")
            ),
        ),
        (
            "the engine has no key".to_string(),
            format!(
                "Analysis: not run — {}",
                EngineUnavailable::NoApiKey.reason("No job was claimed.")
            ),
        ),
    ];
    for command in help::COMMANDS {
        messages.push((
            format!("misuse: {}", command.name),
            help::misuse(command.usage),
        ));
    }
    messages
}

#[test]
fn no_message_this_layer_composes_says_the_same_thing_twice() {
    for (what, message) in composed_messages() {
        assert!(
            phrase_said_twice(&message).is_none(),
            "{what} says {:?} twice in one message:\n{message}",
            phrase_said_twice(&message).unwrap_or_default()
        );
    }
}

/// The seam itself, in the shape that produced it: the storage layer states the two
/// versions, this layer names the command, and neither states the other's half. Asserted on
/// the composed message rather than on either side, because that is where the repetition
/// was — both halves were true on their own.
#[test]
fn a_schema_this_build_does_not_read_is_stated_once_and_repaired_once() {
    let message = workspace::explain(
        &workspace(),
        RepositoryError::SchemaNotThisBuild {
            recorded: 12,
            expected: 13,
        },
    );

    assert!(
        message.contains("12") && message.contains("13"),
        "{message}"
    );
    assert_eq!(
        message.matches("migrat").count(),
        2,
        "the word belongs to the fact and to the command, once each: {message}"
    );
    assert!(
        message.contains("`crosskit migrate`"),
        "the repair is named, and it is the one that reaches every context: {message}"
    );
}

/// The scanner has to see the defect it was written for, or it is a test that passes by
/// looking away. This is the paragraph as it was printed, with both halves in it.
#[test]
fn the_scanner_finds_the_repetition_the_seam_printed() {
    let as_it_was = "this graph records schema version 12 and this build reads version 13; \
                     only a command that opens it for writing migrates it. Any command that \
                     opens this context for writing migrates it.";

    assert_eq!(
        phrase_said_twice(as_it_was).as_deref(),
        Some("for writing migrates it")
    );
}

/// And it does not fire on prose that merely reads alike, or on a message that spells one
/// command twice on purpose.
#[test]
fn a_command_said_twice_and_ordinary_english_are_not_repetitions() {
    assert_eq!(
        phrase_said_twice(
            "`crosskit --context work index recover --check` says whether it still opens; \
             `crosskit --context work index recover` moves the log aside."
        ),
        None
    );
    assert_eq!(phrase_said_twice("it is what it is").as_deref(), None);
}
