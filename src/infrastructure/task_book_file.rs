// The owner's decisions about tasks, kept in a file of their own beside the context's graph
// (ADR 0053).
//
// Not in the graph: the graph has one writer, the background service, and a writing command has
// to stop it first. A decision a person takes in a second must not stop the service, and what
// the graph holds is what was read from disk, while this is what somebody decided.
//
// The file is replaced whole: written beside itself and renamed over the old one, so a reader
// sees either the book before a decision or after it. A file of a version this build does not
// know is refused rather than read as something it is not.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::application::{TaskBookStore, TaskBookStoreError};
use crate::domain::risk_decision::{RiskAnchor, RiskDecision, RiskVerdict};
use crate::domain::{RestoredTask, TaskBook, WorkspaceName};

use super::app_directories::data_directory;
use super::whole_file::replace_whole;

/// The version of the file's shape. Raised when a field changes meaning or goes away; a field
/// added with a default keeps it, so a build that does not know the field still reads the file.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct BookFile {
    version: u32,
    tasks: Vec<TaskEntry>,
    /// The sessions of every candidate the owner said is not a task.
    rejected: Vec<BTreeSet<String>>,
}

#[derive(Serialize, Deserialize)]
struct TaskEntry {
    id: String,
    name: String,
    members: BTreeSet<String>,
    dropped: BTreeSet<String>,
    /// The owner's word on the task's risks (ADR 0061).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    risk_decisions: Vec<RiskDecisionEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished_at_unix_ms: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct RiskDecisionEntry {
    session: String,
    reply: usize,
    /// `accepted` or `resolved`.
    verdict: String,
    reason: String,
    at_unix_ms: i64,
}

/// The book of one context.
pub struct TaskBookFile {
    path: PathBuf,
}

impl TaskBookFile {
    /// The book of `context` in the data directory, beside its graph.
    pub fn of_context(context: &WorkspaceName) -> Result<Self, TaskBookStoreError> {
        let directory =
            data_directory().map_err(|error| TaskBookStoreError::Unreadable(error.to_string()))?;
        Ok(Self::at(directory.join(format!("tasks-{context}.json"))))
    }

    fn at(path: PathBuf) -> Self {
        Self { path }
    }
}

impl TaskBookStore for TaskBookFile {
    fn load(&self) -> Result<TaskBook, TaskBookStoreError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TaskBook::default());
            }
            Err(error) => return Err(unreadable(&self.path, error)),
        };
        let file: BookFile =
            serde_json::from_str(&text).map_err(|error| unreadable(&self.path, error))?;
        if file.version != VERSION {
            return Err(TaskBookStoreError::Unreadable(format!(
                "{} is version {} and this build reads {VERSION}",
                self.path.display(),
                file.version
            )));
        }
        let tasks = file
            .tasks
            .into_iter()
            .map(|task| {
                let risk_decisions = task
                    .risk_decisions
                    .into_iter()
                    .map(|entry| {
                        let verdict = RiskVerdict::parse(&entry.verdict).ok_or_else(|| {
                            unreadable(
                                &self.path,
                                format!("a risk decision says {:?}", entry.verdict),
                            )
                        })?;
                        Ok(RiskDecision {
                            anchor: RiskAnchor {
                                session: entry.session,
                                reply: entry.reply,
                            },
                            verdict,
                            reason: entry.reason,
                            at_unix_ms: entry.at_unix_ms,
                        })
                    })
                    .collect::<Result<Vec<_>, TaskBookStoreError>>()?;
                Ok(RestoredTask {
                    id: task.id,
                    name: task.name,
                    members: task.members,
                    dropped: task.dropped,
                    risk_decisions,
                    finished_at_unix_ms: task.finished_at_unix_ms,
                })
            })
            .collect::<Result<Vec<_>, TaskBookStoreError>>()?;
        Ok(TaskBook::restored(tasks, file.rejected))
    }

    fn save(&self, book: &TaskBook) -> Result<(), TaskBookStoreError> {
        let file = BookFile {
            version: VERSION,
            tasks: book
                .tasks()
                .iter()
                .map(|task| TaskEntry {
                    id: task.id().to_string(),
                    name: task.name().to_string(),
                    members: task.members().clone(),
                    dropped: task.dropped().clone(),
                    risk_decisions: task
                        .risk_decisions()
                        .iter()
                        .map(|decision| RiskDecisionEntry {
                            session: decision.anchor.session.clone(),
                            reply: decision.anchor.reply,
                            verdict: decision.verdict.code().to_string(),
                            reason: decision.reason.clone(),
                            at_unix_ms: decision.at_unix_ms,
                        })
                        .collect(),
                    finished_at_unix_ms: task.finished_at_unix_ms(),
                })
                .collect(),
            rejected: book.rejected().to_vec(),
        };
        let text =
            serde_json::to_string_pretty(&file).map_err(|error| unwritable(&self.path, error))?;
        replace_whole(&self.path, text.as_bytes()).map_err(|error| unwritable(&self.path, error))
    }
}

fn unreadable(path: &Path, error: impl std::fmt::Display) -> TaskBookStoreError {
    TaskBookStoreError::Unreadable(format!("{}: {error}", path.display()))
}

fn unwritable(path: &Path, error: impl std::fmt::Display) -> TaskBookStoreError {
    TaskBookStoreError::Unwritable(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book_path(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("crosskit-task-book-{name}-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("a temporary directory is writable");
        directory.join("tasks-work.json")
    }

    #[test]
    fn a_book_comes_back_as_it_was_saved_and_a_missing_one_is_empty() {
        let path = book_path("round-trip");
        let store = TaskBookFile::at(path.clone());
        assert_eq!(store.load().unwrap(), TaskBook::default());

        let mut book = TaskBook::default();
        book.confirm(
            "Двойная скидка",
            BTreeSet::from(["a".to_string(), "b".to_string()]),
        )
        .unwrap();
        book.drop_member("Двойная скидка", "b").unwrap();
        book.reject(BTreeSet::from(["x".to_string()]));
        book.decide_risk(
            "Двойная скидка",
            RiskDecision {
                anchor: RiskAnchor {
                    session: "a".into(),
                    reply: 12,
                },
                verdict: RiskVerdict::Accepted,
                reason: "бэкап в ночном снапшоте".into(),
                at_unix_ms: 1_790_000_000_000,
            },
        )
        .unwrap();
        book.finish("Двойная скидка", 1_790_000_100_000, 0).unwrap();
        store.save(&book).unwrap();
        assert_eq!(store.load().unwrap(), book);
        assert!(!path.with_extension("new").exists());
        fs::remove_dir_all(path.parent().unwrap()).expect("the temporary directory is removable");
    }

    /// A book written before risk decisions existed reads as a book without them.
    #[test]
    fn a_book_without_the_later_fields_reads() {
        let path = book_path("earlier");
        fs::write(
            &path,
            r#"{"version":1,"tasks":[{"id":"task:a","name":"Скидки","members":["a"],"dropped":[]}],"rejected":[]}"#,
        )
        .unwrap();
        let book = TaskBookFile::at(path.clone()).load().unwrap();
        assert!(book.tasks()[0].risk_decisions().is_empty());
        assert_eq!(book.tasks()[0].finished_at_unix_ms(), None);
        fs::remove_dir_all(path.parent().unwrap()).expect("the temporary directory is removable");
    }

    #[test]
    fn a_book_of_another_version_is_refused() {
        let path = book_path("version");
        fs::write(&path, r#"{"version":2,"tasks":[],"rejected":[]}"#).unwrap();
        assert!(matches!(
            TaskBookFile::at(path.clone()).load(),
            Err(TaskBookStoreError::Unreadable(_))
        ));
        fs::remove_dir_all(path.parent().unwrap()).expect("the temporary directory is removable");
    }
}
