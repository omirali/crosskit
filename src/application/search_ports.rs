// The two sides of search: the index of one context, and the graph that index is derived
// from.
//
// The graph is the single source of truth. The index holds terms and ids and nothing a
// reader is shown: the path of a hit, its text and whether its file is still on disk are
// all read back from the graph at the moment the answer is printed. That is why a result
// can never name a path the policy now hides, and why an index that fell behind shows fewer
// results rather than stale ones.
use std::collections::HashMap;

use crate::domain::{
    DocumentCursor, IndexFingerprint, LocationStatus, NormalizedPath, SourceLanguage,
};

use super::period_scope::PeriodScope;
use super::repository::RepositoryError;

/// One indexed unit: the text of one chunk of the current version of one file.
///
/// The file id travels with it because it is what a replacement is keyed by. When a file
/// gets a new current version, every document of the old one has to go, and the file is the
/// only thing the two versions share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchDocument {
    chunk_id: String,
    file_id: String,
    language: Option<SourceLanguage>,
    text: String,
    conversation_name: Option<String>,
}

impl SearchDocument {
    pub fn new(
        chunk_id: impl Into<String>,
        file_id: impl Into<String>,
        language: Option<SourceLanguage>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            chunk_id: chunk_id.into(),
            file_id: file_id.into(),
            language,
            text: text.into(),
            conversation_name: None,
        }
    }

    /// The same document, marked as the one that carries a conversation's own name.
    ///
    /// One document per conversation and never a name on every chunk of it: a name repeated
    /// on six thousand reply documents would match a question six thousand times over, and
    /// the page would hold one conversation.
    pub fn naming_a_conversation(mut self, name: impl Into<String>) -> Self {
        self.conversation_name = Some(name.into());
        self
    }

    pub fn chunk_id(&self) -> &str {
        &self.chunk_id
    }

    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    /// What the name of this chunk's file declares it is written in, when the name declares
    /// anything. It is decided wherever the path is known and travels with the document,
    /// because the index holds no path to decide it from later.
    pub fn language(&self) -> Option<SourceLanguage> {
        self.language
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The conversation this document names, when it is the document that names one.
    pub fn conversation_name(&self) -> Option<&str> {
        self.conversation_name.as_deref()
    }
}

/// Writing the search index of one context.
///
/// Every call happens after the graph transaction that produced its documents committed,
/// never inside it. The order is the whole design: an index written first could describe a
/// transaction that was then rolled back, and nothing would ever discover it. Written
/// second, the worst case is an index that lags the graph, which `doctor` finds by counting
/// and a rebuild repairs.
///
/// Writing and being visible are two things here, and the distance between them was
/// measured. A pass stages what it has after each batch and makes all of it visible once,
/// at the end. Making every batch visible instead costs 21% of a cold indexing pass on
/// 10 407 files, against 2% for the indexing itself — the price is in the commits and the
/// merges they schedule, not in the text. What deferring it costs is the same failure this
/// whole design already accepts: a pass that dies leaves the index where the previous pass
/// left it, `doctor` says by how much, and the next pass writes it again.
pub trait SearchIndexWriter {
    /// Stages the replacement of every document of every file in `files` by `documents`. A
    /// file not named keeps what it has.
    ///
    /// The files are named apart from the documents because the two are not the same list:
    /// a file whose new current version holds no text has to lose the documents of the
    /// version before it, and it contributes no document to say so.
    fn publish(
        &self,
        files: &[String],
        documents: Vec<SearchDocument>,
    ) -> Result<(), SearchIndexError>;

    /// Stages documents without replacing anything and without making them visible.
    ///
    /// For filling an index that was just cleared, and for nothing else. A rebuild reads
    /// the graph in pages of chunks, and a page boundary falls wherever it falls — inside
    /// one file's chunks as readily as between two files — so replacing by file here would
    /// drop what the previous page of that same file had already written.
    fn append(&self, documents: Vec<SearchDocument>) -> Result<(), SearchIndexError>;

    /// Stages the removal of every document, so a rebuild writes into an empty index.
    ///
    /// Staged, not applied: with `append` and `finish` this makes a rebuild one visible
    /// step, so a rebuild that is interrupted leaves the index it was replacing rather
    /// than an empty one.
    fn clear(&self) -> Result<(), SearchIndexError>;

    /// Makes everything staged visible and lets the index settle, once a run is done
    /// writing.
    ///
    /// Settling is not housekeeping either. Every commit leaves a piece of index behind, and
    /// a query has to ask each piece separately: measured, an index left in pieces answers a
    /// query in 785 µs where a settled one answers in 221 µs. Settling folds them together
    /// and gives up the directory, so another process may write it next.
    fn finish(&self) -> Result<(), SearchIndexError>;
}

/// Reading the search index of one context.
pub trait SearchIndexReader {
    /// The documents matching `query`, best first, starting strictly after `after`, at most
    /// `limit` of them.
    ///
    /// A document and not a chunk: the several chunks of one file that matched are folded
    /// into one row here, where the ranking is, because folding after the fact would give a
    /// page of five rows out of five chunks that named three files.
    ///
    /// `within` keeps only the chunks a period admits, and it is applied here, before the
    /// fold, for the same reason: a file whose best chunk is from another week and whose
    /// second best is from this one is a row of this week, shown by its second best. ADR 0043
    /// measured the alternative — filtering the page afterwards — and what it loses.
    fn matches(
        &self,
        query: &str,
        limit: usize,
        after: Option<&DocumentCursor>,
        within: Option<&PeriodScope>,
    ) -> Result<MatchedDocuments, SearchIndexError>;

    /// For each text, the part of it the same query matched and the terms it matched on.
    ///
    /// The texts come from the graph rather than from the index: the index stores no
    /// content, and a fragment shown to a reader has to be the text the graph holds now.
    fn fragments(
        &self,
        query: &str,
        texts: &[String],
    ) -> Result<Vec<MatchFragment>, SearchIndexError>;

    /// How many documents the index holds and which analyzer wrote them.
    fn state(&self) -> Result<SearchIndexState, SearchIndexError>;
}

/// One page of matched documents, and what folding them had to leave out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedDocuments {
    documents: Vec<DocumentMatch>,
    folding_stopped_at: Option<usize>,
}

impl MatchedDocuments {
    pub fn new(documents: Vec<DocumentMatch>) -> Self {
        Self {
            documents,
            folding_stopped_at: None,
        }
    }

    /// The same page, from a query that matched more distinct files than one fold holds.
    pub fn folding_stopped_at(mut self, ceiling: usize) -> Self {
        self.folding_stopped_at = Some(ceiling);
        self
    }

    /// How many files the fold held, when it could not hold them all. The rows are then the
    /// best of those files and not of every file that matched, and a reader is told so: a
    /// short answer and a wrong one look the same otherwise.
    pub fn ceiling(&self) -> Option<usize> {
        self.folding_stopped_at
    }

    pub fn into_documents(self) -> Vec<DocumentMatch> {
        self.documents
    }
}

/// One document the index matched: the chunk that matched it best, how many of its chunks
/// matched at all, and where the row sits in the order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentMatch {
    chunk_id: String,
    matches: u32,
    cursor: DocumentCursor,
}

impl DocumentMatch {
    pub fn new(chunk_id: impl Into<String>, matches: u32, cursor: DocumentCursor) -> Self {
        Self {
            chunk_id: chunk_id.into(),
            matches,
            cursor,
        }
    }

    /// The chunk this row is shown by: the best-scoring one of the document.
    pub fn chunk_id(&self) -> &str {
        &self.chunk_id
    }

    /// How many chunks of this document matched. One row can stand for several places in
    /// one file, and this is what says so.
    pub fn matches(&self) -> u32 {
        self.matches
    }

    pub fn cursor(&self) -> &DocumentCursor {
        &self.cursor
    }
}

/// The part of one text a query matched, and the terms it matched on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchFragment {
    text: String,
    matched: Vec<String>,
}

impl MatchFragment {
    pub fn new(text: impl Into<String>, matched: Vec<String>) -> Self {
        Self {
            text: text.into(),
            matched,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The words of the text that the query matched, in the order they appear in it.
    pub fn matched(&self) -> &[String] {
        &self.matched
    }
}

/// What an index says about itself: how much it holds, and how it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIndexState {
    documents: u64,
    written_by: Option<IndexFingerprint>,
    writes_with: IndexFingerprint,
}

impl SearchIndexState {
    pub fn new(
        documents: u64,
        written_by: Option<IndexFingerprint>,
        writes_with: IndexFingerprint,
    ) -> Self {
        Self {
            documents,
            written_by,
            writes_with,
        }
    }

    pub fn documents(&self) -> u64 {
        self.documents
    }

    /// How the index records having been written: its fields, its suffix table and its
    /// analyzer. Absent in an empty index that has never been committed to.
    pub fn written_by(&self) -> Option<&IndexFingerprint> {
        self.written_by.as_ref()
    }

    /// How this program would write one now.
    pub fn writes_with(&self) -> &IndexFingerprint {
        &self.writes_with
    }

    /// Whether what is in the index was produced by the rules this program applies now. When
    /// it was not, the index answers by rules nobody applies any more — terms in a form no
    /// query reaches, or a field a query filters on missing from documents written before it
    /// existed — and the repair is a rebuild. An index that holds nothing has nothing to
    /// disagree about.
    pub fn matches_this_build(&self) -> bool {
        match &self.written_by {
            Some(written_by) => written_by == &self.writes_with,
            None => self.documents == 0,
        }
    }
}

/// The graph side of search: what the index is built from, and what a match is resolved
/// against. Every read is of the context's own graph, so nothing here can reach another
/// context's files.
pub trait SearchGraphReader {
    /// One page of the documents the index should hold: the chunks of the current version
    /// of every file, ordered by chunk id, starting after `after_chunk_id`.
    ///
    /// Only current versions. A file's earlier revisions stay in the graph and are not
    /// searchable, which is what keeps the index bounded by the number of files rather than
    /// by the number of revisions.
    fn current_documents(
        &self,
        after_chunk_id: &str,
        limit: usize,
    ) -> Result<Vec<SearchDocument>, RepositoryError>;

    /// The current version of each named file, as the graph holds it. Files with no current
    /// version are absent from the result.
    fn current_versions(
        &self,
        file_ids: &[String],
    ) -> Result<HashMap<String, String>, RepositoryError>;

    /// How many chunks the current versions hold altogether: the number of documents an
    /// index in step with this graph has.
    fn current_chunk_count(&self) -> Result<u64, RepositoryError>;

    /// The text of each named chunk and every place its file is known to be. Chunks the
    /// graph no longer holds are absent from the result.
    fn resolve(&self, chunk_ids: &[String]) -> Result<Vec<ResolvedChunk>, RepositoryError>;
}

/// One matched chunk as the graph holds it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChunk {
    chunk_id: String,
    text: String,
    locations: Vec<(NormalizedPath, LocationStatus)>,
    /// What the analysis concluded this revision is about, when it was analysed at all.
    /// Absent is the ordinary answer, not a failure: most revisions wait for the owner.
    topic: Option<String>,
    /// The conversation this revision was read as, when its file is a transcript.
    conversation: Option<MatchedConversation>,
}

/// The conversation a matched chunk belongs to, as the graph holds it (ADR 0036).
///
/// What makes a row a conversation rather than a file is this and nothing else: the graph
/// read the revision as one. A `.jsonl` that did not parse as a transcript stays a file, and
/// so does its row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedConversation {
    session_id: String,
    name: Option<String>,
    message_count: u64,
    last_message_at_unix_ms: Option<i64>,
}

impl MatchedConversation {
    pub fn new(
        session_id: impl Into<String>,
        name: Option<String>,
        message_count: u64,
        last_message_at_unix_ms: Option<i64>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            name: name.filter(|name| !name.trim().is_empty()),
            message_count,
            last_message_at_unix_ms,
        }
    }

    /// The session the transcript records, which is what its tool resumes it by.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The name the owner or the model gave it; nothing when nobody did.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn message_count(&self) -> u64 {
        self.message_count
    }

    /// When its last reply was said, when the transcript recorded a moment for it.
    pub fn last_message_at_unix_ms(&self) -> Option<i64> {
        self.last_message_at_unix_ms
    }
}

impl ResolvedChunk {
    pub fn new(
        chunk_id: impl Into<String>,
        text: impl Into<String>,
        locations: Vec<(NormalizedPath, LocationStatus)>,
        topic: Option<String>,
    ) -> Self {
        Self {
            chunk_id: chunk_id.into(),
            text: text.into(),
            locations,
            topic,
            conversation: None,
        }
    }

    /// The same chunk, as part of the conversation its revision was read as.
    pub fn in_conversation(mut self, conversation: MatchedConversation) -> Self {
        self.conversation = Some(conversation);
        self
    }

    pub fn chunk_id(&self) -> &str {
        &self.chunk_id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Every path the chunk's file is known to be at, with the status of each. A file can
    /// be at several — a hard link, or a copy the scan has seen under two names — and the
    /// order they arrive in is the graph's, so the choice of which one to show is made
    /// where the policy is known.
    pub fn locations(&self) -> &[(NormalizedPath, LocationStatus)] {
        &self.locations
    }

    pub fn topic(&self) -> Option<&str> {
        self.topic.as_deref()
    }

    pub fn conversation(&self) -> Option<&MatchedConversation> {
        self.conversation.as_ref()
    }
}

/// What went wrong inside the search index.
///
/// Closed and in this program's own words. The library's messages quote the query, name
/// files inside the index directory and describe its internals; none of that is something a
/// reader can act on, and the query and the paths are the reader's own content, so they are
/// not echoed back through an error channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchIndexError {
    /// The index could not be opened, read or written. The operation is this program's own
    /// word for what it was doing.
    Unavailable(&'static str),
    /// Another process is writing the index of this context. Only one may, and the graph
    /// write this one belongs to has already happened.
    Busy,
    /// The query is not something this program can read as a query.
    UnreadableQuery,
    /// The index returned a document this program cannot read back. The index is a
    /// derivative, so the repair is to rebuild it.
    Unreadable(&'static str),
}

impl std::fmt::Display for SearchIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(operation) => {
                write!(f, "the search index of this context could not {operation}")
            }
            Self::Busy => f.write_str(
                "another crosskit process is writing the search index of this context",
            ),
            Self::UnreadableQuery => f.write_str(
                "the query cannot be read; words, \"a phrase\", +required, -excluded, AND, OR and field:word are understood",
            ),
            Self::Unreadable(what) => write!(
                f,
                "the search index of this context could not be read ({what}); rebuild it with `crosskit index rebuild --search`"
            ),
        }
    }
}

impl std::error::Error for SearchIndexError {}

/// Anything that can stop search from answering or from being kept up to date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    Index(SearchIndexError),
    Graph(RepositoryError),
    /// The cursor is not one this program produced, so there is no page it names.
    UnreadableCursor,
    /// A page of this size is not one this program builds.
    LimitOutOfRange {
        asked: usize,
        most: usize,
    },
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(error) => write!(f, "{error}"),
            Self::Graph(error) => write!(f, "{error}"),
            Self::UnreadableCursor => {
                f.write_str("--after takes a cursor from the end of an earlier page of results")
            }
            Self::LimitOutOfRange { asked, most } => write!(
                f,
                "--limit takes between 1 and {most} results a page; {asked} was asked for"
            ),
        }
    }
}

impl std::error::Error for SearchError {}

impl From<SearchIndexError> for SearchError {
    fn from(error: SearchIndexError) -> Self {
        Self::Index(error)
    }
}

impl From<RepositoryError> for SearchError {
    fn from(error: RepositoryError) -> Self {
        Self::Graph(error)
    }
}
