// Every conversation's decision map, kept in a file of its own in `maps-<context>/` beside the
// context's graph (ADR 0055).
//
// Beside the graph and not in it, for the reason the task book is (ADR 0053): the graph has one
// writer, and a map is not what was read from disk but what a model proposed and a quote proved.
// A file is replaced whole, and a file of a shape this build does not know is refused.
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::application::{MapStore, MapStoreFailure, StoredMap, StoredSynthesis, SynthesisStore};
use crate::domain::WorkspaceName;
use crate::domain::decision_map::{
    Branch, BranchState, Evidence, Fork, MapNodes, ProofTally, Risk, RiskLevel, RiskStatus,
};
use crate::domain::task_synthesis::{LineStep, NodeRef, TaskRisk, TaskSynthesis};

use super::app_directories::data_directory;
use super::whole_file::replace_whole;

/// The version of the file's shape. Raised when a field changes meaning or goes away.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct MapFile {
    version: u32,
    session: String,
    question: String,
    mapped_through: usize,
    forks: Vec<ForkEntry>,
    risks: Vec<RiskEntry>,
    kept: usize,
    no_such_reply: usize,
    quote_not_found: usize,
    input_tokens: u64,
    output_tokens: u64,
    cost_micro_usd: u64,
}

#[derive(Serialize, Deserialize)]
struct ForkEntry {
    question: String,
    reply: usize,
    quote: String,
    branches: Vec<BranchEntry>,
}

#[derive(Serialize, Deserialize)]
struct BranchEntry {
    option: String,
    state: String,
    reason: String,
    reply: usize,
    quote: String,
}

#[derive(Serialize, Deserialize)]
struct RiskEntry {
    description: String,
    level: String,
    status: String,
    in_task: bool,
    reply: usize,
    quote: String,
}

/// The maps of one context.
pub struct TaskMapFiles {
    directory: PathBuf,
}

impl TaskMapFiles {
    pub fn of_context(context: &WorkspaceName) -> Result<Self, MapStoreFailure> {
        let data = data_directory().map_err(|error| MapStoreFailure(error.to_string()))?;
        Ok(Self::at(data.join(format!("maps-{context}"))))
    }

    fn at(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn path_of(&self, session_id: &str) -> Result<PathBuf, MapStoreFailure> {
        // A session is a name the graph gave, and it is still checked: a file name is built from
        // it, and nothing but letters, digits and dashes goes into one.
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(MapStoreFailure(format!(
                "{session_id:?} is not a session a file can be named after"
            )));
        }
        Ok(self.directory.join(format!("{session_id}.json")))
    }
}

impl MapStore for TaskMapFiles {
    fn load(&self, session_id: &str) -> Result<Option<StoredMap>, MapStoreFailure> {
        let path = self.path_of(session_id)?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(MapStoreFailure(format!("{}: {error}", path.display()))),
        };
        let file: MapFile = serde_json::from_str(&text)
            .map_err(|error| MapStoreFailure(format!("{}: {error}", path.display())))?;
        if file.version != VERSION {
            return Err(MapStoreFailure(format!(
                "{} is version {} and this build reads {VERSION}",
                path.display(),
                file.version
            )));
        }
        Ok(Some(stored_of(file)))
    }

    fn save(&self, map: &StoredMap) -> Result<(), MapStoreFailure> {
        let path = self.path_of(&map.session_id)?;
        fs::create_dir_all(&self.directory)
            .map_err(|error| MapStoreFailure(format!("{}: {error}", self.directory.display())))?;
        let text = serde_json::to_string_pretty(&file_of(map))
            .map_err(|error| MapStoreFailure(error.to_string()))?;
        replace_whole(&path, text.as_bytes())
            .map_err(|error| MapStoreFailure(format!("{}: {error}", path.display())))
    }
}

#[derive(Serialize, Deserialize)]
struct SynthesisFile {
    version: u32,
    task: String,
    question: String,
    fingerprint: String,
    sessions: Vec<String>,
    main_line: Vec<StepEntry>,
    risks: Vec<TaskRiskEntry>,
    findings: Vec<TaskRiskEntry>,
    cost_micro_usd: u64,
}

#[derive(Serialize, Deserialize)]
struct StepEntry {
    title: String,
    state: String,
    nodes: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct TaskRiskEntry {
    title: String,
    level: String,
    status: String,
    nodes: Vec<String>,
}

impl SynthesisStore for TaskMapFiles {
    fn load_synthesis(&self, task_id: &str) -> Result<Option<StoredSynthesis>, MapStoreFailure> {
        let path = self.path_of(&task_file_name(task_id))?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(MapStoreFailure(format!("{}: {error}", path.display()))),
        };
        let file: SynthesisFile = serde_json::from_str(&text)
            .map_err(|error| MapStoreFailure(format!("{}: {error}", path.display())))?;
        if file.version != VERSION {
            return Err(MapStoreFailure(format!(
                "{} is version {} and this build reads {VERSION}",
                path.display(),
                file.version
            )));
        }
        let codes = |codes: Vec<String>| {
            codes
                .iter()
                .filter_map(|code| NodeRef::parse(code))
                .collect()
        };
        let risks = |entries: Vec<TaskRiskEntry>| -> Vec<TaskRisk> {
            entries
                .into_iter()
                .filter_map(|entry| {
                    Some(TaskRisk {
                        title: entry.title,
                        level: RiskLevel::parse(&entry.level)?,
                        status: RiskStatus::parse(&entry.status)?,
                        nodes: codes(entry.nodes),
                    })
                })
                .collect()
        };
        Ok(Some(StoredSynthesis {
            task_id: file.task,
            version: file.question,
            fingerprint: file.fingerprint,
            sessions: file.sessions,
            synthesis: TaskSynthesis {
                main_line: file
                    .main_line
                    .into_iter()
                    .filter_map(|step| {
                        Some(LineStep {
                            title: step.title,
                            state: BranchState::parse(&step.state)?,
                            nodes: codes(step.nodes),
                        })
                    })
                    .collect(),
                risks: risks(file.risks),
                findings: risks(file.findings),
            },
            cost_micro_usd: file.cost_micro_usd,
        }))
    }

    fn save_synthesis(&self, synthesis: &StoredSynthesis) -> Result<(), MapStoreFailure> {
        let path = self.path_of(&task_file_name(&synthesis.task_id))?;
        fs::create_dir_all(&self.directory)
            .map_err(|error| MapStoreFailure(format!("{}: {error}", self.directory.display())))?;
        let codes = |nodes: &[NodeRef]| nodes.iter().map(|node| node.code()).collect();
        let risks = |risks: &[TaskRisk]| -> Vec<TaskRiskEntry> {
            risks
                .iter()
                .map(|risk| TaskRiskEntry {
                    title: risk.title.clone(),
                    level: risk.level.code().to_string(),
                    status: risk.status.code().to_string(),
                    nodes: codes(&risk.nodes),
                })
                .collect()
        };
        let file = SynthesisFile {
            version: VERSION,
            task: synthesis.task_id.clone(),
            question: synthesis.version.clone(),
            fingerprint: synthesis.fingerprint.clone(),
            sessions: synthesis.sessions.clone(),
            main_line: synthesis
                .synthesis
                .main_line
                .iter()
                .map(|step| StepEntry {
                    title: step.title.clone(),
                    state: step.state.code().to_string(),
                    nodes: codes(&step.nodes),
                })
                .collect(),
            risks: risks(&synthesis.synthesis.risks),
            findings: risks(&synthesis.synthesis.findings),
            cost_micro_usd: synthesis.cost_micro_usd,
        };
        let text = serde_json::to_string_pretty(&file)
            .map_err(|error| MapStoreFailure(error.to_string()))?;
        replace_whole(&path, text.as_bytes())
            .map_err(|error| MapStoreFailure(format!("{}: {error}", path.display())))
    }
}

/// The file a task's join is kept in: `task-<id>`, with the id's separator spelled as a dash.
fn task_file_name(task_id: &str) -> String {
    format!("task-{}", task_id.replace(':', "-"))
}

fn file_of(map: &StoredMap) -> MapFile {
    MapFile {
        version: VERSION,
        session: map.session_id.clone(),
        question: map.question_version.clone(),
        mapped_through: map.mapped_through,
        forks: map
            .nodes
            .forks
            .iter()
            .map(|fork| ForkEntry {
                question: fork.question.clone(),
                reply: fork.evidence.reply,
                quote: fork.evidence.quote.clone(),
                branches: fork
                    .branches
                    .iter()
                    .map(|branch| BranchEntry {
                        option: branch.option.clone(),
                        state: branch.state.code().to_string(),
                        reason: branch.reason.clone(),
                        reply: branch.evidence.reply,
                        quote: branch.evidence.quote.clone(),
                    })
                    .collect(),
            })
            .collect(),
        risks: map
            .nodes
            .risks
            .iter()
            .map(|risk| RiskEntry {
                description: risk.description.clone(),
                level: risk.level.code().to_string(),
                status: risk.status.code().to_string(),
                in_task: risk.in_task,
                reply: risk.evidence.reply,
                quote: risk.evidence.quote.clone(),
            })
            .collect(),
        kept: map.tally.kept,
        no_such_reply: map.tally.no_such_reply,
        quote_not_found: map.tally.quote_not_found,
        input_tokens: map.input_tokens,
        output_tokens: map.output_tokens,
        cost_micro_usd: map.cost_micro_usd,
    }
}

/// A map as kept. A node with a code this build does not name is left out: it was written by a
/// build with a closed list this one does not share.
fn stored_of(file: MapFile) -> StoredMap {
    let at = |reply, quote| Evidence { reply, quote };
    StoredMap {
        session_id: file.session,
        question_version: file.question,
        mapped_through: file.mapped_through,
        nodes: MapNodes {
            forks: file
                .forks
                .into_iter()
                .map(|fork| Fork {
                    question: fork.question,
                    evidence: at(fork.reply, fork.quote),
                    branches: fork
                        .branches
                        .into_iter()
                        .filter_map(|branch| {
                            Some(Branch {
                                option: branch.option,
                                state: BranchState::parse(&branch.state)?,
                                reason: branch.reason,
                                evidence: at(branch.reply, branch.quote),
                            })
                        })
                        .collect(),
                })
                .collect(),
            risks: file
                .risks
                .into_iter()
                .filter_map(|risk| {
                    Some(Risk {
                        description: risk.description,
                        level: RiskLevel::parse(&risk.level)?,
                        status: RiskStatus::parse(&risk.status)?,
                        in_task: risk.in_task,
                        evidence: at(risk.reply, risk.quote),
                    })
                })
                .collect(),
        },
        tally: ProofTally {
            kept: file.kept,
            no_such_reply: file.no_such_reply,
            quote_not_found: file.quote_not_found,
        },
        input_tokens: file.input_tokens,
        output_tokens: file.output_tokens,
        cost_micro_usd: file.cost_micro_usd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_comes_back_as_it_was_kept_and_a_strange_session_is_refused() {
        let directory = std::env::temp_dir().join(format!("crosskit-maps-{}", std::process::id()));
        let store = TaskMapFiles::at(directory.clone());
        assert_eq!(store.load("a1-b2").unwrap(), None);
        let map = StoredMap {
            session_id: "a1-b2".into(),
            question_version: "decision-map-v2".into(),
            mapped_through: 7,
            nodes: MapNodes {
                forks: vec![Fork {
                    question: "Где считать скидку?".into(),
                    evidence: Evidence {
                        reply: 3,
                        quote: "считать в order.rs".into(),
                    },
                    branches: vec![Branch {
                        option: "order.rs".into(),
                        state: BranchState::AgentDefault,
                        reason: "владелец не ответил".into(),
                        evidence: Evidence {
                            reply: 4,
                            quote: "оставляю в order.rs".into(),
                        },
                    }],
                }],
                risks: vec![Risk {
                    description: "тесты не запускались".into(),
                    level: RiskLevel::High,
                    status: RiskStatus::Open,
                    in_task: true,
                    evidence: Evidence {
                        reply: 6,
                        quote: "тесты не запускал".into(),
                    },
                }],
            },
            tally: ProofTally {
                kept: 3,
                no_such_reply: 0,
                quote_not_found: 1,
            },
            input_tokens: 100,
            output_tokens: 10,
            cost_micro_usd: 340_000,
        };
        store.save(&map).unwrap();
        assert_eq!(store.load("a1-b2").unwrap(), Some(map));
        assert!(store.load("../escape").is_err());
        fs::remove_dir_all(directory).expect("the temporary directory is removable");
    }
}
