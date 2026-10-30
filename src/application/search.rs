// Use case: answer one query inside one context, one page at a time.
//
// A result is a document — one row for one file, however many of its chunks matched. The
// index does the folding, because that is where the ranking is; this reads the rows it gives
// back and turns each into the thing a reader sees.
//
// The index says which documents match and in what order; the graph says what each match is.
// That split is deliberate. A path, a fragment and whether the file is still on disk are
// facts the graph owns, so they are read at the moment the answer is built: a result can
// never name a path the policy has since stopped admitting, and an index that fell behind
// produces fewer results rather than results that were true last week.
use std::collections::HashMap;

use crate::domain::{DocumentCursor, LocationStatus, NormalizedPath};

use super::period_scope::{PeriodEvent, PeriodScope};
use super::search_ports::{
    MatchFragment, MatchedConversation, ResolvedChunk, SearchError, SearchGraphReader,
    SearchIndexReader,
};

/// The largest page this program will build.
///
/// A page is answered by reading the text of every result out of the graph, so the limit
/// bounds how much content one query holds in memory. A caller that wants more asks for the
/// next page, which costs the same as the first.
pub const MAX_LIMIT: usize = 100;

/// One query, as a caller asked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    query: String,
    limit: usize,
    after: Option<DocumentCursor>,
    include_missing: bool,
}

impl SearchRequest {
    /// `after` is the cursor text a previous page ended with. A cursor this program did not
    /// produce is refused rather than ignored: ignoring it would silently answer with the
    /// first page while the caller believed it was reading the second.
    pub fn new(
        query: impl Into<String>,
        limit: usize,
        after: Option<&str>,
        include_missing: bool,
    ) -> Result<Self, SearchError> {
        if limit == 0 || limit > MAX_LIMIT {
            return Err(SearchError::LimitOutOfRange {
                asked: limit,
                most: MAX_LIMIT,
            });
        }
        let after = match after {
            Some(text) => Some(DocumentCursor::decode(text).ok_or(SearchError::UnreadableCursor)?),
            None => None,
        };
        Ok(Self {
            query: query.into(),
            limit,
            after,
            include_missing,
        })
    }

    pub fn limit(&self) -> usize {
        self.limit
    }
}

/// What a query found, and what it did not show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
    results: Vec<SearchResult>,
    next: Option<DocumentCursor>,
    hidden_by_policy: usize,
    missing_left_out: usize,
    matches_the_graph_lost: usize,
    folding_stopped_at: Option<usize>,
    matches_this_build: bool,
    indexed_documents: u64,
}

impl SearchPage {
    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    /// The cursor the next page starts after, when there may be one. A page that filled up
    /// yields one even if nothing follows it, so the page after the last is empty rather
    /// than absent.
    pub fn next(&self) -> Option<&DocumentCursor> {
        self.next.as_ref()
    }

    /// Files only at paths the index policy no longer admits. They are counted and never
    /// named: naming one would publish the path the policy hides.
    pub fn hidden_by_policy(&self) -> usize {
        self.hidden_by_policy
    }

    /// Files with no location left, left out because `--include-missing` was not asked for.
    pub fn missing_left_out(&self) -> usize {
        self.missing_left_out
    }

    /// Files the index holds and the graph does not. The index is behind or ahead of the
    /// graph, which `doctor` reports and a rebuild repairs.
    pub fn matches_the_graph_lost(&self) -> usize {
        self.matches_the_graph_lost
    }

    /// How many files the fold held, when the query matched more of them than one fold
    /// holds. The page is then the best of those files rather than of every file that
    /// matched, and it says so instead of looking like a complete answer.
    pub fn folding_stopped_at(&self) -> Option<usize> {
        self.folding_stopped_at
    }

    /// Whether the index was written by the analyzer this query was read with. When it was
    /// not, the answer is by rules nobody applies any more.
    pub fn matches_this_build(&self) -> bool {
        self.matches_this_build
    }

    /// How many documents the index holds. An answer of nothing from an index of nothing
    /// is a different thing to report than an answer of nothing from a full one.
    pub fn indexed_documents(&self) -> u64 {
        self.indexed_documents
    }

    /// What an empty query answers: nothing found, nothing wrong.
    fn nothing() -> Self {
        Self {
            results: Vec::new(),
            next: None,
            hidden_by_policy: 0,
            missing_left_out: 0,
            matches_the_graph_lost: 0,
            folding_stopped_at: None,
            matches_this_build: true,
            indexed_documents: 0,
        }
    }
}

/// One result: where it is, what matched, and what the graph says about the file now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    path: NormalizedPath,
    fragment: MatchFragment,
    matches: u32,
    state: FileState,
    topic: Option<String>,
    event: Option<PeriodEvent>,
    conversation: Option<MatchedConversation>,
    cursor: DocumentCursor,
}

impl SearchResult {
    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    /// The part of the file's best-matching chunk, and the words it matched on. One chunk
    /// of the several that may have matched: the one the row was ranked by, so the line a
    /// reader sees is the evidence the row is standing on.
    pub fn fragment(&self) -> &MatchFragment {
        &self.fragment
    }

    /// In how many chunks of this file the query matched. Folding turns several lines of a
    /// page into one, and this is what that one line does not hide.
    pub fn matches(&self) -> u32 {
        self.matches
    }

    pub fn state(&self) -> FileState {
        self.state
    }

    /// What the analysis concluded this file's current revision is about, when it was
    /// analysed. Nothing is the ordinary answer: this installation analyses what its rules
    /// cover and waits to be asked about the rest, so a file without a topic is a file
    /// nobody has asked about — not a file something went wrong with.
    pub fn topic(&self) -> Option<&str> {
        self.topic.as_deref()
    }

    /// The event that put this result inside the period it was asked about, when it was asked
    /// about one.
    pub fn event(&self) -> Option<PeriodEvent> {
        self.event
    }

    /// The conversation this row is, when its file was read as one; nothing for a file.
    pub fn conversation(&self) -> Option<&MatchedConversation> {
        self.conversation.as_ref()
    }

    /// The cursor a page that ends with this row continues after.
    pub fn cursor(&self) -> &DocumentCursor {
        &self.cursor
    }
}

/// What the graph says about the file a result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// At least one path the policy admits still holds it.
    Present,
    /// Every location is retired: the content is in the graph, the file is not on disk.
    Missing,
}

/// One match this run may show: where it is and the whole text of the chunk it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSource {
    chunk_id: String,
    path: NormalizedPath,
    text: String,
    topic: Option<String>,
    event: Option<PeriodEvent>,
}

impl SearchSource {
    /// The chunk this text came from. Derived from the file's identity and the hash of its
    /// content, so naming it names what was read without carrying it.
    pub fn chunk_id(&self) -> &str {
        &self.chunk_id
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// What the analysis concluded about this revision, when it was analysed.
    pub fn topic(&self) -> Option<&str> {
        self.topic.as_deref()
    }

    /// The event that put this source inside the period, when the question was about one.
    pub fn event(&self) -> Option<PeriodEvent> {
        self.event
    }
}

/// What one walk of the matches produced, and what it had to leave out.
struct Collected {
    accepted: Vec<Accepted>,
    hidden_by_policy: usize,
    missing_left_out: usize,
    matches_the_graph_lost: usize,
    folding_stopped_at: Option<usize>,
    filled: bool,
}

pub struct SearchService<'a, Index, Graph> {
    index: &'a Index,
    graph: &'a Graph,
}

impl<'a, Index, Graph> SearchService<'a, Index, Graph>
where
    Index: SearchIndexReader,
    Graph: SearchGraphReader,
{
    pub fn new(index: &'a Index, graph: &'a Graph) -> Self {
        Self { index, graph }
    }

    /// One page of results. `is_visible` says which paths this run may name: the paths its
    /// own context covers, minus the ones its policy excludes.
    ///
    /// Whether a file is still on disk is not in the index, so the index is asked for
    /// matches and the graph decides which of them may be shown. When matches are filtered
    /// out, one round of `limit` matches does not fill a page and another round is asked
    /// for. Each round starts strictly after the previous one, so the walk always advances
    /// and always ends; a query whose every match is filtered walks its whole result set
    /// once, which is the honest price of showing only what is there.
    ///
    /// `within` is the period the query was asked about, when it was. Only what the period
    /// admits is ranked at all; nothing outside it can reach the page, not even to fill it.
    pub fn execute(
        &self,
        request: &SearchRequest,
        within: Option<&PeriodScope>,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<SearchPage, SearchError> {
        if request.query.trim().is_empty() {
            return Ok(SearchPage::nothing());
        }
        let state = self.index.state()?;
        let collected = self.collect(request, within, is_visible)?;
        let accepted = collected.accepted;

        // One call for the whole page: the query is read once and every fragment comes from
        // the same reading of it.
        let texts: Vec<String> = accepted.iter().map(|one| one.text.clone()).collect();
        let fragments = self.index.fragments(&request.query, &texts)?;
        let next = collected
            .filled
            .then(|| accepted.last().map(|one| one.cursor.clone()))
            .flatten();
        let hidden_by_policy = collected.hidden_by_policy;
        let missing_left_out = collected.missing_left_out;
        let matches_the_graph_lost = collected.matches_the_graph_lost;
        let folding_stopped_at = collected.folding_stopped_at;

        Ok(SearchPage {
            results: accepted
                .into_iter()
                .zip(fragments)
                .map(|(one, fragment)| SearchResult {
                    path: one.path,
                    fragment,
                    matches: one.matches,
                    state: one.state,
                    topic: one.topic,
                    event: one.event,
                    conversation: one.conversation,
                    cursor: one.cursor,
                })
                .collect(),
            next,
            hidden_by_policy,
            missing_left_out,
            matches_the_graph_lost,
            folding_stopped_at,
            matches_this_build: state.matches_this_build(),
            indexed_documents: state.documents(),
        })
    }

    /// The matches this run may show, in rank order, with the whole text of each.
    ///
    /// Everything that decides what a reader may see happens here and nowhere else: the
    /// index is asked for matches, the graph says which of them still have a location this
    /// context covers, and a file that is gone is left out unless it was asked for. Both
    /// callers — the page a person reads and the excerpts a question carries — go through
    /// it, so a path hidden from one is hidden from the other.
    ///
    /// When matches are filtered out, one round of `limit` matches does not fill the answer
    /// and another round is asked for. Each round starts strictly after the previous one, so
    /// the walk always advances and always ends.
    fn collect(
        &self,
        request: &SearchRequest,
        within: Option<&PeriodScope>,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<Collected, SearchError> {
        let mut cursor = request.after.clone();
        let mut accepted: Vec<Accepted> = Vec::with_capacity(request.limit);
        let mut hidden_by_policy = 0;
        let mut missing_left_out = 0;
        let mut matches_the_graph_lost = 0;
        let mut folding_stopped_at = None;
        let mut filled = false;

        while accepted.len() < request.limit {
            let matched =
                self.index
                    .matches(&request.query, request.limit, cursor.as_ref(), within)?;
            folding_stopped_at = folding_stopped_at.or(matched.ceiling());
            let documents = matched.into_documents();
            let Some(last) = documents.last() else {
                break;
            };
            cursor = Some(last.cursor().clone());

            let chunk_ids: Vec<String> = documents
                .iter()
                .map(|found| found.chunk_id().to_string())
                .collect();
            let mut resolved: HashMap<String, ResolvedChunk> = self
                .graph
                .resolve(&chunk_ids)?
                .into_iter()
                .map(|chunk| (chunk.chunk_id().to_string(), chunk))
                .collect();

            for found in &documents {
                let Some(chunk) = resolved.remove(found.chunk_id()) else {
                    matches_the_graph_lost += 1;
                    continue;
                };
                let Some((path, state)) = shown_location(&chunk, is_visible) else {
                    hidden_by_policy += 1;
                    continue;
                };
                if state == FileState::Missing && !request.include_missing {
                    missing_left_out += 1;
                    continue;
                }
                accepted.push(Accepted {
                    cursor: found.cursor().clone(),
                    chunk_id: found.chunk_id().to_string(),
                    matches: found.matches(),
                    path,
                    state,
                    topic: chunk.topic().map(str::to_string),
                    conversation: chunk.conversation().cloned(),
                    event: within.and_then(|scope| {
                        scope.event_of(found.chunk_id(), found.cursor().file_id())
                    }),
                    text: chunk.text().to_string(),
                });
                if accepted.len() == request.limit {
                    filled = true;
                    break;
                }
            }
        }

        Ok(Collected {
            accepted,
            hidden_by_policy,
            missing_left_out,
            matches_the_graph_lost,
            folding_stopped_at,
            filled,
        })
    }

    /// The same rows as a page, with the whole text of each rather than a fragment of it.
    ///
    /// What a question carries has to be enough to answer from, and a two-line fragment cut
    /// around the matched word is not. Nothing else differs: same order, same limit, same
    /// rules about what may be shown — and, since Stage 33, the same fold, so a question
    /// carries the best chunk of each of five files instead of five chunks of three.
    pub fn sources(
        &self,
        request: &SearchRequest,
        within: Option<&PeriodScope>,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<Vec<SearchSource>, SearchError> {
        if request.query.trim().is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .collect(request, within, is_visible)?
            .accepted
            .into_iter()
            .map(|one| SearchSource {
                topic: one.topic,
                event: one.event,
                chunk_id: one.chunk_id,
                path: one.path,
                text: one.text,
            })
            .collect())
    }
}

/// One row before its fragment has been cut out of its text.
struct Accepted {
    cursor: DocumentCursor,
    chunk_id: String,
    matches: u32,
    path: NormalizedPath,
    state: FileState,
    topic: Option<String>,
    event: Option<PeriodEvent>,
    conversation: Option<MatchedConversation>,
    text: String,
}

/// Which of a file's paths a result names, and what it says about it.
///
/// Only paths this run may name. Content indexed while a path was admitted must not be
/// named after the policy stopped admitting it, and a file whose every path is hidden
/// produces no result at all. Among the paths that remain, one that still holds the file
/// wins over a retired one, and ties are broken by the path itself so two runs of one query
/// name the same place.
fn shown_location(
    chunk: &ResolvedChunk,
    is_visible: &dyn Fn(&NormalizedPath) -> bool,
) -> Option<(NormalizedPath, FileState)> {
    chunk
        .locations()
        .iter()
        .filter(|(path, _)| is_visible(path))
        .min_by(|(left_path, left_status), (right_path, right_status)| {
            retired(*left_status)
                .cmp(&retired(*right_status))
                .then_with(|| left_path.as_str().cmp(right_path.as_str()))
        })
        .map(|(path, status)| {
            (
                path.clone(),
                match status {
                    LocationStatus::Present => FileState::Present,
                    LocationStatus::Missing => FileState::Missing,
                },
            )
        })
}

fn retired(status: LocationStatus) -> bool {
    status == LocationStatus::Missing
}

#[cfg(test)]
mod tests {
    /// A set that normalizes nothing: these tests are about what a fingerprint does, not
    /// about which languages are in it, and building a real one would reach past this layer.
    fn no_normalizations() -> NormalizationSet {
        NormalizationSet::new(Vec::new()).unwrap()
    }

    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{FileState, SearchRequest, SearchService};
    use crate::application::PeriodScope;
    use crate::application::{
        DocumentMatch, MatchFragment, MatchedDocuments, RepositoryError, ResolvedChunk,
        SearchDocument, SearchError, SearchGraphReader, SearchIndexError, SearchIndexReader,
        SearchIndexState,
    };
    use crate::domain::{
        DocumentCursor, IndexFingerprint, LocationStatus, NormalizationSet, NormalizedPath,
    };

    fn path(value: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(value).unwrap()
    }

    /// An index whose answers are written by the test, so the service is measured against
    /// the order and the paging rule rather than against tantivy.
    ///
    /// It answers in rows, because that is what the port promises: the fold lives where the
    /// ranking is, and one row of a real index already stands for one file. Each entry here
    /// is one file with one matching chunk, which is what the file named by a chunk id is.
    struct FakeIndex {
        /// Every row the query has, best first.
        ranked: Vec<(String, f32)>,
        asked: RefCell<Vec<Option<String>>>,
        fingerprint: Option<IndexFingerprint>,
    }

    /// The file a chunk of these tests belongs to. One chunk per file, so the two names say
    /// the same thing and the tests stay readable.
    fn file_of(chunk_id: &str) -> String {
        format!("file of {chunk_id}")
    }

    impl FakeIndex {
        fn new(ranked: &[(&str, f32)]) -> Self {
            Self {
                ranked: ranked
                    .iter()
                    .map(|(id, score)| ((*id).to_string(), *score))
                    .collect(),
                asked: RefCell::new(Vec::new()),
                fingerprint: Some(IndexFingerprint::of(
                    &["text"],
                    1,
                    "test/1",
                    &no_normalizations(),
                )),
            }
        }

        fn rows(&self) -> Vec<(String, DocumentCursor)> {
            let mut rows: Vec<(String, DocumentCursor)> = self
                .ranked
                .iter()
                .map(|(id, score)| (id.clone(), DocumentCursor::new(*score, file_of(id))))
                .collect();
            rows.sort_by(|left, right| left.1.cmp(&right.1));
            rows
        }
    }

    impl SearchIndexReader for FakeIndex {
        fn matches(
            &self,
            _query: &str,
            limit: usize,
            after: Option<&DocumentCursor>,
            within: Option<&PeriodScope>,
        ) -> Result<MatchedDocuments, SearchIndexError> {
            self.asked
                .borrow_mut()
                .push(after.map(|cursor| cursor.file_id().to_string()));
            Ok(MatchedDocuments::new(
                self.rows()
                    .into_iter()
                    .filter(|(chunk_id, cursor)| {
                        within.is_none_or(|scope| scope.admits(chunk_id, cursor.file_id()))
                    })
                    .filter(|(_, cursor)| after.is_none_or(|after| cursor > after))
                    .take(limit)
                    .map(|(chunk_id, cursor)| DocumentMatch::new(chunk_id, 1, cursor))
                    .collect(),
            ))
        }

        fn fragments(
            &self,
            query: &str,
            texts: &[String],
        ) -> Result<Vec<MatchFragment>, SearchIndexError> {
            Ok(texts
                .iter()
                .map(|text| MatchFragment::new(text.clone(), vec![query.to_string()]))
                .collect())
        }

        fn state(&self) -> Result<SearchIndexState, SearchIndexError> {
            Ok(SearchIndexState::new(
                self.ranked.len() as u64,
                self.fingerprint.clone(),
                IndexFingerprint::of(&["text"], 1, "test/1", &no_normalizations()),
            ))
        }
    }

    /// A graph holding one chunk per file, at the paths and statuses the test names.
    struct FakeGraph {
        chunks: HashMap<String, Vec<(NormalizedPath, LocationStatus)>>,
    }

    impl FakeGraph {
        fn new(chunks: &[(&str, &str, LocationStatus)]) -> Self {
            let mut map: HashMap<String, Vec<(NormalizedPath, LocationStatus)>> = HashMap::new();
            for (chunk_id, at, status) in chunks {
                map.entry((*chunk_id).to_string())
                    .or_default()
                    .push((path(at), *status));
            }
            Self { chunks: map }
        }
    }

    impl SearchGraphReader for FakeGraph {
        fn current_documents(
            &self,
            _after_chunk_id: &str,
            _limit: usize,
        ) -> Result<Vec<SearchDocument>, RepositoryError> {
            unreachable!("the query path does not build documents")
        }

        fn current_versions(
            &self,
            _file_ids: &[String],
        ) -> Result<HashMap<String, String>, RepositoryError> {
            unreachable!("the query path does not publish documents")
        }

        fn current_chunk_count(&self) -> Result<u64, RepositoryError> {
            unreachable!("the query path does not count chunks")
        }

        fn resolve(&self, chunk_ids: &[String]) -> Result<Vec<ResolvedChunk>, RepositoryError> {
            Ok(chunk_ids
                .iter()
                .filter_map(|id| {
                    self.chunks.get(id).map(|locations| {
                        ResolvedChunk::new(id, format!("text of {id}"), locations.clone(), None)
                    })
                })
                .collect())
        }
    }

    fn request(limit: usize, after: Option<&str>, include_missing: bool) -> SearchRequest {
        SearchRequest::new("обход", limit, after, include_missing).unwrap()
    }

    fn everything_is_visible() -> impl Fn(&NormalizedPath) -> bool {
        |_| true
    }

    /// Paging is by rank, so a page smaller than the number of matches still walks every
    /// match exactly once, in one order, however many pages it takes.
    #[test]
    fn pages_partition_the_matches_in_a_stable_order() {
        let index = FakeIndex::new(&[
            ("chunk:a", 3.0),
            ("chunk:b", 2.0),
            ("chunk:c", 2.0),
            ("chunk:d", 1.0),
            ("chunk:e", 1.0),
        ]);
        let graph = FakeGraph::new(&[
            ("chunk:a", "/data/a.md", LocationStatus::Present),
            ("chunk:b", "/data/b.md", LocationStatus::Present),
            ("chunk:c", "/data/c.md", LocationStatus::Present),
            ("chunk:d", "/data/d.md", LocationStatus::Present),
            ("chunk:e", "/data/e.md", LocationStatus::Present),
        ]);
        let service = SearchService::new(&index, &graph);

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..4 {
            let page = service
                .execute(
                    &request(2, cursor.as_deref(), false),
                    None,
                    &everything_is_visible(),
                )
                .unwrap();
            seen.extend(
                page.results()
                    .iter()
                    .map(|result| result.path().as_str().to_string()),
            );
            match page.next() {
                Some(next) => cursor = Some(next.encode()),
                None => break,
            }
        }
        assert_eq!(
            seen,
            vec![
                "/data/a.md",
                "/data/b.md",
                "/data/c.md",
                "/data/d.md",
                "/data/e.md"
            ]
        );

        // And the same page asked for twice is the same page.
        let first = service
            .execute(&request(2, None, false), None, &everything_is_visible())
            .unwrap();
        let again = service
            .execute(&request(2, None, false), None, &everything_is_visible())
            .unwrap();
        assert_eq!(first, again);
    }

    /// A file whose locations are all gone keeps its documents. They are out of the way by
    /// default and marked when asked for, which is the difference between "not on disk" and
    /// "never existed".
    #[test]
    fn a_file_with_no_location_left_is_left_out_unless_it_is_asked_for() {
        let index = FakeIndex::new(&[("chunk:here", 2.0), ("chunk:gone", 1.0)]);
        let graph = FakeGraph::new(&[
            ("chunk:here", "/data/here.md", LocationStatus::Present),
            ("chunk:gone", "/data/gone.md", LocationStatus::Missing),
        ]);
        let service = SearchService::new(&index, &graph);

        let default = service
            .execute(&request(10, None, false), None, &everything_is_visible())
            .unwrap();
        assert_eq!(
            default
                .results()
                .iter()
                .map(|result| result.path().as_str())
                .collect::<Vec<_>>(),
            vec!["/data/here.md"]
        );
        assert_eq!(default.missing_left_out(), 1);

        let asked = service
            .execute(&request(10, None, true), None, &everything_is_visible())
            .unwrap();
        assert_eq!(
            asked
                .results()
                .iter()
                .map(|result| (result.path().as_str(), result.state()))
                .collect::<Vec<_>>(),
            vec![
                ("/data/here.md", FileState::Present),
                ("/data/gone.md", FileState::Missing)
            ]
        );
        assert_eq!(asked.missing_left_out(), 0);
    }

    /// A path the policy stopped admitting is not named, not even to say it was skipped.
    #[test]
    fn a_match_whose_paths_are_all_hidden_is_counted_and_never_named() {
        let index = FakeIndex::new(&[("chunk:secret", 2.0), ("chunk:open", 1.0)]);
        let graph = FakeGraph::new(&[
            (
                "chunk:secret",
                "/data/private/secret.md",
                LocationStatus::Present,
            ),
            ("chunk:open", "/data/open.md", LocationStatus::Present),
        ]);
        let service = SearchService::new(&index, &graph);

        let page = service
            .execute(&request(10, None, false), None, &|path: &NormalizedPath| {
                !path.as_str().contains("/private/")
            })
            .unwrap();
        assert_eq!(
            page.results()
                .iter()
                .map(|result| result.path().as_str())
                .collect::<Vec<_>>(),
            vec!["/data/open.md"]
        );
        assert_eq!(page.hidden_by_policy(), 1);
        assert!(
            !format!("{page:?}").contains("secret"),
            "a hidden path must not appear in what the page carries"
        );
    }

    /// A file at two paths is named by the one that still holds it, whichever order the
    /// graph returned them in.
    #[test]
    fn a_file_at_several_paths_is_named_by_one_that_still_holds_it() {
        let index = FakeIndex::new(&[("chunk:linked", 1.0)]);
        for order in [
            vec![
                ("chunk:linked", "/data/retired.md", LocationStatus::Missing),
                ("chunk:linked", "/data/kept.md", LocationStatus::Present),
            ],
            vec![
                ("chunk:linked", "/data/kept.md", LocationStatus::Present),
                ("chunk:linked", "/data/retired.md", LocationStatus::Missing),
            ],
        ] {
            let graph = FakeGraph::new(&order);
            let page = SearchService::new(&index, &graph)
                .execute(&request(10, None, true), None, &everything_is_visible())
                .unwrap();
            assert_eq!(page.results()[0].path().as_str(), "/data/kept.md");
            assert_eq!(page.results()[0].state(), FileState::Present);
        }
    }

    /// Filtering must not cut a page short: another round is asked for until the page is
    /// full or the matches run out.
    #[test]
    fn a_page_is_filled_even_when_most_matches_are_filtered_out() {
        let ranked: Vec<(String, f32)> = (0..10)
            .map(|index| (format!("chunk:{index:02}"), 10.0 - index as f32))
            .collect();
        let index = FakeIndex::new(
            &ranked
                .iter()
                .map(|(id, score)| (id.as_str(), *score))
                .collect::<Vec<_>>(),
        );
        // Only every fifth match may be shown.
        let graph = FakeGraph::new(
            &ranked
                .iter()
                .enumerate()
                .map(|(position, (id, _))| {
                    (
                        id.as_str(),
                        if position % 5 == 0 {
                            "/data/shown.md"
                        } else {
                            "/data/hidden/one.md"
                        },
                        LocationStatus::Present,
                    )
                })
                .collect::<Vec<_>>(),
        );

        let page = SearchService::new(&index, &graph)
            .execute(&request(2, None, false), None, &|path: &NormalizedPath| {
                !path.as_str().contains("/hidden/")
            })
            .unwrap();
        assert_eq!(page.results().len(), 2, "the page must be full");
        assert_eq!(
            page.hidden_by_policy(),
            4,
            "only the matches walked past on the way to a full page are counted"
        );
        assert!(
            index.asked.borrow().len() > 1,
            "filling the page must have asked the index for more than one round"
        );
    }

    /// A match the graph no longer holds is dropped and counted, never shown from whatever
    /// the index remembers.
    #[test]
    fn a_match_the_graph_lost_is_dropped_and_counted() {
        let index = FakeIndex::new(&[("chunk:stale", 2.0), ("chunk:real", 1.0)]);
        let graph = FakeGraph::new(&[("chunk:real", "/data/real.md", LocationStatus::Present)]);
        let page = SearchService::new(&index, &graph)
            .execute(&request(10, None, false), None, &everything_is_visible())
            .unwrap();
        assert_eq!(page.results().len(), 1);
        assert_eq!(page.matches_the_graph_lost(), 1);
    }

    #[test]
    fn an_empty_query_answers_nothing_without_touching_the_index() {
        let index = FakeIndex::new(&[("chunk:a", 1.0)]);
        let graph = FakeGraph::new(&[("chunk:a", "/data/a.md", LocationStatus::Present)]);
        let page = SearchService::new(&index, &graph)
            .execute(
                &SearchRequest::new("   ", 10, None, false).unwrap(),
                None,
                &everything_is_visible(),
            )
            .unwrap();
        assert!(page.results().is_empty());
        assert_eq!(page.next(), None);
        assert!(index.asked.borrow().is_empty());
    }

    #[test]
    fn a_cursor_this_program_did_not_write_is_refused() {
        assert_eq!(
            SearchRequest::new("обход", 10, Some("nonsense"), false).unwrap_err(),
            SearchError::UnreadableCursor
        );
    }

    /// An index written by another analyzer answers by rules nobody applies any more, and
    /// the page says so instead of letting the answer pass for a current one.
    #[test]
    fn an_index_written_by_another_analyzer_is_reported_on_the_page() {
        let mut index = FakeIndex::new(&[("chunk:a", 1.0)]);
        index.fingerprint = Some(IndexFingerprint::stored("tokenizer=old/0;normalizations="));
        let graph = FakeGraph::new(&[("chunk:a", "/data/a.md", LocationStatus::Present)]);
        let page = SearchService::new(&index, &graph)
            .execute(&request(10, None, false), None, &everything_is_visible())
            .unwrap();
        assert!(!page.matches_this_build());
        assert_eq!(
            page.results().len(),
            1,
            "it still answers, with what it has"
        );
    }
}
