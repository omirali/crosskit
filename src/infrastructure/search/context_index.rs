// The search index of one context: a directory of its own, beside that context's graph.
//
// Isolation is storage, not a predicate. There is no shared index with a context column to
// filter on, so a query cannot reach another context's text by being written wrongly — the
// words of another context are not in the files this searcher opens. That is the same
// argument as ADR 0008 makes for the databases, and the reason no function here takes two
// contexts.
//
// What the index holds is terms, chunk ids, file ids and the language a file's name
// declares. No path, no text, no status: everything a reader is shown is read back from the
// graph when the answer is built. So the index cannot leak a path the policy has since
// hidden, cannot show text the graph no longer has, and can be deleted at any moment without
// losing anything but the time it takes to write it again. The language is the one thing here
// that is not read out of the graph, because the graph does not hold it — it is derived from
// the path, by the table in `SourceLanguage`, and the version of that table is part of what
// the index records about itself.
use std::cell::{Cell, RefCell, RefMut};
use std::path::{Path, PathBuf};

use tantivy::directory::MmapDirectory;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser};
use tantivy::schema::{Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions};
use tantivy::snippet::SnippetGenerator;
use tantivy::tokenizer::TokenizerManager;
use tantivy::{Index, IndexWriter, ReloadPolicy, Searcher, Term, doc};

use crate::application::{
    DocumentMatch, MatchFragment, MatchedDocuments, PeriodScope, SearchDocument, SearchIndexError,
    SearchIndexReader, SearchIndexState, SearchIndexWriter,
};
use crate::domain::{
    DocumentCursor, IndexFingerprint, NormalizationSet, SUFFIX_TABLE_VERSION, WorkspaceName,
};

use super::analyzer::{
    ANALYZER, PREFIX_ANALYZER, Reading, analyzer, analyzer_rules, prefix_analyzer,
};
use super::document_collector::DocumentCollector;

const CHUNK_ID: &str = "chunk_id";
const FILE_ID: &str = "file_id";
const SOURCE_LANGUAGE: &str = "source_language";
const TEXT: &str = "text";
/// The conversation's own name, reduced to the first letters of each word.
///
/// One field and not two. Stage 32 wrote the name twice — once through the language packs
/// and once through the prefix rule — on Stage 31's finding that the two reductions fail on
/// different questions. Stage 33 measured them apart through the real command, and the
/// morphological one earned nothing: the prefix field alone answers the fifteen questions
/// exactly as well as both together (0.80 and 15/15 either way), while the morphological one
/// alone gives 0.73. See ADR 0038.
const CONVERSATION_NAME_PREFIX: &str = "conversation_name_prefix";

/// Every field the schema declares, in the order it declares them: half of what an index
/// records about how it was written.
///
/// One list, used twice — `schema()` is checked against it by a test, and the fingerprint is
/// built from it — because an index whose fields nobody wrote down describes itself as
/// whatever this build happens to compile, and a document missing a field a query filters on
/// is invisible to that query while every count still agrees.
const FIELDS: [&str; 5] = [
    CHUNK_ID,
    FILE_ID,
    SOURCE_LANGUAGE,
    TEXT,
    CONVERSATION_NAME_PREFIX,
];

/// What an index written by this build records about itself.
///
/// The tokenizer component names both reductions, not one: an index whose name field was cut
/// at five letters is not the same index as one cut at six, and no other part of the
/// fingerprint would move if that number changed.
fn fingerprint(normalizations: &NormalizationSet) -> IndexFingerprint {
    IndexFingerprint::of(
        &FIELDS,
        SUFFIX_TABLE_VERSION,
        &analyzer_rules(),
        normalizations,
    )
}

/// How much of a chunk a fragment shows. Long enough to read the match in its sentence,
/// short enough for a terminal line to stay a line.
const FRAGMENT_CHARS: usize = 200;

/// How many distinct files one query folds, per segment of the index.
///
/// Folding costs one counter per file a query matched, and nothing in a query bounds that
/// number: a common word in a context covering a home directory matches in as many files as
/// the context holds. The counters are small — an ordinal, a score and a count, and no
/// strings until a row reaches a page — so a hundred thousand of them is a few megabytes,
/// which is what this program can spend beside a scan under its measured memory limit.
///
/// It is a ceiling and not a budget: a query that reaches it is answered from the files the
/// fold did hold, and the page says so. A query matching more than a hundred thousand files
/// has no answer on its first page anyway; what it must not do is look complete.
const FOLD_CEILING: usize = 100_000;

/// Stage 33 measurement: what a match on a conversation's own name is worth against a match
/// in its body.
const NAME_WEIGHT: f32 = 40.0;

/// One indexing thread, with the smallest heap the library accepts — 15 MB, which it
/// refuses to go below.
///
/// Not a default taken for granted: each of the writer's threads holds an arena of this
/// size, and this program is measured against a hard memory limit. One batch of chunk text
/// is bounded by the batch payload limit and fits in it many times over, so more threads
/// would buy nothing but memory the scan beside it needs.
const WRITER_THREADS: usize = 1;
const WRITER_HEAP_BYTES: usize = 15_000_000;

/// After how many published files the writer folds what it has staged into a commit.
///
/// It exists because of what a pass holds until it commits, and that is not the documents:
/// Stage 19 measured the two halves of `publish` apart, and the documents level off at
/// about 27 MB — the writer spills its segments to disk on its own — while the deletions
/// grow in a straight line at 1.24 KB per file with nothing bounding them. The entry is not
/// the term either: a three-byte term costs 1 210 B against 1 240 B for a thirty-eight byte
/// one, so what is held is the structure around the term, which belongs to the library and
/// cannot be made smaller from here. That leaves the interval.
///
/// `usize::MAX` means what Stage 9 chose: one commit, at the end of the pass. The number
/// here is the one Stage 19 picked off the curve; the reasoning is in ADR 0025.
#[cfg(not(feature = "search-commit-benchmark"))]
fn commit_every_files() -> usize {
    usize::MAX
}

/// The same, for a binary built by the search commit benchmark: the interval comes from the
/// environment of the compiler, not of the running program, so a measured binary still has
/// exactly one fixed interval. A value that cannot be used is refused rather than silently
/// replaced, because a benchmark that measured something other than it printed would be
/// worse than no measurement.
#[cfg(feature = "search-commit-benchmark")]
fn commit_every_files() -> usize {
    let Some(value) = option_env!("CROSSKIT_SEARCH_COMMIT_FILES") else {
        return usize::MAX;
    };
    match value.parse::<usize>() {
        Ok(files) if files > 0 => files,
        _ => panic!(
            "CROSSKIT_SEARCH_COMMIT_FILES must be a whole number above zero, received {value:?}"
        ),
    }
}

/// `search-<context>` beside `graph-<context>.lbug`, so the two halves of a context are
/// found the same way. The name comes from the context's own identity, which refuses
/// anything that is not a plain name, so nothing here has to escape a path.
pub fn search_directory(data_directory: &Path, workspace: &WorkspaceName) -> PathBuf {
    data_directory.join(format!("search-{workspace}"))
}

pub struct ContextSearchIndex {
    index: Index,
    fields: Fields,
    fingerprint: IndexFingerprint,
    /// The analyzers a question is read with: one per reading a word may have, each giving
    /// one term per word. The index holds every reading, and a question asks in all of them.
    asking: Vec<TokenizerManager>,
    /// Opened on the first write and held until this index is dropped. Opening it takes an
    /// exclusive lock on the directory, so a command that only reads never takes one and
    /// two writers of one context cannot exist.
    writer: RefCell<Option<IndexWriter>>,
    /// Files published since the last commit. What a pass holds is the deletion queue, and
    /// it holds one entry per published file, so files are what the interval counts.
    files_since_commit: Cell<usize>,
    /// What the index said about itself when it was opened, while that is not what this
    /// build would write.
    ///
    /// A pass that writes into such an index does not relabel it. The documents already in
    /// it were written by other rules and stay that way, so a commit that stamped this
    /// build's fingerprint on them would erase the one signal there is: counts still agree,
    /// and after one ordinary pass `doctor` would call an index fresh when most of it was
    /// written by rules nobody applies any more. It is dropped by `clear`, because a rebuild
    /// is the one write that replaces everything.
    written_by_another_build: RefCell<Option<IndexFingerprint>>,
}

struct Fields {
    chunk_id: Field,
    file_id: Field,
    source_language: Field,
    text: Field,
    conversation_name_prefix: Field,
}

impl ContextSearchIndex {
    /// The index in `directory`, created empty if it is not there yet.
    pub fn open(
        directory: &Path,
        normalizations: &NormalizationSet,
    ) -> Result<Self, SearchIndexError> {
        std::fs::create_dir_all(directory)
            .map_err(|_| SearchIndexError::Unavailable("be created"))?;
        let mmap = MmapDirectory::open(directory)
            .map_err(|_| SearchIndexError::Unavailable("be opened"))?;
        let index = Index::open_or_create(mmap, schema()).map_err(open_failure)?;
        Self::wrap(index, normalizations)
    }

    /// The index in `directory` as it stands, for a command that may not make one.
    ///
    /// `open` creates an empty index where there is none, which is what every command that
    /// indexes wants and what a command that only reports must not do: it would write to
    /// the disk in order to answer a question about the disk, and then answer "empty" about
    /// an index that was empty because of the asking. So this one opens and never creates.
    pub fn open_existing(
        directory: &Path,
        normalizations: &NormalizationSet,
    ) -> Result<Self, SearchIndexError> {
        let mmap = MmapDirectory::open(directory)
            .map_err(|_| SearchIndexError::Unavailable("be opened"))?;
        let index = Index::open(mmap).map_err(open_failure)?;
        Self::wrap(index, normalizations)
    }

    /// The same, for a rebuild, which may throw away an index it cannot open.
    ///
    /// Only a rebuild does this. An index written by a schema this build does not know
    /// cannot be read and cannot be repaired in place, and it holds nothing the graph does
    /// not — so a rebuild is allowed to replace the directory, while every other command
    /// reports the failure and changes nothing.
    pub fn open_for_rebuild(
        directory: &Path,
        normalizations: &NormalizationSet,
    ) -> Result<Self, SearchIndexError> {
        match Self::open(directory, normalizations) {
            Ok(index) => Ok(index),
            Err(_) => {
                std::fs::remove_dir_all(directory)
                    .map_err(|_| SearchIndexError::Unavailable("be replaced"))?;
                Self::open(directory, normalizations)
            }
        }
    }

    fn wrap(index: Index, normalizations: &NormalizationSet) -> Result<Self, SearchIndexError> {
        index
            .tokenizers()
            .register(ANALYZER, analyzer(normalizations, Reading::Every));
        index
            .tokenizers()
            .register(PREFIX_ANALYZER, prefix_analyzer());
        // A question is read one term per word, once for each reading a word may have, and
        // the parses are joined by "or" below. Expanding a question into several readings at
        // one position instead would make the parser read them as a phrase — words that must
        // follow one another — and a phrase of two readings of one word matches nothing.
        let asking: Vec<TokenizerManager> = (0..normalizations.max_readings())
            .map(|ordinal| {
                let manager = TokenizerManager::default();
                manager.register(ANALYZER, analyzer(normalizations, Reading::Only(ordinal)));
                // A question is read for the prefix field too, by the same rule the field was
                // written with. Without it the parser would hand the field whole words and
                // the field holds only their first letters, so it would match nothing at all.
                manager.register(PREFIX_ANALYZER, prefix_analyzer());
                manager
            })
            .collect();
        let schema = index.schema();
        let fields = Fields {
            chunk_id: field(&schema, CHUNK_ID)?,
            file_id: field(&schema, FILE_ID)?,
            source_language: field(&schema, SOURCE_LANGUAGE)?,
            text: field(&schema, TEXT)?,
            conversation_name_prefix: field(&schema, CONVERSATION_NAME_PREFIX)?,
        };
        let fingerprint = fingerprint(normalizations);
        let written_by_another_build =
            stored_fingerprint(&index)?.filter(|stored| *stored != fingerprint);
        Ok(Self {
            index,
            fields,
            fingerprint,
            asking,
            writer: RefCell::new(None),
            files_since_commit: Cell::new(0),
            written_by_another_build: RefCell::new(written_by_another_build),
        })
    }

    /// The writer, opened on first use.
    ///
    /// A lock held by another process is its own answer rather than a generic failure: the
    /// caller may only be indexing, in which case the right outcome is an index that lags
    /// and says so.
    fn writer(&self) -> Result<RefMut<'_, IndexWriter>, SearchIndexError> {
        let mut writer = self.writer.borrow_mut();
        if writer.is_none() {
            *writer = Some(
                self.index
                    .writer_with_num_threads(WRITER_THREADS, WRITER_HEAP_BYTES)
                    .map_err(|error| match error {
                        tantivy::TantivyError::LockFailure(_, _) => SearchIndexError::Busy,
                        _ => SearchIndexError::Unavailable("be written"),
                    })?,
            );
        }
        Ok(RefMut::map(writer, |writer| {
            writer.as_mut().expect("the writer was just opened")
        }))
    }

    /// Makes everything staged visible, and records which analyzer wrote it.
    ///
    /// The fingerprint travels with the commit rather than in a file beside the index: an
    /// index and the rules its contents were produced by must not be able to disagree because
    /// one of two writes failed.
    ///
    /// An index another build wrote keeps saying so. Adding documents to it does not make the
    /// ones already there this build's work, and the payload is what a reader has to go on.
    fn commit(&self, writer: &mut IndexWriter) -> Result<(), SearchIndexError> {
        let mut prepared = writer
            .prepare_commit()
            .map_err(|_| SearchIndexError::Unavailable("be written"))?;
        let borrowed = self.written_by_another_build.borrow();
        let recorded = borrowed.as_ref().unwrap_or(&self.fingerprint);
        prepared.set_payload(recorded.as_str());
        drop(borrowed);
        prepared
            .commit()
            .map_err(|_| SearchIndexError::Unavailable("be written"))?;
        Ok(())
    }

    /// Folds what is staged once `interval` files have been published since the last time.
    ///
    /// A commit is what releases the deletion queue, and the queue is the only thing a pass
    /// holds without bound. It is not done for durability — an index is a derivative and is
    /// rebuilt from the graph — but for the memory an unbounded queue costs.
    ///
    /// The count is reset even when the commit fails, so a writer that cannot commit is
    /// asked again after the next interval rather than on every batch.
    fn commit_if_due(
        &self,
        writer: &mut IndexWriter,
        published: usize,
    ) -> Result<(), SearchIndexError> {
        let interval = commit_every_files();
        let due = self.files_since_commit.get().saturating_add(published);
        if due < interval {
            self.files_since_commit.set(due);
            return Ok(());
        }
        self.files_since_commit.set(0);
        self.commit(writer)
    }

    fn searcher(&self) -> Result<Searcher, SearchIndexError> {
        Ok(self
            .index
            .reader_builder()
            // Manual, because a command reads the index once and ends: the policy that
            // watches for new commits would start a thread for nothing.
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|_| SearchIndexError::Unavailable("be read"))?
            .searcher())
    }

    /// A document whose file's name declares no language this build knows gets no language
    /// field at all, rather than a value meaning none. An absent field is absent: it matches
    /// no query for a language, and it cannot later be mistaken for a decision somebody made.
    fn add_all(
        &self,
        writer: &IndexWriter,
        documents: Vec<SearchDocument>,
    ) -> Result<(), SearchIndexError> {
        for document in documents {
            let mut written = doc!(
                self.fields.chunk_id => document.chunk_id(),
                self.fields.file_id => document.file_id(),
                self.fields.text => document.text(),
            );
            if let Some(language) = document.language() {
                written.add_text(self.fields.source_language, language.as_str());
            }
            if let Some(name) = document.conversation_name() {
                written.add_text(self.fields.conversation_name_prefix, name);
            }
            writer
                .add_document(written)
                .map_err(|_| SearchIndexError::Unavailable("be written"))?;
        }
        Ok(())
    }

    /// The query as the library reads it, over the text field.
    ///
    /// A query it cannot read is refused. It is never widened into one that parses, because
    /// a query that quietly became a different query returns results the reader will
    /// believe are the answer to what they asked.
    /// The question, read in every reading its words may have, joined by "or".
    ///
    /// One reading is the ordinary case and costs one parse. Two declared languages of one
    /// script make two, and a document matches when either reading of the question matches —
    /// which is what lets a word neither language gives away be found by a reader of either.
    fn parse(&self, query: &str) -> Result<Box<dyn Query>, SearchIndexError> {
        let mut readings: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(self.asking.len());
        for manager in &self.asking {
            // Two fields, not one. The body answers what was talked about; the name field
            // answers what the conversation is called, which for "which chat did we …?" is
            // literally the answer. The parser applies each field's own tokenizer to the
            // question, so the name field is asked in prefixes without the caller knowing,
            // and the weight puts its scores on the same scale as the body's.
            let mut parser = QueryParser::new(
                self.index.schema(),
                vec![self.fields.text, self.fields.conversation_name_prefix],
                manager.clone(),
            );
            parser.set_field_boost(self.fields.conversation_name_prefix, NAME_WEIGHT);
            let parsed = parser
                .parse_query(query)
                .map_err(|_| SearchIndexError::UnreadableQuery)?;
            readings.push((Occur::Should, parsed));
        }
        match readings.len() {
            1 => Ok(readings.remove(0).1),
            _ => Ok(Box::new(BooleanQuery::new(readings))),
        }
    }
}

impl SearchIndexWriter for ContextSearchIndex {
    fn publish(
        &self,
        files: &[String],
        documents: Vec<SearchDocument>,
    ) -> Result<(), SearchIndexError> {
        let mut writer = self.writer()?;
        // Deletions first: tantivy applies a deletion to the documents added before it, so
        // the documents added below survive it and the ones of the version they replace —
        // written by an earlier pass, and so earlier still — do not.
        for file in files {
            writer.delete_term(Term::from_field_text(self.fields.file_id, file));
        }
        self.add_all(&writer, documents)?;
        self.commit_if_due(&mut writer, files.len())
    }

    fn append(&self, documents: Vec<SearchDocument>) -> Result<(), SearchIndexError> {
        let writer = self.writer()?;
        self.add_all(&writer, documents)
    }

    /// Emptying is the one write that replaces everything, so it is also the one that lets
    /// the index be labelled by the build doing the writing.
    fn clear(&self) -> Result<(), SearchIndexError> {
        let writer = self.writer()?;
        writer
            .delete_all_documents()
            .map_err(|_| SearchIndexError::Unavailable("be emptied"))?;
        *self.written_by_another_build.borrow_mut() = None;
        Ok(())
    }

    /// Commits what is still staged and closes the writer, which is what waits for the
    /// merges the commits scheduled. A run that never wrote anything has no writer and
    /// nothing to settle.
    fn finish(&self) -> Result<(), SearchIndexError> {
        let Some(mut writer) = self.writer.borrow_mut().take() else {
            return Ok(());
        };
        self.files_since_commit.set(0);
        self.commit(&mut writer)?;
        writer
            .wait_merging_threads()
            .map_err(|_| SearchIndexError::Unavailable("be written"))
    }
}

impl ContextSearchIndex {
    /// One page of rows, folding at most `ceiling` distinct files.
    ///
    /// The ceiling is a parameter of this one function rather than only a constant, so that
    /// what happens when a query reaches it is tested against a real index instead of
    /// described in a comment. Nothing outside this module chooses it: `matches` passes
    /// `FOLD_CEILING` and there is no other caller.
    pub(super) fn matches_folding(
        &self,
        query: &str,
        limit: usize,
        after: Option<&DocumentCursor>,
        within: Option<&PeriodScope>,
        ceiling: usize,
    ) -> Result<MatchedDocuments, SearchIndexError> {
        let searcher = self.searcher()?;
        let query = self.parse(query)?;
        let page = searcher
            .search(
                query.as_ref(),
                &DocumentCollector::new(limit.max(1), after.cloned(), ceiling, FILE_ID, CHUNK_ID)
                    .within(within),
            )
            .map_err(|_| SearchIndexError::Unavailable("be read"))?;
        if page.unreadable > 0 {
            return Err(SearchIndexError::Unreadable(
                "a match has no chunk or file id",
            ));
        }
        let matched = MatchedDocuments::new(
            page.ranked
                .into_iter()
                .map(|row| DocumentMatch::new(row.chunk_id, row.matches, row.cursor))
                .collect(),
        );
        Ok(match page.reached_the_ceiling {
            true => matched.folding_stopped_at(ceiling),
            false => matched,
        })
    }
}

impl SearchIndexReader for ContextSearchIndex {
    fn matches(
        &self,
        query: &str,
        limit: usize,
        after: Option<&DocumentCursor>,
        within: Option<&PeriodScope>,
    ) -> Result<MatchedDocuments, SearchIndexError> {
        self.matches_folding(query, limit, after, within, FOLD_CEILING)
    }
    fn fragments(
        &self,
        query: &str,
        texts: &[String],
    ) -> Result<Vec<MatchFragment>, SearchIndexError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let searcher = self.searcher()?;
        let query = self.parse(query)?;
        let mut generator = SnippetGenerator::create(&searcher, query.as_ref(), self.fields.text)
            .map_err(|_| SearchIndexError::Unavailable("be read"))?;
        generator.set_max_num_chars(FRAGMENT_CHARS);

        Ok(texts
            .iter()
            .map(|text| {
                let snippet = generator.snippet(text);
                if snippet.fragment().is_empty() {
                    // Nothing in this text carries a term of the query: the match was in
                    // the index and the text has since changed. The beginning of what the
                    // graph holds now is shown rather than an empty line.
                    return MatchFragment::new(beginning_of(text), Vec::new());
                }
                let matched = matched_words(snippet.fragment(), snippet.highlighted());
                MatchFragment::new(snippet.fragment(), matched)
            })
            .collect())
    }

    fn state(&self) -> Result<SearchIndexState, SearchIndexError> {
        let documents = self.searcher()?.num_docs();
        Ok(SearchIndexState::new(
            documents,
            stored_fingerprint(&self.index)?,
            self.fingerprint.clone(),
        ))
    }
}

/// The words of a fragment that the query matched, in the order they appear and without
/// repetition: what the result line says it matched on.
fn matched_words(fragment: &str, highlighted: &[std::ops::Range<usize>]) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for range in highlighted {
        let Some(word) = fragment.get(range.clone()) else {
            continue;
        };
        if !words.iter().any(|already| already == word) {
            words.push(word.to_string());
        }
    }
    words
}

/// At most `FRAGMENT_CHARS` characters from the start of a text, cut on a character
/// boundary.
fn beginning_of(text: &str) -> String {
    text.chars().take(FRAGMENT_CHARS).collect()
}

fn schema() -> Schema {
    let mut builder = Schema::builder();
    // Fast, not stored: the collector reads it per document to break ties in the ranking,
    // and nothing ever reads it out of a stored document.
    builder.add_text_field(CHUNK_ID, TextOptions::default().set_fast(None));
    // Indexed as one raw term, which is what a deletion by file matches on, and fast,
    // because folding a page of rows reads it for every matching document.
    builder.add_text_field(
        FILE_ID,
        TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("raw")
                    .set_index_option(IndexRecordOption::Basic),
            )
            .set_fast(None),
    );
    // Indexed as one raw term, like the file id and for the same reason: `php` is a value to
    // select on, not a word to be normalized. Running it through the analyzer would stem it
    // and fold it in with the prose, so a document about PHP would answer a query for the
    // language and a file written in it might not.
    builder.add_text_field(
        SOURCE_LANGUAGE,
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("raw")
                .set_index_option(IndexRecordOption::Basic),
        ),
    );
    // Indexed with positions, so a phrase query is answerable; not stored, because the
    // graph holds the text.
    builder.add_text_field(
        TEXT,
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(ANALYZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        ),
    );
    // The conversation's name. Stored nowhere: the graph holds the name, and this exists to
    // be matched against. A document that is not a conversation's name simply has no such
    // field — an absent field is absent, and cannot be mistaken for an empty name.
    builder.add_text_field(
        CONVERSATION_NAME_PREFIX,
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(PREFIX_ANALYZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        ),
    );
    builder.build()
}

/// What the index's last commit recorded about how it was written, if it has ever been
/// committed to.
fn stored_fingerprint(index: &Index) -> Result<Option<IndexFingerprint>, SearchIndexError> {
    Ok(index
        .load_metas()
        .map_err(|_| SearchIndexError::Unavailable("be read"))?
        .payload
        .map(IndexFingerprint::stored))
}

/// An index whose fields are not the ones this build declares is named as that, and not
/// reported as a directory that would not open.
///
/// The library compares the whole schema and refuses, which is the right refusal and the
/// wrong sentence: the repair is the same rebuild as for every other way an index can be out
/// of step, and a reader told only that something "could not be opened" has no reason to
/// think so.
fn open_failure(error: tantivy::TantivyError) -> SearchIndexError {
    match error {
        tantivy::TantivyError::SchemaError(_) => {
            SearchIndexError::Unreadable("the index was written with other fields")
        }
        _ => SearchIndexError::Unavailable("be opened"),
    }
}

fn field(schema: &Schema, name: &'static str) -> Result<Field, SearchIndexError> {
    schema
        .get_field(name)
        .map_err(|_| SearchIndexError::Unreadable("the index was written with other fields"))
}

#[cfg(test)]
mod tests {
    use super::{FIELDS, schema};

    /// The fields an index says it has are the fields it has.
    ///
    /// Two lists exist because each is needed in a different shape — one builds a schema,
    /// the other is written into the index's description of itself — and a field added to
    /// one and not the other is the failure this whole stage is about: an index that
    /// describes itself as something it is not, with every count still agreeing.
    #[test]
    fn the_description_names_every_field_the_schema_declares_and_no_other() {
        let declared: Vec<String> = schema()
            .fields()
            .map(|(_, entry)| entry.name().to_string())
            .collect();

        assert_eq!(
            declared, FIELDS,
            "the schema and the description must agree"
        );
    }
}
