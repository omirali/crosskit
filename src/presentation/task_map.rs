// `crosskit [--context <name>] tasks map <task> [--build] [--json]`.
//
// The decision map of a confirmed task (ADR 0055): for every conversation of it, the forks where
// an approach was chosen, the branches with what became of them, and the risks left without an
// answer, each node proved by a quote from the reply it names. Without `--build` it shows what
// was kept and asks nothing of a model. With it, it asks about the replies no map covers yet —
// which takes the context's permission to send content out, and costs the subscription's limit.
//
// The replies are read from the graph, which the background service holds while it runs; so
// they are asked of the service first (`task-speech`), and read here only when it is not there.
// The model is asked from here, never from the service: a build takes minutes.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::application::{
    ConversationSpeech, MapMerger, MapProgress, MappedConversation, MergeInput, MergedProposal,
    ShownSynthesis, SpokenReply, SynthesisError, TaskBookStore, TaskMapError, TaskMapping,
    TaskSynthesizing, speech_of,
};
use crate::domain::conversation::Speaker;
use crate::domain::decision_map::{BranchState, RiskLevel, RiskStatus};
use crate::domain::task_synthesis::{NodeRef, TaskRisk};
use crate::domain::{CivilDate, WorkspaceName};
use crate::infrastructure::analysis::map_engine;
use crate::infrastructure::{TaskBookFile, TaskMapFiles};

use super::machine_answer::{self, SCHEMA, object_line, refusal_line};
use super::reading_ground::{OwnRun, ReadingGround};
use super::refusal::Refusal;
use super::service_client;
use super::workspace;

const BUILD_FLAG: &str = "--build";
/// Narrows the map to one conversation of the task, named by its session or the start of it.
const ONLY_FLAG: &str = "--only";
/// Shows every conversation's own map below the task's.
const DETAIL_FLAG: &str = "--detail";
/// Joins the conversations' maps already built into the task's, asking nothing about any
/// conversation's replies.
const JOIN_FLAG: &str = "--join";
const DAY_MS: i64 = 86_400_000;
/// How much of a conversation's name a source of the task's map shows.
const SHORT_LABEL_CHARACTERS: usize = 28;
/// How much of a question, reason or description one line of the text map shows.
const LINE_CHARACTERS: usize = 140;

/// What `tasks map` was asked: the task, at most one conversation of it, and how.
struct MapWords {
    task: String,
    only: Option<String>,
    machine: bool,
    build: bool,
    detail: bool,
    join: bool,
}

fn map_words(arguments: &[String]) -> Result<MapWords, String> {
    let flags = [machine_answer::FLAG, BUILD_FLAG, DETAIL_FLAG, JOIN_FLAG];
    let has = |flag: &str| arguments.iter().any(|one| one == flag);
    let mut words: Vec<&String> = arguments
        .iter()
        .filter(|one| !flags.contains(&one.as_str()))
        .collect();
    let only = match words.iter().position(|word| *word == ONLY_FLAG) {
        Some(at) if at + 1 < words.len() => {
            let session = words[at + 1].clone();
            words.drain(at..=at + 1);
            Some(session)
        }
        Some(_) => return Err(format!("{ONLY_FLAG} names one conversation")),
        None => None,
    };
    let [task] = words.as_slice() else {
        return Err(format!(
            "Name one task: crosskit tasks map <task> [{BUILD_FLAG}] [{ONLY_FLAG} <conversation>] [--json]"
        ));
    };
    Ok(MapWords {
        task: (*task).clone(),
        only,
        machine: has(machine_answer::FLAG),
        build: has(BUILD_FLAG),
        detail: has(DETAIL_FLAG),
        join: has(JOIN_FLAG),
    })
}

/// Keeps the one conversation `only` names, or says why it names not exactly one.
fn narrowed(speech: &mut TaskSpeech, only: Option<&str>) -> Result<(), String> {
    let Some(only) = only else {
        return Ok(());
    };
    speech
        .conversations
        .retain(|conversation| conversation.session_id.starts_with(only));
    if speech.conversations.len() != 1 {
        return Err(format!(
            "{only} names {} conversation(s) of «{}»; name exactly one.",
            speech.conversations.len(),
            speech.task
        ));
    }
    Ok(())
}

/// The maps as kept and, for the whole task, its join: what showing reads, asking nothing.
fn shown(
    speech: &TaskSpeech,
    store: &TaskMapFiles,
    whole_task: bool,
) -> Result<(Vec<MappedConversation>, Option<ShownSynthesis>), String> {
    // The question version only decides which maps count; the engine that would ask it is not
    // made.
    let engine = ShowOnly;
    let mapped = TaskMapping::new(&engine, store)
        .show(&speech.conversations)
        .map_err(sentence_of)?;
    let joined = if whole_task {
        TaskSynthesizing::new(&engine, store)
            .show(&speech.task_id, &mapped)
            .map_err(join_sentence_of)?
    } else {
        None
    };
    Ok((mapped, joined))
}

/// Runs `tasks map …`; `arguments` start after `map`.
pub(super) fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    let words = map_words(arguments)?;
    let mut speech = speech_of_task(&words.task, context)?;
    narrowed(&mut speech, words.only.as_deref())?;
    let ground = OwnRun::load();
    let loaded = ground.policy().map_err(String::from)?;
    let chosen = workspace::choose(loaded, context).map_err(String::from)?;
    let store = TaskMapFiles::of_context(&chosen).map_err(|failure| failure.0)?;
    // The task's join is made of every conversation's map, so a view of one conversation
    // neither makes nor shows it.
    let whole_task = words.only.is_none();
    let (build, join) = (words.build, words.join);
    if join && (build || !whole_task) {
        return Err(format!(
            "{JOIN_FLAG} joins the maps of the whole task: it takes neither {BUILD_FLAG} nor {ONLY_FLAG}"
        ));
    }
    let (mapped, joined) = if build || join {
        let permit = loaded
            .workspaces()
            .content_egress(&chosen)
            .permit()
            .ok_or_else(|| {
                format!(
                    "Context {chosen} does not let its content leave this machine, and a map is \
                     built by a model. Add `send_content_outside = true` to `[workspace.{chosen}]` \
                     if that is what you want."
                )
            })?;
        let engine = map_engine(loaded.engine(), permit)?;
        eprintln!(
            "Building with `claude --model {}`; each window below is one call.",
            engine.model()
        );
        let mapping = TaskMapping::new(&engine, &store);
        let mapped = if join {
            mapping.show(&speech.conversations)
        } else {
            mapping.build(&speech.task, &speech.conversations, &mut report_progress)
        }
        .map_err(sentence_of)?;
        let joined = if whole_task {
            eprintln!("  Joining the conversations' maps into the task's…");
            TaskSynthesizing::new(&engine, &store)
                .build(&speech.task, &speech.task_id, &mapped)
                .map_err(join_sentence_of)?
        } else {
            None
        };
        (mapped, joined)
    } else {
        shown(&speech, &store, whole_task)?
    };
    if words.machine {
        println!("{}", map_line(&chosen, &speech, &mapped, joined.as_ref()));
        return Ok(());
    }
    match &joined {
        Some(joined) => {
            print_task_map(&speech.task, &mapped, joined);
            if words.detail {
                print_map(&speech.task, &mapped, build);
            } else {
                println!("Every conversation's own map: add {DETAIL_FLAG}.");
            }
        }
        None => print_map(&speech.task, &mapped, build),
    }
    Ok(())
}

/// A task's map as `tasks map <task> --json` shows it, on whatever ground the graph is read
/// from: what the service answers to `task-map`. It only shows. A build asks a model, and the
/// service does not go to a model on anybody's behalf.
pub(super) fn machine_line(
    arguments: &[String],
    context: Option<&str>,
    ground: &dyn ReadingGround,
) -> (String, Result<(), Refusal>) {
    let mut selected: Option<WorkspaceName> = None;
    match map_answer(arguments, context, &mut selected, ground) {
        Ok(line) => (line, Ok(())),
        Err(refusal) => (refusal_line(&refusal, selected.as_ref()), Err(refusal)),
    }
}

fn map_answer(
    arguments: &[String],
    context: Option<&str>,
    selected: &mut Option<WorkspaceName>,
    ground: &dyn ReadingGround,
) -> Result<String, Refusal> {
    let words = map_words(arguments).map_err(Refusal::usage)?;
    if words.build || words.join {
        return Err(Refusal::usage(format!(
            "task-map shows a map; {BUILD_FLAG} and {JOIN_FLAG} ask a model and run from `crosskit tasks map`"
        )));
    }
    let loaded = ground.policy()?;
    let chosen = workspace::choose(loaded, context)?;
    *selected = Some(chosen.clone());
    let mut speech = match task_speech_on(&words.task, &chosen, ground)? {
        Ok(speech) => speech,
        Err(missing) => return Ok(object_line(&missing)),
    };
    narrowed(&mut speech, words.only.as_deref()).map_err(Refusal::usage)?;
    let store = TaskMapFiles::of_context(&chosen).map_err(|failure| Refusal::failed(failure.0))?;
    let (mapped, joined) = shown(&speech, &store, words.only.is_none()).map_err(Refusal::failed)?;
    Ok(map_line(&chosen, &speech, &mapped, joined.as_ref()))
}

fn join_sentence_of(error: SynthesisError) -> String {
    match error {
        SynthesisError::Store(failure) => {
            format!("The task's map could not be kept: {}", failure.0)
        }
        SynthesisError::Engine(failure) => {
            format!("The conversations' maps could not be joined: {}", failure.0)
        }
    }
}

fn sentence_of(error: TaskMapError) -> String {
    match error {
        TaskMapError::Store(failure) => format!("The maps could not be kept: {}", failure.0),
    }
}

/// A map engine that only names the question: what showing needs, with nothing to ask.
struct ShowOnly;

impl MapMerger for ShowOnly {
    fn merge_version(&self) -> &str {
        crate::infrastructure::analysis::MAP_MERGE_VERSION
    }

    fn merge(
        &self,
        _task: &str,
        _conversations: &[MergeInput<'_>],
    ) -> Result<MergedProposal, crate::application::MapEngineFailure> {
        Err(crate::application::MapEngineFailure(
            "showing a map asks nothing".to_string(),
        ))
    }
}

impl crate::application::MapEngine for ShowOnly {
    fn question_version(&self) -> &str {
        crate::infrastructure::analysis::MAP_QUESTION_VERSION
    }

    fn propose(
        &self,
        _task: &str,
        _window: &[&SpokenReply],
        _replies: usize,
    ) -> Result<crate::application::ProposedStretch, crate::application::MapEngineFailure> {
        Err(crate::application::MapEngineFailure(
            "showing a map asks nothing".to_string(),
        ))
    }
}

fn report_progress(progress: MapProgress<'_>) {
    match progress {
        MapProgress::Asking {
            conversation,
            first_reply,
            last_reply,
        } => eprintln!("  {conversation}: replies {first_reply}–{last_reply}…"),
        MapProgress::Failed {
            conversation,
            reason,
        } => eprintln!("  {conversation}: stopped here — {reason}. The next build starts here."),
    }
}

/// A task's name and its conversations' replies.
struct TaskSpeech {
    task: String,
    task_id: String,
    conversations: Vec<ConversationSpeech>,
}

fn speech_of_task(task: &str, context: Option<&str>) -> Result<TaskSpeech, String> {
    let arguments = vec![task.to_string(), machine_answer::FLAG.to_string()];
    if let Some(line) = service_client::asked("task-speech", &arguments, context) {
        if let Ok(answer) = serde_json::from_str::<SpeechAnswer>(&line) {
            return Ok(answer.into_speech());
        }
        if let Ok(missing) = serde_json::from_str::<MissingTask>(&line) {
            return Err(format!("No task is called «{}».", missing.missing_task));
        }
    }
    let (line, outcome) = speech_line(&arguments, context, &OwnRun::load());
    outcome.map_err(String::from)?;
    match serde_json::from_str::<SpeechAnswer>(&line) {
        Ok(answer) => Ok(answer.into_speech()),
        Err(_) => match serde_json::from_str::<MissingTask>(&line) {
            Ok(missing) => Err(format!("No task is called «{}».", missing.missing_task)),
            Err(error) => Err(format!("The replies could not be read back: {error}")),
        },
    }
}

/// The replies of a task's conversations as one line, on whatever ground the graph is read from:
/// what the service answers to `task-speech`.
pub(super) fn speech_line(
    arguments: &[String],
    context: Option<&str>,
    ground: &dyn ReadingGround,
) -> (String, Result<(), Refusal>) {
    let mut selected: Option<WorkspaceName> = None;
    match speech_answer(arguments, context, &mut selected, ground) {
        Ok(line) => (line, Ok(())),
        Err(refusal) => (refusal_line(&refusal, selected.as_ref()), Err(refusal)),
    }
}

fn speech_answer(
    arguments: &[String],
    context: Option<&str>,
    selected: &mut Option<WorkspaceName>,
    ground: &dyn ReadingGround,
) -> Result<String, Refusal> {
    let words: Vec<&String> = arguments
        .iter()
        .filter(|one| *one != machine_answer::FLAG)
        .collect();
    let [task] = words.as_slice() else {
        return Err(Refusal::usage("task-speech takes one task"));
    };
    let loaded = ground.policy()?;
    let chosen = workspace::choose(loaded, context)?;
    *selected = Some(chosen.clone());
    let speech = match task_speech_on(task, &chosen, ground)? {
        Ok(speech) => speech,
        Err(missing) => return Ok(object_line(&missing)),
    };
    Ok(object_line(&SpeechAnswer {
        schema: SCHEMA,
        task: speech.task,
        task_id: speech.task_id,
        conversations: speech
            .conversations
            .into_iter()
            .map(|conversation| SpokenConversation {
                session: conversation.session_id,
                name: conversation.name,
                first_at_unix_ms: conversation.first_at_unix_ms,
                replies: conversation
                    .replies
                    .into_iter()
                    .map(|reply| Spoken {
                        n: reply.number,
                        owner: reply.speaker == Speaker::Owner,
                        text: reply.text,
                    })
                    .collect(),
            })
            .collect(),
    }))
}

/// The replies of the task the book calls `task`, by name or id, read from `chosen`'s graph on
/// `ground`; or the name that is no task.
fn task_speech_on(
    task: &str,
    chosen: &WorkspaceName,
    ground: &dyn ReadingGround,
) -> Result<Result<TaskSpeech, MissingTask>, Refusal> {
    let book = TaskBookFile::of_context(chosen)
        .and_then(|file| file.load())
        .map_err(|error| Refusal::failed(error.to_string()))?;
    let Some(found) = book
        .tasks()
        .iter()
        .find(|one| one.name() == task || one.id() == task)
    else {
        return Ok(Err(MissingTask {
            schema: SCHEMA,
            missing_task: task.to_string(),
        }));
    };
    let manager = ground.graph(chosen)?;
    let repository = manager.repository();
    let sessions: BTreeSet<String> = found.members().clone();
    let conversations = speech_of(&repository, &sessions).map_err(|error| {
        Refusal::of_graph(
            &error,
            format!("The conversations could not be read: {error}"),
        )
    })?;
    Ok(Ok(TaskSpeech {
        task: found.name().to_string(),
        task_id: found.id().to_string(),
        conversations,
    }))
}

#[derive(Serialize, Deserialize)]
struct MissingTask {
    schema: u32,
    missing_task: String,
}

#[derive(Serialize, Deserialize)]
struct SpeechAnswer {
    schema: u32,
    task: String,
    task_id: String,
    conversations: Vec<SpokenConversation>,
}

#[derive(Serialize, Deserialize)]
struct SpokenConversation {
    session: String,
    name: Option<String>,
    first_at_unix_ms: Option<i64>,
    replies: Vec<Spoken>,
}

#[derive(Serialize, Deserialize)]
struct Spoken {
    n: usize,
    owner: bool,
    text: String,
}

impl SpeechAnswer {
    fn into_speech(self) -> TaskSpeech {
        TaskSpeech {
            task: self.task,
            task_id: self.task_id,
            conversations: self
                .conversations
                .into_iter()
                .map(|conversation| ConversationSpeech {
                    session_id: conversation.session,
                    name: conversation.name,
                    first_at_unix_ms: conversation.first_at_unix_ms,
                    replies: conversation
                        .replies
                        .into_iter()
                        .map(|reply| SpokenReply {
                            number: reply.n,
                            speaker: if reply.owner {
                                Speaker::Owner
                            } else {
                                Speaker::Model
                            },
                            text: reply.text,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

/// Where a node of the join stands: its conversation's short name and the reply it quotes.
fn place_of(
    node: NodeRef,
    joined: &ShownSynthesis,
    mapped: &[MappedConversation],
) -> Option<(String, usize)> {
    let session = joined.stored.sessions.get(node.conversation())?;
    let conversation = mapped.iter().find(|c| &c.session_id == session)?;
    let map = conversation.map.as_ref()?;
    let reply = match node {
        NodeRef::Fork { index, .. } => map.nodes.forks.get(index)?.evidence.reply,
        NodeRef::Risk { index, .. } => map.nodes.risks.get(index)?.evidence.reply,
    };
    let label: String = conversation
        .name
        .as_deref()
        .unwrap_or(&conversation.session_id)
        .chars()
        .take(SHORT_LABEL_CHARACTERS)
        .collect();
    Some((label, reply))
}

/// `«Договоры и пакеты…» [62], «Counterparty данные…» [297]`: where a joined item comes from.
fn sources_of(nodes: &[NodeRef], joined: &ShownSynthesis, mapped: &[MappedConversation]) -> String {
    nodes
        .iter()
        .filter_map(|node| place_of(*node, joined, mapped))
        .map(|(label, reply)| format!("«{label}» [{reply}]"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_task_map(task: &str, mapped: &[MappedConversation], joined: &ShownSynthesis) {
    let synthesis = &joined.stored.synthesis;
    let mapped_count = mapped.iter().filter(|c| c.map.is_some()).count();
    println!(
        "{task} — the task's decision map, joined from {mapped_count} of {} conversation(s)",
        mapped.len()
    );
    if joined.stale {
        println!("Some conversations' maps grew after this was joined: --build joins them again.");
    }
    println!();
    println!("Main line ({})", synthesis.main_line.len());
    for step in &synthesis.main_line {
        println!("  {} {}", mark_of(step.state), cut(&step.title));
        println!("      {}", sources_of(&step.nodes, joined, mapped));
    }
    let print_risks = |risks: &[TaskRisk]| {
        let mut sorted: Vec<&TaskRisk> = risks.iter().collect();
        sorted.sort_by_key(|risk| (risk.status != RiskStatus::Open, risk.level));
        for risk in sorted {
            let raised = match risk.conversations() {
                0 | 1 => String::new(),
                n => format!(" — raised in {n} conversations"),
            };
            let status = match risk.status {
                RiskStatus::Open => String::new(),
                other => format!(" ({})", other.code()),
            };
            println!(
                "  ⚠ {:<8} {}{status}{raised}",
                level_of(risk.level),
                cut(&risk.title)
            );
            println!("      {}", sources_of(&risk.nodes, joined, mapped));
        }
    };
    println!();
    println!(
        "Risks of the task ({} open of {})",
        synthesis
            .risks
            .iter()
            .filter(|risk| risk.status == RiskStatus::Open)
            .count(),
        synthesis.risks.len()
    );
    print_risks(&synthesis.risks);
    if !synthesis.findings.is_empty() {
        println!();
        println!("Findings outside the task ({})", synthesis.findings.len());
        print_risks(&synthesis.findings);
    }
    println!();
    println!(
        "Every line names the conversation and the reply its quote is in; joining cost {:.2} $ by the tool's count.",
        joined.stored.cost_micro_usd as f64 / 1_000_000.0
    );
    println!("Marks: ✓ chosen  ✗ rejected  ○ deferred  ? open  ᵃ agent's default  ⊘ abandoned");
}

fn print_map(task: &str, mapped: &[MappedConversation], built: bool) {
    let waiting = mapped.iter().filter(|c| c.unmapped_replies() > 0).count();
    println!("{task} — decision map, {} conversation(s)", mapped.len());
    if waiting > 0 && !built {
        println!(
            "{waiting} conversation(s) have replies no map covers yet: crosskit tasks map \"{task}\" {BUILD_FLAG}"
        );
    }
    let mut outside: Vec<(String, String)> = Vec::new();
    let (mut kept, mut dropped, mut cost) = (0, 0, 0);
    for conversation in mapped {
        let title = conversation
            .name
            .clone()
            .unwrap_or_else(|| conversation.session_id.chars().take(8).collect());
        println!();
        println!(
            "━━ {}  {title}  ({} replies{})",
            day_of(conversation.first_at_unix_ms),
            conversation.replies,
            match conversation.unmapped_replies() {
                0 => String::new(),
                n => format!(", {n} not mapped"),
            }
        );
        let Some(map) = &conversation.map else {
            continue;
        };
        kept += map.tally.kept;
        dropped += map.tally.dropped();
        cost += map.cost_micro_usd;
        // Windows are asked in order, but a fork names the reply it was posed in, and the map
        // reads in the order the conversation did.
        let mut forks: Vec<_> = map.nodes.forks.iter().collect();
        forks.sort_by_key(|fork| fork.evidence.reply);
        if !forks.is_empty() {
            println!("  Forks ({})", forks.len());
        }
        for fork in forks {
            println!("  ◆ [{}] {}", fork.evidence.reply, cut(&fork.question));
            for branch in &fork.branches {
                let reason = if branch.reason.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", cut(&branch.reason))
                };
                println!(
                    "     {} [{}] {}{reason}",
                    mark_of(branch.state),
                    branch.evidence.reply,
                    cut(&branch.option)
                );
            }
        }
        let mut risks: Vec<_> = map.nodes.risks.iter().filter(|risk| risk.in_task).collect();
        risks.sort_by_key(|risk| (risk.level, risk.evidence.reply));
        let (open, settled): (Vec<_>, Vec<_>) = risks
            .into_iter()
            .partition(|risk| risk.status == RiskStatus::Open);
        if !open.is_empty() {
            println!("  Open risks ({})", open.len());
        }
        for risk in open {
            println!(
                "  ⚠ {:<8} [{}] {}",
                level_of(risk.level),
                risk.evidence.reply,
                cut(&risk.description)
            );
        }
        if !settled.is_empty() {
            println!("  Closed or accepted ({})", settled.len());
        }
        for risk in settled {
            println!(
                "    {} {} [{}] {}",
                risk.status.code(),
                level_of(risk.level),
                risk.evidence.reply,
                cut(&risk.description)
            );
        }
        for risk in map.nodes.risks.iter().filter(|risk| !risk.in_task) {
            outside.push((
                title.clone(),
                format!(
                    "{} [{}] {}",
                    level_of(risk.level),
                    risk.evidence.reply,
                    cut(&risk.description)
                ),
            ));
        }
    }
    if !outside.is_empty() {
        println!();
        println!("Findings outside the task:");
        for (title, line) in outside {
            println!("  ⇢ {line}  ({title})");
        }
    }
    println!();
    println!(
        "Kept {kept} node(s) a quote proves; dropped {dropped} the model could not quote. Spent so far: {:.2} $ by the tool's count.",
        cost as f64 / 1_000_000.0
    );
    println!("Marks: ✓ chosen  ✗ rejected  ○ deferred  ? open  ᵃ agent's default  ⊘ abandoned");
}

/// A line of the text map: the start of a long sentence, cut at a character and said to be cut.
/// The whole of it is in `--json` and in the stored map.
fn cut(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    if flat.chars().count() <= LINE_CHARACTERS {
        return flat;
    }
    let mut kept: String = flat.chars().take(LINE_CHARACTERS).collect();
    kept.push('…');
    kept
}

fn mark_of(state: BranchState) -> &'static str {
    match state {
        BranchState::Chosen => "✓",
        BranchState::Rejected => "✗",
        BranchState::Deferred => "○",
        BranchState::Open => "?",
        BranchState::AgentDefault => "ᵃ",
        BranchState::Abandoned => "⊘",
    }
}

fn level_of(level: RiskLevel) -> &'static str {
    match level {
        RiskLevel::Critical => "CRITICAL",
        RiskLevel::High => "high",
        RiskLevel::Medium => "medium",
        RiskLevel::Info => "info",
    }
}

fn day_of(unix_ms: Option<i64>) -> String {
    unix_ms.map_or_else(
        || "?".to_string(),
        |ms| CivilDate::from_days(ms.div_euclid(DAY_MS)).to_string(),
    )
}

#[derive(Serialize)]
struct MapAnswer<'a> {
    schema: u32,
    context: &'a str,
    task: &'a str,
    question: &'a str,
    /// The task's own map, joined from the conversations'; nothing when none is kept.
    joined: Option<Joined>,
    conversations: Vec<MapConversation<'a>>,
}

#[derive(Serialize)]
struct Joined {
    question: String,
    /// The conversations' maps grew after it was joined.
    stale: bool,
    main_line: Vec<JoinedItem>,
    risks: Vec<JoinedItem>,
    findings: Vec<JoinedItem>,
}

#[derive(Serialize)]
struct JoinedItem {
    title: String,
    /// A branch state for a step of the main line, a risk status for a risk.
    state: &'static str,
    level: Option<&'static str>,
    sources: Vec<JoinedSource>,
}

#[derive(Serialize)]
struct JoinedSource {
    session: String,
    node: String,
    reply: Option<usize>,
}

fn joined_of(joined: &ShownSynthesis, mapped: &[MappedConversation]) -> Joined {
    let sources = |nodes: &[NodeRef]| -> Vec<JoinedSource> {
        nodes
            .iter()
            .filter_map(|node| {
                Some(JoinedSource {
                    session: joined.stored.sessions.get(node.conversation())?.clone(),
                    node: node.code(),
                    reply: place_of(*node, joined, mapped).map(|(_, reply)| reply),
                })
            })
            .collect()
    };
    let risks = |risks: &[TaskRisk]| -> Vec<JoinedItem> {
        risks
            .iter()
            .map(|risk| JoinedItem {
                title: risk.title.clone(),
                state: risk.status.code(),
                level: Some(risk.level.code()),
                sources: sources(&risk.nodes),
            })
            .collect()
    };
    let synthesis = &joined.stored.synthesis;
    Joined {
        question: joined.stored.version.clone(),
        stale: joined.stale,
        main_line: synthesis
            .main_line
            .iter()
            .map(|step| JoinedItem {
                title: step.title.clone(),
                state: step.state.code(),
                level: None,
                sources: sources(&step.nodes),
            })
            .collect(),
        risks: risks(&synthesis.risks),
        findings: risks(&synthesis.findings),
    }
}

#[derive(Serialize)]
struct MapConversation<'a> {
    session: &'a str,
    name: Option<&'a str>,
    first_at_unix_ms: Option<i64>,
    replies: usize,
    mapped_through: usize,
    forks: Vec<MapFork<'a>>,
    risks: Vec<MapRisk<'a>>,
    kept: usize,
    dropped: usize,
}

#[derive(Serialize)]
struct MapFork<'a> {
    question: &'a str,
    reply: usize,
    /// Said by the owner, not by the agent.
    owner: bool,
    quote: &'a str,
    branches: Vec<MapBranch<'a>>,
}

#[derive(Serialize)]
struct MapBranch<'a> {
    option: &'a str,
    state: &'static str,
    reason: &'a str,
    reply: usize,
    owner: bool,
    quote: &'a str,
}

#[derive(Serialize)]
struct MapRisk<'a> {
    description: &'a str,
    level: &'static str,
    status: &'static str,
    in_task: bool,
    reply: usize,
    owner: bool,
    quote: &'a str,
}

fn map_line(
    context: &WorkspaceName,
    speech: &TaskSpeech,
    mapped: &[MappedConversation],
    joined: Option<&ShownSynthesis>,
) -> String {
    object_line(&MapAnswer {
        schema: SCHEMA,
        context: context.as_str(),
        task: &speech.task,
        question: crate::infrastructure::analysis::MAP_QUESTION_VERSION,
        joined: joined.map(|joined| joined_of(joined, mapped)),
        conversations: mapped
            .iter()
            .map(|conversation| {
                let map = conversation.map.as_ref();
                let owner = owner_of(speech, &conversation.session_id);
                MapConversation {
                    session: &conversation.session_id,
                    name: conversation.name.as_deref(),
                    first_at_unix_ms: conversation.first_at_unix_ms,
                    replies: conversation.replies,
                    mapped_through: map.map_or(0, |map| map.mapped_through),
                    forks: map
                        .into_iter()
                        .flat_map(|map| &map.nodes.forks)
                        .map(|fork| MapFork {
                            question: &fork.question,
                            reply: fork.evidence.reply,
                            owner: owner(fork.evidence.reply),
                            quote: &fork.evidence.quote,
                            branches: fork
                                .branches
                                .iter()
                                .map(|branch| MapBranch {
                                    option: &branch.option,
                                    state: branch.state.code(),
                                    reason: &branch.reason,
                                    reply: branch.evidence.reply,
                                    owner: owner(branch.evidence.reply),
                                    quote: &branch.evidence.quote,
                                })
                                .collect(),
                        })
                        .collect(),
                    risks: map
                        .into_iter()
                        .flat_map(|map| &map.nodes.risks)
                        .map(|risk| MapRisk {
                            description: &risk.description,
                            level: risk.level.code(),
                            status: risk.status.code(),
                            in_task: risk.in_task,
                            reply: risk.evidence.reply,
                            owner: owner(risk.evidence.reply),
                            quote: &risk.evidence.quote,
                        })
                        .collect(),
                    kept: map.map_or(0, |map| map.tally.kept),
                    dropped: map.map_or(0, |map| map.tally.dropped()),
                }
            })
            .collect(),
    })
}

/// Whether a reply of the conversation `session` was said by the owner, by its number.
fn owner_of<'a>(speech: &'a TaskSpeech, session: &str) -> impl Fn(usize) -> bool + 'a {
    let replies = speech
        .conversations
        .iter()
        .find(|conversation| conversation.session_id == session)
        .map_or(&[][..], |conversation| conversation.replies.as_slice());
    move |number| {
        replies
            .iter()
            .any(|reply| reply.number == number && reply.speaker == Speaker::Owner)
    }
}
