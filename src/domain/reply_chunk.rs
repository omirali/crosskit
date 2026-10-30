// When the reply a chunk of a conversation was cut from was said.
//
// The graph holds both halves and no edge between them. A conversation's chunks are its name
// (when it has one) and then every reply in the order it was said, a reply longer than one
// chunk taking several in a row; its messages are the same replies, numbered in the same
// order, each with its moment. What is not stored is how many chunks each reply took — and
// the schema is not changed by this stage to store it.
//
// It does not have to be. A reply takes at least one chunk, so if a conversation has `extra`
// more reply chunks than replies, the reply behind chunk `j` is one of `j - extra ..= j`.
// When no reply was cut — the ordinary case: a chunk holds 16 KiB and a reply rarely does —
// `extra` is zero and the answer is exact. When some were, the answer is the span of moments
// of every reply it could be, which is honest: a chunk is placed in a period only when every
// reply it could have come from was said inside it.

use std::ops::RangeInclusive;

/// What the graph can say about one chunk of a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkMoment {
    /// The conversation's own name. It was not said at a moment; it names everything that was.
    Name,
    /// Cut from a reply said between these two moments, inclusive. One moment when the reply
    /// is known exactly.
    Said { earliest: i64, latest: i64 },
    /// Nothing can be said, and a period never includes it. Why is kept, because a period
    /// that leaves a chunk out has to say so and say what stopped it.
    NotKnown(Unplaceable),
}

/// Why a chunk could not be placed in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unplaceable {
    /// A reply it could have come from has no recorded moment.
    ReplyWithoutMoment,
    /// The chunks and the replies do not line up as one conversation: fewer chunks than
    /// replies, or an ordinal missing from the middle.
    CountsDoNotFit,
}

/// The most bytes one chunk of a reply holds. A longer reply goes into the graph as consecutive
/// pieces of at most this many bytes, each ending on a character boundary.
pub const MOST_BYTES_PER_REPLY_CHUNK: usize = 16 * 1024;
/// How far short of the most a cut can end: a piece backs off to the start of a character, and
/// no character is wider than four bytes.
const WIDEST_CHARACTER_BYTES: usize = 4;

/// Which replies a chunk of a conversation could have been cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkSource {
    /// The conversation's own name.
    Name,
    /// One of the replies at these positions, in the order they were said.
    Replies(RangeInclusive<usize>),
    /// The chunks and the replies are not of one conversation: fewer chunks than replies.
    DoNotFit,
}

/// The replies behind every chunk of one conversation, by ordinal: the one rule both the
/// moment of a chunk and its speaker are read by.
///
/// `named` is whether chunk zero is the name; `replies` is how many replies the conversation
/// has.
pub fn replies_behind(named: bool, chunk_count: usize, replies: usize) -> Vec<ChunkSource> {
    let name_chunks = usize::from(named && chunk_count > 0);
    let reply_chunks = chunk_count - name_chunks;
    let mut sources = Vec::with_capacity(chunk_count);
    if name_chunks == 1 {
        sources.push(ChunkSource::Name);
    }
    // Fewer chunks than replies means the two halves are not of the same conversation, and
    // no chunk can be placed.
    let Some(extra) = reply_chunks.checked_sub(replies) else {
        sources.resize(chunk_count, ChunkSource::DoNotFit);
        return sources;
    };
    for chunk in 0..reply_chunks {
        let first = chunk.saturating_sub(extra);
        let last = chunk.min(replies.saturating_sub(1));
        sources.push(ChunkSource::Replies(first..=last));
    }
    sources
}

/// The one reply behind every chunk, read from the chunks' lengths, or nothing when the lengths
/// do not add up to the replies.
///
/// Every piece of a cut reply but its last is within a character of the most a chunk holds,
/// and a reply that was not cut is shorter than that almost always. So a chunk that full
/// continues into the next one, and any other ends its reply. When counting replies that way
/// gives exactly as many as the conversation has, every chunk's reply is known — where
/// `replies_behind` can only give a span. When it does not — a reply happened to end exactly
/// at the size of a cut — nothing is claimed, and the span is what there is.
pub fn exact_replies_behind(
    named: bool,
    chunk_lengths: &[usize],
    replies: usize,
) -> Option<Vec<ChunkSource>> {
    let name_chunks = usize::from(named && !chunk_lengths.is_empty());
    let mut sources = Vec::with_capacity(chunk_lengths.len());
    if name_chunks == 1 {
        sources.push(ChunkSource::Name);
    }
    let mut reply = 0;
    for length in &chunk_lengths[name_chunks..] {
        if reply >= replies {
            return None;
        }
        sources.push(ChunkSource::Replies(reply..=reply));
        let continues = *length > MOST_BYTES_PER_REPLY_CHUNK - WIDEST_CHARACTER_BYTES;
        if !continues {
            reply += 1;
        }
    }
    (reply == replies).then_some(sources)
}

/// The moment of every chunk of one conversation, by ordinal.
///
/// `named` is whether chunk zero is the name; `reply_moments` is each reply's moment by its
/// ordinal, `None` where the transcript recorded none.
pub fn chunk_moments(
    named: bool,
    chunk_count: usize,
    reply_moments: &[Option<i64>],
) -> Vec<ChunkMoment> {
    replies_behind(named, chunk_count, reply_moments.len())
        .into_iter()
        .map(|source| match source {
            ChunkSource::Name => ChunkMoment::Name,
            ChunkSource::Replies(replies) => {
                span_of(reply_moments.get(replies).unwrap_or_default())
            }
            ChunkSource::DoNotFit => ChunkMoment::NotKnown(Unplaceable::CountsDoNotFit),
        })
        .collect()
}

fn span_of(candidates: &[Option<i64>]) -> ChunkMoment {
    let mut earliest = i64::MAX;
    let mut latest = i64::MIN;
    for candidate in candidates {
        let Some(at) = candidate else {
            return ChunkMoment::NotKnown(Unplaceable::ReplyWithoutMoment);
        };
        earliest = earliest.min(*at);
        latest = latest.max(*at);
    }
    if candidates.is_empty() {
        return ChunkMoment::NotKnown(Unplaceable::CountsDoNotFit);
    }
    ChunkMoment::Said { earliest, latest }
}

#[cfg(test)]
mod tests {
    use super::{ChunkMoment, Unplaceable, chunk_moments};

    fn said(at: i64) -> ChunkMoment {
        ChunkMoment::Said {
            earliest: at,
            latest: at,
        }
    }

    /// No reply was cut: every chunk is its reply, exactly.
    #[test]
    fn when_no_reply_was_cut_every_chunk_is_its_own_reply() {
        assert_eq!(
            chunk_moments(true, 4, &[Some(10), Some(20), Some(30)]),
            vec![ChunkMoment::Name, said(10), said(20), said(30)]
        );
        assert_eq!(
            chunk_moments(false, 2, &[Some(10), Some(20)]),
            vec![said(10), said(20)]
        );
    }

    /// One reply took two chunks: the chunks around the cut are placed by the span of the
    /// replies they could be.
    #[test]
    fn a_cut_reply_widens_the_span_of_the_chunks_it_could_be() {
        // Replies at 10, 20, 30; four reply chunks, so one reply was cut in two.
        assert_eq!(
            chunk_moments(false, 4, &[Some(10), Some(20), Some(30)]),
            vec![
                said(10),
                ChunkMoment::Said {
                    earliest: 10,
                    latest: 20
                },
                ChunkMoment::Said {
                    earliest: 20,
                    latest: 30
                },
                said(30),
            ]
        );
    }

    /// A reply with no moment makes every chunk that could be it unplaceable.
    #[test]
    fn a_reply_without_a_moment_places_nothing_that_could_be_it() {
        assert_eq!(
            chunk_moments(false, 3, &[Some(10), None, Some(30)]),
            vec![
                said(10),
                ChunkMoment::NotKnown(Unplaceable::ReplyWithoutMoment),
                said(30)
            ]
        );
    }

    /// Counts that cannot belong to one conversation place nothing.
    #[test]
    fn fewer_chunks_than_replies_places_nothing() {
        assert_eq!(
            chunk_moments(true, 2, &[Some(10), Some(20)]),
            vec![
                ChunkMoment::Name,
                ChunkMoment::NotKnown(Unplaceable::CountsDoNotFit)
            ]
        );
        assert_eq!(chunk_moments(true, 1, &[]), vec![ChunkMoment::Name]);
        assert_eq!(chunk_moments(false, 0, &[]), vec![]);
    }

    /// A reply cut in two is told apart from two replies by the length of its first piece.
    #[test]
    fn a_full_chunk_continues_its_reply_and_any_other_ends_it() {
        use super::{ChunkSource, MOST_BYTES_PER_REPLY_CHUNK, exact_replies_behind};
        let full = MOST_BYTES_PER_REPLY_CHUNK - 2;
        assert_eq!(
            exact_replies_behind(true, &[12, 40, full, 900, 30], 3),
            Some(vec![
                ChunkSource::Name,
                ChunkSource::Replies(0..=0),
                ChunkSource::Replies(1..=1),
                ChunkSource::Replies(1..=1),
                ChunkSource::Replies(2..=2),
            ])
        );
        // Counted that way there are two replies, and the conversation has three.
        assert_eq!(exact_replies_behind(false, &[full, 900, 30], 3), None);
    }
}
