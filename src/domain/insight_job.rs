// The insight outbox entry: one pending enrichment task per immutable revision, written
// in the same transaction as the revision so a future worker can never miss one.
//
// A job names the revision and its content hash and nothing else: no path, file name,
// event or chunk text. It is not a cache of provider output and carries no result, and it
// carries no time of its own — where it stands in the queue is the age of its revision,
// which only the graph knows (ADR 0030).
use super::evidence::FileVersion;

/// The enrichment pipeline this outbox feeds. A new pipeline version means every revision
/// needs a new job; the job id embeds the version so both can coexist.
const INSIGHT_PIPELINE_VERSION: &str = "insight-outbox-v1";

/// Where a job stands. `Queued` is where every job begins and where a recovered one
/// returns; `Dead` is the only state a job never leaves on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightJobState {
    Queued,
    Claimed,
    Done,
    Failed,
    Dead,
    /// The policy no longer asks for this revision: the file it belongs to is at no path
    /// this context still indexes. Terminal, and not a failure — nothing went wrong, the
    /// question was withdrawn. It is counted apart from `dead`, which is work that was
    /// tried until its attempts ran out.
    Skipped,
}

impl InsightJobState {
    /// Every state, so a summary can never quietly forget to count one.
    pub const ALL: [Self; 6] = [
        Self::Queued,
        Self::Claimed,
        Self::Done,
        Self::Failed,
        Self::Dead,
        Self::Skipped,
    ];

    /// The stored spelling. It is written to disk, so it is spelled once here and read
    /// back through `from_label`; no other module may invent a second spelling.
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Dead => "dead",
            Self::Skipped => "skipped",
        }
    }

    /// The state a stored label means, or nothing when the label is not one this program
    /// wrote. A caller that cannot name a state must refuse rather than guess.
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.label() == label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsightJob {
    id: String,
    source_revision_id: String,
    input_content_hash: String,
    pipeline_version: String,
    state: InsightJobState,
}

impl InsightJob {
    /// The job of `version` for the current pipeline. Deterministic: two passes over the
    /// same revision derive the same job, field for field, so a retry finds its job
    /// instead of creating a second one.
    ///
    /// It deliberately takes no moment from `version`. A revision is identified by its
    /// content, so a pass that re-reads an unchanged file builds the same revision
    /// stamped with the clock of *that* read; only the graph holds when the revision was
    /// first observed, and that is the moment the queue is ordered by. Deriving it here
    /// would stamp a job with the age of the pass that queued it — which is what put a
    /// mask approved after the fact ahead of the queue (ADR 0030).
    pub fn queued_for(version: &FileVersion) -> Self {
        Self {
            id: Self::id_of_revision(version.id()),
            source_revision_id: version.id().to_string(),
            input_content_hash: version.content_hash().to_string(),
            pipeline_version: INSIGHT_PIPELINE_VERSION.to_string(),
            state: InsightJobState::Queued,
        }
    }

    /// The id of the job of revision `revision_id`, whether or not one was ever queued. A scope
    /// of revisions is turned into a scope of jobs by this and nothing else.
    pub fn id_of_revision(revision_id: &str) -> String {
        format!("insight-job:{INSIGHT_PIPELINE_VERSION}:{revision_id}")
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The `FileVersion` this job enriches.
    pub fn source_revision_id(&self) -> &str {
        &self.source_revision_id
    }

    pub fn input_content_hash(&self) -> &str {
        &self.input_content_hash
    }

    pub fn pipeline_version(&self) -> &str {
        &self.pipeline_version
    }

    pub fn state(&self) -> InsightJobState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::{INSIGHT_PIPELINE_VERSION, InsightJob, InsightJobState};
    use crate::domain::{EvidenceCommit, EvidenceError, FileObservation, FileVersion};

    fn version() -> FileVersion {
        FileVersion::new("file:7@hash:a", "file:7", "hash:a", 100).unwrap()
    }

    #[test]
    fn a_job_is_derived_deterministically_from_its_revision_alone() {
        let first = InsightJob::queued_for(&version());
        let second = InsightJob::queued_for(&version());

        assert_eq!(first, second);
        assert_eq!(first.id(), "insight-job:insight-outbox-v1:file:7@hash:a");
        assert_eq!(first.source_revision_id(), "file:7@hash:a");
        assert_eq!(first.input_content_hash(), "hash:a");
        assert_eq!(first.pipeline_version(), INSIGHT_PIPELINE_VERSION);
        assert_eq!(first.state(), InsightJobState::Queued);
    }

    /// The same revision read again, at a later clock, derives the same job. Nothing in it
    /// may carry the moment of the pass: that moment is not the revision's age, and a job
    /// that carried it would take its place in the queue from whichever pass queued it.
    #[test]
    fn re_reading_a_revision_later_derives_the_very_same_job() {
        let later = FileVersion::new("file:7@hash:a", "file:7", "hash:a", 9_000).unwrap();

        assert_eq!(
            InsightJob::queued_for(&version()),
            InsightJob::queued_for(&later)
        );
    }

    #[test]
    fn a_different_revision_yields_a_different_job() {
        let other = FileVersion::new("file:7@hash:b", "file:7", "hash:b", 200).unwrap();

        let job = InsightJob::queued_for(&version());
        let other_job = InsightJob::queued_for(&other);

        assert_ne!(job.id(), other_job.id());
        assert_ne!(job.input_content_hash(), other_job.input_content_hash());
    }

    #[test]
    fn a_job_only_exists_for_a_valid_revision() {
        // The revision constructor is the validation gate: a job can never be derived
        // from a revision without an id, file or content hash.
        assert_eq!(
            FileVersion::new("", "file:7", "hash:a", 100).unwrap_err(),
            EvidenceError::MissingField("file version id")
        );
        assert_eq!(
            FileVersion::new("file:7@hash:a", "file:7", "", 100).unwrap_err(),
            EvidenceError::MissingField("content hash")
        );
    }

    #[test]
    fn every_state_has_one_stored_spelling_that_reads_back_as_itself() {
        let mut labels: Vec<&str> = InsightJobState::ALL.iter().map(|s| s.label()).collect();
        for state in InsightJobState::ALL {
            assert_eq!(InsightJobState::from_label(state.label()), Some(state));
        }
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), InsightJobState::ALL.len(), "{labels:?}");
        // A label this program never wrote names no state, so a reader refuses instead of
        // treating an unknown row as queued.
        assert_eq!(InsightJobState::from_label("running"), None);
        assert_eq!(InsightJobState::from_label(""), None);
    }

    #[test]
    fn an_evidence_commit_exposes_exactly_the_job_of_its_version() {
        let version = version();
        let observation =
            FileObservation::new("observation:1", "file:7", version.id(), 100).unwrap();
        let commit = EvidenceCommit::new(version.clone(), observation, vec![]).unwrap();

        assert_eq!(commit.insight_job(), InsightJob::queued_for(&version));
    }
}
