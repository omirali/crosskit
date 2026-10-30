// The owner's decisions about tasks: which conversations make a task, what it is called, which
// conversation does not belong, which candidate is not a task at all (ADR 0053).
//
// Stitching proposes; the owner decides, and a decision outlives every recomputation. A
// conversation dropped from a task does not come back when stitching ties it again, and a
// rejected candidate is not proposed again. What stitching ties to a task after it was
// confirmed is proposed for that task, not added to it.
//
// Conversations are named by session, the one key that does not change as a transcript grows.
use std::collections::BTreeSet;

/// A task the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedTask {
    id: String,
    name: String,
    members: BTreeSet<String>,
    dropped: BTreeSet<String>,
}

impl ConfirmedTask {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The sessions of the task's conversations.
    pub fn members(&self) -> &BTreeSet<String> {
        &self.members
    }

    /// Sessions the owner took out of the task. Stitching never proposes them for it again.
    pub fn dropped(&self) -> &BTreeSet<String> {
        &self.dropped
    }

    fn answers_to(&self, reference: &str) -> bool {
        self.id == reference || self.name == reference
    }
}

/// Why a decision cannot be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskBookError {
    /// A task needs a name a person can find it by.
    EmptyName,
    /// Another task is already called this.
    NameTaken(String),
    /// No task has this name or id.
    NoSuchTask(String),
    /// The conversation already belongs to another task; it has to be dropped there first.
    InAnotherTask { session: String, task: String },
    /// The conversation is not a member of this task.
    NotInTask { session: String, task: String },
    /// A task needs at least one conversation.
    NoConversations,
}

/// How the owner's decisions lay over what stitching found now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrangement {
    /// For every confirmed task, in the book's order: the sessions stitching ties to it that
    /// are neither its members nor dropped from it.
    pub proposed: Vec<BTreeSet<String>>,
    /// The candidates, by their index, that touch no confirmed task and were not rejected.
    pub open: Vec<usize>,
}

/// Every decision the owner took in one context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskBook {
    tasks: Vec<ConfirmedTask>,
    rejected: Vec<BTreeSet<String>>,
}

impl TaskBook {
    /// A book read back from storage. Nothing is checked here that a decision did not check
    /// when it was taken; a book is only ever written by decisions.
    pub fn restored(
        tasks: Vec<(String, String, BTreeSet<String>, BTreeSet<String>)>,
        rejected: Vec<BTreeSet<String>>,
    ) -> Self {
        Self {
            tasks: tasks
                .into_iter()
                .map(|(id, name, members, dropped)| ConfirmedTask {
                    id,
                    name,
                    members,
                    dropped,
                })
                .collect(),
            rejected,
        }
    }

    pub fn tasks(&self) -> &[ConfirmedTask] {
        &self.tasks
    }

    /// The sessions of every candidate the owner said is not a task.
    pub fn rejected(&self) -> &[BTreeSet<String>] {
        &self.rejected
    }

    /// The task a conversation belongs to.
    pub fn task_of(&self, session: &str) -> Option<&ConfirmedTask> {
        self.tasks
            .iter()
            .find(|task| task.members.contains(session))
    }

    /// Keeps `sessions` as a task called `name`.
    pub fn confirm(
        &mut self,
        name: &str,
        sessions: BTreeSet<String>,
    ) -> Result<&ConfirmedTask, TaskBookError> {
        let name = self.free_name(name)?;
        let first = sessions
            .iter()
            .next()
            .ok_or(TaskBookError::NoConversations)?;
        self.refuse_taken(&sessions)?;
        let id = self.free_id(first);
        self.tasks.push(ConfirmedTask {
            id,
            name,
            members: sessions,
            dropped: BTreeSet::new(),
        });
        Ok(&self.tasks[self.tasks.len() - 1])
    }

    /// Adds `sessions` to `task`. A session dropped from it before comes back: adding is the
    /// owner changing their mind.
    pub fn add(&mut self, task: &str, sessions: BTreeSet<String>) -> Result<(), TaskBookError> {
        let index = self.index_of(task)?;
        let elsewhere = BTreeSet::from_iter(
            sessions
                .iter()
                .filter(|session| !self.tasks[index].members.contains(*session))
                .cloned(),
        );
        self.refuse_taken(&elsewhere)?;
        let target = &mut self.tasks[index];
        for session in sessions {
            target.dropped.remove(&session);
            target.members.insert(session);
        }
        Ok(())
    }

    /// Takes `session` out of `task`, for good: stitching will not propose it for this task
    /// again.
    pub fn drop_member(&mut self, task: &str, session: &str) -> Result<(), TaskBookError> {
        let index = self.index_of(task)?;
        let target = &mut self.tasks[index];
        if !target.members.remove(session) {
            return Err(TaskBookError::NotInTask {
                session: session.to_string(),
                task: target.name.clone(),
            });
        }
        target.dropped.insert(session.to_string());
        Ok(())
    }

    /// Says the candidate of `sessions` is not a task: it is not proposed again.
    pub fn reject(&mut self, sessions: BTreeSet<String>) {
        if !sessions.is_empty() && !self.rejected.contains(&sessions) {
            self.rejected.push(sessions);
        }
    }

    pub fn rename(&mut self, task: &str, name: &str) -> Result<(), TaskBookError> {
        let index = self.index_of(task)?;
        if self.tasks[index].name == name.trim() {
            return Ok(());
        }
        let name = self.free_name(name)?;
        self.tasks[index].name = name;
        Ok(())
    }

    /// The decisions laid over `candidates`, each the sessions of one candidate stitching
    /// found now.
    ///
    /// A candidate that shares a conversation with a confirmed task is proposed for the task
    /// sharing the most; the rest of its conversations are what is proposed. A candidate whose
    /// conversations were all rejected together, or all left out of tasks they were dropped
    /// from, is not open. Everything else is.
    pub fn arrange(&self, candidates: &[BTreeSet<String>]) -> Arrangement {
        let mut proposed = vec![BTreeSet::new(); self.tasks.len()];
        let mut open = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let closest = self
                .tasks
                .iter()
                .enumerate()
                .map(|(task, confirmed)| (confirmed.members.intersection(candidate).count(), task))
                .filter(|(shared, _)| *shared > 0)
                .max();
            if let Some((_, task)) = closest {
                let confirmed = &self.tasks[task];
                proposed[task].extend(
                    candidate
                        .iter()
                        .filter(|session| {
                            !confirmed.members.contains(*session)
                                && !confirmed.dropped.contains(*session)
                                && self.task_of(session).is_none()
                        })
                        .cloned(),
                );
                continue;
            }
            let rejected = self.rejected.iter().any(|set| candidate.is_subset(set));
            let dropped = self
                .tasks
                .iter()
                .any(|task| candidate.is_subset(&task.dropped));
            if !rejected && !dropped {
                open.push(index);
            }
        }
        Arrangement { proposed, open }
    }

    fn index_of(&self, reference: &str) -> Result<usize, TaskBookError> {
        self.tasks
            .iter()
            .position(|task| task.answers_to(reference))
            .ok_or_else(|| TaskBookError::NoSuchTask(reference.to_string()))
    }

    fn free_name(&self, name: &str) -> Result<String, TaskBookError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(TaskBookError::EmptyName);
        }
        if self.tasks.iter().any(|task| task.name == name) {
            return Err(TaskBookError::NameTaken(name.to_string()));
        }
        Ok(name.to_string())
    }

    fn refuse_taken(&self, sessions: &BTreeSet<String>) -> Result<(), TaskBookError> {
        for session in sessions {
            if let Some(task) = self.task_of(session) {
                return Err(TaskBookError::InAnotherTask {
                    session: session.clone(),
                    task: task.name.clone(),
                });
            }
        }
        Ok(())
    }

    /// An id from the first conversation, which no rename changes; a second task started from
    /// the same conversation after the first dropped it gets a number.
    fn free_id(&self, first_session: &str) -> String {
        let base = format!("task:{first_session}");
        let taken = |id: &str| self.tasks.iter().any(|task| task.id == id);
        if !taken(&base) {
            return base;
        }
        (2..)
            .map(|n| format!("{base}:{n}"))
            .find(|id| !taken(id))
            .unwrap_or(base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(sessions: &[&str]) -> BTreeSet<String> {
        sessions.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_confirmed_task_is_found_by_its_name_or_id_and_names_are_unique() {
        let mut book = TaskBook::default();
        let id = book
            .confirm("Двойная скидка", set(&["a", "b"]))
            .unwrap()
            .id()
            .to_string();
        assert_eq!(id, "task:a");
        assert_eq!(
            book.confirm("Двойная скидка", set(&["c"])),
            Err(TaskBookError::NameTaken("Двойная скидка".into()))
        );
        book.rename(&id, "Скидки в корзине").unwrap();
        assert_eq!(book.task_of("b").unwrap().name(), "Скидки в корзине");
        assert_eq!(
            book.confirm("  ", set(&["c"])),
            Err(TaskBookError::EmptyName)
        );
    }

    #[test]
    fn a_conversation_belongs_to_one_task() {
        let mut book = TaskBook::default();
        book.confirm("Скидки", set(&["a", "b"])).unwrap();
        assert_eq!(
            book.confirm("Возвраты", set(&["b", "c"])),
            Err(TaskBookError::InAnotherTask {
                session: "b".into(),
                task: "Скидки".into()
            })
        );
        book.drop_member("Скидки", "b").unwrap();
        book.confirm("Возвраты", set(&["b", "c"])).unwrap();
    }

    #[test]
    fn what_stitching_ties_to_a_task_later_is_proposed_and_a_dropped_one_never_again() {
        let mut book = TaskBook::default();
        book.confirm("Скидки", set(&["a", "b"])).unwrap();
        book.drop_member("Скидки", "b").unwrap();
        let candidates = [set(&["a", "b", "c"]), set(&["x", "y"])];
        let arranged = book.arrange(&candidates);
        assert_eq!(arranged.proposed, [set(&["c"])]);
        assert_eq!(arranged.open, [1]);

        book.add("Скидки", set(&["c"])).unwrap();
        assert!(book.arrange(&candidates).proposed[0].is_empty());
    }

    #[test]
    fn a_rejected_candidate_is_not_proposed_again_while_it_stays_within_what_was_rejected() {
        let mut book = TaskBook::default();
        book.reject(set(&["x", "y"]));
        assert!(book.arrange(&[set(&["x", "y"])]).open.is_empty());
        assert!(book.arrange(&[set(&["x"])]).open.is_empty());
        assert_eq!(book.arrange(&[set(&["x", "y", "z"])]).open, [0]);
    }

    #[test]
    fn a_session_taken_by_another_task_is_not_proposed_for_this_one() {
        let mut book = TaskBook::default();
        book.confirm("Скидки", set(&["a", "b", "b2"])).unwrap();
        book.confirm("Возвраты", set(&["c"])).unwrap();
        let arranged = book.arrange(&[set(&["a", "b", "c", "d"])]);
        assert_eq!(arranged.proposed, [set(&["d"]), set(&[])]);
    }
}
