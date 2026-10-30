// The whole loop of `ask`, end to end, with everything real except the model.
//
// A real scan writes a real graph and a real index; the question goes through the same
// search the `search` command uses, the same budget, the same cache on disk; and a fake
// provider stands where the network would be. What that leaves untested is the quality of
// an answer, which no fake can tell us anything about — it needs a key and a person reading
// the answers, and the procedure for that is `docs/ask-quality-run.md`.
//
// What it does test is everything else: that a question reaches the right excerpts, that the
// budget bounds what leaves, that the sources under an answer are this program's own paths,
// that a hidden path is not sent, and that the same question twice costs one call.
use std::cell::RefCell;
use std::fs;
use std::sync::Arc;

use super::filesystem::DescriptorRelativeWalker;
use super::index_pipeline_tests::test_directory;
use super::ladybug::LadybugDatabaseManager;
use super::ladybug::test_support::TestDatabase;
use super::search::{ContextSearchIndex, built_in_normalizations, search_directory};
use crate::application::{
    AnswerProvider, AnswerRequest, AskFailure, AskReport, AskService, InitialScanService,
    SearchIndexUpdate, SourceWalker,
};
use crate::domain::{
    Answer, AskBudget, ContentEgress, ContentEgressPermit, IndexPolicy, NormalizedPath, Question,
    WorkspaceName,
};
use crate::infrastructure::{FileAnswerCache, LadybugSearchGraph, ask_cache_directory};

/// A model that answers from a script and keeps what it was given.
struct ScriptedModel {
    text: String,
    cites: Vec<i64>,
    seen: RefCell<Vec<Vec<(usize, String)>>>,
}

impl ScriptedModel {
    fn answering(text: &str, cites: Vec<i64>) -> Self {
        Self {
            text: text.to_string(),
            cites,
            seen: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.seen.borrow().len()
    }

    /// Every excerpt it was handed, in the order it was handed them.
    fn last_excerpts(&self) -> Vec<(usize, String)> {
        self.seen.borrow().last().cloned().unwrap_or_default()
    }
}

impl AnswerProvider for ScriptedModel {
    fn answer(&self, request: AnswerRequest<'_>) -> Result<Answer, AskFailure> {
        self.seen.borrow_mut().push(
            request
                .excerpts()
                .iter()
                .map(|excerpt| (excerpt.ordinal(), excerpt.text().to_string()))
                .collect(),
        );
        Answer::parse(&self.text, &self.cites, request.excerpts().len())
            .map_err(|_| AskFailure::Schema)
    }
}

/// A context with its graph, its index and its answer cache, over a tree on disk.
struct Context {
    _database: TestDatabase,
    manager: Arc<LadybugDatabaseManager>,
    graph: LadybugSearchGraph,
    index: ContextSearchIndex,
    cache: FileAnswerCache,
    cache_directory: std::path::PathBuf,
    root: std::path::PathBuf,
}

fn context(name: &str) -> Context {
    let database = TestDatabase::new(&format!("ask-pipeline-{name}"));
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();
    let data = test_directory(&format!("{name}-data"));
    let workspace = WorkspaceName::parse("bench").unwrap();
    let cache_directory = ask_cache_directory(&data, &workspace);
    Context {
        graph: LadybugSearchGraph::new(Arc::clone(&manager)),
        index: ContextSearchIndex::open(
            &search_directory(&data, &workspace),
            &built_in_normalizations(),
        )
        .unwrap(),
        cache: FileAnswerCache::new(cache_directory.clone()),
        cache_directory,
        _database: database,
        manager,
        root: test_directory(name),
    }
}

impl Context {
    fn write(&self, name: &str, text: &str) {
        fs::write(self.root.join(name), text).unwrap();
    }

    /// `index init`, the way the command runs it.
    fn index_the_tree(&self) {
        let repository = self.manager.repository();
        let update = SearchIndexUpdate::new(&self.index, &self.graph);
        let root = DescriptorRelativeWalker.resolve_root(&self.root).unwrap();
        InitialScanService::new(
            &DescriptorRelativeWalker,
            &repository,
            &IndexPolicy::new(vec![], 100 * 1024 * 1024),
            &analysing_everything(),
        )
        .keeping_search_index(&update)
        .execute(&root)
        .unwrap();
    }

    fn ask(
        &self,
        model: &ScriptedModel,
        budget: AskBudget,
        question: &str,
    ) -> Result<AskReport, AskFailure> {
        self.ask_seeing(model, budget, question, &|_| true)
    }

    fn ask_seeing(
        &self,
        model: &ScriptedModel,
        budget: AskBudget,
        question: &str,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<AskReport, AskFailure> {
        AskService::new(
            &self.index,
            &self.graph,
            model,
            &self.cache,
            budget,
            "a-model",
            "ask-v1",
            permit(),
        )
        .execute(&Question::parse(question).unwrap(), None, is_visible)
    }

    fn at(&self, name: &str) -> String {
        DescriptorRelativeWalker
            .resolve_root(&self.root)
            .unwrap()
            .join(name)
            .unwrap()
            .as_str()
            .to_string()
    }
}

fn permit() -> ContentEgressPermit {
    ContentEgress::Allowed.permit().unwrap()
}

fn budget() -> AskBudget {
    AskBudget::new(8, 40_000)
}

/// The ordinary case: a question reaches the file that answers it, and the source under the
/// answer is that file's path — named by this program, from the number the model cited.
#[test]
fn a_question_is_answered_from_the_files_that_matched_and_the_sources_are_ours() {
    let context = context("answered");
    context.write(
        "reconcile.md",
        "Сверщик обходит дерево заново и помечает исчезнувшие локации как missing.",
    );
    context.write(
        "coffee.md",
        "Совершенно другой текст про кофе, помол и воду.",
    );
    context.index_the_tree();
    let model = ScriptedModel::answering("Сверка обходит дерево и помечает исчезнувшее.", vec![1]);

    let report = context.ask(&model, budget(), "что делает сверщик").unwrap();

    assert_eq!(
        report.answer().text(),
        "Сверка обходит дерево и помечает исчезнувшее."
    );
    let cited: Vec<String> = report
        .cited()
        .iter()
        .map(|excerpt| excerpt.path().as_str().to_string())
        .collect();
    assert_eq!(cited, [context.at("reconcile.md")]);
    assert!(report.bytes_sent() > 0);
    assert!(!report.answered_from_cache());
}

/// What leaves the machine is the text of the excerpts and nothing about where they live:
/// the model sees numbers, and the paths stay here.
#[test]
fn the_model_is_given_text_and_numbers_and_no_paths() {
    let context = context("no-paths");
    context.write("reconcile.md", "Сверщик обходит дерево заново.");
    context.index_the_tree();
    let model = ScriptedModel::answering("ответ", vec![1]);

    context.ask(&model, budget(), "сверщик").unwrap();

    let excerpts = model.last_excerpts();
    assert_eq!(excerpts.len(), 1);
    assert_eq!(excerpts[0].0, 1);
    assert!(excerpts[0].1.contains("Сверщик обходит дерево"));
    assert!(
        !excerpts[0].1.contains("reconcile.md") && !excerpts[0].1.contains('/'),
        "a path reached the model: {:?}",
        excerpts[0].1
    );
}

/// A path this context's policy hides is not shown and not sent: the rule that keeps it out
/// of a page keeps it out of a question.
#[test]
fn a_hidden_path_is_neither_sent_nor_named() {
    let context = context("hidden");
    context.write("public.md", "Открытая заметка про сверку и обход дерева.");
    context.write("private.md", "Закрытая заметка про сверку и обход дерева.");
    context.index_the_tree();
    let model = ScriptedModel::answering("ответ", vec![1]);

    let report = context
        .ask_seeing(
            &model,
            budget(),
            "сверка",
            &|path: &NormalizedPath| !path.as_str().ends_with("private.md"),
        )
        .unwrap();

    let paths: Vec<String> = report
        .sources()
        .iter()
        .map(|excerpt| excerpt.path().as_str().to_string())
        .collect();
    assert_eq!(paths, [context.at("public.md")]);
    let sent = model.last_excerpts();
    assert_eq!(sent.len(), 1);
    assert!(
        !sent[0].1.contains("Закрытая"),
        "the hidden file's text left the machine: {sent:?}"
    );
}

/// The budget bounds what leaves, and the report says how much of it was used.
#[test]
fn the_budget_bounds_what_leaves_and_the_report_says_so() {
    let context = context("budget");
    for ordinal in 0..6 {
        context.write(
            &format!("note-{ordinal}.md"),
            "Заметка про сверку и обход дерева, одна из многих.",
        );
    }
    context.index_the_tree();
    let model = ScriptedModel::answering("ответ", vec![]);

    let report = context
        .ask(&model, AskBudget::new(2, 40_000), "сверка")
        .unwrap();

    assert_eq!(report.sources().len(), 2);
    assert_eq!(model.last_excerpts().len(), 2);
    assert_eq!(report.budget().max_sources(), 2);
}

/// The same question over the same sources is answered once and remembered on disk.
#[test]
fn the_same_question_twice_costs_one_call() {
    let context = context("cached");
    context.write("reconcile.md", "Сверщик обходит дерево заново.");
    context.index_the_tree();
    let model = ScriptedModel::answering("ответ", vec![1]);

    let first = context.ask(&model, budget(), "что делает сверщик").unwrap();
    let second = context.ask(&model, budget(), "что делает сверщик").unwrap();

    assert_eq!(model.calls(), 1);
    assert_eq!(first.answer(), second.answer());
    assert!(second.answered_from_cache());
    // And it is on disk, not in memory: a second process would find it too.
    let entries: Vec<_> = fs::read_dir(&context.cache_directory).unwrap().collect();
    assert_eq!(entries.len(), 1);
}

/// A question nothing matches sends nothing at all.
#[test]
fn a_question_nothing_matches_never_reaches_the_model() {
    let context = context("nothing");
    context.write("reconcile.md", "Сверщик обходит дерево заново.");
    context.index_the_tree();
    let model = ScriptedModel::answering("ответ", vec![]);

    let failure = context
        .ask(&model, budget(), "квантовая хромодинамика")
        .unwrap_err();

    assert_eq!(failure, AskFailure::NothingToAnswerFrom);
    assert_eq!(model.calls(), 0);
}

/// An answer citing a source it was not given is refused whole, and nothing of it is kept:
/// the one shape of wrong answer that would put a file in front of a reader.
#[test]
fn an_answer_citing_what_it_was_not_given_is_refused() {
    let context = context("invented");
    context.write("reconcile.md", "Сверщик обходит дерево заново.");
    context.index_the_tree();
    let model = ScriptedModel::answering("ответ про файл, которого не давали", vec![7]);

    let failure = context.ask(&model, budget(), "сверщик").unwrap_err();

    assert_eq!(failure, AskFailure::Schema);
    let again = context.ask(&model, budget(), "сверщик").unwrap_err();
    assert_eq!(again, AskFailure::Schema, "and nothing of it was cached");
}

/// A question written the way people write questions — punctuation, quotes, a colon — finds
/// what it is about. What is looked for is the words; the rest would be instructions to a
/// query parser that nobody meant to give.
#[test]
fn a_question_full_of_punctuation_still_finds_its_answer() {
    let context = context("punctuation");
    context.write(
        "permit.md",
        "ContentEgressPermit нельзя сконструировать вне domain::workspace: запрещающий \
         контекст не может его выдать.",
    );
    context.write("other.md", "Совершенно другой текст про кофе и помол.");
    context.index_the_tree();
    let model = ScriptedModel::answering("ответ", vec![1]);

    let report = context
        .ask(
            &model,
            budget(),
            "что такое `ContentEgressPermit`: зачем он нужен? (+обязательно)",
        )
        .unwrap();

    assert_eq!(
        report.cited()[0].path().as_str(),
        context.at("permit.md"),
        "the question found the file it is about"
    );
}

/// These tests are about the walk, the policy or the pipeline, not about which revisions are
/// worth analysing, so every path is one the installation analyses.
fn analysing_everything() -> crate::domain::AnalysisRules {
    crate::domain::AnalysisRules::new(
        vec![crate::domain::PathPattern::parse("**/*").unwrap()],
        Vec::new(),
    )
}
