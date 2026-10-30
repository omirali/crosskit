// What the two candidate rules for "the owner named this file" give on the real corpus.
//
// A measurement and not a test: it reads the transcripts of whoever runs it, so it is
// `#[ignore]`d and takes the corpus from the environment rather than from a constant —
// `.claude/rules/00-core.md`, "Ничего личного в исходниках". Nothing here opens a database,
// writes anything or calls a model; the product's own reader and assembly are what produce
// every number below, so what is measured is what the pass would write.
//
//   CROSSKIT_CHATS_ROOT=~/.claude/projects \
//     cargo test --release --offline --lib -- --ignored --nocapture the_two_rules
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::transcript_reader::read_transcript;
use crate::application::conversation_assembly::ConversationAssembly;
use crate::domain::conversation::Conversation;
use crate::domain::conversation::Speaker;
use crate::domain::{MentionRule, bytes_outside_quoted_blocks, file_name_of};

/// Where the transcripts are. The same variable Stage 31's measurement takes its corpus
/// from, because it is the same corpus and a second name for it would be a second thing to
/// keep in step.
const CORPUS_ROOT: &str = "CROSSKIT_CHATS_ROOT";

/// What the index policy of this installation excludes from the `chats` context: the tool's
/// own output and the transcripts of subagents. Named here so the measurement reads the
/// corpus the product reads, rather than 5.9 GB of what was read for it.
const NOT_THE_CONVERSATION: [&str; 2] = ["tool-results", "subagents"];

fn corpus_root() -> Option<PathBuf> {
    std::env::var_os(CORPUS_ROOT).map(PathBuf::from)
}

fn transcripts(root: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skipped = path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| NOT_THE_CONVERSATION.contains(&name));
            if !skipped {
                transcripts(&path, found);
            }
            continue;
        }
        if path.extension().and_then(OsStr::to_str) == Some("jsonl") {
            found.push(path);
        }
    }
}

fn read(path: &Path) -> Option<Conversation> {
    let fallback = path.file_stem()?.to_str()?.to_string();
    let mut assembly = ConversationAssembly::for_session(fallback, u64::MAX);
    read_transcript(path, |line| {
        let _ = assembly.absorb(line);
    })
    .ok()?;
    let read = assembly.finish();
    match read.outgrew_the_budget() {
        true => None,
        false => Some(read.into_conversation()),
    }
}

/// The three candidates, in the order they were considered.
const BY_NAME: &str = "(б) имя файла";
const SPOKEN: &str = "(в) сказанное имя";
const CANDIDATES: [(&str, MentionRule); 3] = [
    ("(а) весь путь", MentionRule::WholePath),
    (BY_NAME, MentionRule::FileName),
    (SPOKEN, MentionRule::SpokenName),
];

/// Whether the owner named a path under one rule, over the owner's replies only.
fn says_the_owner_named_it(conversation: &Conversation, path: &str, rule: MentionRule) -> bool {
    conversation
        .replies()
        .iter()
        .filter(|reply| reply.speaker() == Speaker::Owner)
        .any(|reply| rule.names(path, reply.text()))
}

/// A file name that looks like something a harness made rather than something a person works
/// on: this is what the eye is looking for in the samples, and it is printed as a class
/// rather than asserted, because the choice of rule is a decision and not a test.
fn looks_like_an_artifact(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    name.ends_with(".output")
        || name.ends_with(".tmp")
        || (stem.len() >= 8
            && stem
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
            && stem.chars().any(|character| character.is_ascii_digit())
            && !stem.contains('_'))
}

#[test]
#[ignore = "читает транскрипты владельца: CROSSKIT_CHATS_ROOT=… cargo test --release --offline --lib -- --ignored --nocapture the_two_rules"]
fn the_two_rules_of_naming_measured_on_the_real_corpus() {
    let Some(root) = corpus_root() else {
        println!("{CORPUS_ROOT} не задан: мерить нечего, и выдумывать корпус измерение не будет");
        return;
    };
    let mut files = Vec::new();
    transcripts(&root, &mut files);
    files.sort();

    let mut conversations = 0_usize;
    let mut touch_rows = 0_usize;
    // Verdict of the stage 37 review: how much of what every document calls the owner's words
    // was said by the harness. Bytes, because a share of the corpus is what those documents
    // state and a count of replies would not correct it.
    let mut transcript_bytes = 0_u64;
    let mut reply_bytes = 0_u64;
    let mut owner_bytes = 0_u64;
    let mut owner_bytes_outside_blocks = 0_u64;
    let mut owner_replies = 0_usize;
    let mut owner_replies_with_a_block = 0_usize;
    let mut by_verb: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut by_source: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut paths: BTreeSet<String> = BTreeSet::new();
    let mut with_touches = 0_usize;

    // A pair is one conversation and one path, which is the unit an answer has a row for.
    let mut pairs = 0_usize;
    let mut pairs_written = 0_usize;
    let mut read_only = 0_usize;
    let mut named: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut read_only_named: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut read_only_artifacts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut read_once = 0_usize;
    let mut kept_by_name_alone: BTreeSet<String> = BTreeSet::new();
    let mut kept_by_spoken: BTreeSet<String> = BTreeSet::new();
    // Verdict З: a pair the strict answer keeps whose path is somewhere nobody works — the
    // tool's own scratch directory, or a name that no person typed.
    let mut strict_but_temporary: BTreeSet<String> = BTreeSet::new();

    for file in &files {
        let Some(conversation) = read(file) else {
            continue;
        };
        conversations += 1;
        transcript_bytes += std::fs::metadata(file)
            .map(|data| data.len())
            .unwrap_or_default();
        for reply in conversation.replies() {
            let text = reply.text();
            reply_bytes += text.len() as u64;
            if reply.speaker() != Speaker::Owner {
                continue;
            }
            owner_replies += 1;
            owner_bytes += text.len() as u64;
            let spoken = bytes_outside_quoted_blocks(text) as u64;
            owner_bytes_outside_blocks += spoken;
            if spoken < text.len() as u64 {
                owner_replies_with_a_block += 1;
            }
        }
        if !conversation.touches().is_empty() {
            with_touches += 1;
        }
        let mut of_path: BTreeMap<&str, (bool, u32)> = BTreeMap::new();
        for touch in conversation.touches() {
            touch_rows += 1;
            *by_verb.entry(touch.verb().as_str()).or_default() += 1;
            *by_source.entry(touch.source().as_str()).or_default() += 1;
            paths.insert(touch.path().to_string());
            let entry = of_path.entry(touch.path()).or_insert((false, 0));
            entry.0 |= touch.verb().is_deliberate();
            entry.1 = entry.1.max(touch.times());
        }
        for (path, (written, times)) in of_path {
            pairs += 1;
            let verdicts: Vec<(&'static str, bool)> = CANDIDATES
                .iter()
                .map(|(label, rule)| (*label, says_the_owner_named_it(&conversation, path, *rule)))
                .collect();
            for (label, yes) in &verdicts {
                *named.entry(label).or_default() += usize::from(*yes);
            }
            let by_spoken_rule = verdicts.iter().any(|(label, yes)| *label == SPOKEN && *yes);
            if (written || by_spoken_rule)
                && (path.contains("/tmp/")
                    || path.contains("/var/folders/")
                    || file_name_of(path).is_some_and(looks_like_an_artifact))
            {
                strict_but_temporary.insert(path.to_string());
            }
            if written {
                pairs_written += 1;
                continue;
            }
            read_only += 1;
            read_once += usize::from(times <= 1);
            let artifact = file_name_of(path).is_some_and(looks_like_an_artifact);
            for (label, yes) in &verdicts {
                if !yes {
                    continue;
                }
                *read_only_named.entry(label).or_default() += 1;
                if artifact {
                    *read_only_artifacts.entry(label).or_default() += 1;
                }
            }
            let by_name = verdicts
                .iter()
                .any(|(label, yes)| *label == BY_NAME && *yes);
            let by_spoken = verdicts.iter().any(|(label, yes)| *label == SPOKEN && *yes);
            if let Some(name) = file_name_of(path) {
                if by_name && !by_spoken {
                    kept_by_name_alone.insert(name.to_string());
                }
                if by_spoken {
                    kept_by_spoken.insert(name.to_string());
                }
            }
        }
    }

    println!("А. ЧТО В КОРПУСЕ");
    println!("  транскриптов прочитано       {}", files.len());
    println!("  бесед собрано                {conversations}");
    println!("  бесед со ссылками            {with_touches}");
    println!("  строк FileTouch              {touch_rows}");
    println!("  различных путей              {}", paths.len());
    for (verb, count) in &by_verb {
        println!("  глагол {verb:<22} {count}");
    }
    for (source, count) in &by_source {
        println!("  источник {source:<20} {count}");
    }
    println!();

    println!("Ж. СКОЛЬКО ИЗ «СЛОВ ВЛАДЕЛЬЦА» СКАЗАЛА ОБВЯЗКА");
    println!("  байт транскриптов              {transcript_bytes}");
    println!(
        "  байт реплик                    {reply_bytes} ({:.2} % корпуса)",
        percent64(reply_bytes, transcript_bytes)
    );
    println!(
        "  из них владельца               {owner_bytes} ({:.2} МиБ) в {owner_replies} репликах",
        mib(owner_bytes)
    );
    println!(
        "  из них вне закрытых блоков     {owner_bytes_outside_blocks} ({:.2} МиБ)",
        mib(owner_bytes_outside_blocks)
    );
    println!(
        "  сказала обвязка                {} ({:.0} % слов владельца)",
        owner_bytes - owner_bytes_outside_blocks,
        percent64(owner_bytes - owner_bytes_outside_blocks, owner_bytes)
    );
    println!(
        "  реплик владельца с блоком      {owner_replies_with_a_block} ({:.0} %)",
        percent(owner_replies_with_a_block, owner_replies)
    );
    println!(
        "  доля корпуса без этих блоков   {:.2} % против {:.2} %",
        percent64(
            reply_bytes - (owner_bytes - owner_bytes_outside_blocks),
            transcript_bytes
        ),
        percent64(reply_bytes, transcript_bytes)
    );
    println!();

    println!("Б. КАНДИДАТЫ НА «ВЛАДЕЛЕЦ НАЗВАЛ ФАЙЛ» (пар «беседа + файл»: {pairs})");
    println!("  правило                      пар   из них только читали   из них артефакты");
    for (label, _) in CANDIDATES {
        println!(
            "  {label:<26} {:>5} {:>21} {:>18}",
            named.get(label).copied().unwrap_or_default(),
            read_only_named.get(label).copied().unwrap_or_default(),
            read_only_artifacts.get(label).copied().unwrap_or_default(),
        );
    }
    let lost: Vec<&String> = kept_by_name_alone
        .iter()
        .filter(|name| !looks_like_an_artifact(name))
        .collect();
    println!(
        "  имён, которые держит только «имя файла»: {} (из них похожи на артефакт {})",
        kept_by_name_alone.len(),
        kept_by_name_alone.len() - lost.len()
    );
    println!("  то, что «сказанное имя» на этом теряет — глазами:");
    for name in lost {
        println!("      {name}");
    }
    println!(
        "  имён, которые держит «сказанное имя» (все, глазами): {}",
        kept_by_spoken.len()
    );
    for name in &kept_by_spoken {
        println!("      {name}");
    }
    println!();

    println!("З. ЧТО ПОПАЛО В СТРОГИЙ ВИД И НЕ ДОЛЖНО БЫЛО (глазами)");
    println!(
        "  пар в строгом виде, чей путь — временный каталог или артефакт: {}",
        strict_but_temporary.len()
    );
    for path in strict_but_temporary.iter().take(12) {
        println!("      {path}");
    }
    println!();

    println!("В. СТРОГИЙ ВИД ПРОТИВ ПОЛНОГО (правило продукта — «сказанное имя»)");
    let read_only_named_by_name = read_only_named.get(SPOKEN).copied().unwrap_or_default();
    let strict = pairs_written + read_only_named_by_name;
    println!("  пар всего                             {pairs}");
    println!("  писали или правили                    {pairs_written}");
    println!("  только читали                         {read_only}");
    println!("  из них владелец назвал (правило (б))  {read_only_named_by_name}");
    println!("  из них прочитано ровно один раз       {read_once}");
    println!(
        "  входит в умолчание                    {strict} ({:.0} %)",
        percent(strict, pairs)
    );
    println!(
        "  остаётся за флагом                    {} ({:.0} %)",
        pairs - strict,
        percent(pairs - strict, pairs)
    );
}

fn percent(part: usize, whole: usize) -> f64 {
    match whole {
        0 => 0.0,
        whole => part as f64 * 100.0 / whole as f64,
    }
}

fn percent64(part: u64, whole: u64) -> f64 {
    match whole {
        0 => 0.0,
        whole => part as f64 * 100.0 / whole as f64,
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
