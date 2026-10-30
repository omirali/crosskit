// The stage «Карта решений»: what a model makes of a confirmed task's conversations as a map of
// forks, branches and risks, what one costs, and how much of it a quote proves (ADR 0054).
//
//   CROSSKIT_MAP_CONTEXT=work CROSSKIT_MAP_TASK="<task name>" \
//   CROSSKIT_MAP_OUT=<a directory outside the repository> \
//   [CROSSKIT_MAP_SESSIONS=<session prefixes, comma-separated>] [CROSSKIT_MAP_MODELS=haiku,sonnet] \
//     cargo test --release --offline --features decision-map-measurement \
//     --lib decision_map:: -- --ignored --nocapture
//
// Ignored, because it reads a real graph and calls a model. The graph is opened read-only and
// closed before the first call; the background service holds it while it runs, so it has to be
// stopped for the moment of reading (`crosskit service stop`, then `start`). A call needs the
// context's `send_content_outside`. Stdout carries counts, seconds and dollars only; the maps,
// which quote the conversations, go to `CROSSKIT_MAP_OUT`.
use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;

use super::question::{QUESTION_VERSION, instructions, rendered, schema, windows};
use super::speech::Reply;
use super::verdict::{Tally, checked};
use crate::application::{TaskBookStore, speech_of};
use crate::domain::conversation::Speaker;
use crate::domain::{ContentEgressPermit, WorkspaceName};
use crate::infrastructure::TaskBookFile;
use crate::infrastructure::analysis::ask_installed_tool;
use crate::infrastructure::config::load_index_policy;
use crate::infrastructure::ladybug::LadybugDatabaseManager;
use crate::infrastructure::outside_repository::refuse_inside_the_repository;

const CONTEXT_VARIABLE: &str = "CROSSKIT_MAP_CONTEXT";
const TASK_VARIABLE: &str = "CROSSKIT_MAP_TASK";
const OUT_VARIABLE: &str = "CROSSKIT_MAP_OUT";
const SESSIONS_VARIABLE: &str = "CROSSKIT_MAP_SESSIONS";
const MODELS_VARIABLE: &str = "CROSSKIT_MAP_MODELS";
const DEFAULT_MODELS: &str = "haiku,sonnet";
/// Without sessions named, the measurement takes this many of the task's conversations: the
/// smallest that still have something to decide in them.
const DEFAULT_CONVERSATIONS: usize = 2;
const FEWEST_REPLIES: usize = 20;

struct Setup {
    context: WorkspaceName,
    task: String,
    out: PathBuf,
    sessions: Option<Vec<String>>,
    models: Vec<String>,
    permit: ContentEgressPermit,
}

#[test]
#[ignore = "замер карты решений: CROSSKIT_MAP_CONTEXT=work CROSSKIT_MAP_TASK=… CROSSKIT_MAP_OUT=… cargo test --release --offline --features decision-map-measurement --lib decision_map:: -- --ignored --nocapture"]
fn what_a_decision_map_of_a_task_costs_and_how_much_of_it_is_proved() {
    let setup = match setup() {
        Ok(setup) => setup,
        Err(message) => {
            println!("{message}");
            return;
        }
    };
    let conversations = match conversations_of(&setup) {
        Ok(conversations) => conversations,
        Err(message) => {
            println!("{message}");
            return;
        }
    };
    println!("## Замер карты решений ({QUESTION_VERSION})\n");
    println!("| беседа | реплик | байт речи | окон |");
    println!("| --- | ---: | ---: | ---: |");
    for (number, (_, replies)) in conversations.iter().enumerate() {
        let bytes: usize = replies.iter().map(|reply| reply.text.len()).sum();
        println!(
            "| {} | {} | {bytes} | {} |",
            number + 1,
            replies.len(),
            windows(replies).len()
        );
    }

    println!(
        "\n| беседа | модель | с | токенов на входе | на выходе | $ | развилок | веток | рисков | доказано | нет реплики | цитаты нет | сбоев |"
    );
    println!(
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"
    );
    for (number, (session, replies)) in conversations.iter().enumerate() {
        for model in &setup.models {
            let run = map_of(&setup, session, replies, model);
            println!(
                "| {} | {model} | {:.0} | {} | {} | {:.3} | {} | {} | {} | {} из {} | {} | {} | {} |",
                number + 1,
                run.seconds,
                run.input_tokens,
                run.output_tokens,
                run.cost_usd,
                run.tally.forks,
                run.tally.branches,
                run.tally.risks,
                run.tally.proved,
                run.tally.nodes(),
                run.tally.no_such_reply,
                run.tally.quote_not_found,
                run.failures
            );
        }
    }
    println!("\nКарты с цитатами записаны вне репозитория, в {OUT_VARIABLE}.");
}

fn setup() -> Result<Setup, String> {
    let variable = |name: &str| {
        std::env::var(name).map_err(|_| {
            format!("{name} не задан. Нужны {CONTEXT_VARIABLE}, {TASK_VARIABLE} и {OUT_VARIABLE}. Ничего не измерено.")
        })
    };
    let context = WorkspaceName::parse(&variable(CONTEXT_VARIABLE)?)
        .map_err(|error| format!("{CONTEXT_VARIABLE}: {error}"))?;
    let out = PathBuf::from(variable(OUT_VARIABLE)?);
    std::fs::create_dir_all(&out).map_err(|error| format!("{OUT_VARIABLE}: {error}"))?;
    refuse_inside_the_repository(&out, OUT_VARIABLE)?;
    let loaded = load_index_policy().map_err(|error| format!("конфигурация: {error}"))?;
    let permit = loaded
        .workspaces()
        .content_egress(&context)
        .permit()
        .ok_or_else(|| {
            format!(
                "Контекст {context} не разрешает отправлять содержимое наружу \
                 (`send_content_outside`). Модель не вызвана, ничего не измерено."
            )
        })?;
    let list = |value: String| -> Vec<String> {
        value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    };
    Ok(Setup {
        context,
        task: variable(TASK_VARIABLE)?,
        out,
        sessions: std::env::var(SESSIONS_VARIABLE).ok().map(list),
        models: list(std::env::var(MODELS_VARIABLE).unwrap_or_else(|_| DEFAULT_MODELS.to_string())),
        permit,
    })
}

/// The task's conversations to measure, read from the graph and the graph closed again.
fn conversations_of(setup: &Setup) -> Result<Vec<(String, Vec<Reply>)>, String> {
    let book = TaskBookFile::of_context(&setup.context)
        .and_then(|file| file.load())
        .map_err(|error| error.to_string())?;
    let task = book
        .tasks()
        .iter()
        .find(|task| task.name() == setup.task)
        .ok_or_else(|| format!("Задачи «{}» в книге нет. Ничего не измерено.", setup.task))?;
    let manager =
        LadybugDatabaseManager::open_workspace_read_only(&setup.context).map_err(|error| {
            format!(
                "Граф не открылся ({error}). Если работает служба, она держит граф: \
             `crosskit service stop`, замер, `crosskit service start`."
            )
        })?;
    let speech =
        speech_of(&manager.repository(), task.members()).map_err(|error| error.to_string())?;
    drop(manager);
    let mut members: Vec<(String, Vec<Reply>)> = speech
        .into_iter()
        .map(|conversation| (conversation.session_id, conversation.replies))
        .collect();
    members.sort_by_key(|(_, replies)| replies.iter().map(|reply| reply.text.len()).sum::<usize>());
    let chosen: Vec<(String, Vec<Reply>)> = match &setup.sessions {
        Some(prefixes) => members
            .into_iter()
            .filter(|(session, _)| {
                prefixes
                    .iter()
                    .any(|prefix| session.starts_with(prefix.as_str()))
            })
            .collect(),
        None => members
            .into_iter()
            .filter(|(_, replies)| replies.len() >= FEWEST_REPLIES)
            .take(DEFAULT_CONVERSATIONS)
            .collect(),
    };
    if chosen.is_empty() {
        return Err("Ни одна беседа задачи не подошла для замера. Ничего не измерено.".to_string());
    }
    Ok(chosen)
}

/// One conversation mapped by one model, window by window.
struct Run {
    seconds: f64,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    tally: Tally,
    failures: usize,
}

fn map_of(setup: &Setup, session: &str, replies: &[Reply], model: &str) -> Run {
    let started = Instant::now();
    let mut run = Run {
        seconds: 0.0,
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: 0.0,
        tally: Tally::default(),
        failures: 0,
    };
    let mut kept = Vec::new();
    for window in windows(replies) {
        let prompt = format!(
            "{}\n\n---\n\n{}",
            instructions(&setup.task),
            rendered(&window)
        );
        let answered = ask_installed_tool(model, &schema(), prompt, &setup.permit);
        let envelope = match answered {
            Ok(envelope) if envelope["structured_output"].is_object() => envelope,
            Ok(envelope) => {
                run.failures += 1;
                kept.push(json!({"failed": envelope["subtype"].clone()}));
                continue;
            }
            Err(reason) => {
                run.failures += 1;
                kept.push(json!({"failed": reason}));
                continue;
            }
        };
        let usage = &envelope["usage"];
        run.input_tokens += [
            "input_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
        ]
        .iter()
        .filter_map(|key| usage[*key].as_u64())
        .sum::<u64>();
        run.output_tokens += usage["output_tokens"].as_u64().unwrap_or(0);
        run.cost_usd += envelope["total_cost_usd"].as_f64().unwrap_or(0.0);
        let mut answer = envelope["structured_output"].clone();
        run.tally.add(checked(&mut answer, &window));
        kept.push(json!({
            "replies": [window.first().map(|r| r.number), window.last().map(|r| r.number)],
            "model": envelope["modelUsage"].as_object().and_then(|usage| usage.keys().next().cloned()),
            "answer": answer,
        }));
    }
    run.seconds = started.elapsed().as_secs_f64();
    let owner_replies = replies
        .iter()
        .filter(|reply| reply.speaker == Speaker::Owner)
        .count();
    let map = json!({
        "question": QUESTION_VERSION,
        "task": setup.task,
        "session": session,
        "model": model,
        "replies": replies.len(),
        "owner_replies": owner_replies,
        "windows": kept,
    });
    let short: String = session.chars().take(8).collect();
    let path = setup.out.join(format!("{short}-{model}.json"));
    let written = serde_json::to_string_pretty(&map)
        .map_err(|error| error.to_string())
        .and_then(|text| std::fs::write(&path, text).map_err(|error| error.to_string()));
    if let Err(error) = written {
        println!("Карта {short}-{model} не записана: {error}");
    }
    run
}
