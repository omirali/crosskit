// Collecting one page of rows: the best `limit` documents that rank strictly after a cursor.
//
// The library ranks chunks; this folds them into documents and decides which slice of that
// ranking is returned. Both halves have to be written rather than taken off the shelf.
//
// Folding, because a row is a file: a page of chunks names one file three times when three of
// its chunks matched, which is what Stage 29 measured on real transcripts. The score of a row
// is `DocumentScore` — the domain's rule, not a sum — and the chunk it shows is its best one,
// so the line a reader sees is the evidence the row was ranked on.
//
// Paging, because tantivy's own top-collector pages by offset, and an offset is a position in
// a list: two rows that score the same have no defined order between them, so a page boundary
// can fall between them differently on each run, and one of them is returned twice while the
// other is never returned at all. The order here is `(score, file id)`, which is a property of
// the rows themselves, so the same query answers the same way every time and page two costs
// what page one did.
//
// A row is excluded from a page exactly when its score ranks it before the cursor. Nothing has
// to remember which files an earlier page showed: a document's score is folded from all of its
// matches, so a file that was shown before has the same score now and lands before the cursor
// on its own.
//
// A period narrows what is folded, and it narrows it here, before the fold (ADR 0043). The
// period names files and chunks by id; each segment turns those ids into the ordinals of its
// own columns once, and a match outside them is passed over before it is counted. Nothing in
// the index changes for that: no field, no rebuild.
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use tantivy::collector::{Collector, SegmentCollector};
use tantivy::columnar::StrColumn;
use tantivy::{DocId, Score, SegmentOrdinal, SegmentReader};

use crate::application::PeriodScope;
use crate::domain::{DocumentCursor, DocumentScore};

/// The best rows of one page, and what the fold could not hold.
pub(super) struct RankedPage {
    pub(super) ranked: Vec<RankedDocument>,
    /// Documents whose chunk or file id the index would not give back. They are counted
    /// rather than skipped: a result silently dropped is the one failure a search must not
    /// have.
    pub(super) unreadable: usize,
    /// Whether some segment met more distinct files than one query folds. The page is then
    /// built from the files the fold did hold, and saying so is the difference between a
    /// short answer and a wrong one.
    pub(super) reached_the_ceiling: bool,
}

/// One row: the file, the chunk that matched it best, and where the row sits in the order.
pub(super) struct RankedDocument {
    pub(super) chunk_id: String,
    pub(super) matches: u32,
    pub(super) cursor: DocumentCursor,
}

pub(super) struct DocumentCollector<'a> {
    limit: usize,
    after: Option<DocumentCursor>,
    /// How many distinct files one segment of one query may fold. See `FOLD_CEILING`.
    ceiling: usize,
    file_id_field: &'static str,
    chunk_id_field: &'static str,
    within: Option<&'a PeriodScope>,
}

impl<'a> DocumentCollector<'a> {
    pub(super) fn new(
        limit: usize,
        after: Option<DocumentCursor>,
        ceiling: usize,
        file_id_field: &'static str,
        chunk_id_field: &'static str,
    ) -> Self {
        Self {
            limit,
            after,
            ceiling,
            file_id_field,
            chunk_id_field,
            within: None,
        }
    }

    /// The same collector, folding only what a period admits.
    pub(super) fn within(mut self, scope: Option<&'a PeriodScope>) -> Self {
        self.within = scope;
        self
    }
}

/// What a period admits in one segment, as ordinals of that segment's columns.
struct Admitted {
    files: HashSet<u64>,
    chunks: HashSet<u64>,
}

impl Admitted {
    fn of(
        scope: &PeriodScope,
        file_ids: &StrColumn,
        chunk_ids: &StrColumn,
    ) -> tantivy::Result<Self> {
        Ok(Self {
            files: ordinals(file_ids, scope.whole_files())?,
            chunks: ordinals(chunk_ids, scope.single_chunks())?,
        })
    }

    fn admits(&self, file: u64, chunk: u64) -> bool {
        self.files.contains(&file) || self.chunks.contains(&chunk)
    }
}

/// The ordinals these ids have in one segment's column. An id the segment does not hold has
/// none: nothing of it is in this segment to admit.
fn ordinals<'id>(
    column: &StrColumn,
    ids: impl Iterator<Item = &'id str>,
) -> tantivy::Result<HashSet<u64>> {
    let mut found = HashSet::new();
    for id in ids {
        if let Some(ordinal) = column.dictionary().term_ord(id)? {
            found.insert(ordinal);
        }
    }
    Ok(found)
}

impl Collector for DocumentCollector<'_> {
    type Fruit = RankedPage;
    type Child = DocumentSegmentCollector;

    fn for_segment(
        &self,
        _segment_ordinal: SegmentOrdinal,
        segment: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        // A segment holding documents without these columns was written against another
        // schema. Refusing here is what turns that into an error the caller can report,
        // rather than a page that quietly leaves those documents out.
        let file_ids = column(segment, self.file_id_field)?;
        let chunk_ids = column(segment, self.chunk_id_field)?;
        let admitted = match self.within {
            Some(scope) => Some(Admitted::of(scope, &file_ids, &chunk_ids)?),
            None => None,
        };
        Ok(DocumentSegmentCollector {
            limit: self.limit,
            after: self.after.clone(),
            ceiling: self.ceiling,
            file_ids,
            chunk_ids,
            admitted,
            folded: HashMap::new(),
            reached_the_ceiling: false,
            unreadable: 0,
            scratch: String::new(),
        })
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    /// Rows of different segments are merged by file id, and not appended.
    ///
    /// A file's documents are written by one `publish` and so live in one segment, which is
    /// what makes the per-segment fold the whole fold. Merging by id rather than trusting
    /// that costs one comparison per row and makes the one thing this stage is about — a
    /// file appearing twice on a page — impossible however the segments were written.
    fn merge_fruits(&self, segments: Vec<RankedPage>) -> tantivy::Result<RankedPage> {
        let mut by_file: HashMap<String, RankedDocument> = HashMap::new();
        let mut unreadable = 0;
        let mut reached_the_ceiling = false;
        for segment in segments {
            unreadable += segment.unreadable;
            reached_the_ceiling |= segment.reached_the_ceiling;
            for row in segment.ranked {
                match by_file.entry(row.cursor.file_id().to_string()) {
                    Entry::Occupied(mut held) => held.get_mut().absorb(row),
                    Entry::Vacant(free) => {
                        free.insert(row);
                    }
                }
            }
        }
        let mut ranked: Vec<RankedDocument> = by_file.into_values().collect();
        ranked.sort_by(|left, right| left.cursor.cmp(&right.cursor));
        ranked.truncate(self.limit);
        Ok(RankedPage {
            ranked,
            unreadable,
            reached_the_ceiling,
        })
    }
}

impl RankedDocument {
    /// The same file, seen in another segment: the row keeps the better match and the two
    /// counts add up.
    fn absorb(&mut self, other: Self) {
        self.matches = self.matches.saturating_add(other.matches);
        if other.cursor < self.cursor {
            self.chunk_id = other.chunk_id;
            self.cursor = other.cursor;
        }
    }
}

/// One file while its matches are still arriving, keyed by the ordinals of the segment it
/// lives in. Ordinals rather than ids because a term dictionary is sorted, so comparing them
/// compares the ids they stand for, and the strings are only ever read for the rows that end
/// up on a page.
struct Folding {
    score: DocumentScore,
    /// The best-scoring chunk seen so far: its score, and its ordinal in this segment.
    best: BestChunk,
}

#[derive(Clone, Copy)]
struct BestChunk {
    score: Score,
    chunk: u64,
}

/// One row of this segment, still keyed by ordinals: everything the page needs except the
/// two ids, which are read back only for the rows that survive the cut.
struct Kept {
    score: f32,
    file: u64,
    matches: u32,
    chunk: u64,
}

pub(super) struct DocumentSegmentCollector {
    limit: usize,
    after: Option<DocumentCursor>,
    ceiling: usize,
    file_ids: StrColumn,
    chunk_ids: StrColumn,
    /// What a period admits in this segment, when the query is about one.
    admitted: Option<Admitted>,
    folded: HashMap<u64, Folding>,
    reached_the_ceiling: bool,
    unreadable: usize,
    scratch: String,
}

impl SegmentCollector for DocumentSegmentCollector {
    type Fruit = RankedPage;

    /// Every matching chunk is folded, and none is pruned by its score.
    ///
    /// A page of chunks could stop reading a document the moment its score fell below the
    /// worst kept, because a chunk's score is its whole worth. A row's is not: the tenth
    /// match of a file still moves the score of its row, so the fold has to see all of them.
    /// That is the price of folding, and it is measured in ADR 0038.
    fn collect(&mut self, doc: DocId, score: Score) {
        let Some(file) = self.file_ids.term_ords(doc).next() else {
            self.unreadable += 1;
            return;
        };
        let Some(chunk) = self.chunk_ids.term_ords(doc).next() else {
            self.unreadable += 1;
            return;
        };
        if self
            .admitted
            .as_ref()
            .is_some_and(|admitted| !admitted.admits(file, chunk))
        {
            return;
        }
        let room = self.folded.len() < self.ceiling;
        match self.folded.entry(file) {
            Entry::Occupied(mut held) => held.get_mut().fold(score, chunk),
            Entry::Vacant(free) if room => {
                free.insert(Folding {
                    score: DocumentScore::nothing().and(score),
                    best: BestChunk { score, chunk },
                });
            }
            Entry::Vacant(_) => self.reached_the_ceiling = true,
        }
    }

    /// The rows of this segment that rank after the cursor, best first, at most `limit`.
    ///
    /// The cut happens on ordinals and only the survivors' ids are read back, so a query that
    /// matched ten thousand files pays for ten thousand counters and for `limit` strings.
    fn harvest(self) -> RankedPage {
        let Self {
            limit,
            after,
            file_ids,
            chunk_ids,
            folded,
            reached_the_ceiling,
            mut unreadable,
            mut scratch,
            ..
        } = self;

        // Ranked on ordinals and cut before a single id is read back. A term dictionary is
        // sorted, so the ordinals of one segment are in the order of the ids they stand for,
        // and the row order is the same either way. Reading the ids of every folded file
        // instead costs a string and a dictionary lookup per file: measured on the owner's
        // largest working context, that was the difference between 1.4 ms and 9.8 ms on one
        // query, because a page of five is five ids and a query can fold a thousand files.
        let mut kept: Vec<Kept> = Vec::with_capacity(folded.len());
        for (file, folding) in folded {
            let score = folding.score.value();
            if ranks_before(score, file, after.as_ref(), &file_ids, &mut scratch) {
                continue;
            }
            kept.push(Kept {
                score,
                file,
                matches: folding.score.matches(),
                chunk: folding.best.chunk,
            });
        }
        kept.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.file.cmp(&right.file))
        });
        kept.truncate(limit);

        let mut ranked = Vec::with_capacity(kept.len());
        for row in kept {
            let (Some(file_id), Some(chunk_id)) = (
                text(&file_ids, row.file, &mut scratch),
                text(&chunk_ids, row.chunk, &mut scratch),
            ) else {
                unreadable += 1;
                continue;
            };
            ranked.push(RankedDocument {
                chunk_id,
                matches: row.matches,
                cursor: DocumentCursor::new(row.score, file_id),
            });
        }
        RankedPage {
            ranked,
            unreadable,
            reached_the_ceiling,
        }
    }
}

impl Folding {
    /// One more match of the same file: it moves the score, and it becomes the row's shown
    /// chunk when it is the best one so far. Ties go to the lower ordinal, which is the lower
    /// chunk id, so a row shows the same chunk on every run.
    fn fold(&mut self, score: Score, chunk: u64) {
        self.score = self.score.and(score);
        if score > self.best.score || (score == self.best.score && chunk < self.best.chunk) {
            self.best = BestChunk { score, chunk };
        }
    }
}

/// Whether a row belongs to a page already answered.
///
/// The score alone decides it except on an exact tie, which is the only case that costs a
/// dictionary lookup: ties are broken by the file id, and an ordinal cannot be compared
/// against the id a cursor carries.
fn ranks_before(
    score: f32,
    file: u64,
    after: Option<&DocumentCursor>,
    file_ids: &StrColumn,
    scratch: &mut String,
) -> bool {
    let Some(after) = after else {
        return false;
    };
    if score != after.score() {
        return score > after.score();
    }
    match text(file_ids, file, scratch) {
        Some(file_id) => DocumentCursor::new(score, file_id) <= *after,
        // A file id that cannot be read is not silently placed on one side of the boundary;
        // `harvest` counts it as unreadable when it tries again.
        None => false,
    }
}

fn text(column: &StrColumn, ordinal: u64, scratch: &mut String) -> Option<String> {
    scratch.clear();
    match column.ord_to_str(ordinal, scratch) {
        Ok(true) => Some(scratch.clone()),
        _ => None,
    }
}

fn column(segment: &SegmentReader, name: &'static str) -> tantivy::Result<StrColumn> {
    segment.fast_fields().str(name)?.ok_or_else(|| {
        tantivy::TantivyError::SchemaError(format!("the search index has no {name} column"))
    })
}
