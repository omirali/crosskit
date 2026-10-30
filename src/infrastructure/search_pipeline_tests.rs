// End to end, with a real graph and a real scan behind it: what the search index holds
// after a directory is indexed, what happens to it when the files move on, and what a
// query answers about them.
//
// The index-level rules — isolation, both languages, paging, the fingerprint — are held by
// `infrastructure::search::tests` with no graph in the way. What is here needs one: which
// version is current, whether a file still has a location, and whether the index and the
// graph agree.
use std::fs;
use std::sync::Arc;

use super::filesystem::DescriptorRelativeWalker;
use super::index_pipeline_tests::test_directory;
use super::ladybug::LadybugDatabaseManager;
use super::ladybug::test_support::{TestDatabase, count};
use super::search::{
    ContextSearchIndex, built_in_normalizations, load_normalizations, search_directory,
};
use crate::application::{
    FileState, InitialScanService, ReconcileService, SearchIndexService, SearchIndexUpdate,
    SearchRequest, SearchService, SourceWalker,
};
use crate::domain::{IndexPolicy, NormalizationSet, NormalizedPath, WorkspaceName};
use crate::infrastructure::LadybugSearchGraph;

/// A context: its graph, its search index in a directory of its own, and the tree it
/// indexes.
pub(super) struct Context {
    _database: TestDatabase,
    pub(super) manager: Arc<LadybugDatabaseManager>,
    pub(super) graph: LadybugSearchGraph,
    pub(super) index: ContextSearchIndex,
    pub(super) directory: std::path::PathBuf,
    root: std::path::PathBuf,
    content_limit: u64,
}

pub(super) fn context(name: &str) -> Context {
    let database = TestDatabase::new(&format!("search-pipeline-{name}"));
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let root = test_directory(name);
    let data = test_directory(&format!("{name}-data"));
    let workspace = WorkspaceName::parse("bench").unwrap();
    let directory = search_directory(&data, &workspace);
    let index = ContextSearchIndex::open(&directory, &built_in_normalizations()).unwrap();
    Context {
        graph: LadybugSearchGraph::new(Arc::clone(&manager)),
        _database: database,
        manager,
        index,
        directory,
        root,
        content_limit: 100 * 1024 * 1024,
    }
}

impl Context {
    pub(super) fn write(&self, name: &str, text: &str) {
        fs::write(self.root.join(name), text).unwrap();
    }

    fn resolved_root(&self) -> NormalizedPath {
        DescriptorRelativeWalker.resolve_root(&self.root).unwrap()
    }

    fn policy(&self) -> IndexPolicy {
        IndexPolicy::new(vec![], self.content_limit)
    }

    /// The same context, with a content limit small enough for a test to outgrow.
    pub(super) fn limiting_content(mut self, bytes: u64) -> Self {
        self.content_limit = bytes;
        self
    }

    /// `index init`, keeping the search index up to date the way the command does.
    pub(super) fn init(&self) {
        let repository = self.manager.repository();
        let update = SearchIndexUpdate::for_run(&self.index, &self.graph);
        let report = InitialScanService::new(
            &DescriptorRelativeWalker,
            &repository,
            &self.policy(),
            &analysing_everything(),
        )
        .keeping_search_index(&update)
        .execute(&self.resolved_root())
        .unwrap();
        assert!(
            report.search_index_failures().is_empty(),
            "{:?}",
            report.search_index_failures()
        );
    }

    /// `index reconcile`, the same way.
    pub(super) fn reconcile(&self) {
        let repository = self.manager.repository();
        let update = SearchIndexUpdate::for_run(&self.index, &self.graph);
        let report = ReconcileService::new(
            &DescriptorRelativeWalker,
            &repository,
            &self.policy(),
            &analysing_everything(),
        )
        .keeping_search_index(&update)
        .execute(&self.resolved_root())
        .unwrap();
        assert!(
            report.scan().search_index_failures().is_empty(),
            "{:?}",
            report.scan().search_index_failures()
        );
    }

    /// A scan that keeps no search index: what a run by an older build, or one whose index
    /// was locked by another process, leaves behind.
    fn reconcile_without_the_index(&self) {
        let repository = self.manager.repository();
        ReconcileService::new(
            &DescriptorRelativeWalker,
            &repository,
            &self.policy(),
            &analysing_everything(),
        )
        .execute(&self.resolved_root())
        .unwrap();
    }

    fn search(&self, query: &str, include_missing: bool) -> Vec<(String, FileState)> {
        self.page(query, 10, include_missing, |_| true)
    }

    fn page(
        &self,
        query: &str,
        limit: usize,
        include_missing: bool,
        is_visible: impl Fn(&NormalizedPath) -> bool,
    ) -> Vec<(String, FileState)> {
        let request = SearchRequest::new(query, limit, None, include_missing).unwrap();
        SearchService::new(&self.index, &self.graph)
            .execute(&request, None, &is_visible)
            .unwrap()
            .results()
            .iter()
            .map(|result| (result.path().as_str().to_string(), result.state()))
            .collect()
    }

    fn status(&self) -> crate::application::SearchIndexReport {
        SearchIndexService::new(&self.index, &self.graph)
            .status()
            .unwrap()
    }

    fn rebuild(&self) -> crate::application::SearchIndexReport {
        SearchIndexService::new(&self.index, &self.graph)
            .rebuild()
            .unwrap()
    }

    pub(super) fn at(&self, name: &str) -> String {
        self.resolved_root()
            .join(name)
            .unwrap()
            .as_str()
            .to_string()
    }

    /// Opens this context's index again with another declared set of languages, closing
    /// the one that was open. Closing it matters: an open index holds the writer lock of
    /// its directory, and only one writer of a context may exist.
    fn reopen_with(&mut self, normalizations: &NormalizationSet) {
        self.index = ContextSearchIndex::open(&self.directory, normalizations).unwrap();
    }

    /// Throws the index directory away and opens what a rebuild would open.
    fn lose_the_index(&mut self) {
        self.index =
            ContextSearchIndex::open(&test_directory("placeholder"), &built_in_normalizations())
                .unwrap();
        fs::remove_dir_all(&self.directory).unwrap();
        self.index =
            ContextSearchIndex::open_for_rebuild(&self.directory, &built_in_normalizations())
                .unwrap();
    }
}

/// The ordinary case, end to end: a scan writes the graph and the index together, and a
/// query in either language finds the file it came from, marked as present.
#[test]
fn a_scan_makes_its_own_text_findable() {
    let context = context("findable");
    context.write(
        "notes.md",
        "Сверщик обходит индекс и reports deleted locations",
    );
    context.write("other.md", "совершенно другой текст про кофе");
    context.init();

    assert_eq!(
        context.search("индексы", false),
        vec![(context.at("notes.md"), FileState::Present)]
    );
    assert_eq!(
        context.search("reported", false),
        vec![(context.at("notes.md"), FileState::Present)]
    );
    let status = context.status();
    assert_eq!(status.documents(), 2);
    assert!(status.counts_agree() && status.matches_this_build());
}

/// A file with new content has its old documents replaced, not added to: the words of the
/// version that is no longer current are not findable, and nothing is stored twice.
#[test]
fn a_new_current_version_leaves_no_trace_of_the_old_one() {
    let context = context("new-version");
    context.write("notes.md", "первая ревизия про хранилище");
    context.init();
    assert_eq!(context.search("первая", false).len(), 1);

    context.write("notes.md", "вторая ревизия про хранилище");
    context.reconcile();

    assert!(
        context.search("первая", false).is_empty(),
        "the replaced version must not answer any more"
    );
    assert_eq!(
        context.search("вторая", false),
        vec![(context.at("notes.md"), FileState::Present)]
    );
    assert_eq!(
        context.search("хранилище", false).len(),
        1,
        "one document per current chunk, not one per revision"
    );
    // The graph keeps both revisions; the index holds only the current one, which is what
    // bounds it by the number of files instead of the number of edits.
    assert_eq!(
        count(&context.manager, "MATCH (:FileVersion) RETURN COUNT(*);"),
        2
    );
    let status = context.status();
    assert_eq!((status.documents(), status.chunks()), (1, 1));
}

/// A file whose locations are all gone keeps its documents, marked. By default they are
/// out of the way; asked for, they come back and say what they are.
#[test]
fn a_file_that_disappeared_is_left_out_by_default_and_marked_when_asked_for() {
    let context = context("disappeared");
    context.write("gone.md", "исчезнувший файл про хранилище");
    context.write("kept.md", "оставшийся файл про хранилище");
    context.init();
    assert_eq!(context.search("хранилище", false).len(), 2);

    fs::remove_file(context.root.join("gone.md")).unwrap();
    context.reconcile();

    assert_eq!(
        context.search("хранилище", false),
        vec![(context.at("kept.md"), FileState::Present)],
        "a file with no location left is not in the default answer"
    );
    assert_eq!(
        context.search("исчезнувший", true),
        vec![(context.at("gone.md"), FileState::Missing)],
        "asked for, it comes back and says it is gone"
    );
    // Its documents were never dropped: the index still holds one per current chunk.
    let status = context.status();
    assert_eq!((status.documents(), status.chunks()), (2, 2));
}

/// The index is a derivative: deleting it costs one pass over the graph, the graph itself
/// does not change, and search answers as it did.
#[test]
fn an_index_thrown_away_is_written_again_from_the_graph_without_changing_it() {
    let mut context = context("rebuild");
    context.write(
        "notes.md",
        "Сверщик обходит индекс и сообщает об удалённых локациях",
    );
    context.write(
        "other.md",
        "The reconciler walks the index and reports deleted locations",
    );
    context.init();
    let before = graph_shape(&context.manager);
    let answered = context.search("индексы", false);
    assert_eq!(answered.len(), 1);

    context.lose_the_index();
    let report = context.rebuild();

    assert_eq!(report.documents(), report.chunks());
    assert_eq!(
        graph_shape(&context.manager),
        before,
        "rebuilding must read the graph and write nothing to it"
    );
    assert_eq!(
        context.search("индексы", false),
        answered,
        "the rebuilt index must answer what the old one answered"
    );
}

/// A scan whose index was not written leaves the index behind the graph. The counts say so,
/// the missing text really is missing, and a rebuild puts it back.
#[test]
fn an_index_left_behind_by_a_scan_is_seen_by_the_counts_and_repaired_by_a_rebuild() {
    let context = context("behind");
    context.write("first.md", "первая заметка про хранилище");
    context.init();
    assert!(context.status().counts_agree());

    context.write("second.md", "вторая заметка, которой индекс ещё не видел");
    context.reconcile_without_the_index();

    let status = context.status();
    assert_eq!((status.documents(), status.chunks()), (1, 2));
    assert!(
        !status.counts_agree(),
        "an index that missed a file must not claim to be in step"
    );
    assert!(
        status.matches_this_build(),
        "being behind is not a disagreement about the analyzer"
    );
    assert!(
        context.search("вторая", false).is_empty(),
        "the text it missed really is missing, which is what the count is about"
    );

    let repaired = context.rebuild();
    assert_eq!((repaired.documents(), repaired.chunks()), (2, 2));
    assert!(repaired.counts_agree());
    assert_eq!(context.search("вторая", false).len(), 1);
}

/// The same index behind the same graph, repaired by an ordinary pass instead of a rebuild.
///
/// This is the state a pass leaves when its graph transaction committed and its index was
/// never made visible — the index writer stages documents and commits once, at the end, so
/// a pass that does not reach the end leaves the index exactly where the previous pass left
/// it. What the next ordinary pass does about it is the whole claim: every pass reads every
/// file it is allowed to read and publishes the documents of whatever version the graph
/// then holds as current, so the index is written again without anybody asking for a
/// rebuild.
///
/// What this does not show is a process killed in the middle, which is a different state
/// and not this one: the engine keeps committed batches in a log beside the database until
/// it checkpoints, so a killed pass leaves a context that does not open at all. That is
/// `tests/cli_unclean_stop.rs` and ADR 0014.
#[test]
fn an_index_left_behind_by_a_scan_is_also_repaired_by_the_next_ordinary_pass() {
    let context = context("behind-then-reconciled");
    context.write("first.md", "первая заметка про хранилище");
    context.init();

    context.write("second.md", "вторая заметка, которой индекс ещё не видел");
    context.reconcile_without_the_index();
    let behind = context.status();
    assert_eq!((behind.documents(), behind.chunks()), (1, 2));

    // Nothing on disk changed since the pass that was lost, so this pass has no new
    // content to read: it repairs the index out of what it reads again anyway.
    context.reconcile();

    let repaired = context.status();
    assert_eq!((repaired.documents(), repaired.chunks()), (2, 2));
    assert!(repaired.counts_agree() && repaired.matches_this_build());
    assert_eq!(
        context.search("вторая", false),
        vec![(context.at("second.md"), FileState::Present)],
        "the text the lost pass had committed must answer again"
    );
    assert_eq!(
        context.search("первая", false).len(),
        1,
        "and the text that was already in the index is not stored twice"
    );
}

/// An index written by other text rules answers by rules nobody applies any more. The
/// counts agree, so only the fingerprint shows it — and the same rebuild repairs it.
#[test]
fn an_index_written_by_another_analyzer_is_seen_by_the_fingerprint_and_repaired_by_a_rebuild() {
    let mut context = context("fingerprint");
    context.write("notes.md", "русские индексы и english trees");
    context.init();
    assert!(context.status().matches_this_build());

    // The same index and the same graph, a different declared set of languages.
    context.reopen_with(&load_normalizations(&["en"]).unwrap());
    let status = context.status();
    assert!(
        status.counts_agree(),
        "every document is there, which is why the counts cannot catch this"
    );
    assert!(!status.matches_this_build());
    assert!(
        context.search("индексы", false).is_empty(),
        "the disagreement is real: the term the Russian rule wrote is not the one this \
         analyzer looks for"
    );

    let repaired = context.rebuild();
    assert!(repaired.matches_this_build() && repaired.counts_agree());
}

/// A path the policy no longer admits is not named by a result, and the fact that
/// something was left out is reported as a count rather than as a path.
#[test]
fn a_result_at_a_path_the_policy_hides_is_never_named() {
    let context = context("hidden");
    context.write("public.md", "открытая заметка про хранилище");
    context.write("private.md", "закрытая заметка про хранилище");
    context.init();

    let visible = context.page("хранилище", 10, false, |path: &NormalizedPath| {
        !path.as_str().ends_with("private.md")
    });
    assert_eq!(visible, vec![(context.at("public.md"), FileState::Present)]);
}

/// Two contexts, two graphs, two index directories: no query in one reaches the text of
/// the other, and the sentinel is findable only where it was written.
#[test]
fn a_sentinel_written_in_one_context_is_not_reachable_from_the_other() {
    const SENTINEL: &str = "ZZQQPIPELINESENTINEL";
    let work = context("isolation-work");
    work.write(
        "secret.md",
        &format!("внутренний документ {SENTINEL} не для чужих"),
    );
    work.init();
    let personal = context("isolation-personal");
    personal.write("notes.md", "личные заметки про отпуск and english notes");
    personal.init();

    assert_eq!(work.search(SENTINEL, true).len(), 1);
    for attempt in [SENTINEL, "внутренний", "документ", "*"] {
        let found = personal.page(attempt, 100, true, |_| true);
        assert!(
            found.iter().all(
                |(path, _)| path.starts_with(personal.root.to_str().unwrap())
                    || path.contains("/private")
            ),
            "{attempt:?} answered {found:?} from the personal context"
        );
        assert!(
            !found.iter().any(|(path, _)| path.contains("secret.md")),
            "{attempt:?} reached the other context"
        );
    }
    assert_ne!(work.directory, personal.directory);
}

/// The whole of the graph a rebuild must not touch.
fn graph_shape(manager: &Arc<LadybugDatabaseManager>) -> Vec<i64> {
    [
        "MATCH (:File) RETURN COUNT(*);",
        "MATCH (:FileVersion) RETURN COUNT(*);",
        "MATCH (:ContentChunk) RETURN COUNT(*);",
        "MATCH (:FileLocation) RETURN COUNT(*);",
        "MATCH (:FileLocation {status: 'present'}) RETURN COUNT(*);",
        "MATCH (:InsightJob) RETURN COUNT(*);",
        "MATCH (:File)-[:CURRENT_VERSION]->(:FileVersion) RETURN COUNT(*);",
    ]
    .iter()
    .map(|query| count(manager, query))
    .collect()
}

/// These tests are about the search pipeline, not about which revisions are worth
/// analysing, so every path is one the installation analyses.
fn analysing_everything() -> crate::domain::AnalysisRules {
    crate::domain::AnalysisRules::new(
        vec![crate::domain::PathPattern::parse("**/*").unwrap()],
        Vec::new(),
    )
}

/// The language a file's name declares is written by the scan and written again, the same
/// way, by a rebuild from the graph.
///
/// The two write paths reach it differently and that is the risk: a scan knows the path it
/// just walked, while a rebuild has only the graph, which holds no language and has to read
/// the file's names back. An index whose answer depended on which of the two wrote it would
/// make `index rebuild --search` change what a query returns, and nothing would say so.
#[test]
fn the_language_a_name_declares_survives_a_rebuild_unchanged() {
    let context = context("language");
    context.write("routes.py", "обход индекс python");
    context.write("Controller.php", "обход индекс php");
    context.write("notes.md", "обход индекс markdown");
    context.write("LICENSE", "обход индекс без имени");
    context.init();

    let selected = |query: &str| {
        let mut paths: Vec<String> = context
            .search(query, false)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        paths.sort();
        paths
    };

    let python = selected("source_language:python");
    let php = selected("source_language:php");
    let everything = selected("обход");
    assert_eq!(python, vec![context.at("routes.py")]);
    assert_eq!(php, vec![context.at("Controller.php")]);
    assert_eq!(
        everything.len(),
        4,
        "every file holds the word: {everything:?}"
    );
    // The file whose name declares nothing is in the index and in no language.
    assert!(selected("source_language:markdown").contains(&context.at("notes.md")));
    assert!(
        !php.contains(&context.at("LICENSE")) && !python.contains(&context.at("LICENSE")),
        "a name that declares nothing is claimed by nothing"
    );

    let report = context.rebuild();
    assert!(report.counts_agree() && report.matches_this_build());
    assert_eq!(selected("source_language:python"), python);
    assert_eq!(selected("source_language:php"), php);
    assert_eq!(selected("обход"), everything);
}
