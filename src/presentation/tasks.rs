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

use crate::application::{
    Board, BoardTask, KeptProgress, TaskBoard, TaskBoardError, TaskDecision, TaskMember, Tie,
};
use crate::domain::risk_decision::RiskVerdict;
use crate::domain::task_stitching::session::Platform;
use crate::domain::task_stitching::stitching::LinkSignal;
use crate::domain::{CivilDate, TaskBookError, WorkspaceName};
use crate::infrastructure::analysis::MAP_QUESTION_VERSION;
use crate::infrastructure::{TaskBookFile, TaskMapFiles, home_directory, unix_now_ms};

use super::machine_answer::{self, FailureCategory, SCHEMA, object_line, refusal_line};
use super::reading_ground::{OwnRun, ReadingGround};
use super::refusal::Refusal;
use super::risk_decision_lines::RiskDecisionLine;
use super::service_client;
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] tasks [--json]
       crosskit [--context <name>] tasks confirm <conversation> --name <name> [--json]
       crosskit [--context <name>] tasks reject <conversation> [--json]
       crosskit [--context <name>] tasks merge <task> <conversation> [--json]
       crosskit [--context <name>] tasks add <task> <conversation> [--json]
       crosskit [--context <name>] tasks drop <task> <conversation> [--json]
       crosskit [--context <name>] tasks rename <task> <name> [--json]
       crosskit [--context <name>] tasks accept|resolve <task> <conversation> <reply> --reason <why> [--json]
       crosskit [--context <name>] tasks reopen <task> <conversation> <reply> [--json]
       crosskit [--context <name>] tasks finish|resume <task> [--json]
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
    let maps = TaskMapFiles::of_context(&chosen).map_err(|failure| Refusal::failed(failure.0))?;
    let standing = KeptProgress::new(&maps, MAP_QUESTION_VERSION);
    let board = TaskBoard::new(&repository, &book, &home).with_progress(&standing);
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
        [
            verb @ ("accept" | "resolve"),
            task,
            session,
            reply,
            "--reason",
            reason,
        ] => TaskDecision::Risk {
            task: owned(task),
            session: owned(session),
            reply: reply_number(reply)?,
            verdict: Some(if *verb == "accept" {
                RiskVerdict::Accepted
            } else {
                RiskVerdict::Resolved
            }),
            reason: owned(reason),
            at_unix_ms: unix_now_ms(),
        },
        ["reopen", task, session, reply] => TaskDecision::Risk {
            task: owned(task),
            session: owned(session),
            reply: reply_number(reply)?,
            verdict: None,
            reason: String::new(),
            at_unix_ms: unix_now_ms(),
        },
        ["finish", task] => TaskDecision::Finish {
            task: owned(task),
            at_unix_ms: unix_now_ms(),
        },
        ["resume", task] => TaskDecision::Resume { task: owned(task) },
        _ => return Err(format!("Not a tasks command: {}", words.join(" "))),
    };
    Ok(Some(decision))
}

/// The reply a risk's quote is in, as `tasks map` prints it in brackets.
fn reply_number(word: &str) -> Result<usize, String> {
    word.trim_matches(['[', ']'])
        .parse()
        .map_err(|_| format!("{word} is not a reply number"))
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
        TaskBoardError::Progress(failure) => {
            return Unanswered::Refused(Refusal::failed(format!(
                "Where the task stands could not be read from its maps: {}",
                failure.0
            )));
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
            TaskBookError::EmptyReason => Declined::EmptyReason,
            TaskBookError::NoRiskDecision { task, anchor } => Declined::NoRiskDecision {
                task,
                session: anchor.session,
                reply: anchor.reply,
            },
            TaskBookError::OpenCriticalRisks { task, count } => {
                Declined::OpenCriticalRisks { task, count }
            }
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
    EmptyReason,
    NoRiskDecision {
        task: String,
        session: String,
        reply: usize,
    },
    OpenCriticalRisks {
        task: String,
        count: usize,
    },
}

impl Declined {
    fn category(&self) -> FailureCategory {
        match self {
            Self::UnknownConversation { .. }
            | Self::NoSuchTask { .. }
            | Self::NoRiskDecision { .. } => FailureCategory::NotFound,
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
            Self::EmptyReason => "A decision on a risk says why: --reason \"<why>\".".to_string(),
            Self::NoRiskDecision {
                task,
                session,
                reply,
            } => format!(
                "Nothing was decided on the risk at reply {reply} of {session} in «{task}», so \
                 there is nothing to reopen."
            ),
            Self::OpenCriticalRisks { task, count } => format!(
                "«{task}» still has {count} open critical risk(s). Accept or resolve them with a \
                 reason first: crosskit tasks accept|resolve \"{task}\" <conversation> <reply> \
                 --reason \"<why>\"."
            ),
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
    /// Where the task stands by its maps (ADR 0061).
    #[serde(default)]
    progress: Option<Progress>,
    /// When the owner marked it done.
    #[serde(default)]
    finished_at_unix_ms: Option<i64>,
    /// The owner's word on its risks, by the place each risk's quote stands.
    #[serde(default)]
    risk_decisions: Vec<RiskDecisionLine>,
}

#[derive(Serialize, Deserialize)]
struct Progress {
    conversations: usize,
    mapped: usize,
    steps: usize,
    /// A branch state: `open` means the task waits for the owner.
    head: Option<String>,
    open_risks: usize,
    open_critical: usize,
    decided_risks: usize,
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
        confirmed: board.tasks().iter().map(confirmed_of).collect(),
        tasks: board
            .candidates()
            .iter()
            .map(|candidate| Candidate {
                conversations: conversations_of(candidate.members()),
            })
            .collect(),
    })
}

fn confirmed_of(task: &BoardTask) -> Confirmed {
    Confirmed {
        id: task.id().to_string(),
        name: task.name().to_string(),
        conversations: conversations_of(task.members()),
        proposed: conversations_of(task.proposed()),
        progress: task.progress().map(|progress| Progress {
            conversations: progress.conversations,
            mapped: progress.mapped,
            steps: progress.steps,
            head: progress.head.map(|state| state.code().to_string()),
            open_risks: progress.open_risks,
            open_critical: progress.open_critical,
            decided_risks: progress.decided_risks,
        }),
        finished_at_unix_ms: task.confirmed().finished_at_unix_ms(),
        risk_decisions: task
            .confirmed()
            .risk_decisions()
            .iter()
            .map(RiskDecisionLine::from)
            .collect(),
    }
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
    let done = if task.finished_at_unix_ms.is_some() {
        " — done"
    } else {
        ""
    };
    println!(
        "{} — {} conversation(s), {}{done}",
        task.name,
        task.conversations.len(),
        span_of(&task.conversations)
    );
    if let Some(progress) = &task.progress {
        println!("  {}", progress_line(progress));
    }
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

/// Where a task stands, in one line.
fn progress_line(progress: &Progress) -> String {
    if progress.mapped == 0 {
        return format!(
            "Not mapped: crosskit tasks map <task> --build maps its {} conversation(s).",
            progress.conversations
        );
    }
    let head = match progress.head.as_deref() {
        Some("open") => ", waits for your decision",
        _ => "",
    };
    format!(
        "Mapped {} of {} · main line {} step(s){head} · open risks {} ({} critical) · decided {}",
        progress.mapped,
        progress.conversations,
        progress.steps,
        progress.open_risks,
        progress.open_critical,
        progress.decided_risks
    )
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
