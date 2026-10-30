// What a reading command reads from: the configuration, and the graph of the context it was
// asked about.
//
// Two grounds exist. A command run by itself reads the configuration once and opens the
// graph for reading, as every reading command always has. The background service answers the
// same commands from the configuration it read when it started and from the graphs it
// already holds for writing — which is what lets a reader beside a pass answer without
// `busy` (ADR 0050). The commands themselves do not know which ground they stand on.
use std::sync::Arc;

use crate::domain::WorkspaceName;
use crate::infrastructure::{LadybugDatabaseManager, LoadedIndexPolicy, load_index_policy};

use super::refusal::Refusal;
use super::workspace;

pub(super) trait ReadingGround {
    fn policy(&self) -> Result<&LoadedIndexPolicy, Refusal>;
    fn graph(&self, workspace: &WorkspaceName) -> Result<Arc<LadybugDatabaseManager>, Refusal>;
}

/// A command run by itself.
pub(super) struct OwnRun {
    loaded: Result<LoadedIndexPolicy, Refusal>,
}

impl OwnRun {
    pub(super) fn load() -> Self {
        Self {
            loaded: load_index_policy().map_err(Refusal::of_configuration),
        }
    }
}

impl ReadingGround for OwnRun {
    fn policy(&self) -> Result<&LoadedIndexPolicy, Refusal> {
        self.loaded.as_ref().map_err(Clone::clone)
    }

    fn graph(&self, workspace: &WorkspaceName) -> Result<Arc<LadybugDatabaseManager>, Refusal> {
        workspace::open_for_reading_or_refuse(workspace)
    }
}
