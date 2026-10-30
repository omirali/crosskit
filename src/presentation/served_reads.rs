// What the background service answers on its socket: the machine answers of `search`,
// `graph`, `knowledge map`, `tasks` and `tasks map`, read from the graphs its watchers already
// hold (ADR 0050).
//
// The answer is produced by the very function the command runs by itself — only the ground
// differs — so a line from the socket is the line the CLI would print. `ask` is not here: it
// calls a model and writes the cache of its answers, and the service does not go to a model
// on anybody's behalf.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;

use crate::application::RepositoryError;
use crate::domain::WorkspaceName;
use crate::infrastructure::service::{
    PROTOCOL, Refused, ServedReads, ServiceRequest, refusal_line,
};
use crate::infrastructure::{LadybugDatabaseManager, LoadedIndexPolicy};

use super::machine_answer::{contexts_line, object_line};
use super::reading_ground::ReadingGround;
use super::refusal::Refusal;
use super::{graph, knowledge, search, task_map, tasks, workspace};

/// The graphs the service's watchers hold, by context, and the configuration they were read
/// with.
pub(super) struct HeldGraphs<'a> {
    loaded: &'a LoadedIndexPolicy,
    config_digest: Option<String>,
    held: Mutex<BTreeMap<String, Arc<LadybugDatabaseManager>>>,
}

impl<'a> HeldGraphs<'a> {
    pub(super) fn new(loaded: &'a LoadedIndexPolicy, config_digest: Option<String>) -> Self {
        Self {
            loaded,
            config_digest,
            held: Mutex::new(BTreeMap::new()),
        }
    }

    /// The watcher of `context` holds this graph from now on.
    pub(super) fn hold(&self, context: &WorkspaceName, manager: Arc<LadybugDatabaseManager>) {
        self.contexts().insert(context.to_string(), manager);
    }

    /// The watcher of `context` stopped; its graph is no longer the service's to read.
    pub(super) fn release(&self, context: &WorkspaceName) {
        self.contexts().remove(context.as_str());
    }

    fn contexts(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<LadybugDatabaseManager>>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ReadingGround for HeldGraphs<'_> {
    fn policy(&self) -> Result<&LoadedIndexPolicy, Refusal> {
        Ok(self.loaded)
    }

    /// Asked only after `answer` saw the context held; a watcher that stopped in between is
    /// a context being written by nobody the service can read through, which is `busy`.
    fn graph(&self, context: &WorkspaceName) -> Result<Arc<LadybugDatabaseManager>, Refusal> {
        self.contexts()
            .get(context.as_str())
            .cloned()
            .ok_or_else(|| {
                Refusal::of_graph(
                    &RepositoryError::GraphBusy,
                    format!("the service stopped watching context {context}"),
                )
            })
    }
}

/// A reading command's machine answer, on a ground: what `search`, `graph` and `knowledge`
/// each offer as `machine_line`.
type MachineLine = fn(&[String], Option<&str>, &dyn ReadingGround) -> (String, Result<(), Refusal>);

#[derive(Serialize)]
struct Hello<'a> {
    protocol: u32,
    schema: u32,
    pid: u32,
    /// The contexts the service answers for right now.
    served: Vec<String>,
    contexts: Vec<&'a str>,
}

impl ServedReads for HeldGraphs<'_> {
    fn answer(&self, request: &ServiceRequest) -> String {
        if request.protocol != PROTOCOL {
            return refusal_line(Refused::ProtocolMismatch);
        }
        if request.config_digest.is_some() && request.config_digest != self.config_digest {
            return refusal_line(Refused::ConfigurationChanged);
        }
        let names = self.loaded.workspaces().names();
        let reading: MachineLine = match request.op.as_str() {
            "hello" => {
                return object_line(&Hello {
                    protocol: PROTOCOL,
                    schema: 2,
                    pid: std::process::id(),
                    served: self.contexts().keys().cloned().collect(),
                    contexts: names.iter().map(WorkspaceName::as_str).collect(),
                });
            }
            "contexts" => return contexts_line(names),
            "search" => search::machine_line,
            "graph" => graph::machine_line,
            "knowledge-map" => knowledge::machine_line,
            "tasks" => tasks::machine_line,
            "task-speech" => task_map::speech_line,
            "task-map" => task_map::machine_line,
            _ => return refusal_line(Refused::Malformed),
        };
        // A context the configuration names and the service does not hold right now is the
        // client's to read: it is starting, waiting for another writer, or stopped. One the
        // configuration does not name is answered here, by the command's own refusal.
        let Ok(chosen) = workspace::choose(self.loaded, request.context.as_deref()) else {
            return reading(&request.arguments, request.context.as_deref(), self).0;
        };
        let Some(graph) = self.contexts().get(chosen.as_str()).cloned() else {
            return refusal_line(Refused::NotServed);
        };
        // Held for the whole question: no commit and no checkpoint of this graph runs while
        // the question reads it (`DatabaseOwner::readers`).
        let _reading = graph.reading();
        reading(&request.arguments, request.context.as_deref(), self).0
    }
}
