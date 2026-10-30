// What the index itself has to do, without a graph in the way: isolate one context from
// another, serve two languages at once, keep a word it cannot normalize findable by its own
// form, page the same way twice, and say which rules wrote it.
//
// The tests that need a graph — a rebuild from it, a file whose locations are gone, an
// index left behind by a scan — live in `infrastructure::search_pipeline_tests`, where a
// real database and the real scan are in reach.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::application::{SearchIndexError, SearchIndexReader, SearchIndexWriter};
use crate::domain::{DocumentCursor, NormalizationSet, SourceLanguage};

use super::context_index::ContextSearchIndex;
use super::language_pack::{built_in_normalizations, load_normalizations};

/// A string that exists in one context and nowhere else, so finding it anywhere else is a
/// breach rather than a coincidence.
const SENTINEL: &str = "ZZQQSENTINELXYZ";

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn scratch(name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "crosskit-search-{name}-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    path
}

/// An index of `documents`, each `(chunk id, file id, text)`, in a directory of its own.
fn index_of(name: &str, documents: &[(&str, &str, &str)]) -> (PathBuf, ContextSearchIndex) {
    index_with(name, &built_in_normalizations(), documents)
}

fn index_with(
    name: &str,
    normalizations: &NormalizationSet,
    documents: &[(&str, &str, &str)],
) -> (PathBuf, ContextSearchIndex) {
    let directory = scratch(name);
    let index = ContextSearchIndex::open(&directory, normalizations).unwrap();
    if !documents.is_empty() {
        index.append(build(documents)).unwrap();
        // Staged documents become visible when the run that wrote them is done, exactly as
        // a rebuild makes its own visible.
        index.finish().unwrap();
    }
    (directory, index)
}

fn build(documents: &[(&str, &str, &str)]) -> Vec<crate::application::SearchDocument> {
    documents
        .iter()
        .map(|(chunk, file, text)| {
            crate::application::SearchDocument::new(*chunk, *file, None, *text)
        })
        .collect()
}

fn found(index: &ContextSearchIndex, query: &str) -> Vec<String> {
    page(index, query, 50, None)
}

fn page(
    index: &ContextSearchIndex,
    query: &str,
    limit: usize,
    after: Option<&DocumentCursor>,
) -> Vec<String> {
    rows(index, query, limit, after)
        .into_iter()
        .map(|found| found.chunk_id().to_string())
        .collect()
}

fn rows(
    index: &ContextSearchIndex,
    query: &str,
    limit: usize,
    after: Option<&DocumentCursor>,
) -> Vec<crate::application::DocumentMatch> {
    index
        .matches(query, limit, after, None)
        .unwrap_or_else(|error| panic!("{query:?}: {error}"))
        .into_documents()
}

/// Isolation the way Stage 4 established it for the databases: the text of one context is
/// not in the files the other context's searcher opens, so no query can reach it. The
/// queries below are the ones someone would try if they were trying.
#[test]
fn a_sentinel_in_one_context_is_never_found_from_another() {
    let (work_directory, work) = index_of(
        "isolation-work",
        &[(
            "chunk:work",
            "file:work",
            &format!("внутренний документ работодателя {SENTINEL} не для чужих глаз"),
        )],
    );
    let (personal_directory, personal) = index_of(
        "isolation-personal",
        &[(
            "chunk:personal",
            "file:personal",
            "личные заметки про отпуск and some english notes",
        )],
    );

    assert_eq!(
        found(&work, SENTINEL),
        vec!["chunk:work"],
        "the sentinel must be findable in the context that holds it"
    );

    for attempt in [
        SENTINEL,
        &SENTINEL.to_lowercase(),
        "ZZQQSENTINELXYZ OR отпуск",
        "*",
        "text:ZZQQSENTINELXYZ",
        "text:*",
        "внутренний",
        "работодателя",
        "NOT отпуск",
    ] {
        // A query this build cannot read finds nothing, which is also not a breach.
        let hits = match personal.matches(attempt, 100, None, None) {
            Ok(hits) => hits.into_documents(),
            Err(SearchIndexError::UnreadableQuery) => continue,
            Err(error) => panic!("{attempt:?}: {error}"),
        };
        assert!(
            hits.iter()
                .all(|found| found.chunk_id().starts_with("chunk:personal")),
            "{attempt:?} returned {hits:?} from the personal index, and not all of it is personal"
        );
    }

    assert_ne!(
        work_directory, personal_directory,
        "each context must hold its index in a directory of its own"
    );
}

/// One sentence, two languages, and each word found by any of its forms. This is the case
/// a single stemmer per index cannot serve at all (ADR 0011).
#[test]
fn one_sentence_in_two_languages_is_found_from_either() {
    let (_directory, index) = index_of(
        "bilingual",
        &[
            (
                "chunk:mixed",
                "file:mixed",
                "The walker обходит trees и пишет индексы в хранилище",
            ),
            (
                "chunk:other",
                "file:other",
                "совершенно другой текст про кофе",
            ),
        ],
    );

    for query in [
        "обход",
        "обходит",
        "индекс",
        "индексов",
        "walker",
        "tree",
        "trees",
    ] {
        assert_eq!(found(&index, query), vec!["chunk:mixed"], "{query}");
    }
    // And both halves of the same sentence at once.
    assert_eq!(found(&index, "индексы walker"), vec!["chunk:mixed"]);
}

/// A Kazakh word is stored in the form it was written in, so it is found by that form, and
/// a Russian word in the same sentence is still found by any of its forms.
#[test]
fn a_kazakh_word_is_found_by_its_own_form_while_russian_beside_it_is_stemmed() {
    let (_directory, index) = index_of(
        "kazakh",
        &[(
            "chunk:kk",
            "file:kk",
            "мүмкіндік қазақша кітаптар и русские индексы рядом",
        )],
    );

    assert_eq!(found(&index, "мүмкіндік"), vec!["chunk:kk"]);
    assert_eq!(found(&index, "қазақша"), vec!["chunk:kk"]);
    assert_eq!(found(&index, "кітаптар"), vec!["chunk:kk"]);
    // Nothing here knows Kazakh morphology, and the index does not pretend to: another
    // form of the same word is another word until Stage 10 says otherwise.
    assert!(found(&index, "кітап").is_empty());
    // The Russian word beside it is stemmed as usual.
    assert_eq!(found(&index, "индекс"), vec!["chunk:kk"]);
    assert_eq!(found(&index, "индексами"), vec!["chunk:kk"]);
}

/// Paging walks every match exactly once, in one order, and the same page asked for twice
/// is the same page.
#[test]
fn pages_smaller_than_the_result_set_are_stable_and_repeatable() {
    let documents: Vec<(String, String, String)> = (0..7)
        .map(|index| {
            (
                format!("chunk:{index:02}"),
                format!("file:{index:02}"),
                // The same term in every document, so the scores tie and the order has to
                // be decided by the chunk id rather than by chance.
                "обход индекса".to_string(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str, &str)> = documents
        .iter()
        .map(|(chunk, file, text)| (chunk.as_str(), file.as_str(), text.as_str()))
        .collect();
    let (_directory, index) = index_of("paging", &borrowed);

    let mut walked = Vec::new();
    let mut cursor: Option<DocumentCursor> = None;
    for _ in 0..5 {
        let matches = index
            .matches("обход", 3, cursor.as_ref(), None)
            .unwrap()
            .into_documents();
        if matches.is_empty() {
            break;
        }
        walked.extend(matches.iter().map(|found| found.chunk_id().to_string()));
        cursor = Some(matches.last().unwrap().cursor().clone());
    }
    let every_chunk: Vec<String> = documents
        .iter()
        .map(|(chunk, _, _)| chunk.clone())
        .collect();
    assert_eq!(walked, every_chunk, "every match exactly once, in order");

    // The same page twice, and the whole walk twice.
    assert_eq!(page(&index, "обход", 3, None), walked[..3]);
    let second = index
        .matches("обход", 3, None, None)
        .unwrap()
        .into_documents();
    assert_eq!(
        page(&index, "обход", 3, Some(second.last().unwrap().cursor())),
        walked[3..6]
    );
}

/// A file with a new current version keeps the documents of that version and no others:
/// the words of the version it replaced are gone, and nothing is stored twice.
#[test]
fn a_new_current_version_replaces_the_documents_of_the_one_before_it() {
    let (_directory, index) = index_of(
        "replacement",
        &[
            ("chunk:a@1:0", "file:a", "первая ревизия про хранилище"),
            ("chunk:a@1:1", "file:a", "и её второй фрагмент"),
            ("chunk:b@1:0", "file:b", "другой файл про хранилище"),
        ],
    );
    assert_eq!(found(&index, "хранилище").len(), 2);

    index
        .publish(
            &["file:a".to_string()],
            build(&[("chunk:a@2:0", "file:a", "вторая ревизия про хранилище")]),
        )
        .unwrap();
    index.finish().unwrap();

    assert_eq!(
        found(&index, "первая"),
        Vec::<String>::new(),
        "the replaced version must leave nothing behind"
    );
    assert_eq!(found(&index, "фрагмент"), Vec::<String>::new());
    assert_eq!(found(&index, "вторая"), vec!["chunk:a@2:0"]);
    let mut both = found(&index, "хранилище");
    both.sort();
    assert_eq!(
        both,
        vec!["chunk:a@2:0", "chunk:b@1:0"],
        "one document per current chunk, and the other file untouched"
    );
    assert_eq!(index.state().unwrap().documents(), 2);
}

/// The index records which rules wrote it — its fields, the version of the suffix table its
/// language field came from, and its analyzer — and an index written by other rules is seen
/// as such rather than answering as if nothing had changed.
///
/// The string is pinned here rather than built from the same constants it is compared
/// against, because a test that composed it the way the code does would pass through any
/// change to either.
#[test]
fn the_index_records_the_rules_that_wrote_it() {
    let both = built_in_normalizations();
    let (directory, index) = index_with(
        "fingerprint",
        &both,
        &[("chunk:a", "file:a", "индексы и trees")],
    );
    let state = index.state().unwrap();
    assert!(state.matches_this_build());
    assert_eq!(
        state.written_by().map(|written| written.to_string()),
        Some(
            "fields=chunk_id,file_id,source_language,text,conversation_name_prefix;\
         suffixes=1;tokenizer=simple-lowercase/1+prefix-5/1;normalizations=ru/1,en/1"
                .to_string()
        )
    );
    drop(index);

    let english_only = load_normalizations(&["en"]).unwrap();
    let reopened = ContextSearchIndex::open(&directory, &english_only).unwrap();
    let state = reopened.state().unwrap();
    assert!(
        !state.matches_this_build(),
        "an index written by another set of normalizations must not pass as this one's"
    );
    // And the disagreement is real: the term the Russian rule produced is not the term
    // this analyzer now looks for.
    assert!(found(&reopened, "индексы").is_empty());
}

/// An index nobody has written to holds nothing, agrees with itself about the analyzer,
/// and answers a query with no results rather than with a failure.
#[test]
fn an_empty_index_answers_nothing_and_is_not_out_of_step() {
    let (_directory, index) = index_of("empty", &[]);
    let state = index.state().unwrap();
    assert_eq!(state.documents(), 0);
    assert_eq!(state.written_by(), None);
    assert!(state.matches_this_build());
    assert!(found(&index, "обход").is_empty());
}

/// A query this program cannot read is refused rather than widened into one it can: a
/// query that quietly became a different query answers something nobody asked.
#[test]
fn a_query_that_cannot_be_read_is_refused() {
    let (_directory, index) = index_of("unreadable", &[("chunk:a", "file:a", "обход")]);
    assert_eq!(
        index.matches("обход AND (", 10, None, None).unwrap_err(),
        SearchIndexError::UnreadableQuery
    );
    assert_eq!(
        index.fragments("обход AND (", &["обход".to_string()]),
        Err(SearchIndexError::UnreadableQuery)
    );
}

/// A result line has to say what matched, and the fragment is cut from the text the caller
/// passes in — the index stores none.
#[test]
fn a_fragment_names_the_words_that_matched() {
    let text = format!(
        "{} Сверщик обходит индекс и сообщает об удалённых локациях. {}",
        "предисловие ".repeat(20),
        "послесловие ".repeat(20)
    );
    let (_directory, index) = index_of("fragment", &[("chunk:a", "file:a", &text)]);
    let fragments = index.fragments("обход локация", &[text]).unwrap();

    let fragment = &fragments[0];
    assert!(
        fragment.text().contains("обходит") || fragment.text().contains("локациях"),
        "the fragment must be cut where the match is: {:?}",
        fragment.text()
    );
    assert!(
        fragment
            .matched()
            .iter()
            .any(|word| word == "обходит" || word == "локациях"),
        "the words it matched on must be named: {:?}",
        fragment.matched()
    );
}

/// Kazakh declared: the forms of one word find each other, which is the whole difference a
/// language pack makes. Without it every form finds only itself.
#[test]
fn kazakh_forms_of_one_word_find_each_other() {
    let documents = [
        ("chunk:1", "file:1", "кітаптар сөресінде тұр"),
        ("chunk:2", "file:2", "бір кітапты оқыдым"),
        ("chunk:3", "file:3", "мүмкіндіктер туралы мақала"),
    ];
    let (_directory, without) = index_with("kk-without", &built_in_normalizations(), &documents);
    let (_second, with) = index_with(
        "kk-with",
        &load_normalizations(&["ru", "en", "kk"]).unwrap(),
        &documents,
    );

    assert_eq!(found(&without, "кітап"), Vec::<String>::new());
    assert_eq!(found(&with, "кітап"), ["chunk:1", "chunk:2"]);
    assert_eq!(found(&with, "кітаптар"), ["chunk:1", "chunk:2"]);
    assert_eq!(found(&with, "мүмкіндік"), ["chunk:3"]);
}

/// A word neither language gives away is found by a reader of either. The document holds
/// both readings and the question is asked in both, which is what makes an ambiguous word
/// belong to the two languages at once instead of to whichever was declared first.
#[test]
fn a_word_neither_language_gives_away_is_found_from_both_sides() {
    let documents = [
        // Kazakh: `елдер` is the plural of `ел`, and every letter of it is Russian too.
        ("chunk:1", "file:1", "көрші елдермен сауда"),
        // Russian, and the same letters: a reader of Russian must still find this one.
        ("chunk:2", "file:2", "хранилище индексов и записей"),
    ];
    let (_directory, index) = index_with(
        "kk-ambiguous",
        &load_normalizations(&["ru", "en", "kk"]).unwrap(),
        &documents,
    );

    // Asked in Kazakh, by another form of the same word.
    assert_eq!(found(&index, "елдер"), ["chunk:1"]);
    assert_eq!(found(&index, "елде"), ["chunk:1"]);
    // Asked in Russian, of the Russian sentence, by forms Russian rules reduce.
    assert_eq!(found(&index, "хранилища"), ["chunk:2"]);
    assert_eq!(found(&index, "записях"), ["chunk:2"]);
}

/// Declaring Kazakh does not take the other languages away: the sentence that was findable
/// by Russian and English forms still is.
#[test]
fn declaring_kazakh_leaves_russian_and_english_as_they_were() {
    let documents = [(
        "chunk:1",
        "file:1",
        "Сверщик обходит индекс и reports deleted locations",
    )];
    let (_directory, before) = index_with("kk-before", &built_in_normalizations(), &documents);
    let (_second, after) = index_with(
        "kk-after",
        &load_normalizations(&["ru", "en", "kk"]).unwrap(),
        &documents,
    );

    for query in ["индексы", "обход", "reported", "location"] {
        assert_eq!(found(&before, query), ["chunk:1"], "{query} before");
        assert_eq!(found(&after, query), ["chunk:1"], "{query} after");
    }
}

/// The index written by the release before this one, opened by this build.
///
/// Written with tantivy directly and with the schema that build declared — three fields and
/// no language — because the point is an index this program can no longer produce. The
/// payload is the self-description of that build, which named its analyzer and nothing else.
fn index_of_the_previous_schema(name: &str) -> PathBuf {
    use tantivy::schema::{IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
    use tantivy::{Index, doc};

    let directory = scratch(name);
    std::fs::create_dir_all(&directory).unwrap();
    let mut builder = Schema::builder();
    builder.add_text_field("chunk_id", TextOptions::default().set_fast(None));
    builder.add_text_field(
        "file_id",
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("raw")
                .set_index_option(IndexRecordOption::Basic),
        ),
    );
    builder.add_text_field(
        "text",
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("default")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        ),
    );
    let schema = builder.build();
    let index = Index::create_in_dir(&directory, schema.clone()).unwrap();
    let mut writer = index.writer_with_num_threads(1, 15_000_000).unwrap();
    writer
        .add_document(doc!(
            schema.get_field("chunk_id").unwrap() => "chunk:a",
            schema.get_field("file_id").unwrap() => "file:a",
            schema.get_field("text").unwrap() => "indexes and trees",
        ))
        .unwrap();
    let mut prepared = writer.prepare_commit().unwrap();
    prepared.set_payload("tokenizer=simple-lowercase/1;normalizations=ru/1,en/1");
    prepared.commit().unwrap();
    directory
}

/// The test this stage turns on: an index written before the language field existed is
/// recognized as one this build did not write, and is not read as fresh.
///
/// Both halves of the description have to carry it, because an index can be out of step in
/// either. The fields differ here, and that is refused by name — with the repair in the
/// sentence, exactly as a mismatched analyzer is — rather than reported as a directory that
/// would not open, which says nothing a reader can act on. A rebuild is allowed to replace
/// it, because the index holds nothing the graph does not.
#[test]
fn an_index_written_before_the_language_field_is_not_read_as_fresh() {
    let directory = index_of_the_previous_schema("previous-schema");
    let normalizations = built_in_normalizations();

    for opened in [
        ContextSearchIndex::open(&directory, &normalizations),
        ContextSearchIndex::open_existing(&directory, &normalizations),
    ] {
        assert_eq!(
            opened.err(),
            Some(SearchIndexError::Unreadable(
                "the index was written with other fields"
            )),
            "an index with other fields is named, not opened"
        );
    }

    let rebuilt = ContextSearchIndex::open_for_rebuild(&directory, &normalizations).unwrap();
    let state = rebuilt.state().unwrap();
    assert_eq!(state.documents(), 0, "the replacement starts empty");
    assert!(state.matches_this_build());
}

/// The other half of the same failure: the same fields, written from another suffix table.
///
/// Nothing about such an index looks wrong — every document is there and every field is —
/// but half of them were labelled by rows the current table does not have. Only the
/// description says so, which is why the table's version is in it.
#[test]
fn an_index_written_from_another_suffix_table_is_not_read_as_fresh() {
    let (directory, index) = index_of("other-suffix-table", &[("chunk:a", "file:a", "обход")]);
    assert!(index.state().unwrap().matches_this_build());
    drop(index);

    // What this build would write with the table one version older. The string is spelled
    // out because it is what an older build would have committed.
    let older = "fields=chunk_id,file_id,source_language,text;suffixes=0;tokenizer=simple-lowercase/1;normalizations=ru/1,en/1";
    rewrite_payload(&directory, older);

    let reopened = ContextSearchIndex::open(&directory, &built_in_normalizations()).unwrap();
    let state = reopened.state().unwrap();
    assert_eq!(state.documents(), 1, "the documents are all there");
    assert!(
        !state.matches_this_build(),
        "and they were still labelled by a table this build does not have"
    );
}

/// An index another build wrote is not relabelled by a pass that adds to it.
///
/// This is what makes the report above worth anything. The documents already in the index
/// were written by other rules and stay that way, so a commit that stamped this build's
/// description on them would erase the only signal there is — the counts agree either way —
/// and `doctor` would call the index fresh after one ordinary pass. Only a rebuild, which
/// replaces everything, relabels it.
#[test]
fn writing_into_an_index_another_build_wrote_does_not_relabel_it() {
    let (directory, index) = index_of("foreign-label", &[("chunk:a", "file:a", "обход")]);
    drop(index);
    let foreign = "fields=chunk_id,file_id,source_language,text;suffixes=0;tokenizer=simple-lowercase/1;normalizations=ru/1,en/1";
    rewrite_payload(&directory, foreign);

    let index = ContextSearchIndex::open(&directory, &built_in_normalizations()).unwrap();
    index
        .publish(
            &["file:b".to_string()],
            build(&[("chunk:b", "file:b", "новый текст")]),
        )
        .unwrap();
    index.finish().unwrap();

    let state = index.state().unwrap();
    assert_eq!(state.documents(), 2, "the new document is in");
    assert_eq!(
        state.written_by().map(|written| written.to_string()),
        Some(foreign.to_string()),
        "and the index still says who wrote the rest of it"
    );
    assert!(!state.matches_this_build());

    // A rebuild is the write that replaces everything, so it is the one that relabels.
    index.clear().unwrap();
    index
        .append(build(&[("chunk:a", "file:a", "обход")]))
        .unwrap();
    index.finish().unwrap();
    assert!(index.state().unwrap().matches_this_build());
}

/// Commits the same documents again under a different self-description, which is what an
/// index written by another build looks like from this one.
fn rewrite_payload(directory: &PathBuf, payload: &str) {
    use tantivy::Index;
    use tantivy::directory::MmapDirectory;

    let index = Index::open(MmapDirectory::open(directory).unwrap()).unwrap();
    let mut writer: tantivy::IndexWriter = index.writer_with_num_threads(1, 15_000_000).unwrap();
    let mut prepared = writer.prepare_commit().unwrap();
    prepared.set_payload(payload);
    prepared.commit().unwrap();
    writer.wait_merging_threads().unwrap();
}

/// Selecting by language, which is the one thing the field is for today: it is asked for
/// through the query the reader already writes, with no syntax of its own.
#[test]
fn documents_can_be_selected_by_the_language_their_name_declares() {
    let directory = scratch("by-language");
    let index = ContextSearchIndex::open(&directory, &built_in_normalizations()).unwrap();
    index
        .append(vec![
            document("chunk:php", "file:php", Some(SourceLanguage::Php), "обход"),
            document(
                "chunk:python",
                "file:python",
                Some(SourceLanguage::Python),
                "обход",
            ),
            document("chunk:none", "file:none", None, "обход"),
        ])
        .unwrap();
    index.finish().unwrap();

    assert_eq!(found(&index, "source_language:php"), ["chunk:php"]);
    assert_eq!(found(&index, "source_language:python"), ["chunk:python"]);
    // The word every document holds, narrowed by the language of the file it came from.
    assert_eq!(
        found(&index, "обход AND source_language:php"),
        ["chunk:php"]
    );
    // A file whose name declares nothing has no such field, so it answers no query for a
    // language at all — including one for a value meaning "none".
    let mut everything = found(&index, "обход");
    everything.sort();
    assert_eq!(everything, ["chunk:none", "chunk:php", "chunk:python"]);
    assert!(found(&index, "source_language:none").is_empty());
    assert!(found(&index, "source_language:rust").is_empty());
}

/// The language is one term, not words: `php` is matched as it is written and never stemmed
/// or folded in with the prose around it.
#[test]
fn the_language_is_stored_as_one_term_and_is_not_part_of_the_text() {
    let directory = scratch("language-is-a-term");
    let index = ContextSearchIndex::open(&directory, &built_in_normalizations()).unwrap();
    index
        .append(vec![document(
            "chunk:a",
            "file:a",
            Some(SourceLanguage::TypeScript),
            "обход дерева",
        )])
        .unwrap();
    index.finish().unwrap();

    assert_eq!(found(&index, "source_language:typescript"), ["chunk:a"]);
    // Not in the text field: a plain query for the language finds nothing, because the
    // language is not something the document says.
    assert!(found(&index, "typescript").is_empty());
}

fn document(
    chunk: &str,
    file: &str,
    language: Option<SourceLanguage>,
    text: &str,
) -> crate::application::SearchDocument {
    crate::application::SearchDocument::new(chunk, file, language, text)
}

/// A row is a file. Three chunks of one file that matched are one line, and a page of five
/// lines is five different files — which is the whole of what Stage 33 changed for a reader.
#[test]
fn a_file_that_matched_in_three_chunks_takes_one_row_and_a_page_is_that_many_files() {
    let (_directory, index) = index_of(
        "folding",
        &[
            ("chunk:loud:0", "file:loud", "обход индекса и снова обход"),
            ("chunk:loud:1", "file:loud", "ещё раз про обход"),
            ("chunk:loud:2", "file:loud", "и опять обход индекса"),
            ("chunk:quiet:0", "file:quiet", "обход упомянут один раз"),
            ("chunk:third:0", "file:third", "обход и третий файл"),
        ],
    );

    let rows = rows(&index, "обход", 5, None);
    let files: Vec<&str> = rows.iter().map(|row| row.cursor().file_id()).collect();
    assert_eq!(
        files.len(),
        3,
        "three files matched, so three rows: {files:?}"
    );
    assert_eq!(
        files.iter().collect::<std::collections::HashSet<_>>().len(),
        3,
        "no file may be named twice on one page: {files:?}"
    );

    let loud = rows
        .iter()
        .find(|row| row.cursor().file_id() == "file:loud")
        .expect("the file that matched three times has a row");
    assert_eq!(
        loud.matches(),
        3,
        "the row says how many places inside the file it stands for"
    );
    assert!(
        loud.chunk_id().starts_with("chunk:loud:"),
        "the row is shown by a chunk of its own file, {:?}",
        loud.chunk_id()
    );
}

/// The counting rule, on the shape of the failure Stage 29 measured: a pasted log with one
/// word four hundred times in a single reply against a conversation that said it in three.
#[test]
fn saying_it_four_hundred_times_in_one_place_does_not_beat_saying_it_in_three() {
    let log = "контрагент ".repeat(400);
    let (_directory, index) = index_of(
        "counting-rule",
        &[
            ("chunk:log:0", "file:log", "лог выгрузки"),
            ("chunk:log:1", "file:log", log.as_str()),
            ("chunk:talk:0", "file:talk", "разбор"),
            ("chunk:talk:1", "file:talk", "нужен контрагент из реестра"),
            ("chunk:talk:2", "file:talk", "этот контрагент уже заведён"),
            (
                "chunk:talk:3",
                "file:talk",
                "последний контрагент на сегодня",
            ),
        ],
    );

    let rows = rows(&index, "контрагент", 5, None);
    assert_eq!(
        rows.iter()
            .map(|row| row.cursor().file_id())
            .collect::<Vec<_>>(),
        vec!["file:talk", "file:log"],
        "three separate replies must outrank one pasted log"
    );
}

/// Paging over rows: page two neither repeats nor loses a file of page one, and the same
/// page asked for twice is the same page. This is the keyset promise of ADR 0013, now made
/// about files rather than about chunks.
#[test]
fn pages_of_rows_partition_the_files_exactly() {
    let documents: Vec<(String, String, String)> = (0..6)
        .flat_map(|file| {
            // Two chunks per file, so a page of chunks would have named three files where a
            // page of rows names six.
            (0..2).map(move |chunk| {
                (
                    format!("chunk:{file:02}:{chunk}"),
                    format!("file:{file:02}"),
                    "обход индекса".to_string(),
                )
            })
        })
        .collect();
    let borrowed: Vec<(&str, &str, &str)> = documents
        .iter()
        .map(|(chunk, file, text)| (chunk.as_str(), file.as_str(), text.as_str()))
        .collect();
    let (_directory, index) = index_of("row-paging", &borrowed);

    let mut walked: Vec<String> = Vec::new();
    let mut cursor: Option<DocumentCursor> = None;
    for _ in 0..5 {
        let page = rows(&index, "обход", 2, cursor.as_ref());
        if page.is_empty() {
            break;
        }
        walked.extend(page.iter().map(|row| row.cursor().file_id().to_string()));
        cursor = Some(page.last().unwrap().cursor().clone());
    }

    let every_file: Vec<String> = (0..6).map(|file| format!("file:{file:02}")).collect();
    assert_eq!(walked, every_file, "every file exactly once, in order");

    let first = rows(&index, "обход", 2, None);
    let again = rows(&index, "обход", 2, None);
    assert_eq!(
        first.iter().map(|row| row.cursor()).collect::<Vec<_>>(),
        again.iter().map(|row| row.cursor()).collect::<Vec<_>>(),
        "the same page asked for twice is the same page"
    );
}

/// What the cursor does not promise, stated as a test so nobody reads more into it.
///
/// ADR 0013 decision 9: a cursor is a place in a ranking and not a snapshot of the index.
/// A document published between two pages changes what the scores of the existing documents
/// are, so a row can cross the boundary in either direction. What is still owed is that the
/// cursor is read rather than refused, that the answer is well formed, and that no file is
/// named twice **inside** one page.
#[test]
fn a_cursor_taken_before_a_write_is_answered_against_the_index_after_it() {
    let documents: Vec<(String, String, String)> = (0..6)
        .map(|file| {
            (
                format!("chunk:{file:02}:0"),
                format!("file:{file:02}"),
                "обход индекса".to_string(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str, &str)> = documents
        .iter()
        .map(|(chunk, file, text)| (chunk.as_str(), file.as_str(), text.as_str()))
        .collect();
    let (_directory, index) = index_of("cursor-under-a-write", &borrowed);

    let first = rows(&index, "обход", 2, None);
    let cursor = first.last().unwrap().cursor().clone();

    index
        .publish(
            &["file:new".to_string()],
            build(&[("chunk:new:0", "file:new", "обход индекса заново")]),
        )
        .unwrap();
    index.finish().unwrap();

    let second = rows(&index, "обход", 2, Some(&cursor));
    let named: Vec<&str> = second.iter().map(|row| row.cursor().file_id()).collect();
    assert_eq!(
        named.iter().collect::<std::collections::HashSet<_>>().len(),
        named.len(),
        "a file may cross the boundary, but never appear twice in one page: {named:?}"
    );
    assert!(
        !named.is_empty(),
        "the cursor is still read against the index as it is now"
    );
}

/// The fold holds a counter per file a query matched, and a query does not bound that
/// number. When the ceiling is reached the page is built from the files the fold did hold,
/// and it says so — the difference between a short answer and an answer that looks complete.
#[test]
fn a_query_matching_more_files_than_the_fold_holds_says_so() {
    let documents: Vec<(String, String, String)> = (0..8)
        .map(|file| {
            (
                format!("chunk:{file:02}:0"),
                format!("file:{file:02}"),
                "обход индекса".to_string(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str, &str)> = documents
        .iter()
        .map(|(chunk, file, text)| (chunk.as_str(), file.as_str(), text.as_str()))
        .collect();
    let (_directory, index) = index_of("fold-ceiling", &borrowed);

    let within = index.matches_folding("обход", 3, None, None, 8).unwrap();
    assert_eq!(
        within.ceiling(),
        None,
        "eight files fit under a ceiling of eight"
    );
    assert_eq!(within.into_documents().len(), 3);

    let reached = index.matches_folding("обход", 3, None, None, 5).unwrap();
    assert_eq!(
        reached.ceiling(),
        Some(5),
        "a query that met more files than the fold holds must report the ceiling"
    );
    assert_eq!(
        reached.into_documents().len(),
        3,
        "the page is still full, and built out of the files the fold did hold"
    );
}
