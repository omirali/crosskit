// Where an answer already given is kept, so the same question over the same sources is not
// paid for twice.
//
// One file per answer, in a directory of this context's own beside its graph. Not in the
// graph: the schema of the graph is what every earlier stage measured and migrated, and an
// answer is neither evidence nor a job — it is a derivative of both, discardable at any
// moment, and it has no business in a table that costs a migration to change.
//
// What is kept is the answer and the key it answers. The key names its sources by the chunks
// they came from, never by their text, so this directory holds no copy of the context's
// content — only what a model said about it, which the person who asked has already read.
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::application::{AnswerCache, AskFailure};
use crate::domain::{Answer, AskCacheKey, WorkspaceName};

/// `ask-<context>` beside `graph-<context>.lbug`, so the three parts of a context are found
/// the same way. The name comes from the context's own identity, which refuses anything that
/// is not a plain name.
pub fn ask_cache_directory(data_directory: &Path, workspace: &WorkspaceName) -> PathBuf {
    data_directory.join(format!("ask-{workspace}"))
}

pub struct FileAnswerCache {
    directory: PathBuf,
}

impl FileAnswerCache {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// The file one key lives in: the key's digest, because a key holds a question and a
    /// question is not a filename.
    fn entry_path(&self, key: &AskCacheKey) -> PathBuf {
        let digest = Sha256::digest(key.as_str().as_bytes());
        self.directory.join(format!("{digest:x}.json"))
    }
}

/// One answer as it is kept. The key travels with it, so a digest that collided answers the
/// wrong question to nobody.
#[derive(Debug, Serialize, Deserialize)]
struct CachedAnswer {
    key: String,
    answer: String,
    sources: Vec<i64>,
    /// How many excerpts the question carried, so the answer is validated on the way out the
    /// same way it was on the way in.
    sent: usize,
}

impl AnswerCache for FileAnswerCache {
    fn lookup(&self, key: &AskCacheKey) -> Result<Option<Answer>, AskFailure> {
        let path = self.entry_path(key);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(AskFailure::Cache),
        };
        let Ok(cached) = serde_json::from_str::<CachedAnswer>(&text) else {
            // A file this program cannot read is a file it did not write, or wrote in
            // another version. It is not an error: the question is simply asked again.
            return Ok(None);
        };
        if cached.key != key.as_str() {
            return Ok(None);
        }
        // Validated again rather than trusted: a file on disk is not a value, whoever wrote
        // it, and an answer citing a source nobody sent must not come back through here.
        Ok(Answer::parse(&cached.answer, &cached.sources, cached.sent).ok())
    }

    fn store(&self, key: &AskCacheKey, answer: &Answer) -> Result<(), AskFailure> {
        fs::create_dir_all(&self.directory).map_err(|_| AskFailure::Cache)?;
        let cached = CachedAnswer {
            key: key.as_str().to_string(),
            answer: answer.text().to_string(),
            sources: answer.used().iter().map(|used| *used as i64).collect(),
            sent: answer.used().iter().copied().max().unwrap_or(0),
        };
        let text = serde_json::to_string(&cached).map_err(|_| AskFailure::Cache)?;
        fs::write(self.entry_path(key), text).map_err(|_| AskFailure::Cache)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{FileAnswerCache, ask_cache_directory};
    use crate::application::AnswerCache;
    use crate::domain::{Answer, AskCacheKey, Question, WorkspaceName};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn cache(name: &str) -> FileAnswerCache {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "crosskit-ask-cache-{name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        FileAnswerCache::new(directory)
    }

    fn key(question: &str) -> AskCacheKey {
        AskCacheKey::of(
            &Question::parse(question).unwrap(),
            "a-model",
            "ask-v1",
            &[("chunk:a", 100)],
            None,
        )
    }

    #[test]
    fn an_answer_comes_back_as_it_was_stored() {
        let cache = cache("round-trip");
        let answer = Answer::parse("сверка обходит дерево", &[1], 2).unwrap();

        assert_eq!(cache.lookup(&key("что делает сверка")).unwrap(), None);
        cache.store(&key("что делает сверка"), &answer).unwrap();

        assert_eq!(
            cache.lookup(&key("что делает сверка")).unwrap(),
            Some(answer)
        );
    }

    /// Another question, another set of sources, another model or another prompt is another
    /// key, and a miss rather than somebody else's answer.
    #[test]
    fn another_question_is_a_miss() {
        let cache = cache("miss");
        cache
            .store(
                &key("что делает сверка"),
                &Answer::parse("ответ", &[1], 1).unwrap(),
            )
            .unwrap();

        assert_eq!(cache.lookup(&key("что делает индекс")).unwrap(), None);
    }

    /// A file this program did not write, or wrote in another version, is not an error: the
    /// question is asked again.
    #[test]
    fn a_file_that_cannot_be_read_is_a_miss_and_not_a_failure() {
        let cache = cache("unreadable");
        let key = key("что делает сверка");
        cache
            .store(&key, &Answer::parse("ответ", &[1], 1).unwrap())
            .unwrap();
        let path = cache.entry_path(&key);
        std::fs::write(&path, "{ not json").unwrap();

        assert_eq!(cache.lookup(&key).unwrap(), None);
    }

    #[test]
    fn the_directory_is_named_after_the_context() {
        let workspace = WorkspaceName::parse("work").unwrap();
        assert_eq!(
            ask_cache_directory(std::path::Path::new("/data"), &workspace),
            std::path::PathBuf::from("/data/ask-work")
        );
    }
}
