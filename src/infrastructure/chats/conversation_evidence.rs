// One transcript read as the evidence of one revision: a conversation, its replies as chunks,
// and the files it touched.
//
// The chunk is a reply and not a slice of four kilobytes. That is the whole point of the
// profile and not a tidier cut: a reply is the unit a person said something in, so "this term
// was used in three replies" is a fact the graph can hold, and "this term was pasted four
// hundred times inside one log" stops outscoring it. Stage 31 measured that rule and could
// only apply it in a measurement, because nothing in the index had a reply boundary to count
// against; Stage 33 cannot begin until one exists.
//
// A reply longer than the cut is cut, and the rule is named rather than implied: pieces are
// taken on character boundaries, in order, and every piece carries the ordinal of its place
// among all pieces of the revision. A reply of ordinary length is one chunk, which is the
// case that matters.
use std::fs;
use std::io::{BufReader, Seek};

use sha2::{Digest, Sha256};

use super::transcript_reader::read_transcript_from;
use crate::application::conversation_assembly::ConversationAssembly;
use crate::application::{ContentSkipReason, EvidenceRead};
use crate::domain::{
    ChunkScript, ContentChunk, EvidenceCommit, ExtractionProfile, FileObservation, FileVersion,
    MOST_BYTES_PER_REPLY_CHUNK,
};

/// How much of one reply goes into one chunk.
///
/// A ceiling on the exception rather than a size for the normal case: the median reply in the
/// owner's corpus is under a kilobyte, and this exists so that a pasted dump inside a reply
/// cannot make one chunk of a megabyte. Measured against the plain-text profile's 4 KiB, it
/// is deliberately larger — a reply is meant to arrive whole. The number lives in the domain,
/// because reading speech back needs it too: a chunk that full was cut, not ended.
const MOST_BYTES_PER_CHUNK: usize = MOST_BYTES_PER_REPLY_CHUNK;

/// What the extractor decided about a file whose name said "transcript".
pub(crate) enum TranscriptOutcome {
    /// The file was a transcript and this is what it held. Boxed because an `EvidenceRead`
    /// carries a whole commit and the other variant carries nothing: unboxed, every return of
    /// `NotATranscript` would move a commit's worth of stack.
    Read(Box<EvidenceRead>),
    /// The name said transcript and the content did not. No line carried a session and
    /// nothing was said, so this is not a conversation; the caller reads it as plain text.
    ///
    /// This is the parser reporting that it could not parse, which is a fact. It is not a
    /// sniff: `.jsonl` is a container format and a file of measurements in it is an ordinary
    /// document that must keep being indexed as one.
    NotATranscript,
}

/// Reads an already opened transcript into the evidence of one revision.
///
/// `before` is the metadata taken right after opening; the read is discarded when size or
/// modification time moved meanwhile, exactly as the plain-text profile discards it, so a
/// transcript being appended to right now is never published half-read.
///
/// **The limit is on content, and for a transcript the content is the conversation.** The
/// number is not changed and the rule is not relaxed: what changes is what it is measured
/// against. A transcript is a container — Stage 31 measured the talking at 2.46% of its bytes
/// — so comparing the file against a content limit compares the limit with the tool output the
/// conversation was wrapped in. The owner's largest conversation is 58.5 MiB of transcript and
/// 1.5 MB of talking; under the old rule it was invisible to search entirely, and no amount of
/// ranking could reach what was never indexed.
///
/// Memory is bounded by that same number, and bounded as the conversation is read rather than
/// checked once it is whole: the file is taken a line at a time, and the assembly stops the
/// read the moment the speech reaches the limit.
pub(crate) fn read_conversation_evidence(
    file: &mut fs::File,
    before: &fs::Metadata,
    file_id: &str,
    fallback_session: &str,
    max_content_bytes: u64,
) -> Result<TranscriptOutcome, std::io::Error> {
    // The budget is handed to the assembly, not applied to its result: reading stops when the
    // speech reaches the limit, so what is held never exceeds it. Weighing a finished
    // conversation would be a verdict delivered after the cost — the mistake Stage 2S paid for
    // on the other extractor and wrote down in `plain_text_evidence`.
    let mut assembly = ConversationAssembly::for_session(fallback_session, max_content_bytes);
    file.seek(std::io::SeekFrom::Start(0))?;
    read_transcript_from(BufReader::new(&mut *file), |line| assembly.absorb(line))?;
    let read = assembly.finish();

    if read.outgrew_the_budget() {
        // Refused before the second pass: a transcript that is not going to be published has
        // no reason to be hashed, and the conversation in hand is a prefix rather than a
        // shorter conversation.
        return Ok(TranscriptOutcome::Read(Box::new(EvidenceRead::Skipped(
            ContentSkipReason::TooLarge,
        ))));
    }

    let mut hasher = Sha256::new();

    // The hash is of the file as it lies on disk, not of what was extracted from it: the
    // revision is the file, and two profiles reading the same bytes must agree on which
    // revision they are talking about.
    file.seek(std::io::SeekFrom::Start(0))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut read_bytes = 0u64;
    loop {
        let taken = std::io::Read::read(file, &mut buffer)?;
        if taken == 0 {
            break;
        }
        read_bytes += taken as u64;
        hasher.update(&buffer[..taken]);
    }
    let after = file.metadata()?;
    if changed_while_reading(before, &after) || read_bytes != before.len() {
        return Ok(TranscriptOutcome::Read(Box::new(EvidenceRead::Skipped(
            ContentSkipReason::ChangedWhileReading,
        ))));
    }

    // A name or a reply is what makes this a conversation. A `.jsonl` of measurements has
    // neither, and it goes back to the text profile rather than being published as an empty
    // conversation that answers nothing and hides its own content.
    let conversation = read.into_conversation();
    if conversation.replies().is_empty() && conversation.name().is_none() {
        return Ok(TranscriptOutcome::NotATranscript);
    }

    let content_hash = format!("{:x}", hasher.finalize());
    let created_at_unix_ms = current_unix_ms()?;
    let version = FileVersion::new(
        format!("{file_id}@sha256:{content_hash}"),
        file_id,
        content_hash.clone(),
        created_at_unix_ms,
    )
    .map_err(domain_error)?;
    let observation = FileObservation::new(
        format!("observation:{}:{created_at_unix_ms}", version.id()),
        version.file_id(),
        version.id(),
        created_at_unix_ms,
    )
    .map_err(domain_error)?;

    let chunks = chunks_of(&conversation, &version, created_at_unix_ms)?;
    EvidenceCommit::of_source(version, observation, chunks, Some(conversation))
        .map(|commit| TranscriptOutcome::Read(Box::new(EvidenceRead::Commit(commit))))
        .map_err(domain_error)
}

/// The chunks of one conversation: its name first, then every reply in the order it was said.
///
/// **The name is chunk zero, and it is there so a question can reach a conversation by what it
/// is called.** Stage 31 measured the name at 0.80 strict precision against 0.00 for the body
/// alone; one line per conversation against tens of thousands of reply chunks is what that
/// costs.
fn chunks_of(
    conversation: &crate::domain::conversation::Conversation,
    version: &FileVersion,
    created_at_unix_ms: i64,
) -> Result<Vec<ContentChunk>, std::io::Error> {
    let profile = ExtractionProfile::Conversation;
    let config_hash = hash(
        format!(
            "{}:{}:{MOST_BYTES_PER_CHUNK}",
            profile.extractor_id(),
            profile.extractor_version()
        )
        .as_bytes(),
    );
    let mut chunks = Vec::with_capacity(conversation.replies().len() + 1);
    let mut ordinal = 0u32;

    if let Some(name) = conversation.name() {
        push_chunk(
            &mut chunks,
            &mut ordinal,
            version,
            &config_hash,
            created_at_unix_ms,
            name.value(),
        )?;
    }
    for reply in conversation.replies() {
        for piece in pieces_of(reply.text()) {
            push_chunk(
                &mut chunks,
                &mut ordinal,
                version,
                &config_hash,
                created_at_unix_ms,
                piece,
            )?;
        }
    }
    Ok(chunks)
}

#[allow(clippy::too_many_arguments)]
fn push_chunk(
    chunks: &mut Vec<ContentChunk>,
    ordinal: &mut u32,
    version: &FileVersion,
    config_hash: &str,
    created_at_unix_ms: i64,
    text: &str,
) -> Result<(), std::io::Error> {
    let profile = ExtractionProfile::Conversation;
    let text_hash = hash(text.as_bytes());
    let chunk = ContentChunk::new(
        format!(
            "{}:{}/{}:{}:{text_hash}",
            version.id(),
            profile.extractor_id(),
            profile.extractor_version(),
            ordinal
        ),
        version.id(),
        *ordinal,
        // A reply is extracted out of JSON, so it is not a contiguous run of the file's
        // bytes and no honest byte range exists for it. Section 4.5 of the architecture
        // provides for exactly this: a format that gives no stable offsets stores a format
        // pointer and the hash of what was extracted. The pointer here is the ordinal, which
        // is in the id and in the column; the range is zero and claims nothing.
        0,
        0,
        text_hash,
        text,
        ChunkScript::of(text),
        profile.extraction_method(),
        profile.extractor_id(),
        profile.extractor_version(),
        config_hash,
        version.content_hash(),
        created_at_unix_ms,
    )
    .map_err(domain_error)?;
    chunks.push(chunk);
    *ordinal += 1;
    Ok(())
}

/// One reply as the pieces it goes into the graph as: itself, unless it is longer than the
/// cut, and then consecutive pieces of at most that many bytes, each ending on a character
/// boundary.
fn pieces_of(text: &str) -> Vec<&str> {
    if text.len() <= MOST_BYTES_PER_CHUNK {
        return vec![text];
    }
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + MOST_BYTES_PER_CHUNK).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        // A single character wider than the cut would leave `end == start` and loop for ever.
        // It cannot happen with a four-byte ceiling of 16 KiB, and the guard says so rather
        // than trusting it.
        if end == start {
            end = text.len();
        }
        pieces.push(&text[start..end]);
        start = end;
    }
    pieces
}

fn changed_while_reading(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.len() != after.len() || before.modified().ok() != after.modified().ok()
}

fn hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn current_unix_ms() -> Result<i64, std::io::Error> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| std::io::Error::other("system clock is before the epoch"))?;
    i64::try_from(now.as_millis())
        .map_err(|_| std::io::Error::other("system clock is out of range"))
}

fn domain_error(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_that_fits_is_one_piece() {
        assert_eq!(pieces_of("что там?"), vec!["что там?"]);
        assert_eq!(pieces_of(""), vec![""]);
    }

    /// The cut is by bytes and lands on character boundaries: a Russian letter is two bytes,
    /// and a cut inside one would not be text at all.
    #[test]
    fn a_reply_longer_than_the_cut_is_cut_on_character_boundaries() {
        let long = "я".repeat(MOST_BYTES_PER_CHUNK);
        let pieces = pieces_of(&long);
        assert!(pieces.len() > 1, "реплика должна была разрезаться");
        assert_eq!(pieces.concat(), long, "склейка обязана дать исходный текст");
        for piece in &pieces {
            assert!(piece.len() <= MOST_BYTES_PER_CHUNK);
        }
    }
}
