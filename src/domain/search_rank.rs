// Where one row of the results sits in the order they are returned in, which is also what a
// page's cursor is.
//
// A row is one document — one file — and not one match inside it, so the order is: the score
// the document folded out of its matches, best first; documents that score the same, by file
// id ascending. The second half is not decoration. Without it two documents with equal scores
// have no defined order, and then a page boundary can fall between them differently on each
// run, so the same query would return one of them twice and the other never.
//
// Both halves are properties of the row itself — a score and a content-derived id — and
// neither is a position in a list. That is what makes paging a keyset: page two is "what
// ranks after this document", not "skip ten", so it costs the same whether it is page two or
// page two hundred, and asking for it twice gives the same answer.
//
// What it does not survive is the index changing underneath it. The score of a document
// depends on what else the index holds, so a cursor taken before a scan and used after it is
// answered against the new index, and a row can shift across a page boundary. This is a
// search cursor, not a snapshot.
use std::cmp::Ordering;
use std::fmt;

/// What the encoded form starts with, so a cursor that addresses something else is refused
/// rather than read as this one.
///
/// Until Stage 33 a page was a page of chunks and the cursor named a chunk. Both forms are
/// "some bits, a colon, an id", so a chunk cursor handed to this build would decode without
/// complaint and start the answer at a place nobody asked for. The tag is what turns that
/// into the refusal ADR 0013 requires.
const TAG: &str = "document";

/// One row's place in the order, and the cursor a caller passes to continue after it.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentCursor {
    score: f32,
    file_id: String,
}

impl DocumentCursor {
    pub fn new(score: f32, file_id: impl Into<String>) -> Self {
        Self {
            score,
            file_id: file_id.into(),
        }
    }

    pub fn score(&self) -> f32 {
        self.score
    }

    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    /// The cursor as a caller passes it back: the tag, the score's bits in hexadecimal, then
    /// the file id.
    ///
    /// The bits rather than a rounded decimal, because a cursor that does not compare exactly
    /// equal to the score it came from moves a page boundary. The separator is a colon and a
    /// file id holds colons of its own, which is why only the first two are separators.
    pub fn encode(&self) -> String {
        format!("{TAG}:{:08x}:{}", self.score.to_bits(), self.file_id)
    }

    /// A cursor this program produced, or nothing.
    ///
    /// Nothing is not an error here — the caller decides what an unreadable cursor means —
    /// but it is never repaired into a cursor that would silently start somewhere else.
    pub fn decode(value: &str) -> Option<Self> {
        let rest = value.strip_prefix(TAG)?.strip_prefix(':')?;
        let (bits, file_id) = rest.split_once(':')?;
        if file_id.is_empty() {
            return None;
        }
        let score = f32::from_bits(u32::from_str_radix(bits, 16).ok()?);
        if !score.is_finite() {
            return None;
        }
        Some(Self::new(score, file_id))
    }
}

/// The ranking order itself: `Less` means "comes first in the results".
impl Ord for DocumentCursor {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| self.file_id.cmp(&other.file_id))
    }
}

impl PartialOrd for DocumentCursor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Equality has to agree with the order, and the order compares scores by their bits, so two
/// cursors are equal exactly when `cmp` says so.
impl Eq for DocumentCursor {}

impl fmt::Display for DocumentCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::DocumentCursor;

    #[test]
    fn a_higher_score_comes_first_and_an_equal_score_is_broken_by_file_id() {
        let mut rows = [
            DocumentCursor::new(1.5, "file:b"),
            DocumentCursor::new(2.5, "file:z"),
            DocumentCursor::new(1.5, "file:a"),
        ];
        rows.sort();
        assert_eq!(
            rows.iter().map(DocumentCursor::file_id).collect::<Vec<_>>(),
            vec!["file:z", "file:a", "file:b"]
        );
    }

    /// The page after a cursor is everything that ranks strictly after it, which is what
    /// makes a repeated page identical and a boundary row appear exactly once.
    #[test]
    fn a_cursor_divides_the_order_in_two() {
        let cursor = DocumentCursor::new(1.5, "file:b");
        assert!(
            DocumentCursor::new(1.5, "file:c") > cursor,
            "same score, later id"
        );
        assert!(
            DocumentCursor::new(0.5, "file:a") > cursor,
            "lower score wins over id"
        );
        assert!(!(cursor > cursor), "the cursor itself is not after itself");
        assert!(
            !(DocumentCursor::new(1.5, "file:a") > cursor),
            "same score, earlier id"
        );
        assert!(
            !(DocumentCursor::new(2.5, "file:z") > cursor),
            "higher score"
        );
    }

    #[test]
    fn a_cursor_survives_being_written_out_and_read_back() {
        for cursor in [
            DocumentCursor::new(1.5, "file:unix:1:2@sha256:abc"),
            DocumentCursor::new(0.0, "file:a"),
            DocumentCursor::new(f32::MIN_POSITIVE, "file:a"),
        ] {
            let text = cursor.encode();
            assert_eq!(
                DocumentCursor::decode(&text).as_ref(),
                Some(&cursor),
                "{text}"
            );
        }
    }

    #[test]
    fn a_cursor_this_program_did_not_write_is_not_repaired() {
        for text in [
            "",
            "nonsense",
            "document",
            "document:3f800000",
            "document:3f800000:",
            "document::file:a",
            "document:zzzz:file:a",
            // Infinity and NaN bits: a score no query produces, and one that would make
            // every comparison against it false.
            "document:7f800000:file:a",
            "document:7fc00000:file:a",
            // The form this program used while a page was a page of chunks. It decodes
            // under the old rules and names a chunk, so answering it as a document cursor
            // would start the page somewhere nobody asked for.
            "3f800000:chunk:file:unix:1:2@sha256:abc:plain-text:1:0:sha256:def",
        ] {
            assert_eq!(DocumentCursor::decode(text), None, "{text:?}");
        }
    }
}
