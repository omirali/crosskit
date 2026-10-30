// Keeping the search index of a context up to date as a scan publishes sources.
//
// Two rules hold this together, and both are about order.
//
// The graph transaction commits first, and only then is the index written. An index written
// first could describe a transaction that was then rolled back, and no count would ever
// reveal it; written second, the worst case is an index one batch behind, which `doctor`
// finds and a rebuild repairs.
//
// And what goes into the index is decided by the graph, not by the scan. A batch proposes a
// version as the current one, and the graph accepts it only when it is newer than what it
// already holds — a late or repeated observation changes nothing. So after the commit the
// graph is asked which version each file now has, and only the sources whose version won
// are indexed. Without that question, a scan racing another process could put the text of a
// version nobody considers current into the index, with the document count still matching
// and nothing to show for it.
use std::cell::RefCell;
use std::collections::HashMap;

use crate::domain::{ExtractionProfile, SourceCommit, SourceLanguage};

use super::search_ports::{
    SearchDocument, SearchError, SearchGraphReader, SearchIndexReader, SearchIndexWriter,
};

/// The search index a run keeps up to date, and everything that stopped it from doing so.
///
/// A failure is kept rather than returned. The graph write it belongs to has already
/// committed, and failing the command afterwards would report a scan that did happen as one
/// that did not. What it costs is an index that lags, which is discoverable by counting and
/// repaired by rebuilding — that is the whole reason the index is a derivative.
pub struct SearchIndexUpdate<'a> {
    index: &'a dyn SearchIndexWriter,
    graph: &'a dyn SearchGraphReader,
    failures: RefCell<Vec<SearchError>>,
    /// Whether every file this run reads is published, rather than only the ones whose
    /// current version changed. Set when the index was already behind the graph before the
    /// run started; see `republishing_everything`.
    republish_everything: bool,
}

impl<'a> SearchIndexUpdate<'a> {
    pub fn new(index: &'a dyn SearchIndexWriter, graph: &'a dyn SearchGraphReader) -> Self {
        Self {
            index,
            graph,
            failures: RefCell::new(Vec::new()),
            republish_everything: false,
        }
    }

    /// The update a run keeps its index with, in whichever of the two modes fits the index
    /// it finds.
    ///
    /// One place decides, because the decision is a rule of the use case and not of any
    /// caller: an index already behind the graph is repaired by publishing everything the
    /// run reads, and an index in step is kept up to date by publishing only what changed.
    /// A caller that built the update itself would be a second rule.
    ///
    /// Either question failing to answer means behind. That is the safe direction:
    /// publishing everything is always correct and only costs work, while skipping on a
    /// guess would leave an index short of documents with nothing to notice it.
    pub fn for_run<Index>(index: &'a Index, graph: &'a dyn SearchGraphReader) -> Self
    where
        Index: SearchIndexWriter + SearchIndexReader,
    {
        let behind = match (index.state(), graph.current_chunk_count()) {
            (Ok(state), Ok(expected)) => state.documents() != expected,
            _ => true,
        };
        let update = Self::new(index, graph);
        if behind {
            return update.republishing_everything();
        }
        update
    }

    /// Publish every file this run reads, whatever the graph already called current.
    ///
    /// This is what every pass used to do unconditionally, and it is what repairs an index
    /// that fell behind: a pass whose graph transaction committed while its index was never
    /// made visible leaves documents missing, and the next ordinary pass writes them again
    /// out of what it reads anyway. Nobody has to ask for a rebuild.
    ///
    /// It is asked for only when the index is actually behind, because the ordinary case is
    /// the opposite one: a reconcile of an unchanged tree stages every file of the corpus,
    /// and republishing all of them deletes and re-adds text the index already holds. The
    /// deletion queue that costs is the one thing a pass holds without bound — 1.24 KB per
    /// file, measured in Stage 19 — so paying it for work that changes nothing is what this
    /// flag exists to avoid.
    pub fn republishing_everything(mut self) -> Self {
        self.republish_everything = true;
        self
    }

    /// What a batch would add to the index, taken from the sources while they are still in
    /// hand.
    ///
    /// The text is copied here because publishing consumes the sources and the index is
    /// written after the commit. One batch of chunk text is what that copy costs, and the
    /// batch payload limit already bounds it; reading the same text back out of the graph
    /// instead would cost a full extra read of everything just written.
    ///
    /// The language comes from the path this commit was made for, which is the name the scan
    /// just walked. A file at several paths is labelled by the one that was walked last, and
    /// a rebuild — which sees every name the graph knows — may reach a different answer for
    /// exactly those files; the difference is measured and named in ADR 0035.
    pub(super) fn stage(&self, commits: &[SourceCommit]) -> StagedDocuments {
        let files: Vec<StagedFile> = commits
            .iter()
            .filter_map(|commit| Some((commit.evidence()?, commit.location().path())))
            .map(|(evidence, path)| {
                let language = SourceLanguage::of_path(path);
                StagedFile {
                    file_id: evidence.file_version().file_id().to_string(),
                    version_id: evidence.file_version().id().to_string(),
                    documents: evidence
                        .chunks()
                        .iter()
                        .map(|chunk| {
                            let document = SearchDocument::new(
                                chunk.id(),
                                evidence.file_version().file_id(),
                                language,
                                chunk.text(),
                            );
                            // The name chunk of a conversation, decided by the one rule both
                            // builders share so a rebuilt index holds the same documents as a
                            // freshly written one.
                            match ExtractionProfile::of_extractor_id(chunk.extractor_id())
                                .is_some_and(|profile| {
                                    profile.chunk_is_the_conversation_name(chunk.ordinal())
                                }) {
                                true => document.naming_a_conversation(chunk.text()),
                                false => document,
                            }
                        })
                        .collect(),
                }
            })
            .collect();
        StagedDocuments {
            current_before: self.current_versions_of(&files),
            files,
        }
    }

    /// Which version the graph calls current for these files *before* this batch commits.
    ///
    /// It is what tells a source that changed from one that was merely seen again. A scan
    /// reads every file it walks, so a reconcile of an unchanged tree stages every file of
    /// the corpus; without this reading each of them would be deleted from the index and
    /// added back, and Stage 19 measured what that costs — the deletion queue is the one
    /// thing a pass holds without bound, at 1.24 KB per file.
    ///
    /// A graph that cannot answer leaves this empty, which makes every file look changed.
    /// That is the safe direction: the index is written as it was written before this, and
    /// the failure is recorded rather than turned into an index quietly left behind.
    fn current_versions_of(&self, files: &[StagedFile]) -> HashMap<String, String> {
        if files.is_empty() || self.republish_everything {
            return HashMap::new();
        }
        let file_ids: Vec<String> = files.iter().map(|file| file.file_id.clone()).collect();
        match self.graph.current_versions(&file_ids) {
            Ok(current) => current,
            Err(error) => {
                self.record(SearchError::Graph(error));
                HashMap::new()
            }
        }
    }

    /// Stages the documents of the files whose version the graph accepted as current, and
    /// the removal of the documents of the versions they replaced. `finish` makes all of it
    /// visible at the end of the run.
    pub(super) fn publish(&self, staged: StagedDocuments) {
        if staged.files.is_empty() {
            return;
        }
        let file_ids: Vec<String> = staged
            .files
            .iter()
            .map(|file| file.file_id.clone())
            .collect();
        let current = match self.graph.current_versions(&file_ids) {
            Ok(current) => current,
            Err(error) => return self.record(SearchError::Graph(error)),
        };

        // Two conditions, and they are different questions. The first is whose version the
        // graph accepted: a batch that lost the race must not write the index. The second is
        // whether anything about this file's documents changed at all — a file whose current
        // version was already this one holds exactly the documents the index already has, so
        // republishing it would delete and re-add the same text. A reconcile of an unchanged
        // tree stages every file of the corpus and passes the first condition for all of
        // them, which is why the second one exists.
        let accepted: Vec<StagedFile> = staged
            .files
            .into_iter()
            .filter(|file| current.get(&file.file_id) == Some(&file.version_id))
            .filter(|file| {
                self.republish_everything
                    || staged.current_before.get(&file.file_id) != Some(&file.version_id)
            })
            .collect();
        if accepted.is_empty() {
            return;
        }

        // A file the graph had no current version for has no documents in the index either,
        // so there is nothing to delete and the deletion is not staged. That is the whole of
        // a cold pass: every file is new, and an index built without a single deletion holds
        // nothing back — the writer spills its documents to disk on its own, while the
        // deletion queue is what grows unbounded.
        //
        // Trusting the graph about what the index holds is only sound because this mode runs
        // only over an index that is in step with it; a run that found the index behind
        // republishes, and there `current_before` is deliberately empty, so every file takes
        // the deleting path.
        let (fresh, replaced): (Vec<StagedFile>, Vec<StagedFile>) =
            accepted.into_iter().partition(|file| {
                !self.republish_everything && !staged.current_before.contains_key(&file.file_id)
            });

        if !fresh.is_empty() {
            let documents: Vec<SearchDocument> =
                fresh.into_iter().flat_map(|file| file.documents).collect();
            if let Err(error) = self.index.append(documents) {
                self.record(SearchError::Index(error));
            }
        }
        if replaced.is_empty() {
            return;
        }
        // The file ids are named apart from the documents: a file whose new current version
        // holds no text still has to lose the documents of the version before it.
        let files: Vec<String> = replaced.iter().map(|file| file.file_id.clone()).collect();
        let documents: Vec<SearchDocument> = replaced
            .into_iter()
            .flat_map(|file| file.documents)
            .collect();
        if let Err(error) = self.index.publish(&files, documents) {
            self.record(SearchError::Index(error));
        }
    }

    /// Makes everything this run wrote visible and lets the index settle. Called once, when
    /// the run is done publishing.
    pub(super) fn finish(&self) {
        if let Err(error) = self.index.finish() {
            self.record(SearchError::Index(error));
        }
    }

    /// Everything that stopped the index from being written during this run.
    pub fn failures(&self) -> Vec<SearchError> {
        self.failures.borrow().clone()
    }

    /// One failure per batch at most, and the same failure is not recorded twice: a lock
    /// held by another process fails every batch of a long scan, and a hundred identical
    /// lines say no more than one.
    fn record(&self, failure: SearchError) {
        let mut failures = self.failures.borrow_mut();
        if !failures.contains(&failure) {
            failures.push(failure);
        }
    }
}

/// What one batch would put in the index, per file, and what the graph called current
/// before the batch committed.
pub(super) struct StagedDocuments {
    files: Vec<StagedFile>,
    current_before: HashMap<String, String>,
}

struct StagedFile {
    file_id: String,
    version_id: String,
    documents: Vec<SearchDocument>,
}

/// Publishes a batch to the graph and then, when this run keeps a search index, to that
/// index.
///
/// The closure is what the caller publishes with: a plain commit for `index init`, a commit
/// on behalf of a run for `index reconcile`. Both get the same order and the same failure
/// handling, written once.
pub(super) struct IndexingSink<'a, Publish> {
    publish: Publish,
    search: Option<&'a SearchIndexUpdate<'a>>,
}

impl<'a, Publish> IndexingSink<'a, Publish> {
    pub(super) fn new(publish: Publish, search: Option<&'a SearchIndexUpdate<'a>>) -> Self {
        Self { publish, search }
    }
}

impl<Publish> super::policy_scan::SourceSink for IndexingSink<'_, Publish>
where
    Publish: FnMut(Vec<SourceCommit>) -> Result<(), super::repository::RepositoryError>,
{
    fn commit_batch(
        &mut self,
        commits: Vec<SourceCommit>,
    ) -> Result<(), super::repository::RepositoryError> {
        let staged = self.search.map(|search| search.stage(&commits));
        (self.publish)(commits)?;
        if let (Some(search), Some(staged)) = (self.search, staged) {
            search.publish(staged);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::AnalysisRequest;

    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{IndexingSink, SearchIndexUpdate};
    use crate::application::policy_scan::SourceSink;
    use crate::application::{
        RepositoryError, ResolvedChunk, SearchDocument, SearchError, SearchGraphReader,
        SearchIndexError, SearchIndexWriter,
    };
    use crate::domain::{
        ChunkScript, ContentChunk, EvidenceCommit, FileKind, FileLocation, FileObservation,
        FileVersion, NormalizedPath, SourceCommit,
    };

    /// One source with one chunk of `text`, at revision `revision`.
    fn commit(file_id: &str, revision: &str, text: &str) -> SourceCommit {
        let version =
            FileVersion::new(format!("{file_id}@{revision}"), file_id, revision, 100).unwrap();
        let observation = FileObservation::new(
            format!("observation:{file_id}:{revision}"),
            file_id,
            version.id(),
            100,
        )
        .unwrap();
        let chunk = ContentChunk::new(
            format!("chunk:{file_id}:{revision}"),
            version.id(),
            0,
            0,
            text.len() as u64,
            format!("hash:{revision}"),
            text,
            ChunkScript::of(text),
            "plain-text",
            "plain-text",
            "1",
            "config:1",
            version.content_hash(),
            100,
        )
        .unwrap();
        SourceCommit::with_evidence(
            FileLocation::observed(
                file_id,
                NormalizedPath::from_absolute(&format!("/data/{file_id}.md")).unwrap(),
                FileKind::RegularFile,
                text.len() as u64,
                100,
                100,
            )
            .unwrap(),
            EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
            AnalysisRequest::Requested,
        )
        .unwrap()
    }

    /// Records what was written, by which of the two ways, and in which order; fails on
    /// demand. Which way a file takes is the claim of Stage 19: a file the index has no
    /// documents for is appended, and only a file whose documents are being replaced pays
    /// for a deletion.
    #[derive(Default)]
    struct FakeIndex {
        published: RefCell<Vec<(Vec<String>, Vec<String>)>>,
        appended: RefCell<Vec<Vec<String>>>,
        broken: bool,
    }

    fn chunk_ids(documents: &[SearchDocument]) -> Vec<String> {
        documents
            .iter()
            .map(|document| document.chunk_id().to_string())
            .collect()
    }

    impl SearchIndexWriter for FakeIndex {
        fn publish(
            &self,
            files: &[String],
            documents: Vec<SearchDocument>,
        ) -> Result<(), SearchIndexError> {
            if self.broken {
                return Err(SearchIndexError::Busy);
            }
            self.published
                .borrow_mut()
                .push((files.to_vec(), chunk_ids(&documents)));
            Ok(())
        }

        fn append(&self, documents: Vec<SearchDocument>) -> Result<(), SearchIndexError> {
            if self.broken {
                return Err(SearchIndexError::Busy);
            }
            self.appended.borrow_mut().push(chunk_ids(&documents));
            Ok(())
        }

        fn clear(&self) -> Result<(), SearchIndexError> {
            unreachable!("the scan path never clears")
        }

        fn finish(&self) -> Result<(), SearchIndexError> {
            Ok(())
        }
    }

    /// A graph that answers which version it accepted as current.
    /// The graph as the index sees it: what it calls current, and the ability to change
    /// that between the two moments a batch asks. A batch stages before its transaction
    /// commits and publishes after, and telling a file that changed from one that was only
    /// seen again depends on those two answers differing — so a dummy that could not differ
    /// would make the distinction untestable.
    struct FakeGraph {
        current: RefCell<HashMap<String, String>>,
    }

    impl FakeGraph {
        fn holding(pairs: &[(&str, &str)]) -> Self {
            Self {
                current: RefCell::new(
                    pairs
                        .iter()
                        .map(|(file, version)| ((*file).to_string(), (*version).to_string()))
                        .collect(),
                ),
            }
        }

        /// What the commit of a batch does to the graph: these files now have these versions
        /// as current.
        fn accepts(&self, pairs: &[(&str, &str)]) {
            let mut current = self.current.borrow_mut();
            for (file, version) in pairs {
                current.insert((*file).to_string(), (*version).to_string());
            }
        }
    }

    impl SearchGraphReader for FakeGraph {
        fn current_documents(
            &self,
            _after_chunk_id: &str,
            _limit: usize,
        ) -> Result<Vec<SearchDocument>, RepositoryError> {
            unreachable!("the scan path does not rebuild")
        }

        fn current_versions(
            &self,
            file_ids: &[String],
        ) -> Result<HashMap<String, String>, RepositoryError> {
            let current = self.current.borrow();
            Ok(file_ids
                .iter()
                .filter_map(|id| current.get(id).map(|version| (id.clone(), version.clone())))
                .collect())
        }

        fn current_chunk_count(&self) -> Result<u64, RepositoryError> {
            unreachable!("the scan path does not count chunks")
        }

        fn resolve(&self, _chunk_ids: &[String]) -> Result<Vec<ResolvedChunk>, RepositoryError> {
            unreachable!("the scan path does not resolve matches")
        }
    }

    /// The graph commits first. The index is written from what it accepted, and a batch it
    /// rejected leaves the index alone.
    #[test]
    fn only_the_version_the_graph_accepted_is_indexed() {
        let index = FakeIndex::default();
        // Before the batch: neither file has a current version yet for `file:a`, and
        // `file:b` already has one newer than what this batch carries.
        let graph = FakeGraph::holding(&[("file:b", "file:b@already-newer")]);
        let update = SearchIndexUpdate::new(&index, &graph);
        let mut sink = IndexingSink::new(
            |_commits: Vec<SourceCommit>| {
                // The transaction commits: the graph accepts `file:a` and keeps the newer
                // revision of `file:b`.
                graph.accepts(&[("file:a", "file:a@new")]);
                Ok(())
            },
            Some(&update),
        );

        sink.commit_batch(vec![
            commit("file:a", "new", "обход индекса"),
            commit("file:b", "stale", "старая ревизия"),
        ])
        .unwrap();

        assert_eq!(
            index.appended.borrow().as_slice(),
            &[vec!["chunk:file:a:new".to_string()]],
            "the accepted file is new to the graph, so it is added without a deletion"
        );
        assert!(
            index.published.borrow().is_empty(),
            "nothing here replaces documents the index already holds"
        );
        assert!(update.failures().is_empty());
    }

    /// The claim of Stage 19: a pass that reads a file whose current version is already
    /// what it read does not touch the index at all.
    ///
    /// A scan reads every file it walks, so a reconcile of an unchanged tree stages the
    /// whole corpus and every one of those files is still the current version — which used
    /// to mean the whole index was deleted and written again on every pass. The deletion
    /// queue that costs is the one thing a pass holds without bound.
    #[test]
    fn a_file_whose_current_version_did_not_change_is_not_published_again() {
        let index = FakeIndex::default();
        let graph = FakeGraph::holding(&[]);
        let update = SearchIndexUpdate::new(&index, &graph);
        {
            let mut sink = IndexingSink::new(
                |commits: Vec<SourceCommit>| {
                    for commit in &commits {
                        if let Some(evidence) = commit.evidence() {
                            graph.accepts(&[(
                                evidence.file_version().file_id(),
                                evidence.file_version().id(),
                            )]);
                        }
                    }
                    Ok(())
                },
                Some(&update),
            );

            // The first pass: the file is new to the graph, so its documents are written.
            sink.commit_batch(vec![commit("file:a", "one", "первая ревизия")])
                .unwrap();
            // A second pass over a tree where nothing changed reads the same file again and
            // publishes the same version. The graph already calls it current.
            sink.commit_batch(vec![commit("file:a", "one", "первая ревизия")])
                .unwrap();
            // And a pass where the file did change writes it again.
            sink.commit_batch(vec![commit("file:a", "two", "вторая ревизия")])
                .unwrap();
        }

        assert_eq!(
            index.appended.borrow().as_slice(),
            &[vec!["chunk:file:a:one".to_string()]],
            "the first pass adds a file the index did not have, with no deletion"
        );
        assert_eq!(
            index.published.borrow().as_slice(),
            &[(
                vec!["file:a".to_string()],
                vec!["chunk:file:a:two".to_string()]
            )],
            "only the revision that replaced another one pays for a deletion, and the pass \
             over an unchanged tree between them wrote nothing at all"
        );
        assert!(update.failures().is_empty());
    }

    /// The other half of the rule, and the reason it is safe: a run that finds the index
    /// already behind publishes everything it reads, which is what repairs it.
    #[test]
    fn a_run_over_an_index_that_is_behind_publishes_even_what_did_not_change() {
        let index = FakeIndex::default();
        let graph = FakeGraph::holding(&[("file:a", "file:a@one")]);
        let update = SearchIndexUpdate::new(&index, &graph).republishing_everything();
        {
            let mut sink = IndexingSink::new(|_: Vec<SourceCommit>| Ok(()), Some(&update));
            sink.commit_batch(vec![commit("file:a", "one", "первая ревизия")])
                .unwrap();
        }

        assert_eq!(
            index.published.borrow().as_slice(),
            &[(
                vec!["file:a".to_string()],
                vec!["chunk:file:a:one".to_string()]
            )],
            "an index short of documents is written again out of what the pass reads anyway"
        );
    }

    /// A metadata-only batch carries no evidence, so it neither adds documents nor drops
    /// any: the current version of those files did not change.
    #[test]
    fn a_batch_without_evidence_does_not_touch_the_index() {
        let index = FakeIndex::default();
        let graph = FakeGraph::holding(&[]);
        let update = SearchIndexUpdate::new(&index, &graph);
        let mut sink = IndexingSink::new(|_: Vec<SourceCommit>| Ok(()), Some(&update));

        sink.commit_batch(vec![SourceCommit::metadata_only(
            FileLocation::observed(
                "file:m",
                NormalizedPath::from_absolute("/data/m.png").unwrap(),
                FileKind::RegularFile,
                10,
                100,
                100,
            )
            .unwrap(),
        )])
        .unwrap();

        assert!(index.published.borrow().is_empty());
    }

    /// A failing index does not fail the scan: the graph write already happened, and the
    /// failure is reported instead of being turned into a scan that did not occur.
    #[test]
    fn a_failing_index_is_reported_and_does_not_fail_the_commit() {
        let index = FakeIndex {
            broken: true,
            ..FakeIndex::default()
        };
        let graph = FakeGraph::holding(&[]);
        let update = SearchIndexUpdate::new(&index, &graph);
        let mut committed = 0;
        {
            // Three different files, so all three batches really do reach the index: a file
            // whose current version did not change is not published at all, and three tries
            // are what makes "reported once, not once per batch" a claim about anything.
            let mut sink = IndexingSink::new(
                |commits: Vec<SourceCommit>| {
                    committed += 1;
                    for commit in &commits {
                        if let Some(evidence) = commit.evidence() {
                            graph.accepts(&[(
                                evidence.file_version().file_id(),
                                evidence.file_version().id(),
                            )]);
                        }
                    }
                    Ok(())
                },
                Some(&update),
            );
            for file in ["file:a", "file:b", "file:c"] {
                sink.commit_batch(vec![commit(file, "new", "обход")])
                    .unwrap();
            }
        }
        assert_eq!(committed, 3, "every batch must still have been committed");
        assert_eq!(
            update.failures(),
            vec![SearchError::Index(SearchIndexError::Busy)],
            "the same failure is reported once, not once per batch"
        );
    }

    /// A run that keeps no index writes none, and asks the graph nothing about versions.
    #[test]
    fn a_run_without_a_search_index_writes_none() {
        let mut published = Vec::new();
        {
            let mut sink = IndexingSink::new(
                |commits: Vec<SourceCommit>| {
                    published.push(commits.len());
                    Ok(())
                },
                None,
            );
            sink.commit_batch(vec![commit("file:a", "new", "обход")])
                .unwrap();
        }
        assert_eq!(published, vec![1]);
    }
}
