// Plain-text evidence extraction: a stable read, SHA-256 identities and whitespace-aware
// chunking of one already opened file.
use std::fs;
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::application::{ContentSkipReason, EvidenceRead};
use crate::domain::{ChunkScript, ContentChunk, EvidenceCommit, FileObservation, FileVersion};

const CHUNK_BYTES: usize = 4 * 1024;

/// The chunk size a run uses. Production has exactly one, compiled in above. A test build
/// lets a diagnostic vary it, because telling the cost of content apart from the cost of
/// the number of chunks needs the same content cut both ways; no released binary reads
/// this, and an unset variable keeps the production value.
#[cfg(not(test))]
fn chunk_bytes() -> usize {
    CHUNK_BYTES
}

#[cfg(test)]
pub(super) fn chunk_bytes() -> usize {
    std::env::var("CROSSKIT_DIAGNOSTIC_CHUNK_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|bytes| *bytes > 0)
        .unwrap_or(CHUNK_BYTES)
}
const EXTRACTOR_ID: &str = "plain-text";
const EXTRACTOR_VERSION: &str = "1";
const EXTRACTION_METHOD: &str = "plain-text";
static NEXT_OBSERVATION_NONCE: AtomicU64 = AtomicU64::new(0);

/// Reads `file` completely and builds the evidence commit for `file_id`. `before` is the
/// metadata taken right after opening; the read is discarded when size or modification
/// time moved meanwhile, so a half-written file is never indexed.
pub(super) fn read_plain_text_evidence(
    file: &mut fs::File,
    before: &fs::Metadata,
    file_id: &str,
    max_content_bytes: u64,
) -> Result<EvidenceRead, std::io::Error> {
    if before.len() > max_content_bytes {
        return Ok(EvidenceRead::Skipped(ContentSkipReason::TooLarge));
    }

    // The content is never held whole. The first pass takes its fingerprint and decides
    // whether it is text at all; the second cuts chunks through a window. Stage 2S measured
    // that releasing the text after cutting does not lower the peak, because the peak has
    // already happened by then: it has to never exist.
    if reads_whole_file() {
        return read_whole_file_evidence(file, before, file_id);
    }
    let block = read_block_for(before.len());
    let survey = survey_content(file, block)?;
    let after = file.metadata()?;
    if changed_while_reading(before, &after) {
        return Ok(EvidenceRead::Skipped(
            ContentSkipReason::ChangedWhileReading,
        ));
    }
    let survey = match survey {
        Survey::Text(survey) => survey,
        Survey::Skipped(reason) => return Ok(EvidenceRead::Skipped(reason)),
    };
    let created_at_unix_ms = current_unix_ms()?;
    let content_hash = survey.content_hash;
    let version = FileVersion::new(
        format!("{file_id}@sha256:{content_hash}"),
        file_id,
        content_hash.clone(),
        created_at_unix_ms,
    )
    .map_err(domain_error)?;
    let nonce = NEXT_OBSERVATION_NONCE.fetch_add(1, Ordering::Relaxed);
    let observed_at_unix_ns = current_unix_ns()?;
    let observation = FileObservation::new(
        format!("observation:{}:{observed_at_unix_ns}:{nonce}", version.id()),
        version.file_id(),
        version.id(),
        created_at_unix_ms,
    )
    .map_err(domain_error)?;
    let config_hash =
        hash(format!("{EXTRACTOR_ID}:{EXTRACTOR_VERSION}:{}", chunk_bytes()).as_bytes());
    file.seek(std::io::SeekFrom::Start(0))?;
    // A source with no text anywhere is published with no chunks. Its version still lands,
    // so a file whose text was deleted stops being current at the version that had it and
    // loses its documents; it is the empty file's answer, given to the same question.
    let chunks = match survey.holds_text {
        true => chunk_stream(file, block, &version, &config_hash, created_at_unix_ms)?,
        false => Vec::new(),
    };
    // The second pass read the file again, so stability is checked against the same
    // metadata once more: a source that moved between the passes is not published.
    let after = file.metadata()?;
    if changed_while_reading(before, &after) {
        return Ok(EvidenceRead::Skipped(
            ContentSkipReason::ChangedWhileReading,
        ));
    }

    EvidenceCommit::new(version, observation, chunks)
        .map(EvidenceRead::Commit)
        .map_err(domain_error)
}

/// Whether to read the source whole, the way Crosskit did before Stage 2S. Production
/// never does; a test build lets the diagnostic switch paths so both can be measured on one
/// machine at one time, which is the only way to tell a regression from the drift between
/// two series.
#[cfg(not(test))]
fn reads_whole_file() -> bool {
    false
}

#[cfg(test)]
fn reads_whole_file() -> bool {
    std::env::var("CROSSKIT_DIAGNOSTIC_WHOLE_FILE_READ").is_ok_and(|value| value == "1")
}

/// The pre-2S read, kept only as the other side of that comparison.
#[cfg(test)]
fn read_whole_file_evidence(
    file: &mut fs::File,
    before: &fs::Metadata,
    file_id: &str,
) -> Result<EvidenceRead, std::io::Error> {
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or_default());
    file.read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if changed_while_reading(before, &after) {
        return Ok(EvidenceRead::Skipped(
            ContentSkipReason::ChangedWhileReading,
        ));
    }
    if bytes.contains(&0) {
        return Ok(EvidenceRead::Skipped(ContentSkipReason::Binary));
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(EvidenceRead::Skipped(ContentSkipReason::NotUtf8));
    };
    let created_at_unix_ms = current_unix_ms()?;
    let content_hash = hash(text.as_bytes());
    let version = FileVersion::new(
        format!("{file_id}@sha256:{content_hash}"),
        file_id,
        content_hash.clone(),
        created_at_unix_ms,
    )
    .map_err(domain_error)?;
    let nonce = NEXT_OBSERVATION_NONCE.fetch_add(1, Ordering::Relaxed);
    let observed_at_unix_ns = current_unix_ns()?;
    let observation = FileObservation::new(
        format!("observation:{}:{observed_at_unix_ns}:{nonce}", version.id()),
        version.file_id(),
        version.id(),
        created_at_unix_ms,
    )
    .map_err(domain_error)?;
    let config_hash =
        hash(format!("{EXTRACTOR_ID}:{EXTRACTOR_VERSION}:{}", chunk_bytes()).as_bytes());
    let mut chunks = Vec::new();
    let mut window = match text.chars().any(|character| !character.is_whitespace()) {
        true => text,
        false => String::new(),
    };
    let mut start_byte = 0;
    while !window.is_empty() {
        cut_one(
            &mut chunks,
            &mut window,
            &mut start_byte,
            &version,
            &config_hash,
            created_at_unix_ms,
        )?;
    }

    EvidenceCommit::new(version, observation, chunks)
        .map(EvidenceRead::Commit)
        .map_err(domain_error)
}

#[cfg(not(test))]
fn read_whole_file_evidence(
    _file: &mut fs::File,
    _before: &fs::Metadata,
    _file_id: &str,
) -> Result<EvidenceRead, std::io::Error> {
    unreachable!("production never reads the whole file")
}

/// The largest read one call takes. It bounds the window, not the content.
const READ_BLOCK: usize = 256 * 1024;

/// A block never larger than the file it reads. A corpus of small sources is the common
/// case, and giving each of them a full-sized block would cost more than the source: Stage
/// 2S measured the scale corpus getting heavier until the block followed the file size.
fn read_block_for(length: u64) -> usize {
    let length = usize::try_from(length).unwrap_or(READ_BLOCK);
    length.clamp(1, READ_BLOCK)
}

/// What the first pass learned about the content.
struct TextSurvey {
    content_hash: String,
    /// Whether anything in the content is not whitespace. Decided here because this pass
    /// already reads every byte, and deciding it later would mean reading them again.
    holds_text: bool,
}

enum Survey {
    Text(TextSurvey),
    Skipped(ContentSkipReason),
}

/// Reads the whole file once without keeping it: the fingerprint is accumulated, and the
/// checks that decide whether this is text at all are made block by block. A character
/// split across two blocks is carried over rather than rejected.
fn survey_content(file: &mut fs::File, block_size: usize) -> Result<Survey, std::io::Error> {
    let mut hasher = Sha256::new();
    let mut block = vec![0u8; block_size];
    let mut carried: Vec<u8> = Vec::new();
    let mut holds_text = false;

    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        let read_block = &block[..read];
        hasher.update(read_block);
        if read_block.contains(&0) {
            return Ok(Survey::Skipped(ContentSkipReason::Binary));
        }
        carried.extend_from_slice(read_block);
        // An incomplete character at the end of a block is the next block's beginning.
        let valid = match std::str::from_utf8(&carried) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => return Ok(Survey::Skipped(ContentSkipReason::NotUtf8)),
        };
        if !holds_text {
            holds_text = std::str::from_utf8(&carried[..valid])
                .is_ok_and(|text| text.chars().any(|character| !character.is_whitespace()));
        }
        carried.drain(..valid);
    }
    if !carried.is_empty() {
        return Ok(Survey::Skipped(ContentSkipReason::NotUtf8));
    }

    Ok(Survey::Text(TextSurvey {
        content_hash: format!("{:x}", hasher.finalize()),
        holds_text,
    }))
}

/// Cuts the same chunks the whole-file read cut, through a window that never holds more
/// than two chunks plus one block. Boundaries depend only on the bytes of the chunk being
/// cut, so a window of that size reproduces them exactly.
fn chunk_stream(
    file: &mut fs::File,
    block_size: usize,
    version: &FileVersion,
    config_hash: &str,
    created_at_unix_ms: i64,
) -> Result<Vec<ContentChunk>, std::io::Error> {
    let mut chunks = Vec::new();
    let mut window = String::new();
    let mut carried: Vec<u8> = Vec::new();
    let mut block = vec![0u8; block_size];
    let mut start_byte: usize = 0;

    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        carried.extend_from_slice(&block[..read]);
        let valid = match std::str::from_utf8(&carried) {
            Ok(text) => text.len(),
            Err(error) => error.valid_up_to(),
        };
        window.push_str(std::str::from_utf8(&carried[..valid]).map_err(|_| {
            std::io::Error::other("the content stopped being valid UTF-8 mid-read")
        })?);
        carried.drain(..valid);

        // Only cut while the window is long enough that the boundary cannot be the end of
        // the file, which is the one case decided by content that has not been read yet.
        while window.len() >= chunk_bytes() * 2 {
            cut_one(
                &mut chunks,
                &mut window,
                &mut start_byte,
                version,
                config_hash,
                created_at_unix_ms,
            )?;
        }
    }
    if !carried.is_empty() {
        return Err(std::io::Error::other(
            "the content ended with an incomplete character",
        ));
    }
    while !window.is_empty() {
        cut_one(
            &mut chunks,
            &mut window,
            &mut start_byte,
            version,
            config_hash,
            created_at_unix_ms,
        )?;
    }

    Ok(chunks)
}

#[allow(clippy::too_many_arguments)]
fn cut_one(
    chunks: &mut Vec<ContentChunk>,
    window: &mut String,
    start_byte: &mut usize,
    version: &FileVersion,
    config_hash: &str,
    created_at_unix_ms: i64,
) -> Result<(), std::io::Error> {
    let boundary = next_chunk_boundary(window);
    // The one place Crosskit cuts somebody else's text at an index it computed. Every other
    // refusal in this program is typed, and a boundary that is somehow not a character
    // boundary is refused the same way rather than ending the process on a user's file.
    let chunk_text = window.get(..boundary).ok_or_else(|| {
        std::io::Error::other("the content could not be cut on a character boundary")
    })?;
    let text_hash = hash(chunk_text.as_bytes());
    let ordinal = u32::try_from(chunks.len())
        .map_err(|_| std::io::Error::other("too many chunks for one source revision"))?;
    let start = u64::try_from(*start_byte)
        .map_err(|_| std::io::Error::other("chunk start offset exceeds u64"))?;
    let end = u64::try_from(*start_byte + boundary)
        .map_err(|_| std::io::Error::other("chunk end offset exceeds u64"))?;
    let chunk = ContentChunk::new(
        format!(
            "{}:{EXTRACTOR_ID}:{EXTRACTOR_VERSION}:{ordinal}:sha256:{text_hash}",
            version.id()
        ),
        version.id(),
        ordinal,
        start,
        end,
        text_hash,
        chunk_text,
        ChunkScript::of(chunk_text),
        EXTRACTION_METHOD,
        EXTRACTOR_ID,
        EXTRACTOR_VERSION,
        config_hash,
        version.content_hash(),
        created_at_unix_ms,
    )
    .map_err(domain_error)?;
    chunks.push(chunk);
    window.drain(..boundary);
    *start_byte += boundary;
    Ok(())
}

fn changed_while_reading(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.len() != after.len() || before.modified().ok() != after.modified().ok()
}

/// Where the first chunk of `text` ends: after the last whitespace inside it, so a word is
/// not split across two chunks, or at the chunk's full size when it holds none. The window
/// is drained as chunks are cut, so the chunk being cut always starts at its beginning.
///
/// Every index this returns is the end of a character. A search for whitespace answers with
/// the *first* byte of the character it found, and the chunk has to end after its last one:
/// `U+200A` is three bytes wide, and adding 1 instead of the character's width lands inside
/// it. Multi-byte spaces are ordinary in Russian text — `U+00A0` alone is in most documents
/// that were ever pasted out of a word processor.
fn next_chunk_boundary(text: &str) -> usize {
    let target = chunk_bytes().min(text.len());
    if target == text.len() {
        return target;
    }

    let mut boundary = target;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }

    text[..boundary]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        // Whitespace at the very start would cut a chunk of one character; the chunk takes
        // its full width instead.
        .filter(|(relative, _)| *relative > 0)
        .map(|(relative, character)| relative + character.len_utf8())
        .unwrap_or(boundary)
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn current_unix_ms() -> Result<i64, std::io::Error> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(std::io::Error::other)?;
    i64::try_from(duration.as_millis())
        .map_err(|_| std::io::Error::other("current timestamp exceeds i64"))
}

fn current_unix_ns() -> Result<u128, std::io::Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(std::io::Error::other)
}

fn domain_error(error: crate::domain::EvidenceError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}
