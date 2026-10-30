// Stage 40 measurement: how a page is held to a period — candidates Б and В of ADR 0043, side
// by side on one index, and the number that decides against А.
//
//   Б  the period first: the index folds only what the period admits (what the product does);
//   В  the search as before, and rows outside the period thrown away afterwards, round after
//      round until the page is full or the matches run out.
//   А  a moment in every index document — not built: its cost is counted, not timed (below).
//
// Two corpora. Without variables, a synthetic context of conversations over eight weeks is
// written and indexed into a temporary directory. With `CROSSKIT_DATA_DIR` and
// `CROSSKIT_PERIOD_CONTEXT`, an existing context is opened read-only — its graph and its
// index are read and nothing is written. Paths and texts are never printed: the measurement
// prints counts and milliseconds only.
//
//   cargo test --release --offline --lib period_measurement -- --ignored --nocapture
//   CROSSKIT_DATA_DIR=… CROSSKIT_PERIOD_CONTEXT=work \
//     cargo test --release --offline --lib period_measurement -- --ignored --nocapture
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use super::ladybug::LadybugDatabaseManager;
use super::period_pipeline_tests::iso;
use super::search::{ContextSearchIndex, built_in_normalizations, search_directory};
use super::search_pipeline_tests::context;
use crate::application::{PeriodScope, SearchIndexReader};
use crate::domain::{CivilDate, LocalDay, Period, PeriodPhrase, WorkspaceName};
use crate::infrastructure::{LadybugSearchGraph, local_today};

const PAGE: usize = 10;
const REPEATS: usize = 5;

/// Words with nothing of anybody in them, in both languages of the owner's tree.
const QUERIES: [&str; 8] = [
    "ошибка",
    "тест",
    "индекс",
    "сверка",
    "error",
    "config",
    "deploy",
    "api",
];

struct Corpus {
    graph: LadybugSearchGraph,
    index: ContextSearchIndex,
    today: LocalDay,
    _keep: Option<Box<dyn std::any::Any>>,
}

#[test]
#[ignore = "measurement: prints the Stage 40 numbers"]
fn period_measurement() {
    let corpus = match (
        std::env::var_os("CROSSKIT_DATA_DIR"),
        std::env::var("CROSSKIT_PERIOD_CONTEXT"),
    ) {
        (Some(data), Ok(name)) => real(PathBuf::from(data), &name),
        (None, Err(_)) => {
            println!(
                "corpus: synthetic (set CROSSKIT_DATA_DIR and CROSSKIT_PERIOD_CONTEXT for a real context)"
            );
            synthetic()
        }
        _ => panic!("set both CROSSKIT_DATA_DIR and CROSSKIT_PERIOD_CONTEXT, or neither"),
    };
    let documents = corpus.index.state().unwrap().documents();
    println!("index documents: {documents}");

    println!(
        "{:<16} {:>6} {:>7} {:>8} | {:>7} {:>7} | {:>7} {:>7} {:>6} {:>6} | {:>6} {:>6}",
        "period",
        "files",
        "chunks",
        "gather",
        "Б ms",
        "Б rows",
        "В ms",
        "В rows",
        "rounds",
        "1st",
        "all",
        "В lost"
    );
    for (label, period) in periods(corpus.today) {
        measure(&corpus, label, period);
    }
}

fn periods(today: LocalDay) -> Vec<(String, Period)> {
    let mut periods = vec![
        (
            "this week".to_string(),
            Period::of_phrase(PeriodPhrase::ThisWeek, today),
        ),
        (
            "last week".to_string(),
            Period::of_phrase(PeriodPhrase::LastWeek, today),
        ),
    ];
    let monday = Period::of_phrase(PeriodPhrase::LastWeek, today).first_day();
    for back in 1..=4 {
        let first = monday.plus_days(-7 * back);
        let period = Period::of_flags(Some(first), Some(first.plus_days(6)), today, None).unwrap();
        periods.push((format!("week -{}", back + 1), period));
    }
    periods.push((
        "last month".to_string(),
        Period::of_phrase(PeriodPhrase::LastMonth, today),
    ));
    periods
}

fn measure(corpus: &Corpus, label: String, period: Period) {
    let started = Instant::now();
    let scope = PeriodScope::gather(&corpus.graph, period, &|_| true).unwrap();
    let gather_ms = started.elapsed().as_secs_f64() * 1000.0;
    let whole = scope.whole_files().count();
    let chunks = scope.single_chunks().count();

    let (mut scoped_ms, mut scoped_rows, mut filtered_ms, mut filtered_rows) = (0.0, 0, 0.0, 0);
    let (mut rounds, mut first_screen, mut everything, mut lost) = (0, 0, 0, 0);
    for query in QUERIES {
        let (ms, rows) = timed(|| scoped_page(corpus, query, &scope));
        scoped_ms += ms;
        scoped_rows += rows;
        let (ms, (rows, asked, first)) = timed(|| filtered_page(corpus, query, &scope));
        filtered_ms += ms;
        filtered_rows += rows;
        rounds += asked;
        first_screen += first;
        let (all, missed) = completeness(corpus, query, &scope);
        everything += all;
        lost += missed;
    }
    let queries = QUERIES.len() as f64;
    println!(
        "{:<16} {:>6} {:>7} {:>8.1} | {:>7.2} {:>7} | {:>7.2} {:>7} {:>6} {:>6} | {:>6} {:>6}",
        format!("{label} {period}"),
        whole,
        chunks,
        gather_ms,
        scoped_ms / queries,
        scoped_rows,
        filtered_ms / queries,
        filtered_rows,
        rounds,
        first_screen,
        everything,
        lost
    );
}

/// The median of `REPEATS` runs, with what the last one returned.
fn timed<T>(mut run: impl FnMut() -> T) -> (f64, T) {
    let mut times = Vec::with_capacity(REPEATS);
    let mut last = None;
    for _ in 0..REPEATS {
        let started = Instant::now();
        last = Some(run());
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    (times[REPEATS / 2], last.unwrap())
}

/// Б: one page, folded inside the period.
fn scoped_page(corpus: &Corpus, query: &str, scope: &PeriodScope) -> usize {
    corpus
        .index
        .matches(query, PAGE, None, Some(scope))
        .unwrap()
        .into_documents()
        .len()
}

/// В: rounds of the ordinary search, each row kept only when the chunk it is shown by is
/// inside the period. Returns the rows kept, the rounds asked for, and how many rows of the
/// first round — the screen a person would have seen — were inside.
fn filtered_page(corpus: &Corpus, query: &str, scope: &PeriodScope) -> (usize, usize, usize) {
    let (mut kept, mut rounds, mut first) = (0, 0, 0);
    let mut cursor = None;
    while kept < PAGE {
        let rows = corpus
            .index
            .matches(query, PAGE, cursor.as_ref(), None)
            .unwrap()
            .into_documents();
        let Some(last) = rows.last() else {
            break;
        };
        cursor = Some(last.cursor().clone());
        rounds += 1;
        let inside = rows
            .iter()
            .filter(|row| scope.admits(row.chunk_id(), row.cursor().file_id()))
            .count();
        if rounds == 1 {
            first = inside;
        }
        kept += inside;
    }
    (kept.min(PAGE), rounds, first)
}

/// Every file with a matching chunk inside the period (Б walked to the end), and how many of
/// them В never shows by a chunk from inside: its best chunk is from another week, so В either
/// drops the file or would show the wrong reply.
fn completeness(corpus: &Corpus, query: &str, scope: &PeriodScope) -> (usize, usize) {
    let all = walk(corpus, query, Some(scope));
    let inside: HashSet<String> = all.iter().map(|(file, _)| file.clone()).collect();
    let shown_inside: HashSet<String> = walk(corpus, query, None)
        .into_iter()
        .filter(|(file, chunk)| scope.admits(chunk, file))
        .map(|(file, _)| file)
        .collect();
    (inside.len(), inside.difference(&shown_inside).count())
}

fn walk(corpus: &Corpus, query: &str, scope: Option<&PeriodScope>) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut cursor = None;
    loop {
        let page = corpus
            .index
            .matches(query, 100, cursor.as_ref(), scope)
            .unwrap()
            .into_documents();
        let Some(last) = page.last() else {
            return rows;
        };
        cursor = Some(last.cursor().clone());
        rows.extend(page.iter().map(|row| {
            (
                row.cursor().file_id().to_string(),
                row.chunk_id().to_string(),
            )
        }));
    }
}

fn real(data: PathBuf, name: &str) -> Corpus {
    let workspace = WorkspaceName::parse(name).unwrap();
    let manager: Arc<LadybugDatabaseManager> =
        LadybugDatabaseManager::open_workspace_read_only(&workspace).unwrap();
    let index = ContextSearchIndex::open(
        &search_directory(&data, &workspace),
        &built_in_normalizations(),
    )
    .unwrap();
    println!("corpus: a real context, opened read-only");
    Corpus {
        graph: LadybugSearchGraph::new(manager),
        index,
        today: local_today().unwrap(),
        _keep: None,
    }
}

/// Two hundred conversations over eight weeks before a fixed Thursday, each of forty replies
/// spread over up to three weeks and writing two of four hundred files.
fn synthetic() -> Corpus {
    let today = LocalDay::new(CivilDate::new(2026, 10, 1).unwrap(), 5 * 3600);
    let thursday_ms = CivilDate::new(2026, 10, 1).unwrap().days() * 86_400_000;
    let built = context("period-measurement");
    let words = [
        "ошибка",
        "тест",
        "индекс",
        "сверка",
        "error",
        "config",
        "deploy",
        "api",
        "склад",
        "отчёт",
        "очередь",
        "schema",
    ];
    for file in 0..400 {
        built.write(
            &format!("file-{file:03}.md"),
            &format!(
                "{} {} заметка {file}",
                words[file % 12],
                words[(file / 12) % 12]
            ),
        );
    }
    let mut seed: u64 = 40;
    let mut next = move |bound: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) % bound
    };
    for chat in 0..200 {
        let start = thursday_ms - (next(56) as i64 + 1) * 86_400_000;
        let span = (next(21) as i64 + 1) * 86_400_000;
        let mut lines = Vec::new();
        for reply in 0..40 {
            let at = start + span * reply / 40;
            let text = format!(
                "{} {} реплика {reply}",
                words[next(12) as usize],
                words[next(12) as usize]
            );
            lines.push(format!(
                r#"{{"type":"user","sessionId":"c{chat}","timestamp":"{}","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#,
                iso(at)
            ));
            if reply % 20 == 10 {
                lines.push(format!(
                    r#"{{"type":"assistant","sessionId":"c{chat}","timestamp":"{}","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Edit","input":{{"file_path":"{}"}}}}]}}}}"#,
                    iso(at),
                    built.at(&format!("file-{:03}.md", next(400)))
                ));
            }
        }
        built.write(&format!("chat-{chat:03}.jsonl"), &(lines.join("\n") + "\n"));
    }
    let started = Instant::now();
    built.init();
    println!(
        "synthetic corpus indexed in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    let graph = LadybugSearchGraph::new(Arc::clone(&built.manager));
    let index = ContextSearchIndex::open(&built.directory, &built_in_normalizations()).unwrap();
    Corpus {
        graph,
        index,
        today,
        _keep: Some(Box::new(built)),
    }
}

/// Stage 40, P1: every current conversation of a context, sorted by what placing its chunks
/// met. Counts only — no path, no name, no text.
#[test]
#[ignore = "measurement: prints the Stage 40 census of unplaced conversations"]
fn period_unplaced_census() {
    use crate::application::PeriodEventReader;
    let Ok(name) = std::env::var("CROSSKIT_PERIOD_CONTEXT") else {
        println!("set CROSSKIT_DATA_DIR and CROSSKIT_PERIOD_CONTEXT");
        return;
    };
    let corpus = real(PathBuf::new(), &name);
    let all = corpus
        .graph
        .conversations_speaking_between(0, i64::MAX)
        .unwrap();
    println!("conversations with a recorded reply: {}", all.len());
    let (mut chunks, mut replies, mut without_moment) = (0, 0, 0);
    let mut unplaced_conversations = 0;
    let mut unplaced: std::collections::BTreeMap<crate::domain::Unplaceable, usize> =
        std::collections::BTreeMap::new();
    for timeline in &all {
        let census = timeline.census();
        chunks += census.chunks;
        replies += census.replies;
        without_moment += census.replies_without_moment;
        if !census.unplaced.is_empty() {
            unplaced_conversations += 1;
        }
        for (why, count) in census.unplaced {
            *unplaced.entry(why).or_default() += count;
        }
    }
    println!(
        "chunks {chunks}, replies {replies}, replies without a moment {without_moment}, \
         conversations with unplaced chunks {unplaced_conversations}, unplaced by reason {unplaced:?}"
    );
    let behind = corpus.graph.transcripts_behind_since(0).unwrap().len();
    println!("transcripts on disk newer than the graph's copy: {behind}");
}

/// Stage 40, P1: one transcript read by the product's own reader, with the policy's budget
/// and without one. Counts by local week only.
#[test]
#[ignore = "measurement: reads one transcript named by CROSSKIT_PERIOD_TRANSCRIPT"]
fn period_transcript_read() {
    use crate::application::conversation_assembly::ConversationAssembly;
    use std::collections::BTreeMap;
    let Some(path) = std::env::var_os("CROSSKIT_PERIOD_TRANSCRIPT") else {
        println!("set CROSSKIT_PERIOD_TRANSCRIPT");
        return;
    };
    let word = std::env::var("CROSSKIT_PERIOD_WORD").unwrap_or_default();
    for (label, budget) in [("16 MiB", 16 * 1024 * 1024u64), ("unlimited", u64::MAX)] {
        let mut assembly = ConversationAssembly::for_session("probe", budget);
        let file = std::fs::File::open(&path).unwrap();
        super::chats::transcript_reader::read_transcript_from(
            std::io::BufReader::new(file),
            |line| assembly.absorb(line),
        )
        .unwrap();
        let read = assembly.finish();
        let replies = read.conversation().replies();
        let mut weeks: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut without = 0;
        for reply in replies {
            let Some(at) = reply.at_unix_ms() else {
                without += 1;
                continue;
            };
            let day = CivilDate::from_days((at / 1000 + 5 * 3600).div_euclid(86_400));
            let entry = weeks
                .entry(day.monday_of_its_week().to_string())
                .or_default();
            entry.0 += 1;
            if !word.is_empty() && reply.text().to_lowercase().contains(&word) {
                entry.1 += 1;
            }
        }
        println!(
            "{label}: outgrew {}, replies {}, spoken bytes {}, without moment {without}",
            read.outgrew_the_budget(),
            replies.len(),
            replies
                .iter()
                .map(|reply| reply.text().len())
                .sum::<usize>()
        );
        for (monday, (count, with_word)) in weeks {
            println!("  week of {monday}: {count} replies, {with_word} with the word");
        }
    }
}
