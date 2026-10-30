use std::collections::HashSet;
use std::fmt;

use super::chunk_script::ChunkScript;
use super::conversation::Conversation;
use super::insight_job::InsightJob;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    id: String,
    file_id: String,
    content_hash: String,
    created_at_unix_ms: i64,
}

impl FileVersion {
    pub fn new(
        id: impl Into<String>,
        file_id: impl Into<String>,
        content_hash: impl Into<String>,
        created_at_unix_ms: i64,
    ) -> Result<Self, EvidenceError> {
        let id = required("file version id", id.into())?;
        let file_id = required("file id", file_id.into())?;
        let content_hash = required("content hash", content_hash.into())?;

        Ok(Self {
            id,
            file_id,
            content_hash,
            created_at_unix_ms,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    pub fn created_at_unix_ms(&self) -> i64 {
        self.created_at_unix_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileObservation {
    id: String,
    file_id: String,
    file_version_id: String,
    observed_at_unix_ms: i64,
}

impl FileObservation {
    pub fn new(
        id: impl Into<String>,
        file_id: impl Into<String>,
        file_version_id: impl Into<String>,
        observed_at_unix_ms: i64,
    ) -> Result<Self, EvidenceError> {
        Ok(Self {
            id: required("file observation id", id.into())?,
            file_id: required("file observation file id", file_id.into())?,
            file_version_id: required("file observation version id", file_version_id.into())?,
            observed_at_unix_ms,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn file_id(&self) -> &str {
        &self.file_id
    }
    pub fn file_version_id(&self) -> &str {
        &self.file_version_id
    }
    pub fn observed_at_unix_ms(&self) -> i64 {
        self.observed_at_unix_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentChunk {
    id: String,
    source_revision_id: String,
    ordinal: u32,
    start_byte: u64,
    end_byte: u64,
    text_hash: String,
    text: String,
    script: ChunkScript,
    extraction_method: String,
    extractor_id: String,
    extractor_version: String,
    config_hash: String,
    input_content_hash: String,
    created_at_unix_ms: i64,
}

impl ContentChunk {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        source_revision_id: impl Into<String>,
        ordinal: u32,
        start_byte: u64,
        end_byte: u64,
        text_hash: impl Into<String>,
        text: impl Into<String>,
        script: ChunkScript,
        extraction_method: impl Into<String>,
        extractor_id: impl Into<String>,
        extractor_version: impl Into<String>,
        config_hash: impl Into<String>,
        input_content_hash: impl Into<String>,
        created_at_unix_ms: i64,
    ) -> Result<Self, EvidenceError> {
        if start_byte > end_byte {
            return Err(EvidenceError::InvalidByteRange {
                start_byte,
                end_byte,
            });
        }

        Ok(Self {
            id: required("content chunk id", id.into())?,
            source_revision_id: required("source revision id", source_revision_id.into())?,
            ordinal,
            start_byte,
            end_byte,
            text_hash: required("text hash", text_hash.into())?,
            text: chunk_text(text.into())?,
            script,
            extraction_method: required("extraction method", extraction_method.into())?,
            extractor_id: required("extractor id", extractor_id.into())?,
            extractor_version: required("extractor version", extractor_version.into())?,
            config_hash: required("configuration hash", config_hash.into())?,
            input_content_hash: required("input content hash", input_content_hash.into())?,
            created_at_unix_ms,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn source_revision_id(&self) -> &str {
        &self.source_revision_id
    }

    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    pub fn start_byte(&self) -> u64 {
        self.start_byte
    }

    pub fn end_byte(&self) -> u64 {
        self.end_byte
    }

    pub fn text_hash(&self) -> &str {
        &self.text_hash
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Which scripts this chunk's text holds. Not a language: see `ChunkScript`.
    pub fn script(&self) -> ChunkScript {
        self.script
    }

    pub fn extraction_method(&self) -> &str {
        &self.extraction_method
    }

    pub fn extractor_id(&self) -> &str {
        &self.extractor_id
    }

    pub fn extractor_version(&self) -> &str {
        &self.extractor_version
    }

    pub fn config_hash(&self) -> &str {
        &self.config_hash
    }

    pub fn input_content_hash(&self) -> &str {
        &self.input_content_hash
    }

    pub fn created_at_unix_ms(&self) -> i64 {
        self.created_at_unix_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceCommit {
    file_version: FileVersion,
    observation: FileObservation,
    chunks: Vec<ContentChunk>,
    /// Boxed because a conversation is the rare case: every commit of every ordinary file
    /// would otherwise carry a conversation's worth of stack to say it has none, and a commit
    /// travels by value through the batch.
    conversation: Option<Box<Conversation>>,
}

impl EvidenceCommit {
    pub fn new(
        file_version: FileVersion,
        observation: FileObservation,
        chunks: Vec<ContentChunk>,
    ) -> Result<Self, EvidenceError> {
        Self::of_source(file_version, observation, chunks, None)
    }

    /// The same, for a source that is a conversation as well as text.
    ///
    /// The conversation travels inside the commit rather than beside it because it is
    /// evidence of the same revision as the chunks: one transaction publishes a source
    /// entirely or not at all, and a conversation written by a second call could survive a
    /// revision that did not.
    pub fn of_source(
        file_version: FileVersion,
        observation: FileObservation,
        chunks: Vec<ContentChunk>,
        conversation: Option<Conversation>,
    ) -> Result<Self, EvidenceError> {
        let conversation = conversation.map(Box::new);
        if observation.file_id() != file_version.file_id()
            || observation.file_version_id() != file_version.id()
        {
            return Err(EvidenceError::ObservationMismatch {
                observation_id: observation.id().to_string(),
            });
        }
        let mut chunk_ids = HashSet::with_capacity(chunks.len());
        let mut ordinals = HashSet::with_capacity(chunks.len());

        for chunk in &chunks {
            if chunk.source_revision_id() != file_version.id() {
                return Err(EvidenceError::ChunkSourceMismatch {
                    chunk_id: chunk.id().to_string(),
                    expected_source_revision_id: file_version.id().to_string(),
                    actual_source_revision_id: chunk.source_revision_id().to_string(),
                });
            }
            if chunk.input_content_hash() != file_version.content_hash() {
                return Err(EvidenceError::ChunkContentMismatch {
                    chunk_id: chunk.id().to_string(),
                    expected_content_hash: file_version.content_hash().to_string(),
                    actual_content_hash: chunk.input_content_hash().to_string(),
                });
            }
            if !chunk_ids.insert(chunk.id()) {
                return Err(EvidenceError::DuplicateChunkId(chunk.id().to_string()));
            }
            if !ordinals.insert(chunk.ordinal()) {
                return Err(EvidenceError::DuplicateChunkOrdinal(chunk.ordinal()));
            }
        }

        Ok(Self {
            file_version,
            observation,
            chunks,
            conversation,
        })
    }

    pub fn file_version(&self) -> &FileVersion {
        &self.file_version
    }

    /// The conversation this revision held, when the source was a transcript.
    pub fn conversation(&self) -> Option<&Conversation> {
        self.conversation.as_deref()
    }

    pub fn observation(&self) -> &FileObservation {
        &self.observation
    }

    pub fn chunks(&self) -> &[ContentChunk] {
        &self.chunks
    }

    /// The outbox entry this commit must publish together with its revision. Derived here
    /// and nowhere else, so no layer can compute a different job for the same revision.
    pub fn insight_job(&self) -> InsightJob {
        InsightJob::queued_for(&self.file_version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceError {
    MissingField(&'static str),
    InvalidByteRange {
        start_byte: u64,
        end_byte: u64,
    },
    ChunkSourceMismatch {
        chunk_id: String,
        expected_source_revision_id: String,
        actual_source_revision_id: String,
    },
    ChunkContentMismatch {
        chunk_id: String,
        expected_content_hash: String,
        actual_content_hash: String,
    },
    ObservationMismatch {
        observation_id: String,
    },
    DuplicateChunkId(String),
    DuplicateChunkOrdinal(u32),
    LocationMismatch {
        location_id: String,
        file_version_id: String,
    },
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "{field} must not be empty"),
            Self::InvalidByteRange {
                start_byte,
                end_byte,
            } => write!(f, "chunk byte range {start_byte}..{end_byte} is invalid"),
            Self::ChunkSourceMismatch {
                chunk_id,
                expected_source_revision_id,
                actual_source_revision_id,
            } => write!(
                f,
                "chunk {chunk_id} belongs to {actual_source_revision_id}, expected {expected_source_revision_id}"
            ),
            Self::ChunkContentMismatch {
                chunk_id,
                expected_content_hash,
                actual_content_hash,
            } => write!(
                f,
                "chunk {chunk_id} has content hash {actual_content_hash}, expected {expected_content_hash}"
            ),
            Self::ObservationMismatch { observation_id } => write!(
                f,
                "observation {observation_id} does not belong to this file version"
            ),
            Self::DuplicateChunkId(id) => write!(f, "duplicate chunk id {id}"),
            Self::DuplicateChunkOrdinal(ordinal) => write!(f, "duplicate chunk ordinal {ordinal}"),
            Self::LocationMismatch {
                location_id,
                file_version_id,
            } => write!(
                f,
                "file version {file_version_id} does not belong to the file at location {location_id}"
            ),
        }
    }
}

impl std::error::Error for EvidenceError {}

fn required(field: &'static str, value: String) -> Result<String, EvidenceError> {
    if value.trim().is_empty() {
        return Err(EvidenceError::MissingField(field));
    }
    Ok(value)
}

/// The text of a chunk, which is held to a weaker rule than an identifier: whitespace is
/// content. A chunk of nothing but spaces covers the bytes between two words as much as any
/// other chunk does, and refusing it would refuse the whole file it sits in — a blank block
/// in the middle of a document is not a reason to index nothing of that document.
fn chunk_text(value: String) -> Result<String, EvidenceError> {
    if value.is_empty() {
        return Err(EvidenceError::MissingField("chunk text"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{
        ChunkScript, ContentChunk, EvidenceCommit, EvidenceError, FileObservation, FileVersion,
    };

    fn file_version() -> FileVersion {
        FileVersion::new("file:1@hash:a", "file:1", "hash:a", 1_700_000_000_000).unwrap()
    }

    fn chunk(source_revision_id: &str, input_content_hash: &str, ordinal: u32) -> ContentChunk {
        ContentChunk::new(
            format!("chunk:{ordinal}"),
            source_revision_id,
            ordinal,
            0,
            5,
            format!("text:{ordinal}"),
            "hello",
            ChunkScript::Latin,
            "plain-text",
            "plain-text",
            "1",
            "config:1",
            input_content_hash,
            1_700_000_000_000,
        )
        .unwrap()
    }

    fn observation(version: &FileVersion) -> FileObservation {
        FileObservation::new(
            "observation:1",
            version.file_id(),
            version.id(),
            1_700_000_000_000,
        )
        .unwrap()
    }

    #[test]
    fn evidence_commit_requires_chunks_to_belong_to_its_revision() {
        let error = EvidenceCommit::new(
            file_version().clone(),
            observation(&file_version()),
            vec![chunk("file:other@hash:a", "hash:a", 0)],
        )
        .unwrap_err();

        assert!(matches!(error, EvidenceError::ChunkSourceMismatch { .. }));
    }

    #[test]
    fn evidence_commit_rejects_duplicate_chunk_ordinals() {
        let version = file_version();
        let error = EvidenceCommit::new(
            version.clone(),
            observation(&version),
            vec![
                chunk(version.id(), version.content_hash(), 0),
                ContentChunk::new(
                    "chunk:other",
                    version.id(),
                    0,
                    5,
                    10,
                    "text:other",
                    "world",
                    ChunkScript::Latin,
                    "plain-text",
                    "plain-text",
                    "1",
                    "config:1",
                    version.content_hash(),
                    1_700_000_000_000,
                )
                .unwrap(),
            ],
        )
        .unwrap_err();

        assert_eq!(error, EvidenceError::DuplicateChunkOrdinal(0));
    }
}
