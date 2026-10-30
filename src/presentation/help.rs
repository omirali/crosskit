// What this program can be asked to do, in one list.
//
// The list is the only place that knows which commands exist, and it does not hold their
// usage text — each command keeps that beside the parser that enforces it, and this points
// at it. So a grammar and the help for it cannot drift: there is one literal per command,
// and changing it changes both.
//
// The same list is what the consistency test walks: every name here has to be a name the
// dispatcher actually takes, and a name the dispatcher does not take must not be here.
use super::{
    ask, contexts, graph, index, knowledge, migrate, policy, search, service, setup, status, tasks,
    touches,
};

/// The option every command that works inside a context accepts. Named here because the
/// dispatcher takes it out before a command sees it, so no command's own usage owns it.
const CONTEXT_OPTION: &str = "--context <name>   the context this run works in; needed only\n                     when more than one is configured";

/// One command: the word a person types, one line on what it is for, and the grammar it
/// enforces.
pub(super) struct CommandHelp {
    pub(super) name: &'static str,
    pub(super) summary: &'static str,
    pub(super) usage: &'static str,
}

/// In the order a person meets them: set the machine up, see where it stands, then the
/// commands that do the work, then the ones that explain what happened.
pub(super) const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        name: "setup",
        summary: "configure and index a machine for the first time, in one command",
        usage: setup::USAGE,
    },
    CommandHelp {
        name: "status",
        summary: "what every context holds, written nowhere and changed nowhere",
        usage: status::USAGE,
    },
    CommandHelp {
        name: "search",
        summary: "find text in one context; --json answers a program with one object",
        usage: search::USAGE,
    },
    CommandHelp {
        name: "ask",
        summary: "ask a question about one context, answered from its own files",
        usage: ask::USAGE,
    },
    CommandHelp {
        name: "tasks",
        summary: "which conversations belong to one task, by what their logs record; --json for a program",
        usage: tasks::USAGE,
    },
    CommandHelp {
        name: "touches",
        summary: "which conversations worked on a file, and what one worked on",
        usage: touches::USAGE,
    },
    CommandHelp {
        name: "graph",
        summary: "the neighbourhood of a conversation or a file: replies, files, projects; --json for a program",
        usage: graph::USAGE,
    },
    CommandHelp {
        name: "knowledge",
        summary: "entities and the facts between them, each with its quote and source; map --json for a program",
        usage: knowledge::USAGE,
    },
    CommandHelp {
        name: "index",
        summary: "index a directory, keep it in step, and repair what a stopped pass left",
        usage: index::USAGE,
    },
    CommandHelp {
        name: "service",
        summary: "keep every context in step in the background, from login on: install, status, logs",
        usage: service::USAGE,
    },
    CommandHelp {
        name: "migrate",
        summary: "bring every context's graph to the schema this build reads",
        usage: migrate::USAGE,
    },
    CommandHelp {
        name: "contexts",
        summary: "propose what the contexts of a directory could be, writing nothing; --json lists the configured ones for a program",
        usage: contexts::USAGE,
    },
    CommandHelp {
        name: "policy",
        summary: "say what the index policy decides about one path, and why",
        usage: policy::USAGE,
    },
    CommandHelp {
        name: "doctor",
        summary: "the storage of one context: schema, engine, search index",
        usage: DOCTOR_USAGE,
    },
];

/// `doctor` takes no arguments at all, so it has no parser to keep a usage line beside.
pub(super) const DOCTOR_USAGE: &str = "Usage: crosskit [--context <name>] doctor";

/// What a command says when what it was given is not its grammar at all.
///
/// The usage line on its own would reach a person as `crosskit failed: Usage: …` — a
/// sentence that starts over in the middle of itself. This continues the sentence, and the
/// grammar follows it. Every command that refuses a shape rather than a value says it this
/// way, so a person who mistypes two different commands is answered in one voice.
pub(super) fn misuse(usage: &str) -> String {
    format!("this is not how the command is used. {usage}")
}

pub(super) fn command_named(name: &str) -> Option<&'static CommandHelp> {
    COMMANDS.iter().find(|command| command.name == name)
}

/// What the whole program does, and where to start.
pub(super) fn print_overview() {
    println!(
        "crosskit {} — a local index of what is on this machine",
        version()
    );
    println!();
    println!("Usage: crosskit [--context <name>] <command> [arguments]");
    println!();
    println!("Commands:");
    let width = COMMANDS
        .iter()
        .map(|command| command.name.len())
        .max()
        .unwrap_or_default();
    for command in COMMANDS {
        println!("  {:width$}  {}", command.name, command.summary);
    }
    println!();
    println!("Options:");
    println!("  {CONTEXT_OPTION}");
    println!("  --help             this text, or the grammar of one command");
    println!("  --version          which crosskit this is");
    println!();
    println!("First run: `crosskit setup <directory>` proposes the contexts for a directory,");
    println!("shows the configuration it would write, asks, and indexes what you accept.");
    println!("`crosskit <command> --help` shows what one command takes.");
}

/// One command's grammar, with the line that says what it is for above it.
pub(super) fn print_command(command: &CommandHelp) {
    println!("crosskit {} — {}", command.name, command.summary);
    println!();
    println!("{}", command.usage);
}

/// The refusal for a word that is not a command. It names what there is instead, because a
/// person who mistyped a command has no way to guess the list.
pub(super) fn unknown_command(name: &str) -> String {
    format!(
        "there is no command {name:?}. The commands are: {}. `crosskit --help` describes each of them.",
        COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The version of this build, from the package metadata rather than from a constant
/// somebody has to remember to change.
pub(super) fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, command_named, unknown_command, version};

    /// Every entry points at the usage of the command it names, and no two entries name the
    /// same command.
    #[test]
    fn the_list_names_each_command_once_and_carries_its_own_usage() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
        let listed = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), listed, "a command is listed twice: {names:?}");

        for command in COMMANDS {
            assert!(
                command.usage.starts_with("Usage: crosskit"),
                "{}'s usage does not read as one: {:?}",
                command.name,
                command.usage
            );
            assert!(
                command.usage.contains(command.name),
                "{}'s usage does not spell the command: {:?}",
                command.name,
                command.usage
            );
            assert!(
                !command.summary.is_empty() && !command.summary.ends_with('.'),
                "{}'s summary is a line, not a sentence: {:?}",
                command.name,
                command.summary
            );
        }
    }

    #[test]
    fn a_command_is_found_by_name_and_nothing_else_is() {
        assert!(command_named("status").is_some());
        assert!(command_named("Status").is_none());
        assert!(command_named("stat").is_none());
        assert!(command_named("").is_none());
    }

    /// The refusal names the alternatives, because a mistyped command gives a person no way
    /// to guess what the right one was.
    #[test]
    fn an_unknown_command_is_refused_with_the_list_of_real_ones() {
        let refused = unknown_command("statuss");
        assert!(refused.contains("\"statuss\""), "{refused}");
        for command in COMMANDS {
            assert!(refused.contains(command.name), "{refused}");
        }
    }

    /// From the package metadata: a constant in the code is a second place to remember.
    #[test]
    fn the_version_is_the_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }
}
