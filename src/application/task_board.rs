// Use case: the tasks of one context as the owner keeps them, and the decisions that change
// them (ADR 0053).
//
// Stitching proposes candidates on every call; the owner's decisions are kept in a book beside
// the graph and laid over them. The board is both: the confirmed tasks with what stitching now
// proposes for each, and the candidates no decision has touched. A decision is taken against
// what stitching found at that moment, saved, and the board is shown again.
//
// A conversation is named by its session, or by the start of it when that is unique among the
// conversations read — a person types eight characters, not thirty-six.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::domain::{ConfirmedTask, TaskBook, TaskBookError};

use super::repository::RepositoryError;
use super::task_candidates::{
    ConversationLogReader, Stitched, TaskCandidate, TaskCandidates, TaskMember,
};

/// The fewest characters of a session that may stand for it.
const SHORTEST_SESSION_PREFIX: usize = 4;

/// Where the owner's decisions are kept.
pub trait TaskBookStore {
    /// The book, or an empty one when no decision was taken yet.
    fn load(&self) -> Result<TaskBook, TaskBookStoreError>;
    fn save(&self, book: &TaskBook) -> Result<(), TaskBookStoreError>;
}

/// Why the book could not be read or written. The sentence is for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskBookStoreError {
    Unreadable(String),
    Unwritable(String),
}

impl fmt::Display for TaskBookStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(reason) => write!(formatter, "the task book cannot be read: {reason}"),
            Self::Unwritable(reason) => {
                write!(formatter, "the task book cannot be written: {reason}")
            }
        }
    }
}

/// One decision of the owner. A conversation is named as `session` above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskDecision {
    /// Keep the candidate holding this conversation as a task — its strongly tied
    /// conversations, or the conversation alone when no candidate holds it.
    Confirm {
        session: String,
        name: String,
    },
    /// The candidate holding this conversation is not a task.
    Reject {
        session: String,
    },
    /// Add the candidate holding this conversation to a task.
    Merge {
        task: String,
        session: String,
    },
    /// Add this one conversation to a task.
    Add {
        task: String,
        session: String,
    },
    /// Take this conversation out of a task, for good.
    Drop {
        task: String,
        session: String,
    },
    Rename {
        task: String,
        name: String,
    },
}

/// Why the board could not be shown or a decision taken.
#[derive(Debug)]
pub enum TaskBoardError {
    Graph(RepositoryError),
    Store(TaskBookStoreError),
    Decision(TaskBookError),
    /// No conversation read answers to this.
    UnknownConversation(String),
    /// More than one conversation starts this way; the sessions are named.
    AmbiguousConversation(String, Vec<String>),
}

/// A confirmed task as it stands now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardTask {
    task: ConfirmedTask,
    members: Vec<TaskMember>,
    proposed: Vec<TaskMember>,
}

impl BoardTask {
    pub fn id(&self) -> &str {
        self.task.id()
    }

    pub fn name(&self) -> &str {
        self.task.name()
    }

    /// Its conversations, earliest first. A conversation the graph no longer holds is left out
    /// of this list and stays in the task.
    pub fn members(&self) -> &[TaskMember] {
        &self.members
    }

    /// What stitching ties to the task now and the owner has not decided on.
    pub fn proposed(&self) -> &[TaskMember] {
        &self.proposed
    }
}

/// The tasks of one context: the confirmed, and the candidates no decision touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    tasks: Vec<BoardTask>,
    candidates: Vec<TaskCandidate>,
}

impl Board {
    pub fn tasks(&self) -> &[BoardTask] {
        &self.tasks
    }

    pub fn candidates(&self) -> &[TaskCandidate] {
        &self.candidates
    }
}

/// The tasks of one context and the decisions on them.
pub struct TaskBoard<'a, Reader: ConversationLogReader, Store: TaskBookStore> {
    reader: &'a Reader,
    store: &'a Store,
    home: &'a str,
}

impl<'a, Reader: ConversationLogReader, Store: TaskBookStore> TaskBoard<'a, Reader, Store> {
    /// `home` is the directory `~/` in a path the owner wrote stands for.
    pub fn new(reader: &'a Reader, store: &'a Store, home: &'a str) -> Self {
        Self {
            reader,
            store,
            home,
        }
    }

    pub fn show(&self) -> Result<Board, TaskBoardError> {
        let book = self.store.load().map_err(TaskBoardError::Store)?;
        Ok(board_of(&book, &self.stitched()?))
    }

    /// Takes `decision` against what stitching finds now, keeps it, and shows the board after
    /// it.
    pub fn decide(&self, decision: &TaskDecision) -> Result<Board, TaskBoardError> {
        let mut book = self.store.load().map_err(TaskBoardError::Store)?;
        let stitched = self.stitched()?;
        apply(&mut book, &stitched, decision)?;
        self.store.save(&book).map_err(TaskBoardError::Store)?;
        Ok(board_of(&book, &stitched))
    }

    fn stitched(&self) -> Result<Stitched, TaskBoardError> {
        TaskCandidates::new(self.reader)
            .find(self.home)
            .map_err(TaskBoardError::Graph)
    }
}

fn apply(
    book: &mut TaskBook,
    stitched: &Stitched,
    decision: &TaskDecision,
) -> Result<(), TaskBoardError> {
    let decided = match decision {
        TaskDecision::Confirm { session, name } => {
            let session = session_named(book, stitched, session)?;
            let sessions = strong_of(stitched, &session);
            book.confirm(name, sessions).map(|_| ())
        }
        TaskDecision::Reject { session } => {
            let session = session_named(book, stitched, session)?;
            let sessions = candidate_holding(stitched, &session)
                .map_or_else(|| BTreeSet::from([session.clone()]), sessions_of);
            book.reject(sessions);
            Ok(())
        }
        TaskDecision::Merge { task, session } => {
            let session = session_named(book, stitched, session)?;
            book.add(task, strong_of(stitched, &session))
        }
        TaskDecision::Add { task, session } => {
            let session = session_named(book, stitched, session)?;
            book.add(task, BTreeSet::from([session]))
        }
        TaskDecision::Drop { task, session } => {
            let session = session_named(book, stitched, session)?;
            book.drop_member(task, &session)
        }
        TaskDecision::Rename { task, name } => book.rename(task, name),
    };
    decided.map_err(TaskBoardError::Decision)
}

/// The session `reference` names: itself, or the one session among those read and kept that
/// starts with it.
fn session_named(
    book: &TaskBook,
    stitched: &Stitched,
    reference: &str,
) -> Result<String, TaskBoardError> {
    let reference = reference.trim();
    let known: BTreeSet<&str> = stitched
        .conversations
        .keys()
        .map(String::as_str)
        .chain(
            book.tasks()
                .iter()
                .flat_map(|task| task.members().iter().chain(task.dropped()))
                .map(String::as_str),
        )
        .collect();
    if known.contains(reference) {
        return Ok(reference.to_string());
    }
    if reference.chars().count() < SHORTEST_SESSION_PREFIX {
        return Err(TaskBoardError::UnknownConversation(reference.to_string()));
    }
    let matching: Vec<&str> = known
        .into_iter()
        .filter(|session| session.starts_with(reference))
        .collect();
    match matching.as_slice() {
        [one] => Ok((*one).to_string()),
        [] => Err(TaskBoardError::UnknownConversation(reference.to_string())),
        many => Err(TaskBoardError::AmbiguousConversation(
            reference.to_string(),
            many.iter().map(|s| (*s).to_string()).collect(),
        )),
    }
}

fn candidate_holding<'s>(stitched: &'s Stitched, session: &str) -> Option<&'s TaskCandidate> {
    stitched.candidates.iter().find(|candidate| {
        candidate
            .members()
            .iter()
            .any(|member| member.session_id() == session)
    })
}

/// The strongly tied conversations of the candidate holding `session`, or `session` alone.
fn strong_of(stitched: &Stitched, session: &str) -> BTreeSet<String> {
    candidate_holding(stitched, session).map_or_else(
        || BTreeSet::from([session.to_string()]),
        |candidate| {
            candidate
                .members()
                .iter()
                .filter(|member| member.tie() == super::task_candidates::Tie::Strong)
                .map(|member| member.session_id().to_string())
                .collect()
        },
    )
}

fn sessions_of(candidate: &TaskCandidate) -> BTreeSet<String> {
    candidate
        .members()
        .iter()
        .map(|member| member.session_id().to_string())
        .collect()
}

fn board_of(book: &TaskBook, stitched: &Stitched) -> Board {
    let candidate_sets: Vec<BTreeSet<String>> =
        stitched.candidates.iter().map(sessions_of).collect();
    let arranged = book.arrange(&candidate_sets);
    // A proposed conversation is shown as the candidate showed it: with the signals that tied it.
    let tied: BTreeMap<&str, &TaskMember> = stitched
        .candidates
        .iter()
        .flat_map(TaskCandidate::members)
        .map(|member| (member.session_id(), member))
        .collect();
    let member = |session: &String| -> Option<TaskMember> {
        tied.get(session.as_str())
            .copied()
            .or_else(|| stitched.conversations.get(session))
            .cloned()
    };
    let earliest_first = |members: &mut Vec<TaskMember>| {
        members.sort_by_key(|member| member.first_at_unix_ms().unwrap_or(i64::MAX));
    };
    let tasks = book
        .tasks()
        .iter()
        .zip(arranged.proposed)
        .map(|(task, proposed)| {
            let mut members: Vec<TaskMember> = task
                .members()
                .iter()
                .filter_map(|session| stitched.conversations.get(session).cloned())
                .collect();
            let mut proposed: Vec<TaskMember> = proposed.iter().filter_map(member).collect();
            earliest_first(&mut members);
            earliest_first(&mut proposed);
            BoardTask {
                task: task.clone(),
                members,
                proposed,
            }
        })
        .collect();
    let candidates = arranged
        .open
        .into_iter()
        .map(|index| stitched.candidates[index].clone())
        .collect();
    Board { tasks, candidates }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::application::task_candidates::ConversationLog;
    use crate::domain::conversation::{Speaker, TouchVerb};

    const HOME: &str = "/Users/alice";
    const HOUR: i64 = 60 * 60 * 1000;

    struct Logs(Vec<ConversationLog>);

    impl ConversationLogReader for Logs {
        fn conversation_logs(&self) -> Result<Vec<ConversationLog>, RepositoryError> {
            Ok(self.0.clone())
        }
    }

    #[derive(Default)]
    struct Kept(RefCell<TaskBook>);

    impl TaskBookStore for Kept {
        fn load(&self) -> Result<TaskBook, TaskBookStoreError> {
            Ok(self.0.borrow().clone())
        }

        fn save(&self, book: &TaskBook) -> Result<(), TaskBookStoreError> {
            *self.0.borrow_mut() = book.clone();
            Ok(())
        }
    }

    /// A conversation in `payments-api` at `hour`: the owner says `words`, then writes or reads
    /// a note.
    fn log(
        session: &str,
        name: &str,
        hour: i64,
        words: &str,
        note: Option<TouchVerb>,
    ) -> ConversationLog {
        ConversationLog {
            session_id: session.into(),
            name: Some(name.into()),
            named: true,
            folder: Some("/Users/alice/.claude/projects/-w-payments-api".into()),
            replies: vec![
                (Speaker::Owner, Some(hour * HOUR)),
                (Speaker::Model, Some(hour * HOUR + 1)),
            ],
            chunks: vec![(0, name.into()), (1, words.into()), (2, "Готово.".into())],
            touches: note
                .map(|verb| {
                    (
                        "/w/payments-api/notes/handoff.md".to_string(),
                        verb,
                        Some(hour * HOUR + 2),
                    )
                })
                .into_iter()
                .collect(),
        }
    }

    /// Three conversations handing one note on, and one about something else.
    fn conversations() -> Logs {
        Logs(vec![
            log(
                "aaaa1111",
                "Двойная скидка в корзине",
                0,
                "Почему дважды?",
                Some(TouchVerb::Wrote),
            ),
            log(
                "bbbb2222",
                "Скидка: продолжение",
                30,
                "Прочитай notes/handoff.md",
                Some(TouchVerb::Read),
            ),
            log(
                "cccc3333",
                "Скидка: тесты",
                60,
                "Открой notes/handoff.md и допиши",
                Some(TouchVerb::Wrote),
            ),
            log("dddd4444", "Отчёт по продажам", 200, "Собери отчёт", None),
        ])
    }

    #[test]
    fn a_confirmed_candidate_becomes_a_task_and_leaves_the_candidates() {
        let (reader, store) = (conversations(), Kept::default());
        let board = TaskBoard::new(&reader, &store, HOME);
        assert_eq!(board.show().unwrap().candidates().len(), 1);

        let after = board
            .decide(&TaskDecision::Confirm {
                session: "bbbb".into(),
                name: "Скидки".into(),
            })
            .unwrap();
        assert!(after.candidates().is_empty());
        let members: Vec<&str> = after.tasks()[0]
            .members()
            .iter()
            .map(TaskMember::session_id)
            .collect();
        assert_eq!(members, ["aaaa1111", "bbbb2222", "cccc3333"]);
        assert_eq!(board.show().unwrap(), after);
    }

    #[test]
    fn a_dropped_conversation_stays_out_and_a_rejected_candidate_stays_away() {
        let (reader, store) = (conversations(), Kept::default());
        let board = TaskBoard::new(&reader, &store, HOME);
        board
            .decide(&TaskDecision::Confirm {
                session: "aaaa".into(),
                name: "Скидки".into(),
            })
            .unwrap();
        let after = board
            .decide(&TaskDecision::Drop {
                task: "Скидки".into(),
                session: "cccc".into(),
            })
            .unwrap();
        assert_eq!(after.tasks()[0].members().len(), 2);
        assert!(after.tasks()[0].proposed().is_empty(), "{after:?}");

        let fresh = Kept::default();
        let other = TaskBoard::new(&reader, &fresh, HOME);
        let after = other
            .decide(&TaskDecision::Reject {
                session: "cccc".into(),
            })
            .unwrap();
        assert!(after.candidates().is_empty());
        assert!(after.tasks().is_empty());
    }

    #[test]
    fn a_conversation_is_named_by_the_start_of_its_session_only_when_that_is_unique() {
        let reader = Logs(vec![
            log("abcd0001", "Первая беседа о скидках", 0, "Привет", None),
            log("abcd0002", "Вторая беседа о скидках", 30, "Привет", None),
        ]);
        let store = Kept::default();
        let board = TaskBoard::new(&reader, &store, HOME);
        let decide = |session: &str| {
            board.decide(&TaskDecision::Confirm {
                session: session.into(),
                name: session.into(),
            })
        };
        assert!(matches!(
            decide("abcd"),
            Err(TaskBoardError::AmbiguousConversation(..))
        ));
        assert!(matches!(
            decide("abc"),
            Err(TaskBoardError::UnknownConversation(..))
        ));
        assert!(decide("abcd0002").is_ok());
    }
}
