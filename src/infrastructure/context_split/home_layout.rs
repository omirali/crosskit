// Where the measurement looks: the owner's home directory, named in the command that runs it.
//
// The path is the machine's, not the program's, so it is asked for at run time and never
// written down (`.claude/rules/00-core.md`, "Ничего личного в исходниках"). Without it the
// measurement says what to set and measures nothing.
use std::path::Path;

use crate::domain::NormalizedPath;

/// The variable naming the home directory to measure.
pub(super) const HOME_VARIABLE: &str = "CROSSKIT_SPLIT_HOME";
/// The variable naming the owner's own labels, a file outside the repository.
pub(super) const TRUTH_VARIABLE: &str = "CROSSKIT_SPLIT_TRUTH";

/// The directories under a home that hold projects, in the order they are walked: the three
/// the stage named, and the inbox beside them, which is where downloaded projects land.
const PROJECT_ROOTS: [&str; 4] = ["Projects", "Documents", "Desktop", "Downloads"];
/// The directories whose children are document folders.
const DOCUMENT_ROOTS: [&str; 3] = ["Documents", "Desktop", "Downloads"];
/// The directory whose children are projects, and whose non-repository children are folders
/// without git like any other.
const PROJECTS: &str = "Projects";

pub(super) struct HomeLayout {
    home: NormalizedPath,
}

impl HomeLayout {
    pub(super) fn from_environment() -> Result<Self, String> {
        let Some(value) = std::env::var_os(HOME_VARIABLE) else {
            return Err(format!(
                "{HOME_VARIABLE} не задан: назовите домашний каталог, который мерить, \
                 например {HOME_VARIABLE}=$HOME. Ничего не измерено."
            ));
        };
        let text = value
            .into_string()
            .map_err(|_| format!("{HOME_VARIABLE} не в UTF-8. Ничего не измерено."))?;
        let resolved = std::fs::canonicalize(&text)
            .map_err(|error| format!("{HOME_VARIABLE}={text}: {error}. Ничего не измерено."))?;
        let resolved = resolved
            .to_str()
            .ok_or_else(|| format!("{HOME_VARIABLE} не в UTF-8. Ничего не измерено."))?;
        let home = NormalizedPath::from_absolute(resolved)
            .map_err(|error| format!("{HOME_VARIABLE}: {error}. Ничего не измерено."))?;
        Ok(Self { home })
    }

    pub(super) fn home(&self) -> &NormalizedPath {
        &self.home
    }

    /// A path under the home, `relative` given with `/` between its segments.
    pub(super) fn under(&self, relative: &str) -> NormalizedPath {
        let joined = format!("{}/{relative}", self.home.as_str());
        NormalizedPath::from_absolute(&joined).unwrap_or_else(|_| self.home.clone())
    }

    pub(super) fn project_roots(&self) -> Vec<NormalizedPath> {
        existing(PROJECT_ROOTS.iter().map(|name| self.under(name)))
    }

    pub(super) fn document_roots(&self) -> Vec<NormalizedPath> {
        existing(DOCUMENT_ROOTS.iter().map(|name| self.under(name)))
    }

    pub(super) fn projects_directory(&self) -> NormalizedPath {
        self.under(PROJECTS)
    }

    pub(super) fn transcripts(&self) -> NormalizedPath {
        self.under(".claude/projects")
    }

    pub(super) fn codex_sessions(&self) -> NormalizedPath {
        self.under(".codex/sessions")
    }

    /// The two places git reads a global identity from.
    pub(super) fn global_git_configs(&self) -> [NormalizedPath; 2] {
        [self.under(".gitconfig"), self.under(".config/git/config")]
    }
}

fn existing(paths: impl Iterator<Item = NormalizedPath>) -> Vec<NormalizedPath> {
    paths
        .filter(|path| Path::new(path.as_str()).is_dir())
        .collect()
}
