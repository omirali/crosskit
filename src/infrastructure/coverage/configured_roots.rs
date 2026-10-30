// The roots of every context, read from the owner's own configuration file.
//
// The audit reads the include lists itself instead of asking `LoadedIndexPolicy`, because
// the loader keeps them as a `WorkspaceMap` — the boundary that answers which context owns
// a path — and nothing in the product ever needs the list of roots back. A measurement may
// not grow the product's API to be measured, so the one thing it needs and cannot get is
// taken from the same file the product reads, and it is the only thing taken from it: the
// policy itself still comes from `load_index_policy`.
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::infrastructure::data_directory;

#[derive(Debug, Deserialize)]
struct RootsFile {
    #[serde(default)]
    workspace: BTreeMap<String, ContextSection>,
}

#[derive(Debug, Default, Deserialize)]
struct ContextSection {
    #[serde(default)]
    include: Vec<String>,
}

/// One context and the directories its configuration names, in the order the file names
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConfiguredContext {
    pub(super) name: String,
    pub(super) roots: Vec<String>,
}

pub(super) fn read_configured_contexts() -> Result<Vec<ConfiguredContext>, String> {
    let path = configuration_path()?;
    let text =
        fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let file: RootsFile =
        toml::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
    let home = env::var("HOME").ok();

    Ok(file
        .workspace
        .into_iter()
        .map(|(name, section)| ConfiguredContext {
            name,
            roots: section
                .include
                .iter()
                .map(|pattern| expand_home(pattern, home.as_deref()))
                .collect(),
        })
        .collect())
}

pub(super) fn configuration_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("CROSSKIT_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    data_directory()
        .map(|directory| directory.join("config.toml"))
        .map_err(|error| error.to_string())
}

/// `~/x` means the same directory here as it does to the loader. A pattern that is already
/// absolute is left exactly as written, including its globs: the audit reports a root it
/// cannot walk rather than guessing what the glob stands for.
fn expand_home(pattern: &str, home: Option<&str>) -> String {
    match (pattern.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => format!("{home}/{rest}"),
        _ => pattern.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::expand_home;

    #[test]
    fn a_root_under_the_home_directory_is_expanded_like_the_loader_expands_it() {
        assert_eq!(
            expand_home("~/Projects/thing", Some("/Users/alice")),
            "/Users/alice/Projects/thing"
        );
    }

    #[test]
    fn an_absolute_root_is_left_exactly_as_written() {
        assert_eq!(
            expand_home("/tree/work", Some("/Users/alice")),
            "/tree/work"
        );
        assert_eq!(
            expand_home("~other/tree", Some("/Users/alice")),
            "~other/tree"
        );
    }
}
