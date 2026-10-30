// `crosskit [--context <name>] ask "<вопрос>"`.
//
// The loop, closed: the index finds what looks relevant, the graph says what it says and
// whether this run may show it, a model answers from those excerpts and nothing else, and
// the sources under the answer are named from this program's own list of what it sent.
//
// A context that did not allow its content to leave cannot be asked at all, and the refusal
// says so with the line that would change it. There is no quieter answer available here: a
// question to a model has no deterministic half that could still run, and pretending
// otherwise would be a command that looks like it worked.
use crate::application::{AskFailure, AskReport, AskService, PeriodQuestion};
use crate::domain::{Period, Question, WorkspaceName};
use crate::infrastructure::analysis;
use crate::infrastructure::{
    FileAnswerCache, LoadedIndexPolicy, ask_cache_directory, data_directory, load_index_policy,
};

use super::context_draft::workspace_header;
use super::help;
use super::period::{self, PeriodFlags, event_label, nothing_inside, print_reading};
use super::search_index::{self, visible_paths};
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] ask \"<question>\" [--since YYYY-MM-DD] [--until YYYY-MM-DD]";

pub fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    let (flags, words) = split_arguments(arguments)?;
    if words.is_empty() {
        return Err(help::misuse(USAGE));
    }
    let question = Question::parse(&words.join(" ")).map_err(|error| error.to_string())?;
    // Before anything opens: a flag that cannot be a period is refused with nothing spent.
    let read = period::read_period(flags, &question.search_words())?;
    if read.period().is_some() && read.words().trim().is_empty() {
        return Err(
            "the question names a period and nothing else to look for in it; ask what about it"
                .to_string(),
        );
    }

    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select(&loaded, context)?;
    // The permission first, before a database is opened or a word is read: a context that
    // does not allow its content to leave is not asked, and is told why.
    let permit = loaded
        .workspaces()
        .content_egress(&selected)
        .permit()
        .ok_or_else(|| refusal_for(&selected))?;
    // The same engine the queue is worked with: one installation answers a question and
    // labels a revision with one thing, or the two would disagree about who answered.
    let provider = analysis::answer_provider(loaded.engine())
        .map_err(|unavailable| unavailable.reason("Nothing was sent and nothing was asked."))?;

    let manager = workspace::open_for_reading(&selected)?;
    let search = search_index::open(&loaded, &selected, &manager)?;
    let cache = FileAnswerCache::new(ask_cache_directory(
        &data_directory().map_err(|error| error.to_string())?,
        &selected,
    ));

    println!("Context: {selected}");
    println!("Question: {question}");
    print_reading(&read);
    let scope = period::scope_of(
        &search.graph,
        read.period(),
        &visible_paths(&loaded, &selected),
    )?;
    let asked_period = scope
        .as_ref()
        .map(|scope| PeriodQuestion::new(read.words(), scope));
    // Before the question is sent, not after the answer is printed: an answer drawn from
    // part of a corpus is not repaired by learning afterwards that it was.
    search.warn_if_written_by_other_rules();
    let report = AskService::new(
        &search.index,
        &search.graph,
        &provider,
        &cache,
        loaded.ask_budget(),
        analysis::model_label(loaded.engine()),
        analysis::ask_prompt_version(loaded.engine()),
        permit,
    )
    .execute(&question, asked_period, &visible_paths(&loaded, &selected))
    .map_err(|failure| match (failure, read.period()) {
        (AskFailure::NothingInPeriod { files_with_events }, Some(period)) => format!(
            "{} Nothing was sent.",
            nothing_inside(period, files_with_events)
        ),
        (failure, _) => explain(failure, &loaded),
    })?;

    render(&report, read.period());
    Ok(())
}

/// The period flags, wherever they stand, and the words of the question around them.
fn split_arguments(arguments: &[String]) -> Result<(PeriodFlags, Vec<&str>), String> {
    let mut flags = PeriodFlags::default();
    let mut words = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if !flags.read(argument, &mut remaining, USAGE)? {
            words.push(argument.as_str());
        }
    }
    Ok((flags, words))
}

fn render(report: &AskReport, period: Option<&Period>) {
    println!();
    println!("{}", report.answer().text());
    println!();
    println!(
        "Sources sent: {} of at most {} ({} bytes of at most {}){}",
        report.sources().len(),
        report.budget().max_sources(),
        report.bytes_sent(),
        report.budget().max_bytes(),
        if report.answered_from_cache() {
            ", answered from the cache"
        } else {
            ""
        }
    );
    let cited = report.cited();
    if cited.is_empty() {
        println!("Sources used: none — the answer cites nothing it was given.");
        return;
    }
    println!("Sources used:");
    for excerpt in cited {
        // The path comes from this program's own list, looked up by the number the model
        // cited. A model that named something else named a number nobody sent, and the
        // answer was refused before this line could be reached.
        match (excerpt.event(), period) {
            (Some(event), Some(period)) => println!(
                "  [{}] {} — {}",
                excerpt.ordinal(),
                excerpt.path(),
                event_label(event, period)
            ),
            _ => println!("  [{}] {}", excerpt.ordinal(), excerpt.path()),
        }
    }
}

/// Why a context that keeps its content to itself cannot be asked, and the one line that
/// would change that.
fn refusal_for(workspace: &WorkspaceName) -> String {
    format!(
        "context {workspace} keeps its content to itself, and a question to a model is \
         content leaving the machine. Nothing here can answer it in part: add \
         `send_content_outside = true` to `{}` if that is what you want, or ask this \
         question in a context that already allows it.",
        workspace_header(workspace)
    )
}

/// What a failure means to the person who asked. Nothing a provider wrote reaches this
/// function: the categories are all it is given.
fn explain(failure: AskFailure, loaded: &LoadedIndexPolicy) -> String {
    match failure {
        AskFailure::NothingToAnswerFrom => "nothing in this context matched the question, so \
             there was nothing to answer from and nothing was sent. `crosskit search` shows \
             what the index holds; `crosskit doctor` says whether it is in step with the \
             graph."
            .to_string(),
        AskFailure::Unavailable => {
            "the model could not be reached; nothing about this context changed".to_string()
        }
        AskFailure::Refused => {
            "the model declined to answer this question; asking it again will not change that"
                .to_string()
        }
        AskFailure::Schema => format!(
            "the model answered in a shape this program does not accept — an empty answer, \
             one longer than an answer, or one citing a source it was not given — and it was \
             refused whole. Nothing of it was kept. The question carried at most {} excerpts.",
            loaded.ask_budget().max_sources()
        ),
        AskFailure::RequestTooLarge => format!(
            "what would have been sent was refused for its size before any model saw it, so \
             the question went unanswered and nothing of this context left the machine. \
             Asking the same thing again sends the same bytes: lower `max_sources` or \
             `max_bytes` under `[ask]`. This one carried at most {} excerpts.",
            loaded.ask_budget().max_sources()
        ),
        other => other.to_string(),
    }
}
