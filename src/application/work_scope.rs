// Use case: which queued work a run takes when the owner names a directory.
//
// The queue is taken oldest revision first (ADR 0030), and on a real tree the oldest is rarely
// what the owner wants read today: the first fifteen jobs of `work` were the documentation of a
// public clone. Two ways to name the work instead, both by a directory of the context:
//
//   * under — the revisions whose file lies under it;
//   * touching — the conversations that worked on files under it, by the same pairs and the
//     same strict rule as `touches`: written, edited, or named by the owner (ADR 0041).
//
// Either way the result is a set of jobs, and the run takes from it in the queue's own order;
// every other job stays where it was. Only the current revision of a file counts: knowledge of a
// replaced revision is not written (ADR 0048), so asking about it would be money for nothing.
use std::collections::BTreeSet;

use crate::domain::{InsightJob, NormalizedPath};

use super::file_touches::{StoredTouch, fold};
use super::repository::RepositoryError;

/// What a directory selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkScope {
    Under(NormalizedPath),
    Touching(NormalizedPath),
}

impl WorkScope {
    pub fn directory(&self) -> &NormalizedPath {
        match self {
            Self::Under(directory) | Self::Touching(directory) => directory,
        }
    }
}

/// The reads a scope needs. All of them are of what is durable, and none writes.
pub trait WorkScopeReader {
    /// The current revision of every file present at a path under `directory`.
    fn current_revisions_under(
        &self,
        directory: &NormalizedPath,
    ) -> Result<Vec<String>, RepositoryError>;

    /// Every touch whose path lies under `directory`.
    fn touches_under(
        &self,
        directory: &NormalizedPath,
    ) -> Result<Vec<StoredTouch>, RepositoryError>;

    /// The current revision of the transcript of each of these sessions.
    fn current_revisions_of_sessions(
        &self,
        sessions: &[String],
    ) -> Result<Vec<String>, RepositoryError>;

    /// How many of these jobs are queued now.
    fn queued_among(&self, job_ids: &[String]) -> Result<usize, RepositoryError>;

    /// How many of these jobs exist at all, in any state. A revision without one is a
    /// revision no analysis rule covered when it was published.
    fn existing_among(&self, job_ids: &[String]) -> Result<usize, RepositoryError>;

    /// How many parts and bytes each of these revisions has, and where it is present.
    fn revision_sizes(&self, revision_ids: &[String])
    -> Result<Vec<RevisionSize>, RepositoryError>;
}

/// One revision's size and the paths its file is present at: what a price is made of and what
/// an approval names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionSize {
    pub revision_id: String,
    pub present_paths: Vec<NormalizedPath>,
    pub parts: usize,
    pub bytes: i64,
}

/// The work a directory selected, and what that amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedWork {
    revision_ids: Vec<String>,
    job_ids: Vec<String>,
    conversations: Option<usize>,
    queued: usize,
    without_job: usize,
}

impl SelectedWork {
    /// The jobs of the selected revisions, whether queued or not; the run takes the queued.
    pub fn job_ids(&self) -> &[String] {
        &self.job_ids
    }

    /// How many revisions the directory selected.
    pub fn revisions(&self) -> usize {
        self.job_ids.len()
    }

    pub fn revision_ids(&self) -> &[String] {
        &self.revision_ids
    }

    /// How many of the selected revisions have no job at all: no analysis rule covered them.
    pub fn without_job(&self) -> usize {
        self.without_job
    }

    /// How many conversations worked under the directory; nothing for `under`.
    pub fn conversations(&self) -> Option<usize> {
        self.conversations
    }

    /// How many of the selected revisions have a job waiting. Fewer than the revisions when
    /// some were already analysed, or no analysis rule covers them — a transcript is not covered
    /// by the default rules and waits on `index review`.
    pub fn queued(&self) -> usize {
        self.queued
    }
}

pub struct WorkScopeService<'a, Reader> {
    reader: &'a Reader,
}

impl<'a, Reader: WorkScopeReader> WorkScopeService<'a, Reader> {
    pub fn new(reader: &'a Reader) -> Self {
        Self { reader }
    }

    pub fn select(&self, scope: &WorkScope) -> Result<SelectedWork, RepositoryError> {
        let (revisions, conversations) = match scope {
            WorkScope::Under(directory) => (self.reader.current_revisions_under(directory)?, None),
            WorkScope::Touching(directory) => {
                let sessions: BTreeSet<String> = fold(self.reader.touches_under(directory)?)
                    .into_iter()
                    .filter(|pair| pair.is_deliberate())
                    .map(|pair| pair.session_id().to_string())
                    .collect();
                let sessions: Vec<String> = sessions.into_iter().collect();
                (
                    self.reader.current_revisions_of_sessions(&sessions)?,
                    Some(sessions.len()),
                )
            }
        };
        let revisions: BTreeSet<String> = revisions.into_iter().collect();
        let job_ids: Vec<String> = revisions
            .iter()
            .map(|revision| InsightJob::id_of_revision(revision))
            .collect();
        let queued = self.reader.queued_among(&job_ids)?;
        let without_job = job_ids.len() - self.reader.existing_among(&job_ids)?;
        Ok(SelectedWork {
            revision_ids: revisions.into_iter().collect(),
            job_ids,
            conversations,
            queued,
            without_job,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{WorkScope, WorkScopeReader, WorkScopeService};
    use crate::application::{RepositoryError, StoredTouch};
    use crate::domain::conversation::TouchVerb;
    use crate::domain::{NormalizedPath, OwnerNaming};

    fn directory() -> NormalizedPath {
        NormalizedPath::from_absolute("/work/registry").unwrap()
    }

    struct Stub {
        asked_sessions: RefCell<Vec<String>>,
    }

    impl WorkScopeReader for Stub {
        fn current_revisions_under(
            &self,
            _directory: &NormalizedPath,
        ) -> Result<Vec<String>, RepositoryError> {
            Ok(vec![
                "f2@b".to_string(),
                "f1@a".to_string(),
                "f1@a".to_string(),
            ])
        }

        fn touches_under(
            &self,
            _directory: &NormalizedPath,
        ) -> Result<Vec<StoredTouch>, RepositoryError> {
            let touch = |session: &str, verb, naming| {
                StoredTouch::new(session, "", "/work/registry/a.md", verb, naming, 1, None)
            };
            Ok(vec![
                touch("edited", TouchVerb::Edited, OwnerNaming::NotNamed),
                touch("named", TouchVerb::Read, OwnerNaming::Named),
                touch("glanced", TouchVerb::Read, OwnerNaming::NotNamed),
                touch("edited", TouchVerb::Read, OwnerNaming::NotNamed),
            ])
        }

        fn current_revisions_of_sessions(
            &self,
            sessions: &[String],
        ) -> Result<Vec<String>, RepositoryError> {
            self.asked_sessions
                .borrow_mut()
                .extend(sessions.iter().cloned());
            Ok(sessions
                .iter()
                .map(|session| format!("chat:{session}@1"))
                .collect())
        }

        fn queued_among(&self, job_ids: &[String]) -> Result<usize, RepositoryError> {
            Ok(job_ids.len() - 1)
        }

        fn existing_among(&self, job_ids: &[String]) -> Result<usize, RepositoryError> {
            Ok(job_ids.len())
        }

        fn revision_sizes(
            &self,
            _revision_ids: &[String],
        ) -> Result<Vec<super::RevisionSize>, RepositoryError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn under_a_directory_is_every_current_revision_there_once_as_its_job() {
        let stub = Stub {
            asked_sessions: RefCell::new(Vec::new()),
        };

        let selected = WorkScopeService::new(&stub)
            .select(&WorkScope::Under(directory()))
            .unwrap();

        assert_eq!(
            selected.job_ids(),
            [
                "insight-job:insight-outbox-v1:f1@a",
                "insight-job:insight-outbox-v1:f2@b"
            ]
        );
        assert_eq!(selected.conversations(), None);
        assert_eq!(selected.queued(), 1);
    }

    /// The same pairs and rule as `touches`: a conversation that only glanced at a file there
    /// did not work on it and is not taken.
    #[test]
    fn touching_a_directory_takes_the_conversations_that_worked_there() {
        let stub = Stub {
            asked_sessions: RefCell::new(Vec::new()),
        };

        let selected = WorkScopeService::new(&stub)
            .select(&WorkScope::Touching(directory()))
            .unwrap();

        assert_eq!(*stub.asked_sessions.borrow(), ["edited", "named"]);
        assert_eq!(selected.conversations(), Some(2));
        assert_eq!(selected.revisions(), 2);
    }
}
