// What Stage 32 actually wrote into the owner's `chats` graph.
//
// A measurement, not a test: it goes to the real database on this machine, so it is
// `#[ignore]`d like every other measurement here and runs on its own.
//
//   cargo test --release --offline --test conversation_facts -- --ignored --nocapture
//
// Read-only and outside any transaction. Nothing here writes, and nothing here opens a second
// context: every number is one context's own, which is what ADR 0008 makes physical.
use std::path::PathBuf;

use lbug::{Connection, Database, SystemConfig, Value};

/// The context this stage changed. Named here because the numbers below are about it.
const CONTEXT: &str = "chats";

/// Where this installation keeps its graphs. `CROSSKIT_DATA_DIR` is the product's own
/// variable, honoured here so the measurement can be pointed at a graph built for it —
/// Stage 37 read a `chats` graph it had just written, rather than migrating the owner's.
fn graph_path(context: &str) -> PathBuf {
    let directory = match std::env::var_os("CROSSKIT_DATA_DIR") {
        Some(directory) => PathBuf::from(directory),
        None => PathBuf::from(std::env::var_os("HOME").expect("HOME"))
            .join("Library/Application Support/Crosskit"),
    };
    directory.join(format!("graph-{context}.lbug"))
}

fn rows(connection: &Connection<'_>, query: &str) -> Vec<Vec<String>> {
    let result = connection
        .query(query)
        .unwrap_or_else(|error| panic!("query {query:?}: {error}"));
    let mut all = Vec::new();
    for values in result {
        all.push(
            values
                .iter()
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    Value::Int64(number) => number.to_string(),
                    other => format!("{other:?}"),
                })
                .collect(),
        );
    }
    all
}

fn one(connection: &Connection<'_>, query: &str) -> String {
    rows(connection, query)
        .first()
        .and_then(|row| row.first().cloned())
        .unwrap_or_else(|| "—".to_string())
}

#[test]
#[ignore = "факты бесед на настоящем дереве: cargo test --release --offline --test conversation_facts -- --ignored --nocapture"]
fn what_stage_32_wrote_about_the_owners_conversations() {
    let path = graph_path(CONTEXT);
    if !path.exists() {
        println!("Контекста {CONTEXT} на этой машине нет: {}", path.display());
        return;
    }
    let database = Database::new(&path, SystemConfig::default()).expect("открыть граф");
    let connection = Connection::new(&database).expect("соединение");

    println!("А. ЧТО ЗАПИСАНО");
    println!(
        "  бесед                        {}",
        one(&connection, "MATCH (c:Conversation) RETURN COUNT(c);")
    );
    println!(
        "  реплик                       {}",
        one(&connection, "MATCH (m:Message) RETURN COUNT(m);")
    );
    println!(
        "  ссылок на файлы              {}",
        one(&connection, "MATCH (t:FileTouch) RETURN COUNT(t);")
    );
    println!(
        "  чанков (реплики + имена)     {}",
        one(
            &connection,
            "MATCH (c:ContentChunk) WHERE c.extractor_id = 'conversation' RETURN COUNT(c);"
        )
    );
    println!(
        "  чанков, прочитанных текстом  {}",
        one(
            &connection,
            "MATCH (c:ContentChunk) WHERE c.extractor_id = 'plain-text' RETURN COUNT(c);"
        )
    );
    println!(
        "  реплик владельца             {}",
        one(
            &connection,
            "MATCH (m:Message) WHERE m.role = 'owner' RETURN COUNT(m);"
        )
    );
    println!(
        "  реплик модели                {}",
        one(
            &connection,
            "MATCH (m:Message) WHERE m.role = 'model' RETURN COUNT(m);"
        )
    );
    println!();

    println!("Б. ИМЯ БЕСЕДЫ");
    for named_by in ["owner", "model", "nobody"] {
        println!(
            "  {named_by:<28} {}",
            one(
                &connection,
                &format!("MATCH (c:Conversation) WHERE c.named_by = '{named_by}' RETURN COUNT(c);")
            )
        );
    }
    println!();

    println!("В. ССЫЛКИ НА ФАЙЛЫ — по глаголу и источнику");
    for source in ["tool_input", "file_history", "command"] {
        for verb in ["read", "wrote", "edited"] {
            let count = one(
                &connection,
                &format!(
                    "MATCH (t:FileTouch) WHERE t.source = '{source}' AND t.verb = '{verb}' RETURN COUNT(t);"
                ),
            );
            if count != "0" {
                println!("  {source:<14} {verb:<8} {count}");
            }
        }
    }
    println!(
        "  различных путей              {}",
        one(
            &connection,
            "MATCH (t:FileTouch) RETURN COUNT(DISTINCT t.path);"
        )
    );
    println!(
        "  бесед со ссылками            {}",
        one(
            &connection,
            "MATCH (c:Conversation)-[:TOUCHED_FILE]->(:FileTouch) RETURN COUNT(DISTINCT c.id);"
        )
    );
    println!();

    println!("Г. КУДА ВЕДУТ ПУТИ");
    let paths: Vec<String> = rows(
        &connection,
        "MATCH (t:FileTouch) RETURN DISTINCT t.path ORDER BY t.path;",
    )
    .into_iter()
    .filter_map(|row| row.into_iter().next())
    .collect();
    // The roots of the owner's configured contexts, read from the configuration rather than
    // written down here, so the split cannot drift from what the installation is set up as.
    let configuration = std::fs::read_to_string(
        PathBuf::from(std::env::var_os("HOME").expect("HOME"))
            .join("Library/Application Support/Crosskit/config.toml"),
    )
    .unwrap_or_default();
    let mut contexts: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Option<String> = None;
    for line in configuration.lines() {
        let line = line.trim();
        if let Some(name) = line
            .strip_prefix("[workspace.")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            current = Some(name.to_string());
            contexts.push((name.to_string(), Vec::new()));
            continue;
        }
        if current.is_some()
            && let Some(start) = line.find('"')
            && let Some(end) = line.rfind('"')
            && end > start
            && let Some((_, roots)) = contexts.last_mut()
        {
            roots.push(line[start + 1..end].to_string());
        }
    }
    let mut nowhere = 0;
    let mut counted: Vec<(String, usize)> = contexts
        .iter()
        .map(|(name, _)| (name.clone(), 0usize))
        .collect();
    for path in &paths {
        match contexts
            .iter()
            .position(|(_, roots)| roots.iter().any(|root| path.starts_with(root.as_str())))
        {
            Some(index) => counted[index].1 += 1,
            None => nowhere += 1,
        }
    }
    for (name, count) in &counted {
        println!("  {name:<28} {count}");
    }
    println!("  ни в одном контексте         {nowhere}");
    println!(
        "  доля «никуда»                {:.0} %",
        nowhere as f64 * 100.0 / paths.len().max(1) as f64
    );
    println!();

    println!("Е. САМАЯ КРУПНАЯ БЕСЕДА");
    let largest = rows(
        &connection,
        "MATCH (c:Conversation) RETURN c.name, c.message_count, c.session_id ORDER BY c.message_count DESC LIMIT 3;",
    );
    for row in largest {
        println!("  {:<44} {} реплик", row[0], row[1]);
    }
    println!();

    println!("Л. «В КАКОМ ЧАТЕ ТРОГАЛИ ЭТОТ ФАЙЛ»");
    let shared = rows(
        &connection,
        "MATCH (c:Conversation)-[:TOUCHED_FILE]->(t:FileTouch) WITH t.path AS path, COUNT(DISTINCT c.id) AS conversations WHERE conversations > 1 RETURN path, conversations ORDER BY conversations DESC, path LIMIT 5;",
    );
    println!("  файлы, которых касалась больше чем одна беседа:");
    for row in &shared {
        println!("    {:<60} {} бесед", row[0], row[1]);
    }
    if shared.is_empty() {
        println!("    — таких нет");
    }
    println!();

    println!("Е. ОТМЕТКИ ЭТАПА 37, КАК ОНИ ЛЕГЛИ В ГРАФ");
    for naming in ["named", "not_named", "not_computed"] {
        println!(
            "  naming = {naming:<20} {}",
            one(
                &connection,
                &format!("MATCH (t:FileTouch) WHERE t.naming = '{naming}' RETURN COUNT(t);")
            )
        );
    }
    println!(
        "  строк, прочитанных больше раза {}",
        one(
            &connection,
            "MATCH (t:FileTouch) WHERE t.times > 1 RETURN COUNT(t);"
        )
    );
    println!(
        "  наибольшее число повторов      {}",
        one(&connection, "MATCH (t:FileTouch) RETURN MAX(t.times);")
    );
    println!(
        "  пар «беседа + файл»            {}",
        one(
            &connection,
            "MATCH (c:Conversation)-[:TOUCHED_FILE]->(t:FileTouch) RETURN COUNT(DISTINCT c.session_id + '|' + t.path);"
        )
    );
    println!(
        "  из них в строгом виде          {}",
        one(
            &connection,
            "MATCH (c:Conversation)-[:TOUCHED_FILE]->(t:FileTouch) WHERE t.verb <> 'read' OR t.naming = 'named' RETURN COUNT(DISTINCT c.session_id + '|' + t.path);"
        )
    );
}

/// Verdict Д: what the second source would be worth, measured and not built.
///
/// Section 5 of the architecture puts a path scraped from a command line on the
/// `observation only` rung, and this is the measurement behind that ruling. Two things are
/// counted: how many path-like words the commands hold, and how many of them name something
/// that exists. Nothing is written to the graph — the point of the number is to say whether
/// writing it would be worth anything.
#[test]
#[ignore = "разрешаемость путей из команд Bash: cargo test --release --offline --test conversation_facts -- --ignored --nocapture"]
fn how_many_paths_in_a_shell_command_are_paths_at_all() {
    let home = std::env::var_os("HOME").expect("HOME");
    let root = PathBuf::from(&home).join(".claude/projects");
    if !root.exists() {
        println!("Корпуса на этой машине нет: {}", root.display());
        return;
    }

    let mut commands = 0u64;
    let mut with_something_path_like = 0u64;
    let mut candidates: Vec<String> = Vec::new();
    walk(&root, &mut |path| {
        if path.extension().is_none_or(|suffix| suffix != "jsonl") {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let blocks = value
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(|content| content.as_array());
            let Some(blocks) = blocks else { continue };
            for block in blocks {
                if block.get("type").and_then(|kind| kind.as_str()) != Some("tool_use") {
                    continue;
                }
                if block.get("name").and_then(|name| name.as_str()) != Some("Bash") {
                    continue;
                }
                let Some(command) = block
                    .get("input")
                    .and_then(|input| input.get("command"))
                    .and_then(|command| command.as_str())
                else {
                    continue;
                };
                commands += 1;
                let found = path_like_words(command);
                if !found.is_empty() {
                    with_something_path_like += 1;
                }
                candidates.extend(found);
            }
        }
    });

    candidates.sort();
    candidates.dedup();
    let existing = candidates
        .iter()
        .filter(|candidate| std::path::Path::new(candidate).exists())
        .count();

    println!("Д. ПУТИ ИЗ КОМАНД BASH — измерено, не реализовано");
    println!("  команд                       {commands}");
    println!(
        "  из них с чем-то похожим      {with_something_path_like} ({:.0} %)",
        with_something_path_like as f64 * 100.0 / commands.max(1) as f64
    );
    println!("  различных кандидатов         {}", candidates.len());
    println!(
        "  из них существуют            {existing} ({:.0} %)",
        existing as f64 * 100.0 / candidates.len().max(1) as f64
    );
    println!(
        "\n  Глагол при этом неизвестен в любом случае: `cat` читает, `sed -i` пишет, `rg` не\n\
         \x20 делает ни того ни другого. Раздел 5 архитектуры называет такой путь observation\n\
         \x20 only, и этот этап его не производит."
    );
}

/// Words in a shell command that look like a path: absolute, or relative with a separator.
/// Deliberately generous — the question is how many of the generous guesses survive contact
/// with the disk.
fn path_like_words(command: &str) -> Vec<String> {
    command
        .split(|character: char| character.is_whitespace() || "\"'`;|&()<>".contains(character))
        .filter(|word| word.contains('/') && word.len() > 2 && !word.contains("://"))
        .map(|word| {
            let trimmed = word.trim_matches(|character| ",:".contains(character));
            match trimmed.strip_prefix("~/") {
                Some(rest) => PathBuf::from(std::env::var_os("HOME").expect("HOME"))
                    .join(rest)
                    .to_string_lossy()
                    .into_owned(),
                None => trimmed.to_string(),
            }
        })
        .filter(|word| word.starts_with('/'))
        .collect()
}

fn walk(directory: &std::path::Path, visit: &mut impl FnMut(&std::path::Path)) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            let name = entry.file_name();
            if name == "tool-results" || name == "subagents" {
                continue;
            }
            walk(&path, visit);
            continue;
        }
        visit(&path);
    }
}
