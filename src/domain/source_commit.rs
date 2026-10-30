// One transaction worth of facts about a single source path: its location, its content
// evidence when the policy allowed reading it, and whether this installation asked for that
// content to be analysed.
use super::evidence::{EvidenceCommit, EvidenceError};
use super::file_location::FileLocation;
use super::insight_job::InsightJob;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCommit {
    location: FileLocation,
    evidence: Option<EvidenceCommit>,
    analysis: AnalysisRequest,
}

/// Whether this revision is one the installation analyses.
///
/// Decided where the rules are known and carried here, so that the one place which derives
/// the outbox entry also knows whether to derive one at all. A revision nobody asked to
/// analyse gets no job — not a job in a new state: stage 21 measured what tens of thousands
/// of queued rows cost, and a queue of work nobody intends to do is the same cost by
/// another name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisRequest {
    Requested,
    NotRequested,
}

impl SourceCommit {
    pub fn metadata_only(location: FileLocation) -> Self {
        Self {
            location,
            evidence: None,
            analysis: AnalysisRequest::NotRequested,
        }
    }

    pub fn with_evidence(
        location: FileLocation,
        evidence: EvidenceCommit,
        analysis: AnalysisRequest,
    ) -> Result<Self, EvidenceError> {
        if evidence.file_version().file_id() != location.file_id() {
            return Err(EvidenceError::LocationMismatch {
                location_id: location.id().to_string(),
                file_version_id: evidence.file_version().id().to_string(),
            });
        }
        Ok(Self {
            location,
            evidence: Some(evidence),
            analysis,
        })
    }

    pub fn location(&self) -> &FileLocation {
        &self.location
    }

    pub fn evidence(&self) -> Option<&EvidenceCommit> {
        self.evidence.as_ref()
    }

    /// The outbox entry this commit publishes with its revision, when one was asked for.
    ///
    /// Nothing else may derive a job: one place decides both whether there is one and what
    /// it is, so no layer can queue work this installation did not ask for.
    pub fn insight_job(&self) -> Option<InsightJob> {
        match self.analysis {
            AnalysisRequest::Requested => self.evidence.as_ref().map(EvidenceCommit::insight_job),
            AnalysisRequest::NotRequested => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnalysisRequest, SourceCommit};
    use crate::domain::{
        EvidenceCommit, EvidenceError, FileKind, FileLocation, FileObservation, FileVersion,
        NormalizedPath,
    };

    fn location(file_id: &str) -> FileLocation {
        FileLocation::observed(
            file_id,
            NormalizedPath::from_absolute("/notes.md").unwrap(),
            FileKind::RegularFile,
            5,
            100,
            200,
        )
        .unwrap()
    }

    fn evidence(file_id: &str) -> EvidenceCommit {
        let version =
            FileVersion::new(format!("{file_id}@hash:a"), file_id, "hash:a", 100).unwrap();
        let observation =
            FileObservation::new("observation:1", file_id, version.id(), 100).unwrap();
        EvidenceCommit::new(version, observation, vec![]).unwrap()
    }

    #[test]
    fn evidence_must_belong_to_the_located_file() {
        assert!(
            SourceCommit::with_evidence(
                location("file:1"),
                evidence("file:1"),
                AnalysisRequest::Requested,
            )
            .is_ok()
        );
        let error = SourceCommit::with_evidence(
            location("file:1"),
            evidence("file:2"),
            AnalysisRequest::Requested,
        )
        .unwrap_err();
        assert!(matches!(error, EvidenceError::LocationMismatch { .. }));
    }
}
