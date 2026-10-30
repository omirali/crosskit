// `crosskit [--context <name>] knowledge [entities [--type <t>] [--limit <N>] | show <name> |
// map [--limit <N>] [--json]]`.
//
// What the knowledge layer holds in one context. With no word, the numbers: how many entities
// and facts, how many proposed facts were refused and why, what it cost and what the queue
// would cost at that rate. `entities` lists them by how many facts they have; `show` is one
// entity with every fact at either end, each with its quote and where it was read; `map` is the
// whole graph, and with `--json` it is the object the panel's knowledge map draws.
//
// Reading only: nothing here opens a graph for writing (ADR 0034), and nothing here asks a
// model.
use crate::application::{KnowledgeMapService, KnowledgeReader, KnowledgeTotals, entities_named};
use crate::domain::WorkspaceName;
use crate::domain::knowledge::{FactSource, KnowledgeMap, MAP_MOST_ENTITIES, MapEntity, MapFact};
use crate::infrastructure::search::AnalyzedEntityNames;

use super::help;
use super::machine_answer::{self, refusal_line};
use super::machine_knowledge::map_line;
use super::reading_ground::{OwnRun, ReadingGround};
use super::refusal::Refusal;
use super::run_cost::dollars;
use super::search::one_line;
use super::search_index::visible_paths;
use super::service_client;
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] knowledge\n       crosskit [--context <name>] knowledge entities [--type <type>] [--limit <N>]\n       crosskit [--context <name>] knowledge show <name>\n       crosskit [--context <name>] knowledge map [--limit <N>] [--json]";

/// How many entities `entities` prints unless asked for another number.
const LISTED_BY_DEFAULT: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    Summary,
    Entities {
        entity_type: Option<String>,
        limit: usize,
    },
    Show {
        name: String,
    },
    Map {
        limit: usize,
        machine: bool,
    },
}

pub(super) fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    if arguments.iter().any(|one| one == machine_answer::FLAG) {
        if let Some(line) = service_client::answered("knowledge-map", arguments, context) {
            println!("{line}");
            return Ok(());
        }
        let (line, outcome) = machine_line(arguments, context, &OwnRun::load());
        println!("{line}");
        return outcome.map_err(String::from);
    }
    let mut selected: Option<WorkspaceName> = None;
    answer(arguments, context, &mut selected, &OwnRun::load()).map_err(String::from)
}

/// `knowledge map --json` as one line, on whatever ground it is read from, and whether it was
/// a refusal. The map is the only part of this command that answers a program.
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
    let Request::Map { limit, .. } = parse(arguments).map_err(Refusal::usage)? else {
        return Err(Refusal::usage(format!(
            "--json is answered by `knowledge map` only. {USAGE}"
        )));
    };
    let loaded = ground.policy()?;
    let chosen = workspace::choose(loaded, context)?;
    *selected = Some(chosen.clone());
    let manager = ground.graph(&chosen)?;
    let repository = manager.repository();
    let visible = visible_paths(loaded, &chosen);
    let map = KnowledgeMapService::new(&repository, &visible)
        .map(limit)
        .map_err(|error| {
            Refusal::of_graph(&error, format!("read the knowledge of {chosen}: {error}"))
        })?;
    let roots: Vec<String> = loaded
        .workspaces()
        .roots_of(&chosen)
        .iter()
        .filter_map(|root| root.directory().map(str::to_string))
        .collect();
    let names = AnalyzedEntityNames::new(loaded.normalizations());
    Ok(map_line(&chosen, &map, loaded.vocabulary(), &roots, &names))
}

fn answer(
    arguments: &[String],
    context: Option<&str>,
    selected: &mut Option<WorkspaceName>,
    ground: &dyn ReadingGround,
) -> Result<(), Refusal> {
    let request = parse(arguments).map_err(Refusal::usage)?;
    let loaded = ground.policy()?;
    let chosen = workspace::choose(loaded, context)?;
    *selected = Some(chosen.clone());
    let manager = ground.graph(&chosen)?;
    let repository = manager.repository();
    let visible = visible_paths(loaded, &chosen);
    let service = KnowledgeMapService::new(&repository, &visible);
    let read = |limit: usize| {
        service.map(limit).map_err(|error| {
            Refusal::of_graph(&error, format!("read the knowledge of {chosen}: {error}"))
        })
    };

    match request {
        Request::Summary => {
            let totals = repository.knowledge_totals().map_err(|error| {
                Refusal::of_graph(&error, format!("count the knowledge of {chosen}: {error}"))
            })?;
            println!("Context: {chosen}");
            print_summary(&totals);
        }
        Request::Entities { entity_type, limit } => {
            println!("Context: {chosen}");
            print_entities(&read(usize::MAX)?, entity_type.as_deref(), limit);
        }
        Request::Show { name } => {
            let map = read(usize::MAX)?;
            let names = AnalyzedEntityNames::new(loaded.normalizations());
            println!("Context: {chosen}");
            print_named(&map, &name, &names);
        }
        Request::Map { limit, .. } => {
            let map = read(limit)?;
            println!("Context: {chosen}");
            print_map(&map);
        }
    }
    Ok(())
}

fn parse(arguments: &[String]) -> Result<Request, String> {
    let machine = arguments.iter().any(|one| one == machine_answer::FLAG);
    let words: Vec<&String> = arguments
        .iter()
        .filter(|one| *one != machine_answer::FLAG)
        .collect();
    let Some((command, rest)) = words.split_first() else {
        return refuse_machine_answer(machine, Request::Summary);
    };
    match command.as_str() {
        "entities" => {
            let mut entity_type = None;
            let mut limit = LISTED_BY_DEFAULT;
            let mut options = rest.iter();
            while let Some(option) = options.next() {
                let mut value = |name: &str| {
                    options
                        .next()
                        .map(|value| value.to_string())
                        .ok_or_else(|| format!("{name} requires a value. {USAGE}"))
                };
                match option.as_str() {
                    "--type" => entity_type = Some(value("--type")?),
                    "--limit" => limit = parse_limit(&value("--limit")?)?,
                    other => return Err(format!("unknown argument {other:?}. {USAGE}")),
                }
            }
            refuse_machine_answer(machine, Request::Entities { entity_type, limit })
        }
        "show" => match rest {
            [] => Err(format!("show needs the name of an entity. {USAGE}")),
            words => refuse_machine_answer(
                machine,
                Request::Show {
                    name: words
                        .iter()
                        .map(|word| word.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                },
            ),
        },
        "map" => {
            let mut limit = MAP_MOST_ENTITIES;
            let mut options = rest.iter();
            while let Some(option) = options.next() {
                match option.as_str() {
                    "--limit" => {
                        let value = options
                            .next()
                            .ok_or_else(|| format!("--limit requires a value. {USAGE}"))?;
                        limit = parse_limit(value)?;
                        if limit > MAP_MOST_ENTITIES {
                            return Err(format!(
                                "--limit is at most {MAP_MOST_ENTITIES} entities: past that the map is not drawn in time (ADR 0048). {USAGE}"
                            ));
                        }
                    }
                    other => return Err(format!("unknown argument {other:?}. {USAGE}")),
                }
            }
            Ok(Request::Map { limit, machine })
        }
        _ => Err(help::misuse(USAGE)),
    }
}

/// Only the map answers a program; a list a person reads is not a contract.
fn refuse_machine_answer(machine: bool, request: Request) -> Result<Request, String> {
    match machine {
        true => Err(format!(
            "--json is answered by `knowledge map` only. {USAGE}"
        )),
        false => Ok(request),
    }
}

fn parse_limit(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|limit| *limit > 0)
        .ok_or_else(|| {
            format!("--limit expects a number of 1 or more, received {value:?}. {USAGE}")
        })
}

fn print_summary(totals: &KnowledgeTotals) {
    println!(
        "Entities: {} ({} spellings glued into them)",
        totals.entities,
        totals.spellings_glued()
    );
    println!("Facts: {}", totals.facts);
    if totals.checked_revisions == 0 {
        println!(
            "No revision has been asked for facts yet: `crosskit index jobs run --limit <N>` asks the queue, in a context that may send its content."
        );
    } else {
        print_checked(totals);
    }
    let rate = totals.rate();
    println!(
        "Still queued: {} jobs, {} parts, {} bytes{}",
        totals.queued_jobs,
        totals.queued_chunks,
        totals.queued_bytes,
        totals
            .forecast_micro_usd()
            .filter(|forecast| *forecast > 0)
            .map(|forecast| format!(
                " — about {} at that rate, one call per job",
                dollars(forecast)
            ))
            .unwrap_or_default(),
    );
    if let Some(rate) = rate {
        println!(
            "Rate: {} per call and {} per 100 KB sent, read off {} paid calls",
            dollars(rate.per_call_micro_usd.round() as i64),
            dollars((rate.per_byte_micro_usd * 100_000.0).round() as i64),
            rate.calls
        );
    }
}

fn print_checked(totals: &KnowledgeTotals) {
    let proposed = totals.facts_kept
        + totals.facts_rejected_by_quote
        + totals.facts_rejected_unnamed
        + totals.facts_rejected_otherwise;
    println!(
        "Revisions asked: {} ({} parts, {} bytes)",
        totals.checked_revisions, totals.checked_chunks, totals.checked_bytes
    );
    println!(
        "Facts proposed: {proposed} — kept {}, rejected because the quote is not in its part {} ({}), because it does not name both ends {} ({}), rejected otherwise {} ({})",
        totals.facts_kept,
        totals.facts_rejected_by_quote,
        share(totals.facts_rejected_by_quote, proposed),
        totals.facts_rejected_unnamed,
        share(totals.facts_rejected_unnamed, proposed),
        totals.facts_rejected_otherwise,
        share(totals.facts_rejected_otherwise, proposed),
    );
    println!(
        "Cost so far: {} input and {} output tokens, {} as the engine reported it{}",
        totals.input_tokens,
        totals.output_tokens,
        dollars(totals.cost_micro_usd),
        totals
            .cost_per_chunk_micro_usd()
            .filter(|per| *per > 0)
            .map(|per| format!(" — {} per part", dollars(per)))
            .unwrap_or_default(),
    );
}

fn share(part: i64, whole: i64) -> String {
    match whole {
        0 => "0 %".to_string(),
        _ => format!("{:.1} %", part as f64 * 100.0 / whole as f64),
    }
}

fn print_entities(map: &KnowledgeMap, entity_type: Option<&str>, limit: usize) {
    let listed: Vec<&MapEntity> = map
        .entities()
        .iter()
        .filter(|entity| entity_type.is_none_or(|wanted| entity.entity_type() == wanted))
        .collect();
    for entity in listed.iter().take(limit) {
        println!(
            "{} ({}) — {} facts, {} sources{}",
            one_line(entity.name()),
            entity.entity_type(),
            entity.degree(),
            entity.files(),
            match entity.spellings().len() {
                1 => String::new(),
                _ => format!(", also written {}", others(entity)),
            }
        );
    }
    println!();
    println!(
        "Entities: {} of {}{}",
        listed.len().min(limit),
        listed.len(),
        entity_type
            .map(|wanted| format!(" of type {wanted}"))
            .unwrap_or_default()
    );
}

fn others(entity: &MapEntity) -> String {
    entity.spellings()[1..]
        .iter()
        .map(|spelling| one_line(spelling))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_named(map: &KnowledgeMap, name: &str, names: &AnalyzedEntityNames) {
    let found = entities_named(map, name, names);
    match found.as_slice() {
        [] => println!(
            "No entity is named {:?}. `knowledge entities` lists them.",
            one_line(name)
        ),
        [entity] => print_entity(map, entity),
        several => {
            println!(
                "{} entities have {:?} in their name:",
                several.len(),
                one_line(name)
            );
            for entity in several {
                println!("  {} ({})", one_line(entity.name()), entity.entity_type());
            }
        }
    }
}

fn print_entity(map: &KnowledgeMap, entity: &MapEntity) {
    println!("{} ({})", one_line(entity.name()), entity.entity_type());
    if !entity.description().is_empty() {
        println!("  {}", one_line(entity.description()));
    }
    if entity.spellings().len() > 1 {
        println!("  also written: {}", others(entity));
    }
    println!(
        "  {} facts, from {} sources",
        entity.degree(),
        entity.files()
    );
    for fact in map
        .facts()
        .iter()
        .filter(|one| one.fact().subject_id == entity.id() || one.fact().object_id == entity.id())
    {
        println!();
        print_fact(map, fact);
    }
}

fn print_map(map: &KnowledgeMap) {
    for fact in map.facts() {
        print_fact(map, fact);
        println!();
    }
    println!(
        "Entities: {} of {}, facts: {} of {}{}",
        map.entities().len(),
        map.found_entities(),
        map.facts().len(),
        map.found_facts(),
        match map.truncated() {
            true => format!(
                " — cut at {} entities, the most connected kept",
                map.limit()
            ),
            false => String::new(),
        }
    );
}

/// «Пакет —belongs-to→ Контрагент», the quote, and where it was read.
fn print_fact(map: &KnowledgeMap, fact: &MapFact) {
    let name = |id: &str| {
        map.entity(id)
            .map(|entity| one_line(entity.name()))
            .unwrap_or_default()
    };
    let stored = fact.fact();
    println!(
        "  {} —{}→ {}  (weight {})",
        name(&stored.subject_id),
        stored.predicate,
        name(&stored.object_id),
        stored.weight
    );
    println!("    «{}»", one_line(&stored.quote));
    println!("    {}", source_line(fact.source(), stored.chunk_ordinal));
}

fn source_line(source: &FactSource, chunk_ordinal: i64) -> String {
    match source {
        FactSource::File { path } => format!("{path}, part {}", chunk_ordinal + 1),
        FactSource::Conversation {
            path,
            session_id,
            name,
            reply,
        } => format!(
            "conversation «{}» (session {session_id}), {} — {path}",
            match one_line(name).as_str() {
                "" => "unnamed".to_string(),
                named => named.to_string(),
            },
            match reply {
                Some(reply) => format!("reply {}", reply + 1),
                None => format!("part {}", chunk_ordinal + 1),
            }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{LISTED_BY_DEFAULT, Request, parse};
    use crate::domain::knowledge::MAP_MOST_ENTITIES;

    fn parsed(arguments: &[&str]) -> Result<Request, String> {
        parse(
            &arguments
                .iter()
                .map(|one| one.to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn each_form_is_read_with_its_defaults() {
        assert_eq!(parsed(&[]).unwrap(), Request::Summary);
        assert_eq!(
            parsed(&["entities"]).unwrap(),
            Request::Entities {
                entity_type: None,
                limit: LISTED_BY_DEFAULT
            }
        );
        assert_eq!(
            parsed(&["entities", "--type", "concept", "--limit", "5"]).unwrap(),
            Request::Entities {
                entity_type: Some("concept".to_string()),
                limit: 5
            }
        );
        assert_eq!(
            parsed(&["show", "реестр", "контрагентов"]).unwrap(),
            Request::Show {
                name: "реестр контрагентов".to_string()
            }
        );
        assert_eq!(
            parsed(&["map", "--json"]).unwrap(),
            Request::Map {
                limit: MAP_MOST_ENTITIES,
                machine: true
            }
        );
    }

    #[test]
    fn what_is_not_the_grammar_is_refused() {
        for wrong in [
            vec!["list"],
            vec!["show"],
            vec!["entities", "--limit", "0"],
            vec!["entities", "--json"],
            vec!["--json"],
            vec!["map", "--limit", "100000"],
            vec!["map", "--depth", "2"],
        ] {
            assert!(parsed(&wrong).is_err(), "{wrong:?} must be refused");
        }
    }
}
