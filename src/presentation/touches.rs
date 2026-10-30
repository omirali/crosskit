// `crosskit [--context <name>] touches <path>` and `touches --in <conversation>`.
//
// The question the whole direction was for — "which chat did we work on this file in" — and
// its other side. One line stands for one conversation, or for one file, folded from however
// many times it was touched.
//
// What the default answer leaves out is counted rather than dropped, and what nothing here
// can know is said in the same breath: a shell command is not a touch, so a file only ever
// grepped for is a file this command has never heard of.
use crate::application::{
    Direction, TouchGroup, TouchQuery, TouchReport, TouchService, TouchedPair,
};
use crate::domain::OwnerNaming;
use crate::domain::conversation::TouchVerb;
use crate::infrastructure::{load_index_policy, utc_stamp};

use super::help;
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] touches <path> [--include-in-passing]\n       crosskit [--context <name>] touches --in <conversation> [--include-in-passing]";

/// What is not a touch, said wherever an empty answer could be read as "nobody worked on it".
///
/// Stage 32 measured why: 2 565 path-shaped words were found in shell command lines and only
/// 39% of them were files that exist, and a command line never says what happened to one —
/// `cat` reads it, `sed -i` rewrites it, `rg` only looks for a word inside it.
const WHAT_IS_NOT_A_TOUCH: &str = "Only the files a tool was handed are known here: reading, writing and editing. A path that appeared in a shell command — grep, cat, a script — is recorded nowhere, so silence about a file is not evidence that nobody opened it.";

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    let asked = Arguments::parse(arguments)?;
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select(&loaded, context)?;
    println!("Context: {selected}");

    let query = TouchQuery::new(&asked.needle, asked.direction, asked.include_in_passing)
        .map_err(|error| error.to_string())?;
    let manager = workspace::open_for_reading(&selected)?;
    let repository = manager.repository();
    let report = TouchService::new(&repository)
        .execute(&query)
        .map_err(|error| error.to_string())?;

    match asked.direction {
        Direction::WhichConversations => println!("File: {}", asked.needle),
        Direction::WhichFiles => println!("Conversation: {}", asked.needle),
    }
    render(&report);
    Ok(())
}

fn render(report: &TouchReport) {
    if report.groups().is_empty() {
        println!("Nothing in this context was touched under that name.");
        println!("{WHAT_IS_NOT_A_TOUCH}");
        return;
    }
    // Said before the groups and not after: a reader who takes the first group for the answer
    // has to meet this first.
    if report.is_ambiguous() {
        println!(
            "That name fits {} of them, and each one is answered separately below.",
            report.groups().len()
        );
    }
    for group in report.groups() {
        println!();
        render_group(group, report.direction());
    }
    if report.in_passing() > 0 && !report.widened() {
        println!();
        println!(
            "Left out, read and never named: {} (add --include-in-passing)",
            report.in_passing()
        );
        if report.unmarked_in_passing() > 0 {
            println!(
                "Of those, {} were stored before this build could tell, so nothing was measured about them. A conversation gains its marks when its transcript is read as a new revision — when the conversation goes on, or when the context is indexed afresh; a pass over a transcript that has not changed rewrites nothing.",
                report.unmarked_in_passing()
            );
        }
    }
}

fn render_group(group: &TouchGroup, direction: Direction) {
    println!("{}", group.heading());
    println!(
        "  {} {}",
        group.pairs().len(),
        match direction {
            Direction::WhichConversations => "conversation(s) worked on it",
            Direction::WhichFiles => "file(s) worked on",
        }
    );
    for (position, pair) in group.pairs().iter().enumerate() {
        render_pair(position + 1, pair, direction);
    }
    if group.pairs().is_empty() {
        println!(
            "  Known here, and only ever read in passing: {}.",
            group.in_passing()
        );
        println!("  {WHAT_IS_NOT_A_TOUCH}");
    }
}

fn render_pair(position: usize, pair: &TouchedPair, direction: Direction) {
    let subject = match direction {
        Direction::WhichConversations => conversation_label(pair),
        Direction::WhichFiles => pair.path().to_string(),
    };
    println!("{position}. [{}] {subject}", verbs(pair));
    let mut about: Vec<String> = Vec::new();
    if let Direction::WhichConversations = direction {
        about.push(format!("session {}", pair.session_id()));
    }
    if pair.times() > 1 {
        about.push(format!("{} times", pair.times()));
    }
    if pair.naming() == OwnerNaming::Named {
        about.push("you named this file".to_string());
    }
    if let Some(at) = pair.at_unix_ms() {
        about.push(format!("first {}", utc_stamp(at)));
    }
    if !about.is_empty() {
        println!("   {}", about.join(", "));
    }
}

/// A conversation is shown by its name, and by its session when nobody named it. An empty
/// label would be a row a person cannot ask about again.
fn conversation_label(pair: &TouchedPair) -> String {
    match pair.conversation_name().trim() {
        "" => format!("(unnamed, session {})", pair.session_id()),
        name => one_line(name),
    }
}

fn verbs(pair: &TouchedPair) -> String {
    pair.verbs()
        .map(TouchVerb::as_str)
        .collect::<Vec<_>>()
        .join("+")
}

/// A conversation names itself, and a name comes out of somebody's file. It never becomes
/// more than one line of output — the rule `search` follows for a fragment (ADR 0038).
fn one_line(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    let mut spaced = true;
    for character in text.chars() {
        if character.is_whitespace() || character.is_control() {
            if !spaced {
                line.push(' ');
                spaced = true;
            }
            continue;
        }
        line.push(character);
        spaced = false;
    }
    line.trim_end().to_string()
}

#[derive(Debug)]
struct Arguments {
    needle: String,
    direction: Direction,
    include_in_passing: bool,
}

impl Arguments {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut words: Vec<&str> = Vec::new();
        let mut conversation: Option<String> = None;
        let mut include_in_passing = false;
        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            match argument.as_str() {
                "--in" => {
                    conversation = Some(
                        remaining
                            .next()
                            .ok_or_else(|| format!("--in requires a value. {USAGE}"))?
                            .clone(),
                    );
                }
                "--include-in-passing" => include_in_passing = true,
                other if other.starts_with("--") => {
                    return Err(format!("unknown argument {other:?}. {USAGE}"));
                }
                other => words.push(other),
            }
        }
        match (conversation, words.is_empty()) {
            // Both sides at once is two questions, and answering the first of them silently
            // is how a person is told something they did not ask.
            (Some(_), false) => Err(format!(
                "a file and a conversation cannot both be asked about in one run. {USAGE}"
            )),
            (Some(conversation), true) => Ok(Self {
                needle: conversation,
                direction: Direction::WhichFiles,
                include_in_passing,
            }),
            (None, false) => Ok(Self {
                needle: words.join(" "),
                direction: Direction::WhichConversations,
                include_in_passing,
            }),
            (None, true) => Err(help::misuse(USAGE)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Arguments, String> {
        Arguments::parse(
            &arguments
                .iter()
                .map(|argument| (*argument).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn a_path_asks_which_conversations_touched_it() {
        let asked = parse(&["draft_service.py"]).unwrap();
        assert_eq!(asked.needle, "draft_service.py");
        assert_eq!(asked.direction, Direction::WhichConversations);
        assert!(!asked.include_in_passing);
    }

    #[test]
    fn a_conversation_asks_which_files_it_touched() {
        let asked = parse(&["--in", "Беседа Б", "--include-in-passing"]).unwrap();
        assert_eq!(asked.needle, "Беседа Б");
        assert_eq!(asked.direction, Direction::WhichFiles);
        assert!(asked.include_in_passing);
    }

    #[test]
    fn asking_both_sides_at_once_is_refused() {
        let error = parse(&["--in", "беседа", "draft_service.py"]).unwrap_err();
        assert!(error.contains("cannot both be asked"), "{error}");
    }

    #[test]
    fn a_run_with_nothing_to_look_for_is_a_usage_mistake() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["--include-in-passing"]).is_err());
        assert!(parse(&["--in"]).is_err());
    }

    #[test]
    fn an_option_this_command_does_not_have_is_refused() {
        let error = parse(&["a.rs", "--strict"]).unwrap_err();
        assert!(error.contains("--strict"), "{error}");
    }

    /// A name comes out of somebody's transcript, so it can hold anything a file can.
    #[test]
    fn a_name_is_folded_into_one_line() {
        assert_eq!(one_line("две\nстроки\tи\r\nтабы  "), "две строки и табы");
        assert_eq!(one_line("a\u{1b}[31mb"), "a [31mb");
    }
}
