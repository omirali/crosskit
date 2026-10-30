// `crosskit contexts propose <root>`: walk a tree and print a draft of the contexts it
// suggests, with the evidence behind every line of it.
//
// The command reads and prints. It writes no configuration, opens no database and builds no
// index, so what it prints is a proposal in the literal sense: the operator pastes the part
// they agree with. `crosskit setup` walks the same way and asks the person instead of
// drafting by host (ADR 0042).
use std::path::{Path, PathBuf};

use crate::application::ContextProposalService;
use crate::domain::{ContextProposal, NormalizedPath, TreeEvidence};
use crate::infrastructure::{DescriptorRelativeWalker, load_index_policy};

use super::context_draft;
use super::help;
use super::machine_answer;
use super::refusal::Refusal;

pub(super) const USAGE: &str =
    "Usage: crosskit contexts propose <directory> | crosskit contexts --json";

pub fn run(arguments: &[String]) -> Result<(), String> {
    if let [flag] = arguments
        && flag == machine_answer::FLAG
    {
        return list_for_a_program();
    }
    let [command, root] = arguments else {
        return Err(help::misuse(USAGE));
    };
    if command != "propose" {
        return Err(help::misuse(USAGE));
    }

    let proposal = propose(&PathBuf::from(root))?;
    match context_draft::render(&proposal) {
        Some(draft) => print!("{draft}"),
        None => print!("{}", context_draft::render_empty(&proposal)),
    }
    Ok(())
}

/// The contexts the configuration declares, as the machine answer: what an application offers
/// to switch between. Read from the configuration alone — a context is its own database
/// (ADR 0008), and naming them opens none of them.
fn list_for_a_program() -> Result<(), String> {
    match load_index_policy() {
        Ok(loaded) => {
            machine_answer::print_contexts(loaded.workspaces().names());
            Ok(())
        }
        Err(error) => {
            let refusal = Refusal::of_configuration(error);
            machine_answer::print_refusal(&refusal, None);
            Err(refusal.into())
        }
    }
}

/// The draft for one directory, walked the way the index walks.
pub(super) fn propose(root: &Path) -> Result<ContextProposal, String> {
    let walker = DescriptorRelativeWalker;
    let home = home_directory();
    ContextProposalService::new(&walker)
        .execute(root, home.as_ref())
        .map_err(|error| error.to_string())
}

/// What one walk of `root` saw, before any draft is made of it. `setup` groups this its own
/// way, so both commands stand on one walk even though they draft differently.
pub(super) fn survey(root: &Path) -> Result<TreeEvidence, String> {
    let walker = DescriptorRelativeWalker;
    let home = home_directory();
    ContextProposalService::new(&walker)
        .evidence(root, home.as_ref())
        .map_err(|error| error.to_string())
}

/// The person's home directory as the walk would see it. It is resolved because a home
/// path can itself be a symlink, and the standard layout is only read for the children of
/// this exact directory.
pub(super) fn home_directory() -> Option<NormalizedPath> {
    let home = std::env::var_os("HOME")?;
    let resolved = std::fs::canonicalize(PathBuf::from(home)).ok()?;
    NormalizedPath::from_absolute(&resolved.to_string_lossy()).ok()
}
