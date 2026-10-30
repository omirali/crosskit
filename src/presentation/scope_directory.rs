// A directory named on the command line, as a scope of work: resolved the way the scan
// resolves its roots, and refused when the context does not cover it. One reading for the two
// commands that take one — `index jobs run` and `index review approve`.
use std::path::{Path, PathBuf};

use crate::application::WorkScope;
use crate::domain::{NormalizedPath, WorkspaceName};
use crate::infrastructure::LoadedIndexPolicy;

/// A directory as typed, and which of the two questions is asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AskedScope {
    Under(String),
    Touching(String),
}

/// The directory as the graph spells paths — absolute and resolved, as the scan resolves its
/// roots — and refused when this context does not cover it: no file the context holds lies
/// under it, so the run would select nothing and could only mislead.
pub(super) fn resolve(
    asked: &AskedScope,
    loaded: &LoadedIndexPolicy,
    context: &WorkspaceName,
) -> Result<WorkScope, String> {
    let typed = match asked {
        AskedScope::Under(directory) | AskedScope::Touching(directory) => directory,
    };
    let absolute = match Path::new(typed).is_absolute() {
        true => PathBuf::from(typed),
        false => std::env::current_dir()
            .map_err(|error| format!("the current directory cannot be read: {error}"))?
            .join(typed),
    };
    let resolved = std::fs::canonicalize(&absolute)
        .map_err(|error| format!("{typed:?} cannot be read as a directory: {error}"))?;
    let directory = NormalizedPath::from_absolute(&resolved.to_string_lossy())
        .map_err(|error| format!("{typed:?} is not a path this program reads: {error}"))?;
    if !covers(loaded, context, &directory) {
        return Err(format!(
            "{} is not in context {context}: no root of it lies there. `crosskit status` lists the roots of every context",
            directory.as_str()
        ));
    }
    Ok(match asked {
        AskedScope::Under(_) => WorkScope::Under(directory),
        AskedScope::Touching(_) => WorkScope::Touching(directory),
    })
}

/// Whether the directory lies in the context, or holds one of its roots.
fn covers(loaded: &LoadedIndexPolicy, context: &WorkspaceName, directory: &NormalizedPath) -> bool {
    if loaded.workspaces().workspace_of(directory) == Some(context) {
        return true;
    }
    let inside = format!("{}/", directory.as_str().trim_end_matches('/'));
    loaded
        .workspaces()
        .roots_of(context)
        .iter()
        .filter_map(|root| root.directory())
        .any(|root| root.starts_with(&inside))
}
