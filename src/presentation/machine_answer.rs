// The machine answer: what `--json` prints, declared once.
//
// Every command that answers a program rather than a person prints one object of this schema
// on stdout and nothing else there — a page, a list of contexts, or a refusal. The schema has
// a number, and the number changes whenever a field changes meaning or goes away; a field
// added beside the others does not change it. A reader checks the number first and refuses an
// answer it was not written for, rather than reading one field as another.
//
// The objects of `graph --json` are declared beside this file, in `machine_graph`, under the
// same number.
//
// What is in here is only what the core holds today (apps/panel/docs/row-shape.md). Groups,
// "recent" on an empty query and a topic filter are not fields with an empty value: they are
// absent, because the core cannot answer them and an empty field would read as "none".
//
// Nothing a person did not put there reaches a field. No engine message, no path outside a
// result's own `path`, no sentence: a failure is a category from a closed list, and the
// sentence a person would read goes to stderr, where it always went.
use serde::Serialize;

use crate::application::{FileState, MatchedConversation, SearchPage, SearchResult};
use crate::domain::{Period, WorkspaceName};

use super::refusal::Refusal;
use super::search::one_line;

/// The flag every command that has a machine answer takes.
pub(super) const FLAG: &str = "--json";

/// The number of this schema. Raised when a field changes meaning or is removed.
///
/// 2 (UI-4b): in a graph view a path outside the context became a state of a file node
/// (`file.state = "outside"`) instead of a kind of its own, and a touch says `by_time`. The
/// page of `search` and the list of contexts did not change; they carry the one number.
pub(super) const SCHEMA: u32 = 2;

/// Why a command did not answer, as a program can act on it.
///
/// Closed: a reader can show every one of these and need not guess at another. A failure
/// this program cannot place more exactly is `failed`, which says "nothing you can do about
/// it from here" rather than inventing a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FailureCategory {
    /// The arguments, the cursor or the page size are not something this command takes.
    Usage,
    /// The query is not something the index can read as a query.
    UnreadableQuery,
    /// The configuration could not be read or says something this build refuses.
    Configuration,
    /// No context is configured at all.
    NoContext,
    /// Several contexts are configured and none was named.
    ContextRequired,
    /// The named context is not one the configuration declares.
    UnknownContext,
    /// The context is configured and nothing has indexed it yet.
    NotIndexed,
    /// Another process is writing this context's graph or index right now.
    Busy,
    /// The graph was written by another build's schema; `crosskit migrate` brings it here.
    NeedsMigration,
    /// The graph's log ends where its writer was stopped; `index recover` is the repair.
    NeedsRecovery,
    /// The search index cannot be read back; `index rebuild --search` writes it again.
    IndexUnreadable,
    /// The focus of a graph view names nothing this context holds or its policy admits.
    NotFound,
    /// Anything else. The reason is on stderr.
    Failed,
}

#[derive(Serialize)]
struct Refused<'a> {
    schema: u32,
    error: RefusedBody<'a>,
}

#[derive(Serialize)]
struct RefusedBody<'a> {
    category: FailureCategory,
    /// The context the run had selected when it stopped, when it got that far.
    context: Option<&'a str>,
}

/// Prints a refusal as the one object on stdout.
pub(super) fn print_refusal(refusal: &Refusal, context: Option<&WorkspaceName>) {
    println!("{}", refusal_line(refusal, context));
}

/// A refusal as the line `print_refusal` prints.
pub(super) fn refusal_line(refusal: &Refusal, context: Option<&WorkspaceName>) -> String {
    object_line(&Refused {
        schema: SCHEMA,
        error: RefusedBody {
            category: refusal.category(),
            context: context.map(WorkspaceName::as_str),
        },
    })
}

#[derive(Serialize)]
struct Contexts<'a> {
    schema: u32,
    contexts: Vec<&'a str>,
}

/// The contexts the configuration declares, in its order.
pub(super) fn print_contexts(names: &[WorkspaceName]) {
    println!("{}", contexts_line(names));
}

/// The contexts as the line `print_contexts` prints.
pub(super) fn contexts_line(names: &[WorkspaceName]) -> String {
    object_line(&Contexts {
        schema: SCHEMA,
        contexts: names.iter().map(WorkspaceName::as_str).collect(),
    })
}

/// One page of `search`, or the answer to a query with nothing to look for.
#[derive(Serialize)]
struct Page<'a> {
    schema: u32,
    context: &'a str,
    /// The query as typed.
    query: &'a str,
    /// The words that were looked for: the query without the period phrase, when it had one.
    looked_for: &'a str,
    period: Option<PeriodDays>,
    results: Vec<Row<'a>>,
    /// The cursor the next page starts after, passed back as `--after`. Nothing when the
    /// page did not fill; a page that filled has one even if nothing follows it.
    next: Option<String>,
    not_shown: NotShown,
    /// What the index says about itself. Nothing when the query had nothing to look for and
    /// no index was opened to ask.
    index: Option<IndexHealth>,
}

#[derive(Serialize)]
struct PeriodDays {
    /// Local days, `YYYY-MM-DD`, both included.
    first_day: String,
    last_day: String,
    /// Conversations written to in or after the period whose newer part the graph lacks.
    behind_transcripts: usize,
    /// Conversations that spoke in the period with chunks that could not be placed in it.
    unplaced_conversations: usize,
}

#[derive(Serialize)]
struct NotShown {
    /// Files only at paths this context's policy no longer admits. Counted, never named.
    by_policy: usize,
    /// Files with no location left, left out because `--include-missing` was not given.
    missing: usize,
    /// Files the index holds and the graph does not: the index is out of step.
    lost_by_graph: usize,
}

#[derive(Serialize)]
struct IndexHealth {
    documents: u64,
    /// Whether the index was written by the rules this build reads it with.
    written_by_this_build: bool,
    /// How many files the fold held, when the query matched more than one answer folds.
    folding_stopped_at: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum RowKind {
    Conversation,
    File,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum RowState {
    Present,
    Missing,
}

#[derive(Serialize)]
struct Row<'a> {
    kind: RowKind,
    /// Unique within a page and stable across pages: a reader may key a row by it.
    cursor: String,
    path: &'a str,
    state: RowState,
    /// The best-matching place in the file, folded into one line.
    fragment: String,
    /// The words of the query that place matched on, as they are written in it.
    matched: &'a [String],
    /// In how many places of the file the query matched.
    matches: u32,
    topic: Option<&'a str>,
    /// Present exactly when `kind` is `conversation`.
    conversation: Option<Conversation<'a>>,
}

#[derive(Serialize)]
struct Conversation<'a> {
    /// What the tool resumes the conversation by.
    session_id: &'a str,
    name: Option<&'a str>,
    message_count: u64,
    last_message_at_unix_ms: Option<i64>,
}

/// What `search` read out of the query before it looked.
pub(super) struct Asked<'a> {
    pub(super) context: &'a WorkspaceName,
    pub(super) query: &'a str,
    pub(super) looked_for: &'a str,
    pub(super) period: Option<&'a Period>,
    pub(super) gaps: Option<(usize, usize)>,
}

/// One page of `search`, as the one line of its machine answer.
pub(super) fn page_line(asked: &Asked<'_>, page: &SearchPage) -> String {
    object_line(&Page {
        index: Some(IndexHealth {
            documents: page.indexed_documents(),
            written_by_this_build: page.matches_this_build(),
            folding_stopped_at: page.folding_stopped_at(),
        }),
        results: page.results().iter().map(row).collect(),
        next: page.next().map(|cursor| cursor.encode()),
        not_shown: NotShown {
            by_policy: page.hidden_by_policy(),
            missing: page.missing_left_out(),
            lost_by_graph: page.matches_the_graph_lost(),
        },
        ..header(asked)
    })
}

/// The answer to a query with nothing to look for: no rows, and no index was asked.
pub(super) fn nothing_to_look_for_line(asked: &Asked<'_>) -> String {
    object_line(&header(asked))
}

fn header<'a>(asked: &Asked<'a>) -> Page<'a> {
    Page {
        schema: SCHEMA,
        context: asked.context.as_str(),
        query: asked.query,
        looked_for: asked.looked_for,
        period: asked.period.map(|period| {
            let (behind_transcripts, unplaced_conversations) = asked.gaps.unwrap_or((0, 0));
            PeriodDays {
                first_day: period.first_day().to_string(),
                last_day: period.last_day().to_string(),
                behind_transcripts,
                unplaced_conversations,
            }
        }),
        results: Vec::new(),
        next: None,
        not_shown: NotShown {
            by_policy: 0,
            missing: 0,
            lost_by_graph: 0,
        },
        index: None,
    }
}

fn row(result: &SearchResult) -> Row<'_> {
    let conversation = result.conversation().map(conversation);
    Row {
        kind: match conversation {
            Some(_) => RowKind::Conversation,
            None => RowKind::File,
        },
        cursor: result.cursor().encode(),
        path: result.path().as_str(),
        state: match result.state() {
            FileState::Present => RowState::Present,
            FileState::Missing => RowState::Missing,
        },
        fragment: one_line(result.fragment().text()),
        matched: result.fragment().matched(),
        matches: result.matches(),
        topic: result.topic(),
        conversation,
    }
}

fn conversation(conversation: &MatchedConversation) -> Conversation<'_> {
    Conversation {
        session_id: conversation.session_id(),
        name: conversation.name(),
        message_count: conversation.message_count(),
        last_message_at_unix_ms: conversation.last_message_at_unix_ms(),
    }
}

/// One object, one line. Serializing these types cannot fail — every key is a string and
/// no value is a float — so the fallback is an object a reader refuses by its number rather
/// than a half-written line.
pub(super) fn object_line(object: &impl Serialize) -> String {
    serde_json::to_string(object).unwrap_or_else(|_| "{\"schema\":0}".to_string())
}

#[cfg(test)]
mod tests {
    use super::FailureCategory;

    /// The categories are a contract with every reader, spelled out here and not derived.
    #[test]
    fn a_category_is_written_in_kebab_case() {
        let written = serde_json::to_string(&[
            FailureCategory::UnreadableQuery,
            FailureCategory::NotIndexed,
            FailureCategory::Busy,
            FailureCategory::Failed,
        ])
        .unwrap();
        assert_eq!(
            written,
            r#"["unreadable-query","not-indexed","busy","failed"]"#
        );
    }
}
