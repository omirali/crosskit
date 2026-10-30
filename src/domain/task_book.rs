// The owner's decisions about tasks: which conversations make a task, what it is called, which
// conversation does not belong, which candidate is not a task at all (ADR 0053).
//
// Stitching proposes; the owner decides, and a decision outlives every recomputation. A
// conversation dropped from a task does not come back when stitching ties it again, and a
// rejected candidate is not proposed again. What stitching ties to a task after it was
// confirmed is proposed for that task, not added to it.
//
// Conversations are named by session, the one key that does not change as a transcript grows.
//
// The book also keeps the owner's word on the task's risks and whether the task is done
// (ADR 0061). A task with an open critical risk is not marked done: the risk is accepted or
// resolved first, with a reason (`docs/decision-map.md`, section 5).
use std::collections::BTreeSet;

use super::risk_decision::{RiskAnchor, RiskDecision};

/// A task the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedTask {
    id: String,
    name: String,
    members: BTreeSet<String>,
    dropped: BTreeSet<String>,
    risk_decisions: Vec<RiskDecision>,
    finished_at_unix_ms: Option<i64>,
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

    /// The owner's word on the task's risks, one per anchor.
    pub fn risk_decisions(&self) -> &[RiskDecision] {
        &self.risk_decisions
    }

    /// When the owner marked the task done; nothing while it is not.
    pub fn finished_at_unix_ms(&self) -> Option<i64> {
        self.finished_at_unix_ms
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
    /// A decision on a risk keeps why it was taken.
    EmptyReason,
    /// The owner never decided on the risk at this place, so there is nothing to reopen.
    NoRiskDecision { task: String, anchor: RiskAnchor },
    /// The task still has open critical risks; they are accepted or resolved first.
    OpenCriticalRisks { task: String, count: usize },
}

/// A task as its store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredTask {
    pub id: String,
    pub name: String,
    pub members: BTreeSet<String>,
    pub dropped: BTreeSet<String>,
    pub risk_decisions: Vec<RiskDecision>,
    pub finished_at_unix_ms: Option<i64>,
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
    pub fn restored(tasks: Vec<RestoredTask>, rejected: Vec<BTreeSet<String>>) -> Self {
        Self {
            tasks: tasks
                .into_iter()
                .map(|task| ConfirmedTask {
                    id: task.id,
                    name: task.name,
                    members: task.members,
                    dropped: task.dropped,
                    risk_decisions: task.risk_decisions,
                    finished_at_unix_ms: task.finished_at_unix_ms,
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

    /// The task called `reference`, by its name or id.
    pub fn task(&self, reference: &str) -> Option<&ConfirmedTask> {
        self.tasks.iter().find(|task| task.answers_to(reference))
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
            risk_decisions: Vec::new(),
            finished_at_unix_ms: None,
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
    /// Keeps the owner's word on the risk at `decision.anchor`, replacing an earlier one there.
    pub fn decide_risk(&mut self, task: &str, decision: RiskDecision) -> Result<(), TaskBookError> {
        if decision.reason.trim().is_empty() {
            return Err(TaskBookError::EmptyReason);
        }
        let index = self.index_of(task)?;
        if !self.tasks[index].members.contains(&decision.anchor.session) {
            return Err(TaskBookError::NotInTask {
                session: decision.anchor.session.clone(),
                task: self.tasks[index].name.clone(),
            });
        }
        let decisions = &mut self.tasks[index].risk_decisions;
        decisions.retain(|kept| kept.anchor != decision.anchor);
        decisions.push(RiskDecision {
            reason: decision.reason.trim().to_string(),
            ..decision
        });
        Ok(())
    }

    /// Takes the owner's word on the risk at `anchor` back: the risk is the model's again.
    pub fn reopen_risk(&mut self, task: &str, anchor: &RiskAnchor) -> Result<(), TaskBookError> {
        let index = self.index_of(task)?;
        let target = &mut self.tasks[index];
        let before = target.risk_decisions.len();
        target.risk_decisions.retain(|kept| &kept.anchor != anchor);
        if target.risk_decisions.len() == before {
            return Err(TaskBookError::NoRiskDecision {
                task: target.name.clone(),
                anchor: anchor.clone(),
            });
        }
        Ok(())
    }

    /// Marks the task done, unless `open_critical` of its risks are still open and critical.
    pub fn finish(
        &mut self,
        task: &str,
        at_unix_ms: i64,
        open_critical: usize,
    ) -> Result<(), TaskBookError> {
        let index = self.index_of(task)?;
        let target = &mut self.tasks[index];
        if open_critical > 0 {
            return Err(TaskBookError::OpenCriticalRisks {
                task: target.name.clone(),
                count: open_critical,
            });
        }
        target.finished_at_unix_ms.get_or_insert(at_unix_ms);
        Ok(())
    }

    /// Takes the task back into work.
    pub fn resume(&mut self, task: &str) -> Result<(), TaskBookError> {
        let index = self.index_of(task)?;
        self.tasks[index].finished_at_unix_ms = None;
        Ok(())
    }

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
    use crate::domain::risk_decision::RiskVerdict;

    fn accepted(session: &str, reply: usize, reason: &str, at_unix_ms: i64) -> RiskDecision {
        RiskDecision {
            anchor: RiskAnchor {
                session: session.into(),
                reply,
            },
            verdict: RiskVerdict::Accepted,
            reason: reason.into(),
            at_unix_ms,
        }
    }

    #[test]
    fn a_risk_decision_keeps_its_reason_replaces_the_earlier_one_and_can_be_taken_back() {
        let mut book = TaskBook::default();
        book.confirm("Скидки", set(&["a"])).unwrap();
        assert_eq!(
            book.decide_risk("Скидки", accepted("a", 3, "  ", 1)),
            Err(TaskBookError::EmptyReason)
        );
        book.decide_risk("Скидки", accepted("a", 3, "есть снапшот", 1))
            .unwrap();
        book.decide_risk("Скидки", accepted("a", 3, " снапшот за ночь ", 2))
            .unwrap();
        let decisions = book.tasks()[0].risk_decisions();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].reason, "снапшот за ночь");

        let anchor = decisions[0].anchor.clone();
        book.reopen_risk("Скидки", &anchor).unwrap();
        assert!(book.tasks()[0].risk_decisions().is_empty());
        assert!(matches!(
            book.reopen_risk("Скидки", &anchor),
            Err(TaskBookError::NoRiskDecision { .. })
        ));
    }

    /// The concept's rule: a task with an open critical risk is not quietly done.
    #[test]
    fn a_task_with_open_critical_risks_is_not_marked_done() {
        let mut book = TaskBook::default();
        book.confirm("Скидки", set(&["a"])).unwrap();
        assert_eq!(
            book.finish("Скидки", 5, 2),
            Err(TaskBookError::OpenCriticalRisks {
                task: "Скидки".into(),
                count: 2
            })
        );
        book.finish("Скидки", 5, 0).unwrap();
        book.finish("Скидки", 9, 0).unwrap();
        assert_eq!(
            book.tasks()[0].finished_at_unix_ms(),
            Some(5),
            "done once, when first said"
        );
        book.resume("Скидки").unwrap();
        assert_eq!(book.tasks()[0].finished_at_unix_ms(), None);
    }

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
