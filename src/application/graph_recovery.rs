// Use case: make a context that a stopped writer left unopenable open again.
//
// The storage engine folds committed batches into the database only when it checkpoints,
// and a write-ahead log that ends where its writer was stopped is one it refuses to
// replay. Every command on that context then fails, and nothing about the graph is
// readable — this is the one repair that changes that.
//
// What the repair may cost is bounded by what the graph is: every node in it is derived
// from a file on disk, so the work the stopped pass had committed is time, not data. The
// decision here is therefore narrow — move the logs out of the way while the engine says
// they are why it will not open, and never when it says anything else.
//
// The logs go in steps, and the engine is asked after each. A writer stopped in a pass
// leaves the ordinary log; one stopped inside a checkpoint leaves the log being folded in
// and its shadow pages (2026-10-30, ADR 0060). The engine finishes a checkpoint whose log is
// whole by itself, so the checkpoint's files are moved only when moving the ordinary log
// did not make the graph open.
use std::path::PathBuf;

use super::repository::RepositoryError;

/// What a recovery is allowed to do to a graph.
///
/// There is no operation for deleting anything, and that is the point: the log holds the
/// only copy of what the stopped pass wrote, unreadable as it is, and a program that
/// cannot delete it cannot delete it by mistake either.
pub trait GraphMaintenance {
    /// Opens the graph for writing and lets it go again, answering only whether it opened.
    ///
    /// For writing, because a graph another process is writing to must come back as busy
    /// rather than as broken: the two states look alike from a reader and are opposite in
    /// what they ask of an operator.
    fn probe(&self) -> Result<(), RepositoryError>;

    /// Moves the next of the logs beside the graph to names of their own inside the same
    /// directory, and answers with where they went: the ordinary write-ahead log first, and
    /// once it is gone, an interrupted checkpoint's log with its shadow pages. Nothing when
    /// no log is left.
    fn move_log_aside(&self) -> Result<Vec<PathBuf>, RepositoryError>;
}

/// What a recovery did, in the words of what changed on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryReport {
    /// The graph opened as it was. Nothing was moved, because nothing was in the way.
    NothingToRecover,
    /// The logs were moved aside and the graph opens again.
    Recovered { logs_moved_to: Vec<PathBuf> },
    /// The logs were moved aside and the graph still does not open. Both halves are
    /// reported: the operator has to know that the files moved, and why it did not help.
    MovedButStillClosed {
        logs_moved_to: Vec<PathBuf>,
        failure: RepositoryError,
    },
}

pub struct GraphRecoveryService<'a, Maintenance> {
    maintenance: &'a Maintenance,
}

impl<'a, Maintenance: GraphMaintenance> GraphRecoveryService<'a, Maintenance> {
    pub fn new(maintenance: &'a Maintenance) -> Self {
        Self { maintenance }
    }

    /// Looks first and acts only on the one failure this repair is for.
    ///
    /// A graph that opens is left alone. A graph another process is writing to is left
    /// alone and reported as busy — moving the log of a live writer would take away the
    /// work it is in the middle of. Any other failure is reported unchanged, because
    /// moving a file cannot be the answer to a question nobody asked.
    pub fn execute(&self) -> Result<RecoveryReport, RepositoryError> {
        match self.maintenance.probe() {
            Ok(()) => return Ok(RecoveryReport::NothingToRecover),
            Err(RepositoryError::GraphLogUnreadable { .. }) => {}
            Err(other) => return Err(other),
        }

        let mut logs_moved_to: Vec<PathBuf> = Vec::new();
        let mut refused: Option<RepositoryError> = None;
        // Every step moves files that are then no longer there, so the steps run out.
        loop {
            let moved = self.maintenance.move_log_aside()?;
            if moved.is_empty() {
                let Some(failure) = refused else {
                    return Err(RepositoryError::Storage(
                        "the graph did not open because of its write-ahead log, and that log is no longer there; another process may be recovering this context at the same time".to_string(),
                    ));
                };
                return Ok(RecoveryReport::MovedButStillClosed {
                    logs_moved_to,
                    failure,
                });
            }
            logs_moved_to.extend(moved);
            match self.maintenance.probe() {
                Ok(()) => return Ok(RecoveryReport::Recovered { logs_moved_to }),
                Err(failure @ RepositoryError::GraphLogUnreadable { .. }) => {
                    refused = Some(failure)
                }
                Err(failure) => {
                    return Ok(RecoveryReport::MovedButStillClosed {
                        logs_moved_to,
                        failure,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use super::{GraphMaintenance, GraphRecoveryService, RecoveryReport, RepositoryError};

    /// A graph that answers the probe from a script, one answer per call, and records
    /// every move.
    struct ScriptedGraph {
        answers: RefCell<Vec<Result<(), RepositoryError>>>,
        moves: RefCell<usize>,
        /// What each step moves, in order; a step past the last moves nothing.
        steps: RefCell<Vec<Vec<PathBuf>>>,
    }

    impl ScriptedGraph {
        fn answering(answers: Vec<Result<(), RepositoryError>>) -> Self {
            Self::stepping(
                answers,
                vec![vec![PathBuf::from("/data/graph-work.lbug.wal.aside-1")]],
            )
        }

        fn stepping(answers: Vec<Result<(), RepositoryError>>, steps: Vec<Vec<PathBuf>>) -> Self {
            Self {
                answers: RefCell::new(answers),
                moves: RefCell::new(0),
                steps: RefCell::new(steps),
            }
        }

        fn without_a_log(answers: Vec<Result<(), RepositoryError>>) -> Self {
            Self::stepping(answers, Vec::new())
        }

        fn moved(&self) -> usize {
            *self.moves.borrow()
        }
    }

    impl GraphMaintenance for ScriptedGraph {
        fn probe(&self) -> Result<(), RepositoryError> {
            let mut answers = self.answers.borrow_mut();
            assert!(!answers.is_empty(), "the graph was probed once too often");
            answers.remove(0)
        }

        fn move_log_aside(&self) -> Result<Vec<PathBuf>, RepositoryError> {
            *self.moves.borrow_mut() += 1;
            let mut steps = self.steps.borrow_mut();
            Ok(if steps.is_empty() {
                Vec::new()
            } else {
                steps.remove(0)
            })
        }
    }

    fn unreadable() -> RepositoryError {
        RepositoryError::GraphLogUnreadable {
            log: PathBuf::from("/data/graph-work.lbug.wal"),
        }
    }

    #[test]
    fn a_graph_that_opens_is_left_exactly_as_it_was() {
        let graph = ScriptedGraph::answering(vec![Ok(())]);

        let report = GraphRecoveryService::new(&graph).execute().unwrap();

        assert_eq!(report, RecoveryReport::NothingToRecover);
        assert_eq!(graph.moved(), 0, "a healthy graph must not be touched");
    }

    #[test]
    fn a_log_the_engine_refuses_is_moved_aside_once_and_the_graph_opens() {
        let graph = ScriptedGraph::answering(vec![Err(unreadable()), Ok(())]);

        let report = GraphRecoveryService::new(&graph).execute().unwrap();

        assert_eq!(
            report,
            RecoveryReport::Recovered {
                logs_moved_to: vec![PathBuf::from("/data/graph-work.lbug.wal.aside-1")],
            }
        );
        assert_eq!(graph.moved(), 1);
    }

    /// A writer stopped inside a checkpoint: moving the ordinary log is not enough, and the
    /// checkpoint's own log and shadow pages go next. Each step is asked about before the
    /// next one is taken.
    #[test]
    fn an_interrupted_checkpoint_is_moved_aside_after_the_ordinary_log() {
        let checkpoint = vec![
            PathBuf::from("/data/graph-work.lbug.wal.checkpoint.aside-1"),
            PathBuf::from("/data/graph-work.lbug.shadow.aside-1"),
        ];
        let graph = ScriptedGraph::stepping(
            vec![Err(unreadable()), Err(unreadable()), Ok(())],
            vec![
                vec![PathBuf::from("/data/graph-work.lbug.wal.aside-1")],
                checkpoint.clone(),
            ],
        );

        let report = GraphRecoveryService::new(&graph).execute().unwrap();

        let mut all = vec![PathBuf::from("/data/graph-work.lbug.wal.aside-1")];
        all.extend(checkpoint);
        assert_eq!(report, RecoveryReport::Recovered { logs_moved_to: all });
        assert_eq!(graph.moved(), 2);
    }

    /// Every log moved and the engine still refuses: the moves are reported with the
    /// refusal, and nothing claims a repair.
    #[test]
    fn a_graph_still_refused_with_no_log_left_reports_what_moved() {
        let graph = ScriptedGraph::answering(vec![Err(unreadable()), Err(unreadable())]);

        let report = GraphRecoveryService::new(&graph).execute().unwrap();

        assert!(
            matches!(&report, RecoveryReport::MovedButStillClosed { logs_moved_to, .. } if logs_moved_to.len() == 1),
            "{report:?}"
        );
        assert_eq!(graph.moved(), 2);
    }

    /// The repair is for one failure. A graph somebody is writing to is not broken, and
    /// taking its log away would take away the pass in flight.
    #[test]
    fn a_graph_another_process_is_writing_to_is_reported_and_not_touched() {
        let graph = ScriptedGraph::answering(vec![Err(RepositoryError::GraphBusy)]);

        let error = GraphRecoveryService::new(&graph).execute().unwrap_err();

        assert_eq!(error, RepositoryError::GraphBusy);
        assert_eq!(graph.moved(), 0);
    }

    #[test]
    fn any_other_failure_is_reported_unchanged_and_nothing_is_moved() {
        let graph = ScriptedGraph::answering(vec![Err(RepositoryError::Storage(
            "open LadybugDB: something else entirely".to_string(),
        ))]);

        let error = GraphRecoveryService::new(&graph).execute().unwrap_err();

        assert!(matches!(error, RepositoryError::Storage(_)));
        assert_eq!(graph.moved(), 0);
    }

    /// Moving the log is not a promise that the graph opens. When it does not, both facts
    /// are reported: the file moved, and this is what the engine still says.
    #[test]
    fn a_graph_that_stays_closed_after_the_move_reports_both() {
        let graph = ScriptedGraph::answering(vec![
            Err(unreadable()),
            Err(RepositoryError::Storage("open LadybugDB: no".to_string())),
        ]);

        let report = GraphRecoveryService::new(&graph).execute().unwrap();

        assert!(matches!(report, RecoveryReport::MovedButStillClosed { .. }));
        assert_eq!(graph.moved(), 1);
    }

    /// Two recoveries at once, or a log somebody removed by hand between the two steps:
    /// the second one has nothing to move and says so instead of claiming a repair.
    #[test]
    fn a_log_that_vanished_between_the_two_steps_is_not_reported_as_a_repair() {
        let graph = ScriptedGraph::without_a_log(vec![Err(unreadable())]);

        let error = GraphRecoveryService::new(&graph).execute().unwrap_err();

        assert!(error.to_string().contains("no longer there"), "{error}");
    }
}
