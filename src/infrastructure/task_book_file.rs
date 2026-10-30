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
use crate::domain::{TaskBook, WorkspaceName};

use super::app_directories::data_directory;
use super::whole_file::replace_whole;

/// The version of the file's shape. Raised when a field changes meaning or goes away.
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
        Ok(TaskBook::restored(
            file.tasks
                .into_iter()
                .map(|task| (task.id, task.name, task.members, task.dropped))
                .collect(),
            file.rejected,
        ))
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
        store.save(&book).unwrap();
        assert_eq!(store.load().unwrap(), book);
        assert!(!path.with_extension("new").exists());
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
