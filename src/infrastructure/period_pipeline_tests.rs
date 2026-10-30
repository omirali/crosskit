// End to end, with a real graph, a real scan and a real index: what a period admits.
//
// The rules of a period — its days, which chunk is which reply, which touch is deliberate —
// are held by unit tests with no storage in the way. What is here needs the storage: that the
// graph hands back the moments the transcripts recorded, that a touch reaches the file at its
// path, and that the index ranks inside the scope and nothing outside it reaches a page.
use std::time::{SystemTime, UNIX_EPOCH};

use super::search_pipeline_tests::{Context, context};
use crate::application::{PeriodEvent, PeriodScope, SearchRequest, SearchService};
use crate::domain::conversation::TouchVerb;
use crate::domain::{CivilDate, LocalDay, Period, PeriodPhrase};
use crate::infrastructure::local_today;

/// Thursday 1 October 2026 at UTC+05:00. Last week is 21–27 September.
fn thursday() -> LocalDay {
    LocalDay::new(CivilDate::new(2026, 10, 1).unwrap(), 5 * 3600)
}

/// Noon, local time, on a day of September 2026, in milliseconds.
fn september_noon(day: u8) -> i64 {
    (CivilDate::new(2026, 9, day).unwrap().days() * 86_400 + 7 * 3600) * 1000
}

/// A moment as a transcript writes it: `2026-09-23T07:00:00.000Z`.
pub(super) fn iso(unix_ms: i64) -> String {
    let seconds = unix_ms.div_euclid(1000);
    let date = CivilDate::from_days(seconds.div_euclid(86_400));
    let second = seconds.rem_euclid(86_400);
    format!(
        "{date}T{:02}:{:02}:{:02}.000Z",
        second / 3600,
        (second % 3600) / 60,
        second % 60
    )
}

fn said(session: &str, at: i64, text: &str) -> String {
    format!(
        r#"{{"type":"user","sessionId":"{session}","timestamp":"{}","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#,
        iso(at)
    )
}

fn wrote(session: &str, at: i64, path: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{session}","timestamp":"{}","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Write","input":{{"file_path":"{path}"}}}}]}}}}"#,
        iso(at)
    )
}

fn transcript(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Every path of a page asked within `scope`, and the event of each.
fn page(
    context: &Context,
    query: &str,
    scope: Option<&PeriodScope>,
) -> Vec<(String, Option<PeriodEvent>, String)> {
    let request = SearchRequest::new(query, 20, None, false).unwrap();
    SearchService::new(&context.index, &context.graph)
        .execute(&request, scope, &|_| true)
        .unwrap()
        .results()
        .iter()
        .map(|result| {
            (
                result
                    .path()
                    .as_str()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_string(),
                result.event(),
                result.fragment().text().to_string(),
            )
        })
        .collect()
}

fn names(rows: &[(String, Option<PeriodEvent>, String)]) -> Vec<&str> {
    let mut names: Vec<&str> = rows.iter().map(|(name, _, _)| name.as_str()).collect();
    names.sort_unstable();
    names
}

/// Conversations in three weeks, and files written in two of them: the page of last week
/// holds last week and nothing else — and of a conversation that ran over two weeks, the reply
/// of the right one.
#[test]
fn a_page_within_a_period_holds_only_that_period() {
    let context = context("period-three-weeks");
    let notes = context.at("notes.md");
    let older = context.at("older.md");
    context.write("notes.md", "сверка остатков по складу");
    context.write("older.md", "сверка прошлого квартала");
    context.write(
        "week-38.jsonl",
        &transcript(&[
            said("w38", september_noon(15), "сверка пятнадцатого"),
            wrote("w38", september_noon(15), &older),
        ]),
    );
    context.write(
        "week-39.jsonl",
        &transcript(&[
            said("w39", september_noon(23), "сверка двадцать третьего"),
            wrote("w39", september_noon(23), &notes),
        ]),
    );
    context.write(
        "week-40.jsonl",
        &transcript(&[said("w40", september_noon(30), "сверка тридцатого")]),
    );
    context.write(
        "two-weeks.jsonl",
        &transcript(&[
            said("both", september_noon(16), "сверка шестнадцатого"),
            said("both", september_noon(24), "сверка двадцать четвёртого"),
        ]),
    );
    context.init();

    let everything = page(&context, "сверка", None);
    assert_eq!(everything.len(), 6, "{everything:?}");

    let last_week = Period::of_phrase(PeriodPhrase::LastWeek, thursday());
    let scope = PeriodScope::gather(&context.graph, last_week, &|_| true).unwrap();
    let within = page(&context, "сверка", Some(&scope));
    assert_eq!(
        names(&within),
        ["notes.md", "two-weeks.jsonl", "week-39.jsonl"],
        "{within:?}"
    );

    let (_, event, fragment) = within
        .iter()
        .find(|(name, _, _)| name == "two-weeks.jsonl")
        .unwrap();
    assert!(fragment.contains("четвёртого"), "{fragment}");
    assert_eq!(
        *event,
        Some(PeriodEvent::Reply {
            at_unix_ms: september_noon(24)
        })
    );
    let (_, event, _) = within
        .iter()
        .find(|(name, _, _)| name == "notes.md")
        .unwrap();
    assert_eq!(
        *event,
        Some(PeriodEvent::Touch {
            at_unix_ms: september_noon(23),
            verb: TouchVerb::Wrote
        })
    );
    // Every source of the page has its event inside the period: the acceptance criterion,
    // checked on every row rather than on one.
    for (name, event, _) in &within {
        let event = event.unwrap_or_else(|| panic!("{name} has no event"));
        assert!(last_week.contains(event.at_unix_ms()), "{name}");
    }
}

/// A file indexed today and written by a conversation a month ago is not "this week": when
/// the indexer saw it is not an event.
#[test]
fn a_file_indexed_today_but_touched_a_month_ago_is_not_this_week() {
    let context = context("period-indexed-today");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let month_ago = now - 30 * 86_400 * 1000;
    let report = context.at("report.md");
    context.write("report.md", "сверка месячного отчёта");
    context.write(
        "chat.jsonl",
        &transcript(&[
            said("s", month_ago, "напиши отчёт"),
            wrote("s", month_ago, &report),
            said("s", now, "сверка закончена"),
        ]),
    );
    context.init();

    assert!(names(&page(&context, "сверка", None)).contains(&"report.md"));

    let this_week = Period::of_phrase(PeriodPhrase::ThisWeek, local_today().unwrap());
    let scope = PeriodScope::gather(&context.graph, this_week, &|_| true).unwrap();
    let within = page(&context, "сверка", Some(&scope));
    assert_eq!(names(&within), ["chat.jsonl"], "{within:?}");
}

/// A conversation that grew past the content limit keeps its earlier revision in the graph,
/// and what it said this week is in no index. The period says so instead of answering with a
/// silent nothing — and the model is told the same sentence.
#[test]
fn a_conversation_that_outgrew_the_limit_is_named_as_not_shown() {
    let context = context("period-outgrew").limiting_content(400);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let month_ago = now - 30 * 86_400 * 1000;
    let mut lines = vec![said("s", month_ago, "сверка начата")];
    context.write("chat.jsonl", &transcript(&lines));
    context.init();
    lines.push(said(
        "s",
        now,
        &format!("сверка продолжена {}", "слово ".repeat(100)),
    ));
    context.write("chat.jsonl", &transcript(&lines));
    context.reconcile();

    assert_eq!(
        names(&page(&context, "сверка", None)),
        ["chat.jsonl"],
        "the earlier revision is still what the index holds"
    );
    let this_week = Period::of_phrase(PeriodPhrase::ThisWeek, local_today().unwrap());
    let scope = PeriodScope::gather(&context.graph, this_week, &|_| true).unwrap();
    assert!(page(&context, "сверка", Some(&scope)).is_empty());
    let sentence = scope.gaps().sentence().expect("a silent nothing");
    assert!(
        sentence.starts_with("Not shown: 1 conversation(s) written to during or after this period"),
        "{sentence}"
    );

    // A path the run may not name is not counted either.
    let hidden = PeriodScope::gather(&context.graph, this_week, &|_| false).unwrap();
    assert_eq!(hidden.gaps().sentence(), None);
}
