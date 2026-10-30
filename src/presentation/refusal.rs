// A command that did not answer: the sentence a person reads, and the category a program
// acts on.
//
// The two are decided together, where the typed error is still in hand. Once a failure has
// become a sentence its kind can only be guessed back out of the words, and a reader that
// guesses from words breaks the first time a sentence is improved.
use crate::application::{GraphError, RepositoryError, SearchError, SearchIndexError};
use crate::domain::WorkspaceError;
use crate::infrastructure::ConfigError;

use super::machine_answer::FailureCategory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Refusal {
    category: FailureCategory,
    sentence: String,
}

impl Refusal {
    pub(super) fn new(category: FailureCategory, sentence: impl Into<String>) -> Self {
        Self {
            category,
            sentence: sentence.into(),
        }
    }

    pub(super) fn usage(sentence: impl Into<String>) -> Self {
        Self::new(FailureCategory::Usage, sentence)
    }

    /// A failure this program cannot place more exactly than "it did not work".
    pub(super) fn failed(sentence: impl Into<String>) -> Self {
        Self::new(FailureCategory::Failed, sentence)
    }

    pub(super) fn category(&self) -> FailureCategory {
        self.category
    }

    /// The configuration, as the command found it.
    pub(super) fn of_configuration(error: ConfigError) -> Self {
        let category = match &error {
            ConfigError::Workspace(workspace) => category_of_workspace(workspace),
            _ => FailureCategory::Configuration,
        };
        Self::new(category, error.to_string())
    }

    /// Choosing the context the run works in.
    pub(super) fn of_workspace(error: WorkspaceError) -> Self {
        Self::new(category_of_workspace(&error), error.to_string())
    }

    /// The graph, with the sentence the caller has already worded for it.
    pub(super) fn of_graph(error: &RepositoryError, sentence: impl Into<String>) -> Self {
        Self::new(category_of_graph(error), sentence)
    }

    pub(super) fn of_search(error: SearchError) -> Self {
        let category = match &error {
            SearchError::Index(index) => category_of_index(index),
            SearchError::Graph(graph) => category_of_graph(graph),
            SearchError::UnreadableCursor | SearchError::LimitOutOfRange { .. } => {
                FailureCategory::Usage
            }
        };
        Self::new(category, error.to_string())
    }

    /// A graph view that did not come out.
    pub(super) fn of_graph_view(error: GraphError) -> Self {
        let category = match &error {
            GraphError::FocusNotFound => FailureCategory::NotFound,
            GraphError::Graph(graph) => category_of_graph(graph),
        };
        Self::new(category, error.to_string())
    }

    pub(super) fn of_index(error: SearchIndexError) -> Self {
        Self::new(category_of_index(&error), error.to_string())
    }
}

/// Every command still returns its failure as the sentence it always returned.
impl From<Refusal> for String {
    fn from(refusal: Refusal) -> Self {
        refusal.sentence
    }
}

fn category_of_workspace(error: &WorkspaceError) -> FailureCategory {
    match error {
        WorkspaceError::NoWorkspaceConfigured => FailureCategory::NoContext,
        WorkspaceError::AmbiguousWorkspace { .. } => FailureCategory::ContextRequired,
        WorkspaceError::UnknownWorkspace { .. } => FailureCategory::UnknownContext,
        WorkspaceError::EmptyName
        | WorkspaceError::NameTooLong { .. }
        | WorkspaceError::NameNotAPlainName(_)
        | WorkspaceError::EgressForUnknownWorkspace(_)
        | WorkspaceError::OverlappingCoverage { .. } => FailureCategory::Configuration,
    }
}

fn category_of_graph(error: &RepositoryError) -> FailureCategory {
    match error {
        RepositoryError::GraphBusy => FailureCategory::Busy,
        RepositoryError::SchemaNotThisBuild { .. } => FailureCategory::NeedsMigration,
        RepositoryError::GraphLogUnreadable { .. } => FailureCategory::NeedsRecovery,
        RepositoryError::Storage(_)
        | RepositoryError::InvariantViolation(_)
        | RepositoryError::StorageExhausted(_)
        | RepositoryError::RollbackFailed(_) => FailureCategory::Failed,
    }
}

fn category_of_index(error: &SearchIndexError) -> FailureCategory {
    match error {
        SearchIndexError::Busy => FailureCategory::Busy,
        SearchIndexError::UnreadableQuery => FailureCategory::UnreadableQuery,
        SearchIndexError::Unreadable(_) => FailureCategory::IndexUnreadable,
        SearchIndexError::Unavailable(_) => FailureCategory::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::{FailureCategory, Refusal};
    use crate::application::{RepositoryError, SearchError, SearchIndexError};

    /// The two failures a reader beside `index watch` meets most: the graph held by the pass,
    /// and the index held by it. Both are the same thing to the person waiting.
    #[test]
    fn a_held_graph_and_a_held_index_are_both_busy() {
        assert_eq!(
            Refusal::of_graph(&RepositoryError::GraphBusy, "held").category(),
            FailureCategory::Busy
        );
        assert_eq!(
            Refusal::of_search(SearchError::Index(SearchIndexError::Busy)).category(),
            FailureCategory::Busy
        );
    }

    /// A query the index cannot read is the person's to fix, not the program's: it has a
    /// category of its own rather than `failed`.
    #[test]
    fn an_unreadable_query_is_not_a_failure_of_the_program() {
        let refusal = Refusal::of_search(SearchError::Index(SearchIndexError::UnreadableQuery));
        assert_eq!(refusal.category(), FailureCategory::UnreadableQuery);
        assert_eq!(
            String::from(refusal),
            SearchIndexError::UnreadableQuery.to_string(),
            "the sentence a person reads is the one it always was"
        );
    }

    #[test]
    fn a_cursor_or_a_page_size_the_command_does_not_take_is_usage() {
        assert_eq!(
            Refusal::of_search(SearchError::UnreadableCursor).category(),
            FailureCategory::Usage
        );
        assert_eq!(
            Refusal::of_search(SearchError::LimitOutOfRange {
                asked: 0,
                most: 100
            })
            .category(),
            FailureCategory::Usage
        );
    }
}
