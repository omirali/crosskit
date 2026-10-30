// Stage UI-3 measurement: where the time of one `search --json` goes inside the process.
//
// The panel measures the whole bridge from outside — a process per query, from the query in
// its field to the rows on screen. What it cannot see is how the process spends its share, so
// this does the same steps `search --json` does, in the same order, and times each of them:
// reading the configuration, opening the graph, opening the index, the query, and the line
// that is printed. Each query opens and closes everything again, because the bridge pays
// that on every query and that price is what is being weighed against a daemon.
//
// Real data only, named by two variables; without them it says what to set and does nothing.
// It opens the context read-only and prints counts and milliseconds, never a path or a text.
//
//   CROSSKIT_BRIDGE_CONTEXT=work CROSSKIT_BRIDGE_QUERIES=<file, one query a line> \
//     cargo test --release --offline --lib bridge_measurement -- --ignored --nocapture
use std::time::Instant;

use crate::application::{SearchRequest, SearchService};
use crate::infrastructure::load_index_policy;

use super::machine_answer::{Asked, page_line};
use super::period;
use super::search_index::{self, visible_paths};
use super::workspace;

const STAGES: [&str; 6] = [
    "configuration",
    "graph open",
    "index open",
    "query",
    "json line",
    "close",
];

#[test]
#[ignore = "measurement: prints the Stage UI-3 numbers on a real context"]
fn bridge_measurement() {
    let (Ok(context), Ok(file)) = (
        std::env::var("CROSSKIT_BRIDGE_CONTEXT"),
        std::env::var("CROSSKIT_BRIDGE_QUERIES"),
    ) else {
        eprintln!(
            "set CROSSKIT_BRIDGE_CONTEXT to a configured context and CROSSKIT_BRIDGE_QUERIES \
             to a file of queries, one a line; nothing was measured"
        );
        return;
    };
    let text = std::fs::read_to_string(&file).expect("the queries file reads");
    let queries: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert!(!queries.is_empty(), "the queries file holds no query");

    // The first query once more in front, as a warm-up: the first open after a pause pays for
    // a cold file cache, which the panel in use rarely does.
    let mut samples: Vec<[f64; 6]> = Vec::new();
    let mut rows = 0;
    for (position, query) in std::iter::once(queries[0])
        .chain(queries.iter().copied())
        .enumerate()
    {
        let (stages, found) = one(&context, query);
        if position == 0 {
            // The first call in a process also pays what a process pays once — statics,
            // the engine's own start — which every bridge query pays again.
            let first: Vec<String> = STAGES
                .iter()
                .zip(stages)
                .map(|(name, ms)| format!("{name} {ms:.1}"))
                .collect();
            println!("first call in this process (ms): {}", first.join(" · "));
            continue;
        }
        samples.push(stages);
        rows += found;
    }
    println!(
        "bridge measurement: {} queries on one context, {rows} rows",
        samples.len()
    );
    let totals: Vec<f64> = samples.iter().map(|stages| stages.iter().sum()).collect();
    println!("{}", summary("in-process total", totals));
    for (position, name) in STAGES.iter().enumerate() {
        let values = samples.iter().map(|stages| stages[position]).collect();
        println!("{}", summary(&format!("  {name}"), values));
    }
}

/// One query the way `search --json` answers it, each step timed in milliseconds.
fn one(context: &str, query: &str) -> ([f64; 6], usize) {
    let mut clock = Instant::now();
    let mut lap = || {
        let elapsed = clock.elapsed().as_secs_f64() * 1000.0;
        clock = Instant::now();
        elapsed
    };

    let read = period::read_period(Default::default(), query).expect("the period reads");
    let loaded = load_index_policy().expect("the configuration loads");
    let selected = workspace::choose(&loaded, Some(context)).expect("the context is configured");
    let configuration = lap();
    let manager = workspace::open_for_reading_or_refuse(&selected).expect("the graph opens");
    let graph_open = lap();
    let search =
        search_index::open_or_refuse(&loaded, &selected, &manager).expect("the index opens");
    let index_open = lap();
    let scope = period::gather(
        &search.graph,
        read.period(),
        &visible_paths(&loaded, &selected),
    )
    .expect("the period gathers");
    let request = SearchRequest::new(read.words(), 8, None, false).expect("the request reads");
    let page = SearchService::new(&search.index, &search.graph)
        .execute(&request, scope.as_ref(), &visible_paths(&loaded, &selected))
        .expect("the query answers");
    let query_time = lap();
    let line = page_line(
        &Asked {
            context: &selected,
            query,
            looked_for: read.words(),
            period: read.period(),
            gaps: None,
        },
        &page,
    );
    let json_line = lap();
    let found = page.results().len();
    drop((line, page, scope, search, manager));
    let close = lap();
    (
        [
            configuration,
            graph_open,
            index_open,
            query_time,
            json_line,
            close,
        ],
        found,
    )
}

fn summary(name: &str, mut values: Vec<f64>) -> String {
    values.sort_by(f64::total_cmp);
    let at = |share: f64| {
        let rank = (share * values.len() as f64).ceil() as usize;
        values[rank.clamp(1, values.len()) - 1]
    };
    format!(
        "{name}: p50 {:.1} ms · p95 {:.1} ms · n {}",
        at(0.50),
        at(0.95),
        values.len()
    )
}
