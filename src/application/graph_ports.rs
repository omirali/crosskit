// What the graph view reads, as records rather than queries.
//
// Every method narrows in the database and none of them decides anything: which pairs are
// drawn, which reply a touch belongs to and what fits in the bound are the view's rules and
// live in `graph_explorer`, where they can be read and tested without a database.
use crate::domain::conversation::Speaker;
use crate::domain::{LocationStatus, NormalizedPath};

use super::file_touches::StoredTouch;
use super::repository::RepositoryError;

/// What the analysis concluded a revision is about, with the result it is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTopic {
    pub result_id: String,
    pub label: String,
}

/// One conversation row: one revision of a transcript read as a conversation. A session that
/// grew is stored once per revision, and `current` says which row is the latest reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConversation {
    pub id: String,
    pub session_id: String,
    /// Empty when nobody named it.
    pub name: String,
    pub message_count: u64,
    pub last_message_at_unix_ms: Option<i64>,
    /// The transcript this row was read from.
    pub transcript_file_id: String,
    /// Whether the row's revision is its transcript's current version.
    pub current: bool,
    pub topic: Option<StoredTopic>,
}

/// One file of this context: every place it is known to be, and its current topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub file_id: String,
    pub locations: Vec<(NormalizedPath, LocationStatus)>,
    pub topic: Option<StoredTopic>,
}

/// The reply a touch followed: the latest reply of its conversation said at or before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReply {
    pub conversation_id: String,
    /// The moment of the touch this reply was asked for.
    pub touched_at_unix_ms: i64,
    pub message_id: String,
    pub ordinal: u64,
    pub speaker: Speaker,
    pub at_unix_ms: i64,
}

pub trait GraphReader {
    /// Every conversation row of these sessions, of every revision.
    fn conversations_of_sessions(
        &self,
        sessions: &[String],
    ) -> Result<Vec<StoredConversation>, RepositoryError>;

    /// Every touch of these sessions, of every revision; folded above, as `touches` folds.
    fn touches_of_sessions(&self, sessions: &[String])
    -> Result<Vec<StoredTouch>, RepositoryError>;

    /// Every touch whose path is one of these, exactly.
    fn touches_at_paths(&self, paths: &[String]) -> Result<Vec<StoredTouch>, RepositoryError>;

    /// The files of this context with a location at each of these paths, as `(path, file)`.
    fn files_at_paths(&self, paths: &[String]) -> Result<Vec<(String, String)>, RepositoryError>;

    /// These files, the ones this context holds.
    fn files(&self, file_ids: &[String]) -> Result<Vec<StoredFile>, RepositoryError>;

    /// For each `(conversation row, moment)`, the reply that touch followed. A moment with no
    /// reply at or before it — the touch came first, or no reply recorded its time — is
    /// absent from the answer.
    fn replies_before(
        &self,
        moments: &[(String, i64)],
    ) -> Result<Vec<StoredReply>, RepositoryError>;
}
