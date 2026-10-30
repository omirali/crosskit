// `crosskit [--context <name>] tasks [--json]`, and the decisions on tasks:
// `tasks confirm <conversation> --name <name>`, `tasks reject <conversation>`,
// `tasks merge <task> <conversation>`, `tasks add <task> <conversation>`,
// `tasks drop <task> <conversation>`, `tasks rename <task> <name>`.
//
// The tasks of one context: the ones the owner confirmed, with what stitching now proposes for
// each, and the candidates no decision has touched (ADR 0052, ADR 0053). A conversation is named
// by its session or the first characters of it, as the listing prints them; a task by its name or
// id. A decision writes only the task book beside the graph and prints the board after it.
//
// Every form is asked of the background service first, because the service holds the graph for
// as long as it runs and nothing else can open it then. The text is printed from the service's
// machine answer, so there is one board and two ways to print it. A decision the service
// declines comes back as what was declined — a kind and its data, never a sentence — and is
// worded here, the same way a run of its own would word it.
use serde::{Deserialize, Serialize};

use crate::application::{Board, TaskBoard, TaskBoardError, TaskDecision, TaskMember, Tie};
use crate::domain::task_stitching::session::Platform;
use crate::domain::task_stitching::stitching::LinkSignal;
use crate::domain::{CivilDate, TaskBookError, WorkspaceName};
use crate::infrastructure::{TaskBookFile, home_directory};

use super::machine_answer::{self, FailureCategory, SCHEMA, object_line, refusal_line};
use super::reading_ground::{OwnRun, ReadingGround};
use super::refusal::Refusal;
use super::service_client;
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] tasks [--json]
       crosskit [--context <name>] tasks confirm <conversation> --name <name> [--json]
       crosskit [--context <name>] tasks reject <conversation> [--json]
       crosskit [--context <name>] tasks merge <task> <conversation> [--json]
       crosskit [--context <name>] tasks add <task> <conversation> [--json]
       crosskit [--context <name>] tasks drop <task> <conversation> [--json]
       crosskit [--context <name>] tasks rename <task> <name> [--json]
       crosskit [--context <name>] tasks map <task> [--build | --join] [--only <conversation>] [--detail] [--json]";

/// How much of an unnamed conversation's opening stands for its name.
const OPENING_CHARACTERS: usize = 80;
/// How much of a session the listing prints: enough to name it back.
const SHOWN_SESSION_CHARACTERS: usize = 8;
const DAY_MS: i64 = 86_400_000;

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    if arguments.first().is_some_and(|word| word == "map") {
        return super::task_map::run(&arguments[1..], context);
    }
    let machine = arguments.iter().any(|one| one == machine_answer::FLAG);
    let mut asking: Vec<String> = arguments.to_vec();
    if !machine {
        asking.push(machine_answer::FLAG.to_string());
    }
    if let Some(line) = service_client::asked("tasks", &asking, context) {
        if let Ok(declined) = serde_json::from_str::<DeclinedLine>(&line) {
            if machine {
                println!("{line}");
            }
            return Err(declined.declined.sentence());
        }
        if let Ok(answer) = serde_json::from_str::<Answer>(&line) {
            if machine {
                println!("{line}");
            } else {
                print_answer(&answer);
            }
            return Ok(());
        }
    }
    let (line, outcome) = machine_line(arguments, context, &OwnRun::load());
    match (machine, outcome) {
        (true, outcome) => {
            println!("{line}");
            outcome.map_err(String::from)
        }
        (false, Ok(())) => {
            let answer: Answer = serde_json::from_str(&line)
                .map_err(|error| format!("The board could not be read back: {error}"))?;
            print_answer(&answer);
            Ok(())
        }
        (false, Err(refusal)) => Err(String::from(refusal)),
    }
}

/// The board, or a decision and the board after it, as one line, on whatever ground the graph
/// is read from — the service's or a run of its own.
pub(super) fn machine_line(
    arguments: &[String],
    context: Option<&str>,
    ground: &dyn ReadingGround,
) -> (String, Result<(), Refusal>) {
    let mut selected: Option<WorkspaceName> = None;
    match answer(arguments, context, &mut selected, ground) {
        Ok((selected, board)) => (board_line(&selected, &board), Ok(())),
        Err(Unanswered::Declined(declined)) => {
            let refusal = Refusal::new(declined.category(), declined.sentence());
            let line = object_line(&DeclinedLine {
                schema: SCHEMA,
                declined,
            });
            (line, Err(refusal))
        }
        Err(Unanswered::Refused(refusal)) => {
            (refusal_line(&refusal, selected.as_ref()), Err(refusal))
        }
    }
}

/// Why there is no board: a decision the book declined, or anything else.
enum Unanswered {
    Declined(Declined),
    Refused(Refusal),
}

impl From<Refusal> for Unanswered {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

fn answer(
    arguments: &[String],
    context: Option<&str>,
    selected: &mut Option<WorkspaceName>,
    ground: &dyn ReadingGround,
) -> Result<(WorkspaceName, Board), Unanswered> {
    let decision =
        parse(arguments).map_err(|sentence| Refusal::usage(format!("{sentence}\n{USAGE}")))?;
    let home = home_directory()
        .map_err(|error| Refusal::failed(format!("The home directory is not known: {error}")))?;
    let home = home.to_string_lossy();
    let loaded = ground.policy()?;
    let chosen = workspace::choose(loaded, context)?;
    *selected = Some(chosen.clone());
    let manager = ground.graph(&chosen)?;
    let repository = manager.repository();
    let book =
        TaskBookFile::of_context(&chosen).map_err(|error| Refusal::failed(error.to_string()))?;
    let board = TaskBoard::new(&repository, &book, &home);
    let shown = match &decision {
        Some(decision) => board.decide(decision),
        None => board.show(),
    };
    shown
        .map(|board| (chosen.clone(), board))
        .map_err(unanswered_of)
}

/// The decision the arguments ask for, or nothing for the listing.
fn parse(arguments: &[String]) -> Result<Option<TaskDecision>, String> {
    let words: Vec<&str> = arguments
        .iter()
        .map(String::as_str)
        .filter(|one| *one != machine_answer::FLAG)
        .collect();
    let owned = |word: &&str| (*word).to_string();
    let decision = match words.as_slice() {
        [] => return Ok(None),
        ["confirm", session, "--name", name] | ["confirm", "--name", name, session] => {
            TaskDecision::Confirm {
                session: owned(session),
                name: owned(name),
            }
        }
        ["reject", session] => TaskDecision::Reject {
            session: owned(session),
        },
        ["merge", task, session] => TaskDecision::Merge {
            task: owned(task),
            session: owned(session),
        },
        ["add", task, session] => TaskDecision::Add {
            task: owned(task),
            session: owned(session),
        },
        ["drop", task, session] => TaskDecision::Drop {
            task: owned(task),
            session: owned(session),
        },
        ["rename", task, name] => TaskDecision::Rename {
            task: owned(task),
            name: owned(name),
        },
        _ => return Err(format!("Not a tasks command: {}", words.join(" "))),
    };
    Ok(Some(decision))
}

fn unanswered_of(error: TaskBoardError) -> Unanswered {
    let declined = match error {
        TaskBoardError::Graph(graph) => {
            let sentence = format!("The conversations could not be read: {graph}");
            return Unanswered::Refused(Refusal::of_graph(&graph, sentence));
        }
        TaskBoardError::Store(store) => {
            return Unanswered::Refused(Refusal::failed(store.to_string()));
        }
        TaskBoardError::UnknownConversation(reference) => {
            Declined::UnknownConversation { reference }
        }
        TaskBoardError::AmbiguousConversation(reference, sessions) => {
            Declined::AmbiguousConversation {
                reference,
                sessions,
            }
        }
        TaskBoardError::Decision(error) => match error {
            TaskBookError::EmptyName => Declined::EmptyName,
            TaskBookError::NameTaken(name) => Declined::NameTaken { name },
            TaskBookError::NoSuchTask(task) => Declined::NoSuchTask { task },
            TaskBookError::InAnotherTask { session, task } => {
                Declined::InAnotherTask { session, task }
            }
            TaskBookError::NotInTask { session, task } => Declined::NotInTask { session, task },
            TaskBookError::NoConversations => Declined::NoConversations,
        },
    };
    Unanswered::Declined(declined)
}

/// A decision the task book declined, as a program reads it: what kind, and about what.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Declined {
    UnknownConversation {
        reference: String,
    },
    AmbiguousConversation {
        reference: String,
        sessions: Vec<String>,
    },
    EmptyName,
    NameTaken {
        name: String,
    },
    NoSuchTask {
        task: String,
    },
    InAnotherTask {
        session: String,
        task: String,
    },
    NotInTask {
        session: String,
        task: String,
    },
    NoConversations,
}

impl Declined {
    fn category(&self) -> FailureCategory {
        match self {
            Self::UnknownConversation { .. } | Self::NoSuchTask { .. } => FailureCategory::NotFound,
            _ => FailureCategory::Usage,
        }
    }

    /// The sentence for a person, the one wording whether the service or this run declined.
    fn sentence(&self) -> String {
        match self {
            Self::UnknownConversation { reference } => format!(
                "No conversation of this context is {reference}. Name one by its session, or by \
                 the first characters `crosskit tasks` prints."
            ),
            Self::AmbiguousConversation {
                reference,
                sessions,
            } => format!(
                "{reference} could be any of {}; give more of the session.",
                sessions.join(", ")
            ),
            Self::EmptyName => "A task needs a name.".to_string(),
            Self::NameTaken { name } => format!("A task is already called «{name}»."),
            Self::NoSuchTask { task } => format!("No task is called «{task}»."),
            Self::InAnotherTask { session, task } => {
                format!("Conversation {session} already belongs to «{task}»; drop it there first.")
            }
            Self::NotInTask { session, task } => {
                format!("Conversation {session} is not in «{task}».")
            }
            Self::NoConversations => "A task needs at least one conversation.".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct DeclinedLine {
    schema: u32,
    declined: Declined,
}

/// The machine answer: the board.
#[derive(Serialize, Deserialize)]
struct Answer {
    schema: u32,
    context: String,
    /// The tasks the owner confirmed.
    confirmed: Vec<Confirmed>,
    /// The candidates no decision has touched.
    tasks: Vec<Candidate>,
}

#[derive(Serialize, Deserialize)]
struct Confirmed {
    id: String,
    name: String,
    conversations: Vec<Conversation>,
    /// What stitching ties to the task now and the owner has not decided on.
    proposed: Vec<Conversation>,
}

#[derive(Serialize, Deserialize)]
struct Candidate {
    conversations: Vec<Conversation>,
}

#[derive(Serialize, Deserialize)]
struct Conversation {
    session: String,
    /// `claude-code` or `codex`: the tool the conversation was held in.
    platform: String,
    name: Option<String>,
    /// The owner's first words, for a conversation with no name.
    opening: Option<String>,
    first_at_unix_ms: Option<i64>,
    last_at_unix_ms: Option<i64>,
    /// `strong` or `neighbour`.
    tie: String,
    signals: Vec<String>,
}

fn conversations_of(members: &[TaskMember]) -> Vec<Conversation> {
    members
        .iter()
        .map(|member| Conversation {
            session: member.session_id().to_string(),
            platform: match member.platform() {
                Platform::ClaudeCode => "claude-code",
                Platform::Codex => "codex",
            }
            .to_string(),
            name: member.name().map(str::to_string),
            opening: member.opening().map(str::to_string),
            first_at_unix_ms: member.first_at_unix_ms(),
            last_at_unix_ms: member.last_at_unix_ms(),
            tie: match member.tie() {
                Tie::Strong => "strong",
                Tie::Neighbour => "neighbour",
            }
            .to_string(),
            signals: member
                .signals()
                .iter()
                .map(|signal| signal_code(*signal).to_string())
                .collect(),
        })
        .collect()
}

fn board_line(context: &WorkspaceName, board: &Board) -> String {
    object_line(&Answer {
        schema: SCHEMA,
        context: context.as_str().to_string(),
        confirmed: board
            .tasks()
            .iter()
            .map(|task| Confirmed {
                id: task.id().to_string(),
                name: task.name().to_string(),
                conversations: conversations_of(task.members()),
                proposed: conversations_of(task.proposed()),
            })
            .collect(),
        tasks: board
            .candidates()
            .iter()
            .map(|candidate| Candidate {
                conversations: conversations_of(candidate.members()),
            })
            .collect(),
    })
}

/// The signal as both answers spell it. A contract with every reader of `--json`, written out
/// here and not derived from the domain's names.
fn signal_code(signal: LinkSignal) -> &'static str {
    match signal {
        LinkSignal::RelayFile => "relay-file",
        LinkSignal::NamedConversation => "named-conversation",
        LinkSignal::SameTrackerTask => "same-tracker-task",
        LinkSignal::SameBranch => "same-branch",
        LinkSignal::SharedWrites => "shared-writes",
        LinkSignal::Neighbours => "neighbours",
    }
}

fn print_answer(answer: &Answer) {
    println!("Context: {}", answer.context);
    if answer.confirmed.is_empty() && answer.tasks.is_empty() {
        println!("No tasks kept, and no conversations tie into one by what their logs record.");
        return;
    }
    if !answer.confirmed.is_empty() {
        println!();
        println!("Tasks ({})", answer.confirmed.len());
        for task in &answer.confirmed {
            print_task(task);
        }
    }
    if !answer.tasks.is_empty() {
        println!();
        println!(
            "Candidates ({}), by what the conversations' logs record",
            answer.tasks.len()
        );
        for (number, candidate) in answer.tasks.iter().enumerate() {
            println!();
            println!(
                "{} — {} conversation(s), {}",
                number + 1,
                candidate.conversations.len(),
                span_of(&candidate.conversations)
            );
            for conversation in &candidate.conversations {
                println!("  {}", conversation_line(conversation, true));
            }
        }
        println!();
        println!("Keep one: crosskit tasks confirm <conversation> --name \"<name>\"");
        println!("Not a task: crosskit tasks reject <conversation>");
    }
}

fn print_task(task: &Confirmed) {
    println!();
    println!(
        "{} — {} conversation(s), {}",
        task.name,
        task.conversations.len(),
        span_of(&task.conversations)
    );
    for conversation in &task.conversations {
        println!("  {}", conversation_line(conversation, false));
    }
    if !task.proposed.is_empty() {
        println!(
            "  Proposed ({}): crosskit tasks add \"{}\" <conversation>, or drop it",
            task.proposed.len(),
            task.name
        );
        for conversation in &task.proposed {
            println!("  + {}", conversation_line(conversation, true));
        }
    }
}

fn conversation_line(conversation: &Conversation, with_signals: bool) -> String {
    let session: String = conversation
        .session
        .chars()
        .take(SHOWN_SESSION_CHARACTERS)
        .collect();
    let mut line = format!(
        "{session}  {}  {}",
        day_of(conversation.first_at_unix_ms),
        title_of(conversation)
    );
    if conversation.tie == "neighbour" {
        line.push_str(" (neighbour only)");
    }
    if with_signals && !conversation.signals.is_empty() {
        line.push_str(&format!("  [{}]", conversation.signals.join(", ")));
    }
    line
}

fn span_of(conversations: &[Conversation]) -> String {
    let first = conversations
        .iter()
        .filter_map(|c| c.first_at_unix_ms)
        .min();
    let last = conversations.iter().filter_map(|c| c.last_at_unix_ms).max();
    format!("{} … {}", day_of(first), day_of(last))
}

fn title_of(conversation: &Conversation) -> String {
    match (&conversation.name, &conversation.opening) {
        (Some(name), _) => name.clone(),
        (None, Some(opening)) => {
            let cut: String = opening
                .chars()
                .take(OPENING_CHARACTERS)
                .map(|c| if c == '\n' { ' ' } else { c })
                .collect();
            format!("«{cut}»")
        }
        (None, None) => conversation.session.clone(),
    }
}

fn day_of(unix_ms: Option<i64>) -> String {
    unix_ms.map_or_else(
        || "?".to_string(),
        |ms| CivilDate::from_days(ms.div_euclid(DAY_MS)).to_string(),
    )
}
