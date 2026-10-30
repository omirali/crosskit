// What the index costs and how well it answers, measured through the production path.
//
// The same corpus, the same queries and the same ground truth as the Stage 8 spike, so the
// numbers here stand beside ADR 0011's and ADR 0012's rather than near them. What changed
// is what is being measured: the index a released Crosskit writes, built by the rebuild
// use case out of a real graph, and queried through the paging collector rather than
// through a top-k call.
//
// The corpus is this repository's own text: Russian prose with English identifiers inside
// it, which is the corpus the whole question is about. It changes as the repository
// changes, so these are baselines, not assertions, and each run prints what it measured.
//
//   cargo test --release --offline --bins search::cost_tests -- --ignored --nocapture --test-threads=1
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::application::SearchGraphReader;
use crate::application::{
    SearchIndexReader, SearchIndexService, SearchRequest, SearchService, SourceBatchWriter,
};
use crate::domain::{
    AnalysisRequest, ChunkScript, ContentChunk, EvidenceCommit, FileObservation, FileVersion,
    NormalizationSet, NormalizedPath, SourceCommit,
};
use crate::infrastructure::LadybugSearchGraph;
use crate::infrastructure::ladybug::LadybugDatabaseManager;
use crate::infrastructure::ladybug::test_support::{TestDatabase, located};

use super::context_index::{ContextSearchIndex, search_directory};
use super::language_pack::{built_in_normalizations, load_normalizations};

/// Hand-declared inflections of one lemma, the same groups ADR 0011 measured the extension
/// with, so the two tables are about the same question.
const RUSSIAN: &[&[&str]] = &[
    &[
        "источник",
        "источника",
        "источников",
        "источники",
        "источнике",
        "источником",
    ],
    &[
        "контекст",
        "контекста",
        "контексте",
        "контекстов",
        "контексты",
    ],
    &["запись", "записи", "записью", "записей"],
    &[
        "транзакция",
        "транзакции",
        "транзакцией",
        "транзакцию",
        "транзакций",
    ],
    &["содержимое", "содержимого", "содержимому", "содержимым"],
    &["файл", "файла", "файлов", "файлом", "файлами", "файлах"],
    &["строка", "строки", "строк", "строку", "строками", "строкой"],
    &["идентичность", "идентичности", "идентичностью"],
    &["размер", "размера", "размером", "размере"],
    &["прогон", "прогона", "прогонов", "прогоне", "прогоны"],
    &["проход", "прохода", "проходом", "проходы", "проходе"],
    &["ошибка", "ошибки", "ошибок", "ошибке", "ошибкой"],
];

const ENGLISH: &[&[&str]] = &[
    &["version", "versions"],
    &["count", "counts", "counted", "counting"],
    &["source", "sources"],
    &["match", "matches", "matched", "matching"],
    &["location", "locations", "located"],
    &["chunk", "chunks", "chunking"],
    &["index", "indexes", "indexed", "indexing"],
    &["return", "returns", "returned", "returning"],
    &["commit", "commits", "committed", "committing"],
    &[
        "connect",
        "connects",
        "connected",
        "connection",
        "connections",
    ],
    &["observe", "observed", "observation", "observations"],
    &["assert", "asserts", "asserted", "assertion", "assertions"],
];

/// One paragraph of the corpus: the text and the chunk id it is indexed under.
#[derive(Clone)]
struct Document {
    id: String,
    text: String,
}

/// Paragraphs of this repository's own prose and source, the roots ADR 0011 used.
fn corpus() -> Vec<Document> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut documents = Vec::new();
    let mut files = Vec::new();
    for entry in ["README.md", "HANDOFF.md", "AGENTS.md", "CLAUDE.md"] {
        files.push(root.join(entry));
    }
    collect(&root.join("docs"), &mut files);
    collect(&root.join("src"), &mut files);
    files.sort();

    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for paragraph in text.split("\n\n") {
            let paragraph = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
            if (60..=2000).contains(&paragraph.len()) {
                let ordinal = documents.len();
                documents.push(Document {
                    // The id the graph will store for this paragraph's chunk, so ground
                    // truth and retrieved matches are keyed alike.
                    id: format!("chunk:file:{ordinal:06}:hash:{ordinal:06}"),
                    text: paragraph,
                });
            }
        }
    }
    documents
}

fn collect(directory: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, into);
        } else if matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("md") | Some("rs")
        ) {
            into.push(path);
        }
    }
}

/// One located file with one revision and one chunk carrying `text`.
fn commit_with_text(file_id: &str, path: &str, revision: &str, text: &str) -> SourceCommit {
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
        located(file_id, path),
        EvidenceCommit::new(version, observation, vec![chunk]).unwrap(),
        AnalysisRequest::Requested,
    )
    .unwrap()
}

/// The words of a text, lowercased and split, but **not** normalized.
///
/// Ground truth is built from these on purpose. Building it with the index's own analyzer
/// would ask whether the index finds what its own stemming says is there — which it does by
/// construction, and which measures nothing. ADR 0011 measured the extension against the
/// declared surface forms, and so does this.
fn surface_terms(text: &str) -> HashSet<String> {
    let mut terms = HashSet::new();
    let mut word = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            word.extend(character.to_lowercase());
            continue;
        }
        if !word.is_empty() {
            terms.insert(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        terms.insert(word);
    }
    terms
}

/// The corpus in a graph, and the index this program writes out of it.
struct Measured {
    _database: TestDatabase,
    graph: LadybugSearchGraph,
    index: ContextSearchIndex,
    directory: PathBuf,
    documents: Vec<Document>,
    text_bytes: u64,
}

fn indexed_corpus(name: &str) -> Measured {
    indexed_corpus_with(name, &built_in_normalizations(), corpus())
}

fn indexed_corpus_with(
    name: &str,
    normalizations: &NormalizationSet,
    documents: Vec<Document>,
) -> Measured {
    let database = TestDatabase::new(&format!("search-cost-{name}"));
    let manager = LadybugDatabaseManager::open(database.path().to_path_buf()).unwrap();

    let mut text_bytes = 0u64;
    let mut commits: Vec<SourceCommit> = Vec::with_capacity(documents.len());
    for (ordinal, document) in documents.iter().enumerate() {
        text_bytes += document.text.len() as u64;
        commits.push(commit_with_text(
            &format!("file:{ordinal:06}"),
            &format!("/corpus/{ordinal:06}.md"),
            &format!("hash:{ordinal:06}"),
            &document.text,
        ));
    }
    for batch in commits.chunks(200) {
        manager.repository().commit_batch(batch.to_vec()).unwrap();
    }

    let root = std::env::temp_dir().join(format!(
        "crosskit-search-cost-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let directory = search_directory(&root, &crate::domain::WorkspaceName::parse("cost").unwrap());
    let index = ContextSearchIndex::open(&directory, normalizations).unwrap();
    let graph = LadybugSearchGraph::new(Arc::clone(&manager));

    let started = Instant::now();
    let report = SearchIndexService::new(&index, &graph).rebuild().unwrap();
    let elapsed = started.elapsed();
    println!(
        "build: {} documents, {:.2} MB of text in {:.2}s — {:.2} s per MB",
        report.documents(),
        text_bytes as f64 / 1e6,
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() / (text_bytes as f64 / 1e6),
    );

    Measured {
        _database: database,
        graph,
        index,
        directory,
        documents,
        text_bytes,
    }
}

fn directory_bytes(directory: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                directory_bytes(&path)
            } else {
                path.metadata().map(|meta| meta.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// Build cost and index size, against the 1.4 s per megabyte of the graph index (ADR 0011)
/// and the 0.13–0.14 s/MB and 0.42× of the spike (ADR 0012).
#[test]
#[ignore = "search measurement: cargo test --release --offline --bins the_cost_of_building_the_index -- --ignored --nocapture --test-threads=1"]
fn the_cost_of_building_the_index() {
    let measured = indexed_corpus("build");
    let on_disk = directory_bytes(&measured.directory);
    println!(
        "corpus: {} paragraphs, {:.2} MB of text",
        measured.documents.len(),
        measured.text_bytes as f64 / 1e6
    );
    println!(
        "index on disk: {:.2} MB — {:.2}x the text",
        on_disk as f64 / 1e6,
        on_disk as f64 / measured.text_bytes as f64
    );
}

/// Query latency, in both languages, on the same corpus.
///
/// Two numbers per language: the matching page the index returns, and the whole answer a
/// command prints — the second reads every result's text and locations out of the graph and
/// cuts a fragment from each, which is the part a reader actually waits for.
#[test]
#[ignore = "search measurement: cargo test --release --offline --bins the_latency_of_a_query -- --ignored --nocapture --test-threads=1"]
fn the_latency_of_a_query() {
    let measured = indexed_corpus("latency");
    let service = SearchService::new(&measured.index, &measured.graph);
    let everything_is_visible = |_: &NormalizedPath| true;

    for (language, groups) in [("russian", RUSSIAN), ("english", ENGLISH)] {
        let mut index_only = Vec::new();
        let mut graph_only = Vec::new();
        let mut whole_page = Vec::new();
        for group in groups {
            for form in *group {
                let started = Instant::now();
                let found = measured
                    .index
                    .matches(form, 10, None, None)
                    .unwrap()
                    .into_documents();
                index_only.push((started.elapsed().as_micros() as u64, found.len()));

                // The same rows, resolved against the graph: the text and the locations of
                // every result on the page.
                let chunk_ids: Vec<String> = found
                    .iter()
                    .map(|found| found.chunk_id().to_string())
                    .collect();
                let started = Instant::now();
                let resolved = measured.graph.resolve(&chunk_ids).unwrap();
                graph_only.push((started.elapsed().as_micros() as u64, resolved.len()));

                let request = SearchRequest::new(*form, 10, None, false).unwrap();
                let started = Instant::now();
                let page = service
                    .execute(&request, None, &everything_is_visible)
                    .unwrap();
                whole_page.push((started.elapsed().as_micros() as u64, page.results().len()));
            }
        }
        for (what, mut samples) in [
            ("index", index_only),
            ("graph", graph_only),
            ("page", whole_page),
        ] {
            samples.sort_unstable();
            let median = samples[samples.len() / 2].0;
            let p90 = samples[samples.len() * 9 / 10].0;
            println!(
                "{language} {what}: {} queries, median {median} us, p90 {p90} us",
                samples.len()
            );
        }
    }
}

/// Recall and precision, both languages at once, against ADR 0011's three rows and the
/// spike's 0.96/1.00.
///
/// Truth for a query is every document holding any declared form of its lemma, judged
/// without the rules under test.
#[test]
#[ignore = "search measurement: cargo test --release --offline --bins recall_and_precision -- --ignored --nocapture --test-threads=1"]
fn recall_and_precision_in_both_languages() {
    let measured = indexed_corpus("recall");

    let mut document_terms: HashMap<&str, HashSet<String>> = HashMap::new();
    for document in &measured.documents {
        document_terms.insert(document.id.as_str(), surface_terms(&document.text));
    }

    println!(
        "{:<10}{:>9}{:>11}{:>9}",
        "language", "recall", "precision", "queries"
    );
    for (language, groups) in [("russian", RUSSIAN), ("english", ENGLISH)] {
        let mut recalls = Vec::new();
        let mut precisions = Vec::new();

        for group in groups {
            let wanted: HashSet<String> = group.iter().map(|form| form.to_lowercase()).collect();
            let truth: HashSet<&str> = measured
                .documents
                .iter()
                .filter(|document| {
                    document_terms[document.id.as_str()]
                        .iter()
                        .any(|term| wanted.contains(term))
                })
                .map(|document| document.id.as_str())
                .collect();
            if truth.is_empty() {
                continue;
            }

            for form in *group {
                let found: HashSet<String> = measured
                    .index
                    .matches(form, measured.documents.len(), None, None)
                    .unwrap()
                    .into_documents()
                    .into_iter()
                    .map(|found| found.chunk_id().to_string())
                    .collect();
                let matched = found
                    .iter()
                    .filter(|id| truth.contains(id.as_str()))
                    .count();
                recalls.push(matched as f64 / truth.len() as f64);
                if !found.is_empty() {
                    precisions.push(matched as f64 / found.len() as f64);
                }
            }
        }

        let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
        println!(
            "{language:<10}{:>9.2}{:>11.2}{:>9}",
            mean(&recalls),
            mean(&precisions),
            recalls.len()
        );
    }
}

/// The Kazakh reference corpus and the groups of forms that judge it.
///
/// Somebody else's text, included under CC BY-SA 4.0 and read from disk at measurement time
/// — never built into the programme, which `fixture_isolation_tests` holds to. Its licence,
/// its source and why it is not a public-domain text are in ADR 0016.
mod kazakh_fixture {
    use std::path::PathBuf;

    use super::Document;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/kazakh")
            .join(name)
    }

    /// The paragraphs of the corpus, one document each, in the order the file lists them.
    pub(super) fn corpus() -> Vec<Document> {
        let text = std::fs::read_to_string(fixture("corpus.txt")).expect("the Kazakh corpus");
        text.lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .enumerate()
            .map(|(ordinal, paragraph)| Document {
                // The id the graph will store for this paragraph's chunk, keyed the same way
                // as the repository corpus above, so truth and matches are comparable.
                id: format!("chunk:file:{ordinal:06}:hash:{ordinal:06}"),
                text: paragraph.to_string(),
            })
            .collect()
    }

    /// The groups of forms, as the file writes them: `lemma | meaning | form form form`.
    pub(super) fn word_forms() -> Vec<(String, Vec<String>)> {
        let text = std::fs::read_to_string(fixture("word-forms.txt")).expect("the Kazakh forms");
        text.lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .map(|line| {
                let mut parts = line.split('|').map(str::trim);
                let lemma = parts.next().expect("a lemma").to_string();
                let _meaning = parts.next();
                let forms = parts
                    .next()
                    .expect("the forms")
                    .split_whitespace()
                    .map(str::to_string)
                    .collect();
                (lemma, forms)
            })
            .collect()
    }
}

/// Recall and precision of one set of form groups against one indexed corpus.
///
/// Truth for a group is every document holding any of its forms, judged from the text
/// itself and not by the rules under test.
fn recall_and_precision(
    measured: &Measured,
    groups: &[(String, Vec<String>)],
) -> (f64, f64, usize) {
    let mut document_terms: HashMap<&str, HashSet<String>> = HashMap::new();
    for document in &measured.documents {
        document_terms.insert(document.id.as_str(), surface_terms(&document.text));
    }
    let mut recalls = Vec::new();
    let mut precisions = Vec::new();
    for (_, forms) in groups {
        let wanted: HashSet<String> = forms.iter().map(|form| form.to_lowercase()).collect();
        let truth: HashSet<&str> = measured
            .documents
            .iter()
            .filter(|document| {
                document_terms[document.id.as_str()]
                    .iter()
                    .any(|term| wanted.contains(term))
            })
            .map(|document| document.id.as_str())
            .collect();
        if truth.is_empty() {
            continue;
        }
        for form in forms {
            let found: HashSet<String> = measured
                .index
                .matches(form, measured.documents.len(), None, None)
                .unwrap()
                .into_documents()
                .into_iter()
                .map(|found| found.chunk_id().to_string())
                .collect();
            let matched = found
                .iter()
                .filter(|id| truth.contains(id.as_str()))
                .count();
            recalls.push(matched as f64 / truth.len() as f64);
            if !found.is_empty() {
                precisions.push(matched as f64 / found.len() as f64);
            }
        }
    }
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    (mean(&recalls), mean(&precisions), recalls.len())
}

/// What declaring Kazakh beside Russian costs the index this repository's own text makes.
///
/// Both configurations over one corpus: how large the index gets, how long a query takes,
/// and whether the two languages that were already there answer as well as they did. The
/// last is the one that decides — a language pack that improves nothing for Kazakh readers
/// while costing Russian ones precision is not a trade anybody asked for.
#[test]
#[ignore = "search measurement: cargo test --release --offline --bins the_cost_of_declaring_kazakh -- --ignored --nocapture --test-threads=1"]
fn the_cost_of_declaring_kazakh() {
    let with_kazakh = load_normalizations(&["ru", "en", "kk"]).unwrap();
    println!(
        "{:<12}{:>12}{:>12}{:>10}{:>10}{:>10}{:>10}",
        "declared", "index MB", "x text", "RU recall", "RU prec", "EN recall", "EN prec"
    );
    for (label, set) in [
        ("ru,en", built_in_normalizations()),
        ("ru,en,kk", with_kazakh),
    ] {
        let measured = indexed_corpus_with(&format!("kazakh-cost-{label}"), &set, corpus());
        let on_disk = directory_bytes(&measured.directory);
        let russian = recall_and_precision(&measured, &groups(RUSSIAN));
        let english = recall_and_precision(&measured, &groups(ENGLISH));
        println!(
            "{label:<12}{:>12.2}{:>12.2}{:>10.2}{:>10.2}{:>10.2}{:>10.2}",
            on_disk as f64 / 1e6,
            on_disk as f64 / measured.text_bytes as f64,
            russian.0,
            russian.1,
            english.0,
            english.1,
        );
    }
}

/// The same two configurations, timed: what a query costs when a word may have two readings.
#[test]
#[ignore = "search measurement: cargo test --release --offline --bins the_latency_of_a_query_with_kazakh -- --ignored --nocapture --test-threads=1"]
fn the_latency_of_a_query_with_kazakh() {
    for (label, set) in [
        ("ru,en", built_in_normalizations()),
        (
            "ru,en,kk",
            load_normalizations(&["ru", "en", "kk"]).unwrap(),
        ),
    ] {
        let measured = indexed_corpus_with(&format!("kazakh-latency-{label}"), &set, corpus());
        let mut samples = Vec::new();
        for group in RUSSIAN.iter().chain(ENGLISH.iter()) {
            for form in *group {
                let started = Instant::now();
                let found = measured
                    .index
                    .matches(form, 10, None, None)
                    .unwrap()
                    .into_documents();
                samples.push((started.elapsed().as_micros(), found.len()));
            }
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2].0;
        let p90 = samples[samples.len() * 9 / 10].0;
        println!(
            "{label}: {} queries, median {median} us, p90 {p90} us",
            samples.len()
        );
    }
}

/// Kazakh measured on the reference corpus, by the groups of forms that came with it.
///
/// The numbers this prints are not published anywhere until the owner has confirmed those
/// groups: a judge nobody has read is not a judge, and a number from it would be a claim
/// about a language on the strength of one reading of it.
#[test]
#[ignore = "search measurement: cargo test --release --offline --bins kazakh_recall_and_precision -- --ignored --nocapture --test-threads=1"]
fn kazakh_recall_and_precision() {
    let documents = kazakh_fixture::corpus();
    let groups = kazakh_fixture::word_forms();
    println!(
        "Kazakh corpus: {} paragraphs, {} form groups",
        documents.len(),
        groups.len()
    );
    for (label, set) in [
        ("ru,en", built_in_normalizations()),
        (
            "ru,en,kk",
            load_normalizations(&["ru", "en", "kk"]).unwrap(),
        ),
    ] {
        let measured = indexed_corpus_with(&format!("kazakh-{label}"), &set, documents.clone());
        let (recall, precision, queries) = recall_and_precision(&measured, &groups);
        let on_disk = directory_bytes(&measured.directory);
        println!(
            "{label}: recall {recall:.2}, precision {precision:.2} over {queries} queries; \
             index {:.2} MB, {:.2}x the text",
            on_disk as f64 / 1e6,
            on_disk as f64 / measured.text_bytes as f64,
        );
    }
}

/// The hand-declared groups in the shape `recall_and_precision` judges by.
fn groups(declared: &[&[&str]]) -> Vec<(String, Vec<String>)> {
    declared
        .iter()
        .map(|forms| {
            (
                forms[0].to_string(),
                forms.iter().map(|form| (*form).to_string()).collect(),
            )
        })
        .collect()
}
