// Use case: the search index of a context against the graph it is derived from — what it
// holds, whether that is what the graph holds, and writing it again from scratch when it
// is not.
//
// Rebuilding is the only repair there is, and that is a property of the design rather than
// a gap in it: the index carries nothing the graph does not, so throwing it away costs one
// pass and loses nothing.
use super::search_ports::{
    SearchError, SearchGraphReader, SearchIndexReader, SearchIndexState, SearchIndexWriter,
};

/// How many documents one page of a rebuild carries.
///
/// The same reasoning as the v12 backfill and the insight reader: chunk text is heavy, so
/// the page is small and the memory of a rebuild does not grow with the size of the corpus.
const REBUILD_PAGE: usize = 200;

pub struct SearchIndexService<'a, Index, Graph> {
    index: &'a Index,
    graph: &'a Graph,
}

impl<'a, Index, Graph> SearchIndexService<'a, Index, Graph>
where
    Index: SearchIndexWriter + SearchIndexReader,
    Graph: SearchGraphReader,
{
    pub fn new(index: &'a Index, graph: &'a Graph) -> Self {
        Self { index, graph }
    }

    /// Empties the index and writes every current version's chunks into it again.
    ///
    /// The graph is only read, and the new index becomes visible in one step at the end, so
    /// a rebuild that is interrupted leaves the index it was replacing — out of step in
    /// whatever way it already was, and no worse.
    pub fn rebuild(&self) -> Result<SearchIndexReport, SearchError> {
        self.index.clear()?;
        let mut after = String::new();
        loop {
            let page = self.graph.current_documents(&after, REBUILD_PAGE)?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.chunk_id().to_string();
            self.index.append(page)?;
        }
        self.index.finish()?;
        self.status()
    }

    /// What the index holds, against what the graph holds.
    pub fn status(&self) -> Result<SearchIndexReport, SearchError> {
        Ok(SearchIndexReport {
            state: self.index.state()?,
            chunks: self.graph.current_chunk_count()?,
        })
    }
}

/// The index and the graph side by side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIndexReport {
    state: SearchIndexState,
    chunks: u64,
}

impl SearchIndexReport {
    pub fn documents(&self) -> u64 {
        self.state.documents()
    }

    /// How many chunks the current versions hold: what an index in step with this graph
    /// would have a document for. Earlier revisions are not counted, because they are not
    /// indexed.
    pub fn chunks(&self) -> u64 {
        self.chunks
    }

    /// What the index says about itself, beside what this build would write.
    pub fn state(&self) -> &SearchIndexState {
        &self.state
    }

    /// Whether the index holds a document per chunk.
    ///
    /// A count, which is a signal and not a proof: equal counts could in principle hide a
    /// swapped document. It is sound for the failure it has to catch — a write that never
    /// happened or a process that died partway — because the index is committed atomically
    /// and a batch that did not land leaves strictly fewer documents. A stronger check
    /// would not earn its cost when the answer to either is the same rebuild.
    pub fn counts_agree(&self) -> bool {
        self.state.documents() == self.chunks
    }

    /// Whether the index was written by the rules this build applies.
    ///
    /// Apart from the counts, because the two are different failures with one repair: an
    /// index behind the graph is missing text, while an index written by other rules holds
    /// what it holds in a form this build's queries do not reach — terms another analyzer
    /// produced, or a field a query filters on that its documents were written without.
    pub fn matches_this_build(&self) -> bool {
        self.state.matches_this_build()
    }
}

#[cfg(test)]
mod tests {
    /// A set that normalizes nothing: see `search.rs`.
    fn no_normalizations() -> NormalizationSet {
        NormalizationSet::new(Vec::new()).unwrap()
    }

    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{REBUILD_PAGE, SearchIndexService};
    use crate::application::{
        MatchFragment, MatchedDocuments, RepositoryError, ResolvedChunk, SearchDocument,
        SearchGraphReader, SearchIndexError, SearchIndexReader, SearchIndexState,
        SearchIndexWriter,
    };
    use crate::domain::{DocumentCursor, IndexFingerprint, NormalizationSet};

    fn fingerprint() -> IndexFingerprint {
        IndexFingerprint::of(&["text"], 1, "test/1", &no_normalizations())
    }

    /// An index that records what it was told to do, in order.
    #[derive(Default)]
    struct FakeIndex {
        documents: RefCell<Vec<String>>,
        calls: RefCell<Vec<String>>,
        written_by: RefCell<Option<IndexFingerprint>>,
    }

    impl SearchIndexWriter for FakeIndex {
        fn publish(
            &self,
            _files: &[String],
            _documents: Vec<SearchDocument>,
        ) -> Result<(), SearchIndexError> {
            unreachable!("a rebuild appends")
        }

        fn append(&self, documents: Vec<SearchDocument>) -> Result<(), SearchIndexError> {
            self.calls
                .borrow_mut()
                .push(format!("append {}", documents.len()));
            self.documents.borrow_mut().extend(
                documents
                    .iter()
                    .map(|document| document.chunk_id().to_string()),
            );
            *self.written_by.borrow_mut() = Some(fingerprint());
            Ok(())
        }

        fn clear(&self) -> Result<(), SearchIndexError> {
            self.calls.borrow_mut().push("clear".to_string());
            self.documents.borrow_mut().clear();
            Ok(())
        }

        fn finish(&self) -> Result<(), SearchIndexError> {
            self.calls.borrow_mut().push("finish".to_string());
            Ok(())
        }
    }

    impl SearchIndexReader for FakeIndex {
        fn matches(
            &self,
            _query: &str,
            _limit: usize,
            _after: Option<&DocumentCursor>,
            _within: Option<&crate::application::PeriodScope>,
        ) -> Result<MatchedDocuments, SearchIndexError> {
            unreachable!("maintenance does not query")
        }

        fn fragments(
            &self,
            _query: &str,
            _texts: &[String],
        ) -> Result<Vec<MatchFragment>, SearchIndexError> {
            unreachable!("maintenance does not query")
        }

        fn state(&self) -> Result<SearchIndexState, SearchIndexError> {
            Ok(SearchIndexState::new(
                self.documents.borrow().len() as u64,
                self.written_by.borrow().clone(),
                fingerprint(),
            ))
        }
    }

    /// A graph holding `chunks` chunks of current versions, paged by chunk id.
    struct FakeGraph {
        chunk_ids: Vec<String>,
    }

    impl FakeGraph {
        fn with(chunks: usize) -> Self {
            Self {
                chunk_ids: (0..chunks)
                    .map(|index| format!("chunk:{index:06}"))
                    .collect(),
            }
        }
    }

    impl SearchGraphReader for FakeGraph {
        fn current_documents(
            &self,
            after_chunk_id: &str,
            limit: usize,
        ) -> Result<Vec<SearchDocument>, RepositoryError> {
            Ok(self
                .chunk_ids
                .iter()
                .filter(|id| id.as_str() > after_chunk_id)
                .take(limit)
                .map(|id| SearchDocument::new(id, "file:one", None, format!("text of {id}")))
                .collect())
        }

        fn current_versions(
            &self,
            _file_ids: &[String],
        ) -> Result<HashMap<String, String>, RepositoryError> {
            unreachable!("maintenance does not publish a batch")
        }

        fn current_chunk_count(&self) -> Result<u64, RepositoryError> {
            Ok(self.chunk_ids.len() as u64)
        }

        fn resolve(&self, _chunk_ids: &[String]) -> Result<Vec<ResolvedChunk>, RepositoryError> {
            unreachable!("maintenance does not resolve matches")
        }
    }

    /// A rebuild empties the index first and then walks the graph in bounded pages, and the
    /// index it leaves is in step.
    #[test]
    fn a_rebuild_empties_the_index_and_writes_every_current_chunk_in_pages() {
        let index = FakeIndex::default();
        let graph = FakeGraph::with(REBUILD_PAGE * 2 + 3);
        let report = SearchIndexService::new(&index, &graph).rebuild().unwrap();

        assert_eq!(
            index.calls.borrow().as_slice(),
            &[
                "clear".to_string(),
                format!("append {REBUILD_PAGE}"),
                format!("append {REBUILD_PAGE}"),
                "append 3".to_string(),
                "finish".to_string(),
            ],
            "one clear, the graph in pages, and one step that makes all of it visible"
        );
        assert_eq!(report.documents(), report.chunks());
        assert!(report.counts_agree() && report.matches_this_build());
    }

    /// An empty graph gives an empty index, which is in step with it rather than behind.
    #[test]
    fn an_empty_graph_leaves_an_empty_index_in_step() {
        let index = FakeIndex::default();
        let graph = FakeGraph::with(0);
        let report = SearchIndexService::new(&index, &graph).rebuild().unwrap();
        assert_eq!(
            index.calls.borrow().as_slice(),
            &["clear".to_string(), "finish".to_string()]
        );
        assert_eq!((report.documents(), report.chunks()), (0, 0));
        assert!(report.counts_agree() && report.matches_this_build());
    }

    /// Fewer documents than chunks is what a scan that could not write the index, or a
    /// rebuild that died partway, leaves behind.
    #[test]
    fn an_index_with_fewer_documents_than_chunks_is_reported_as_behind() {
        let index = FakeIndex::default();
        let graph = FakeGraph::with(10);
        index
            .append(vec![SearchDocument::new(
                "chunk:000000",
                "file:one",
                None,
                "text",
            )])
            .unwrap();

        let report = SearchIndexService::new(&index, &graph).status().unwrap();
        assert_eq!((report.documents(), report.chunks()), (1, 10));
        assert!(!report.counts_agree());
        assert!(
            report.matches_this_build(),
            "the analyzer is the current one"
        );
    }

    /// An index written by another analyzer is out of step even when every document is
    /// there: its terms were produced by rules nobody applies to a query any more.
    #[test]
    fn an_index_written_by_another_analyzer_is_reported_as_out_of_step() {
        let index = FakeIndex::default();
        let graph = FakeGraph::with(1);
        index
            .append(vec![SearchDocument::new(
                "chunk:000000",
                "file:one",
                None,
                "text",
            )])
            .unwrap();
        *index.written_by.borrow_mut() =
            Some(IndexFingerprint::stored("tokenizer=old/0;normalizations="));

        let report = SearchIndexService::new(&index, &graph).status().unwrap();
        assert!(report.counts_agree());
        assert!(!report.matches_this_build());
    }
}
