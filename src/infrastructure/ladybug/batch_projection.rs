// Turns an already bounded batch of sources into the typed rows its stores write.
//
// No Cypher lives here and nothing is read from the database: this is the pure step that
// decides what the batch means, so the stores can write it in set-based statements. Every
// row vector keeps the order of the commits it came from, and an observation additionally
// carries that commit's index, because the durable sequence it receives is allocated
// strictly by it. Errors name identities, never a path, a chunk text or a row.
use std::collections::{HashMap, HashSet};

use crate::application::RepositoryError;
use crate::domain::conversation::{Conversation, NamedBy, Speaker};
use crate::domain::{
    ContentChunk, FileKind, FileLocation, FileObservation, FileVersion, InsightJob, LocationStatus,
    SourceCommit,
};

use super::conversation_store::{TIME_NOT_RECORDED, conversation_id_of, message_id_of};
use super::store_support::u64_to_i64;

/// What one bounded batch writes, grouped by the store responsible for it.
pub(super) struct BatchProjection<'a> {
    pub(super) files: Vec<&'a str>,
    pub(super) locations: Vec<LocationRow<'a>>,
    pub(super) versions: Vec<VersionRow<'a>>,
    pub(super) observations: Vec<ObservationRow<'a>>,
    pub(super) chunks: Vec<ChunkRow<'a>>,
    pub(super) extractors: Vec<ExtractorRow>,
    pub(super) jobs: Vec<JobRow<'a>>,
    pub(super) conversations: Vec<ConversationRow<'a>>,
    pub(super) messages: Vec<MessageRow<'a>>,
    pub(super) touches: Vec<TouchRow<'a>>,
}

/// Whether a batch can be written set-based at all.
pub(super) enum Projected<'a> {
    SetBased(Box<BatchProjection<'a>>),
    /// The batch repeats a location, so one set of rows cannot describe its outcome.
    /// Either the same location appears twice, where the first sighting owns `first_seen`
    /// and the later ones only refresh, or two different locations claim one path, where
    /// each would retire the other. Both are decided by the order of the sources, so they
    /// are written one after another.
    RequiresSequential,
}

pub(super) struct LocationRow<'a> {
    pub(super) id: &'a str,
    pub(super) file_id: &'a str,
    pub(super) normalized_path: &'a str,
    pub(super) display_name: &'a str,
    pub(super) kind: &'static str,
    pub(super) size_bytes: i64,
    pub(super) modified_at_unix_ms: i64,
    pub(super) first_seen_at_unix_ms: i64,
    pub(super) last_seen_at_unix_ms: i64,
    pub(super) status: &'static str,
}

pub(super) struct VersionRow<'a> {
    pub(super) id: &'a str,
    pub(super) file_id: &'a str,
    pub(super) content_hash: &'a str,
    pub(super) created_at_unix_ms: i64,
}

pub(super) struct ObservationRow<'a> {
    pub(super) input_index: usize,
    pub(super) id: &'a str,
    pub(super) file_id: &'a str,
    pub(super) file_version_id: &'a str,
    pub(super) observed_at_unix_ms: i64,
}

pub(super) struct ChunkRow<'a> {
    pub(super) id: &'a str,
    pub(super) source_revision_id: &'a str,
    pub(super) ordinal: i64,
    pub(super) start_byte: i64,
    pub(super) end_byte: i64,
    pub(super) text_hash: &'a str,
    pub(super) text: &'a str,
    pub(super) script: &'static str,
    pub(super) extraction_method: &'a str,
    pub(super) extractor_id: &'a str,
    pub(super) extractor_version: &'a str,
    pub(super) config_hash: &'a str,
    pub(super) input_content_hash: &'a str,
    pub(super) created_at_unix_ms: i64,
    pub(super) extractor_key: String,
}

/// One conversation of the batch. The id is derived from the revision, so a revision
/// published twice yields the same conversation and the second write is a no-op.
pub(super) struct ConversationRow<'a> {
    pub(super) id: String,
    pub(super) source_revision_id: &'a str,
    pub(super) session_id: &'a str,
    pub(super) name: &'a str,
    pub(super) named_by: &'static str,
    pub(super) first_message_at_unix_ms: i64,
    pub(super) last_message_at_unix_ms: i64,
    pub(super) message_count: i64,
    pub(super) created_at_unix_ms: i64,
}

pub(super) struct MessageRow<'a> {
    pub(super) id: String,
    pub(super) conversation_id: String,
    pub(super) role: &'static str,
    pub(super) ordinal: i64,
    pub(super) at_unix_ms: i64,
    pub(super) marker: std::marker::PhantomData<&'a ()>,
}

pub(super) struct TouchRow<'a> {
    pub(super) id: String,
    pub(super) conversation_id: String,
    pub(super) path: &'a str,
    pub(super) verb: &'static str,
    pub(super) source: &'static str,
    pub(super) at_unix_ms: i64,
    pub(super) naming: &'static str,
    pub(super) times: i64,
}

pub(super) struct ExtractorRow {
    pub(super) id: String,
    pub(super) extractor_id: String,
    pub(super) extractor_version: String,
}

pub(super) struct JobRow<'a> {
    pub(super) id: String,
    pub(super) source_revision_id: &'a str,
    pub(super) input_content_hash: &'a str,
    pub(super) pipeline_version: String,
    pub(super) state: &'static str,
}

impl<'a> BatchProjection<'a> {
    /// Projects `commits` in their given order. Rows that repeat an identity are folded
    /// into one only when every immutable field agrees; a disagreement is refused here,
    /// before a single statement runs, exactly as writing them one after another would.
    pub(super) fn of(commits: &'a [SourceCommit]) -> Result<Projected<'a>, RepositoryError> {
        let mut projection = Self {
            files: Vec::new(),
            locations: Vec::with_capacity(commits.len()),
            versions: Vec::new(),
            observations: Vec::new(),
            chunks: Vec::new(),
            extractors: Vec::new(),
            jobs: Vec::new(),
            conversations: Vec::new(),
            messages: Vec::new(),
            touches: Vec::new(),
        };
        let mut seen_files: HashSet<&str> = HashSet::new();
        let mut seen_locations: HashSet<&str> = HashSet::new();
        let mut paths: HashMap<&str, &str> = HashMap::new();
        let mut seen_versions: HashMap<&str, usize> = HashMap::new();
        let mut seen_observations: HashMap<&str, usize> = HashMap::new();
        let mut seen_chunks: HashMap<&str, usize> = HashMap::new();
        let mut seen_extractors: HashSet<String> = HashSet::new();
        let mut seen_jobs: HashMap<String, usize> = HashMap::new();
        let mut seen_conversations: HashSet<&str> = HashSet::new();

        for (input_index, commit) in commits.iter().enumerate() {
            let location = commit.location();
            // The same location twice in one batch: the first sighting owns `first_seen`
            // while the later ones only refresh what may change. One row cannot carry both
            // halves of that rule, so those sources are applied one after another.
            if !seen_locations.insert(location.id()) {
                return Ok(Projected::RequiresSequential);
            }
            // One path holds one file at a time, and a source publishes its own path as
            // present while retiring anything else there. Two different locations at one
            // path would therefore retire each other, and only their order decides the
            // winner; that order only exists when the sources are applied one by one.
            if paths
                .insert(location.path().as_str(), location.id())
                .is_some()
            {
                return Ok(Projected::RequiresSequential);
            }
            if seen_files.insert(location.file_id()) {
                projection.files.push(location.file_id());
            }
            projection.locations.push(location_row(location)?);

            let Some(evidence) = commit.evidence() else {
                continue;
            };
            let version = evidence.file_version();
            if let Some(first) = seen_versions.insert(version.id(), input_index) {
                let first_version = commits[first].evidence().unwrap().file_version();
                if !same_version(first_version, version) {
                    return Err(conflict("file version", version.id()));
                }
            } else {
                projection.versions.push(version_row(version));
            }

            let observation = evidence.observation();
            if let Some(first) = seen_observations.insert(observation.id(), input_index) {
                let first_observation = commits[first].evidence().unwrap().observation();
                if !same_observation(first_observation, observation) {
                    return Err(conflict("file observation", observation.id()));
                }
            } else {
                projection
                    .observations
                    .push(observation_row(input_index, observation));
            }

            for chunk in evidence.chunks() {
                if let Some(first) = seen_chunks.insert(chunk.id(), input_index) {
                    let first_chunk = commits[first]
                        .evidence()
                        .and_then(|evidence| {
                            evidence
                                .chunks()
                                .iter()
                                .find(|other| other.id() == chunk.id())
                        })
                        .ok_or_else(|| conflict("content chunk", chunk.id()))?;
                    if !same_chunk(first_chunk, chunk) {
                        return Err(conflict("content chunk", chunk.id()));
                    }
                    continue;
                }
                let row = chunk_row(chunk)?;
                if seen_extractors.insert(row.extractor_key.clone()) {
                    projection.extractors.push(ExtractorRow {
                        id: row.extractor_key.clone(),
                        extractor_id: chunk.extractor_id().to_string(),
                        extractor_version: chunk.extractor_version().to_string(),
                    });
                }
                projection.chunks.push(row);
            }

            // A transcript brings a conversation with it. It is keyed by the revision, like
            // the chunks, so a batch that repeats a revision projects it once.
            if let Some(conversation) = evidence.conversation()
                && seen_conversations.insert(version.id())
            {
                project_conversation(&mut projection, conversation, version);
            }

            // A revision no analysis rule covers publishes no outbox entry at all. Stage 23
            // decided that rather than a job in a new state: a queue of work nobody intends
            // to do costs the same as a queue of work somebody does.
            if let Some(job) = commit.insight_job() {
                let row = job_row(&job, version);
                if let Some(first) = seen_jobs.insert(row.id.clone(), input_index) {
                    let first_job = commits[first]
                        .insight_job()
                        .expect("a commit that produced a job row produces it again");
                    if !same_job(&first_job, &job) {
                        return Err(conflict("insight job", &row.id));
                    }
                } else {
                    projection.jobs.push(row);
                }
            }
        }
        Ok(Projected::SetBased(Box::new(projection)))
    }
}

/// The rows of one conversation: itself, one message per reply in the order it was said, and
/// one touch per file it named.
fn project_conversation<'a>(
    projection: &mut BatchProjection<'a>,
    conversation: &'a Conversation,
    version: &'a FileVersion,
) {
    let id = conversation_id_of(version.id());
    let (name, named_by) = match conversation.name() {
        Some(name) => (
            name.value(),
            match name.named_by() {
                NamedBy::Owner => "owner",
                NamedBy::Model => "model",
            },
        ),
        // A conversation nobody named carries an empty name and says who named it with the
        // same word: nobody. An empty string here is not a guess — it is the absence of a
        // name, and `named_by` is what a reader checks.
        None => ("", "nobody"),
    };
    projection.conversations.push(ConversationRow {
        id: id.clone(),
        source_revision_id: version.id(),
        session_id: conversation.session_id(),
        name,
        named_by,
        first_message_at_unix_ms: conversation
            .first_spoke_at_unix_ms()
            .unwrap_or(TIME_NOT_RECORDED),
        last_message_at_unix_ms: conversation
            .last_spoke_at_unix_ms()
            .unwrap_or(TIME_NOT_RECORDED),
        message_count: conversation.replies().len() as i64,
        created_at_unix_ms: version.created_at_unix_ms(),
    });
    for (ordinal, reply) in conversation.replies().iter().enumerate() {
        projection.messages.push(MessageRow {
            id: message_id_of(&id, ordinal as u64),
            conversation_id: id.clone(),
            role: match reply.speaker() {
                Speaker::Owner => "owner",
                Speaker::Model => "model",
            },
            ordinal: ordinal as i64,
            at_unix_ms: reply.at_unix_ms().unwrap_or(TIME_NOT_RECORDED),
            marker: std::marker::PhantomData,
        });
    }
    for (ordinal, touch) in conversation.touches().iter().enumerate() {
        projection.touches.push(TouchRow {
            id: format!("{id}:t{ordinal}"),
            conversation_id: id.clone(),
            path: touch.path(),
            verb: touch.verb().as_str(),
            source: touch.source().as_str(),
            at_unix_ms: touch.at_unix_ms().unwrap_or(TIME_NOT_RECORDED),
            naming: touch.naming().as_str(),
            times: i64::from(touch.times()),
        });
    }
}

fn conflict(kind: &str, id: &str) -> RepositoryError {
    RepositoryError::InvariantViolation(format!(
        "{kind} {id} appears twice in one batch with conflicting immutable values"
    ))
}

fn same_version(first: &FileVersion, second: &FileVersion) -> bool {
    first.file_id() == second.file_id() && first.content_hash() == second.content_hash()
}

fn same_observation(first: &FileObservation, second: &FileObservation) -> bool {
    first.file_id() == second.file_id()
        && first.file_version_id() == second.file_version_id()
        && first.observed_at_unix_ms() == second.observed_at_unix_ms()
}

fn same_chunk(first: &ContentChunk, second: &ContentChunk) -> bool {
    first.source_revision_id() == second.source_revision_id()
        && first.ordinal() == second.ordinal()
        && first.start_byte() == second.start_byte()
        && first.end_byte() == second.end_byte()
        && first.text_hash() == second.text_hash()
        && first.text() == second.text()
        && first.script() == second.script()
        && first.extraction_method() == second.extraction_method()
        && first.extractor_id() == second.extractor_id()
        && first.extractor_version() == second.extractor_version()
        && first.config_hash() == second.config_hash()
        && first.input_content_hash() == second.input_content_hash()
}

fn same_job(first: &InsightJob, second: &InsightJob) -> bool {
    first.source_revision_id() == second.source_revision_id()
        && first.input_content_hash() == second.input_content_hash()
        && first.pipeline_version() == second.pipeline_version()
}

fn location_row(location: &FileLocation) -> Result<LocationRow<'_>, RepositoryError> {
    Ok(LocationRow {
        id: location.id(),
        file_id: location.file_id(),
        normalized_path: location.path().as_str(),
        display_name: location.display_name(),
        kind: kind_label(location.kind()),
        size_bytes: u64_to_i64(location.size_bytes(), "location size")?,
        modified_at_unix_ms: location.modified_at_unix_ms(),
        first_seen_at_unix_ms: location.first_seen_at_unix_ms(),
        last_seen_at_unix_ms: location.last_seen_at_unix_ms(),
        status: status_label(location.status()),
    })
}

fn version_row(version: &FileVersion) -> VersionRow<'_> {
    VersionRow {
        id: version.id(),
        file_id: version.file_id(),
        content_hash: version.content_hash(),
        created_at_unix_ms: version.created_at_unix_ms(),
    }
}

fn observation_row(input_index: usize, observation: &FileObservation) -> ObservationRow<'_> {
    ObservationRow {
        input_index,
        id: observation.id(),
        file_id: observation.file_id(),
        file_version_id: observation.file_version_id(),
        observed_at_unix_ms: observation.observed_at_unix_ms(),
    }
}

fn chunk_row(chunk: &ContentChunk) -> Result<ChunkRow<'_>, RepositoryError> {
    Ok(ChunkRow {
        id: chunk.id(),
        source_revision_id: chunk.source_revision_id(),
        ordinal: i64::from(chunk.ordinal()),
        start_byte: u64_to_i64(chunk.start_byte(), "chunk start byte")?,
        end_byte: u64_to_i64(chunk.end_byte(), "chunk end byte")?,
        text_hash: chunk.text_hash(),
        text: chunk.text(),
        script: chunk.script().as_str(),
        extraction_method: chunk.extraction_method(),
        extractor_id: chunk.extractor_id(),
        extractor_version: chunk.extractor_version(),
        config_hash: chunk.config_hash(),
        input_content_hash: chunk.input_content_hash(),
        created_at_unix_ms: chunk.created_at_unix_ms(),
        extractor_key: extractor_key(chunk),
    })
}

fn job_row<'a>(job: &InsightJob, version: &'a FileVersion) -> JobRow<'a> {
    JobRow {
        id: job.id().to_string(),
        source_revision_id: version.id(),
        input_content_hash: version.content_hash(),
        pipeline_version: job.pipeline_version().to_string(),
        state: job.state().label(),
    }
}

/// The length prefix keeps the key unambiguous when an extractor id itself ends with
/// characters that could be read as the start of a version.
pub(super) fn extractor_key(chunk: &ContentChunk) -> String {
    format!(
        "{}:{}{}",
        chunk.extractor_id().len(),
        chunk.extractor_id(),
        chunk.extractor_version()
    )
}

pub(super) fn kind_label(kind: FileKind) -> &'static str {
    match kind {
        FileKind::RegularFile => "file",
        FileKind::Symlink => "symlink",
        FileKind::Special => "special",
    }
}

pub(super) fn status_label(status: LocationStatus) -> &'static str {
    match status {
        LocationStatus::Present => "present",
        LocationStatus::Missing => "missing",
    }
}
