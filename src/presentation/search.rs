// `crosskit [--context <name>] search <query> [--limit N] [--after <cursor>]
// [--include-missing]`.
//
// One page at a time, inside one context. One line stands for one file, however many places
// inside it matched: what it shows is the path, the strongest of those places, the words it
// matched on, how many places there were and what the graph says about the file now —
// everything a reader needs to decide whether to open it, and nothing about any other
// context.
use crate::application::{
    FileState, PeriodScope, SearchPage, SearchRequest, SearchResult, SearchService,
};
use crate::domain::{Period, WorkspaceName};

use super::help;
use super::machine_answer::{self, Asked};
use super::period::{self, PeriodFlags, ReadPeriod, event_label, nothing_inside, print_reading};
use super::reading_ground::{OwnRun, ReadingGround};
use super::refusal::Refusal;
use super::search_index::{self, visible_paths};
use super::service_client;
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] search <query> [--limit <N>] [--after <cursor>] [--include-missing] [--since YYYY-MM-DD] [--until YYYY-MM-DD] [--json]";

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    // Decided before the arguments are read, so a request this command cannot parse is
    // answered in the form it was asked in.
    if arguments.iter().any(|one| one == machine_answer::FLAG) {
        if let Some(line) = service_client::answered("search", arguments, context) {
            println!("{line}");
            return Ok(());
        }
        let (line, outcome) = machine_line(arguments, context, &OwnRun::load());
        println!("{line}");
        return outcome.map_err(String::from);
    }
    answer(arguments, context, &mut TextReport, &OwnRun::load()).map_err(String::from)
}

/// The machine answer as one line, on whatever ground it is read from, and whether it was a
/// refusal.
pub(super) fn machine_line(
    arguments: &[String],
    context: Option<&str>,
    ground: &dyn ReadingGround,
) -> (String, Result<(), Refusal>) {
    let mut report = MachineReport::default();
    let outcome = answer(arguments, context, &mut report, ground);
    if let Err(refusal) = &outcome {
        report.refused(refusal);
    }
    (report.line.unwrap_or_default(), outcome)
}

/// One query, answered through `report`. The order of the steps is the order a person sees
/// their lines in, so a text page reads exactly as it did before a second form existed.
fn answer(
    arguments: &[String],
    context: Option<&str>,
    report: &mut dyn Report,
    ground: &dyn ReadingGround,
) -> Result<(), Refusal> {
    let asked = Arguments::parse(arguments).map_err(Refusal::usage)?;
    let read = period::read_period(asked.period, &asked.query)?;
    let loaded = ground.policy()?;
    let selected = workspace::choose(loaded, context)?;
    report.context(&selected);
    if read.words().trim().is_empty() {
        // Answered before anything opens: there is nothing to look for, and a question
        // nobody asked should not create a database or an index directory.
        report.nothing_to_look_for(&asked.query, &read);
        return Ok(());
    }
    let manager = ground.graph(&selected)?;
    let search = search_index::open_or_refuse(loaded, &selected, &manager)?;
    let scope = period::gather(
        &search.graph,
        read.period(),
        &visible_paths(loaded, &selected),
    )?;
    report.period_gaps(scope.as_ref());

    let request = SearchRequest::new(
        read.words(),
        asked.limit,
        asked.after.as_deref(),
        asked.include_missing,
    )
    .map_err(Refusal::of_search)?;
    let page = SearchService::new(&search.index, &search.graph)
        .execute(&request, scope.as_ref(), &visible_paths(loaded, &selected))
        .map_err(Refusal::of_search)?;
    report.page(&asked.query, &read, &request, scope.as_ref(), &page);
    Ok(())
}

/// Where the answer goes: a page a person reads line by line, or one object a program reads
/// whole. The steps above call these in order and know nothing about either form.
trait Report {
    fn context(&mut self, selected: &WorkspaceName);
    fn nothing_to_look_for(&mut self, query: &str, read: &ReadPeriod);
    fn period_gaps(&mut self, scope: Option<&PeriodScope>);
    fn page(
        &mut self,
        query: &str,
        read: &ReadPeriod,
        request: &SearchRequest,
        scope: Option<&PeriodScope>,
        page: &SearchPage,
    );
    fn refused(&mut self, refusal: &Refusal);
}

/// The text page, printed as each part becomes known.
struct TextReport;

impl Report for TextReport {
    fn context(&mut self, selected: &WorkspaceName) {
        println!("Context: {selected}");
    }

    fn nothing_to_look_for(&mut self, _query: &str, read: &ReadPeriod) {
        print_reading(read);
        match read.period() {
            Some(_) => println!("Empty query: nothing to look for besides the period."),
            None => println!("Empty query: nothing to look for."),
        }
    }

    fn period_gaps(&mut self, scope: Option<&PeriodScope>) {
        if let Some(gaps) = scope.and_then(|scope| scope.gaps().sentence()) {
            println!("{gaps}");
        }
    }

    fn page(
        &mut self,
        query: &str,
        read: &ReadPeriod,
        request: &SearchRequest,
        scope: Option<&PeriodScope>,
        page: &SearchPage,
    ) {
        print_page(query, read, request, scope, page);
    }

    /// The sentence goes to stderr on the way out, as for every other command.
    fn refused(&mut self, _refusal: &Refusal) {}
}

/// The machine answer: nothing is printed until there is one whole object to print.
#[derive(Default)]
struct MachineReport {
    context: Option<WorkspaceName>,
    line: Option<String>,
}

impl MachineReport {
    fn asked<'a>(
        &'a self,
        query: &'a str,
        read: &'a ReadPeriod,
        scope: Option<&PeriodScope>,
    ) -> Option<Asked<'a>> {
        Some(Asked {
            context: self.context.as_ref()?,
            query,
            looked_for: read.words(),
            period: read.period(),
            gaps: scope.map(|scope| {
                (
                    scope.gaps().behind_transcripts(),
                    scope.gaps().unplaced_conversations(),
                )
            }),
        })
    }
}

impl Report for MachineReport {
    fn context(&mut self, selected: &WorkspaceName) {
        self.context = Some(selected.clone());
    }

    fn nothing_to_look_for(&mut self, query: &str, read: &ReadPeriod) {
        self.line = self
            .asked(query, read, None)
            .map(|asked| machine_answer::nothing_to_look_for_line(&asked));
    }

    fn period_gaps(&mut self, _scope: Option<&PeriodScope>) {}

    fn page(
        &mut self,
        query: &str,
        read: &ReadPeriod,
        _request: &SearchRequest,
        scope: Option<&PeriodScope>,
        page: &SearchPage,
    ) {
        self.line = self
            .asked(query, read, scope)
            .map(|asked| machine_answer::page_line(&asked, page));
    }

    fn refused(&mut self, refusal: &Refusal) {
        self.line = Some(machine_answer::refusal_line(refusal, self.context.as_ref()));
    }
}

fn print_page(
    query: &str,
    read: &ReadPeriod,
    request: &SearchRequest,
    scope: Option<&PeriodScope>,
    page: &SearchPage,
) {
    println!("Query: {query}");
    print_reading(read);
    println!(
        "Results: {} (at most {} a page)",
        page.results().len(),
        request.limit()
    );
    for (position, result) in page.results().iter().enumerate() {
        render(position + 1, result, read.period());
    }
    if let (Some(scope), true) = (scope, page.results().is_empty()) {
        println!(
            "{}",
            nothing_inside(scope.period(), scope.files_with_events())
        );
    }

    if page.results().is_empty() && page.indexed_documents() == 0 {
        println!(
            "The search index of this context holds nothing yet. Index a directory, or write it from the graph with `crosskit index rebuild --search`."
        );
    }
    if page.missing_left_out() > 0 {
        println!(
            "Files with no location left, not shown: {} (add --include-missing)",
            page.missing_left_out()
        );
    }
    if page.hidden_by_policy() > 0 {
        println!(
            "Files at paths this context's policy no longer admits, not shown: {}",
            page.hidden_by_policy()
        );
    }
    if page.matches_the_graph_lost() > 0 {
        println!(
            "Files the graph no longer holds, not shown: {}. The index is out of step; `crosskit index rebuild --search` repairs it.",
            page.matches_the_graph_lost()
        );
    }
    if let Some(ceiling) = page.folding_stopped_at() {
        println!(
            "This query matches in more files than one answer folds; these results are the best of {ceiling} of them. Ask it with more words."
        );
    }
    if let Some(next) = page.next() {
        println!("Next page: --after {}", next.encode());
    }
    if !page.matches_this_build() {
        eprintln!("{}", search_index::WRITTEN_BY_OTHER_RULES);
    }
}

fn render(position: usize, result: &SearchResult, period: Option<&Period>) {
    println!(
        "{position}. {} {}",
        state_label(result.state()),
        result.path()
    );
    // What put the file inside the period, in its local time: the evidence the row is there
    // for, and the one thing that lets a reader check it was the right week.
    if let (Some(event), Some(period)) = (result.event(), period) {
        println!("   event: {}", event_label(event, period));
    }
    // What the file is about, when anything was concluded. A file nobody has analysed
    // prints no line at all: an empty label or a dash would say something was tried, and
    // most files here have not been asked about.
    if let Some(topic) = result.topic() {
        println!("   about: {}", one_line(topic));
    }
    println!("   {}", one_line(result.fragment().text()));
    if !result.fragment().matched().is_empty() {
        println!("   matched: {}", result.fragment().matched().join(", "));
    }
    // What one line stands for. A file that matched in one place says nothing here: the
    // fragment above already is the whole of it, and a count of one would be noise on every
    // ordinary result.
    if result.matches() > 1 {
        println!(
            "   and in {} more places in this file",
            result.matches() - 1
        );
    }
}

/// What the graph says about the file, on the same line as its path: a reader has to be
/// able to tell a file that is there from one whose content only survives in the graph.
fn state_label(state: FileState) -> &'static str {
    match state {
        FileState::Present => "[present]",
        FileState::Missing => "[missing]",
    }
}

/// A fragment is cut out of file content, which holds newlines and tabs of its own. They
/// are folded so one result stays one line and nothing in a file can reshape the output.
pub(super) fn one_line(fragment: &str) -> String {
    let mut line = String::with_capacity(fragment.len());
    let mut spaced = true;
    for character in fragment.chars() {
        if character.is_whitespace() || character.is_control() {
            if !spaced {
                line.push(' ');
                spaced = true;
            }
            continue;
        }
        line.push(character);
        spaced = false;
    }
    line.trim_end().to_string()
}

/// The default page. Small, because a terminal page is read rather than scrolled.
const DEFAULT_LIMIT: usize = 10;

#[derive(Debug)]
struct Arguments {
    query: String,
    limit: usize,
    after: Option<String>,
    include_missing: bool,
    period: PeriodFlags,
}

impl Arguments {
    /// Words outside the options make up the query, joined by a space: a query is usually
    /// typed unquoted, and joining is what the shell already did to a quoted one.
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut words: Vec<&str> = Vec::new();
        let mut limit = DEFAULT_LIMIT;
        let mut after = None;
        let mut include_missing = false;
        let mut period = PeriodFlags::default();
        let mut remaining = arguments.iter();
        while let Some(argument) = remaining.next() {
            if period.read(argument, &mut remaining, USAGE)? {
                continue;
            }
            match argument.as_str() {
                "--limit" => {
                    let value = remaining
                        .next()
                        .ok_or_else(|| format!("--limit requires a value. {USAGE}"))?;
                    limit = value.parse().map_err(|_| {
                        format!(
                            "--limit expects a number of 0 or more, received {value:?}. {USAGE}"
                        )
                    })?;
                }
                "--after" => {
                    after = Some(
                        remaining
                            .next()
                            .ok_or_else(|| format!("--after requires a value. {USAGE}"))?
                            .clone(),
                    );
                }
                "--include-missing" => include_missing = true,
                // Read only so it is not taken for a word of the query: which form the
                // answer takes is decided before the arguments are parsed.
                machine_answer::FLAG => {}
                other if other.starts_with("--") => {
                    return Err(format!("unknown argument {other:?}. {USAGE}"));
                }
                other => words.push(other),
            }
        }
        if words.is_empty() && after.is_none() {
            return Err(help::misuse(USAGE));
        }
        Ok(Self {
            query: words.join(" "),
            limit,
            after,
            include_missing,
            period,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Arguments, DEFAULT_LIMIT, one_line};

    fn parse(arguments: &[&str]) -> Result<Arguments, String> {
        Arguments::parse(
            &arguments
                .iter()
                .map(|argument| (*argument).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn the_words_outside_the_options_are_the_query() {
        let asked = parse(&["обход", "индекса", "--limit", "3"]).unwrap();
        assert_eq!(asked.query, "обход индекса");
        assert_eq!(asked.limit, 3);
        assert_eq!(asked.after, None);
        assert!(!asked.include_missing);
    }

    #[test]
    fn options_are_read_wherever_they_appear() {
        let asked = parse(&["--include-missing", "обход", "--after", "3f800000:chunk:a"]).unwrap();
        assert_eq!(asked.query, "обход");
        assert_eq!(asked.after.as_deref(), Some("3f800000:chunk:a"));
        assert!(asked.include_missing);
        assert_eq!(asked.limit, DEFAULT_LIMIT);
    }

    /// An empty query is a question this program answers; no query at all is a usage
    /// mistake, and the two are not the same thing.
    #[test]
    fn an_empty_query_is_accepted_and_no_arguments_are_not() {
        assert_eq!(parse(&[""]).unwrap().query, "");
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn an_option_this_command_does_not_have_is_refused() {
        let error = parse(&["обход", "--fuzzy"]).unwrap_err();
        assert!(error.contains("--fuzzy"), "{error}");
        assert!(parse(&["обход", "--limit"]).is_err());
        assert!(parse(&["обход", "--limit", "many"]).is_err());
    }

    /// A fragment comes out of a file, so it can hold anything a file can. It never
    /// becomes more than one line of output.
    #[test]
    fn a_fragment_is_folded_into_one_line() {
        assert_eq!(one_line("две\nстроки\tи\r\nтабы  "), "две строки и табы");
        assert_eq!(
            one_line("a\u{1b}[31mb"),
            "a [31mb",
            "a control character from a file must not reach the terminal as one"
        );
    }
}
