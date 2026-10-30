// What the measurement reads and where it may write, named in the command that runs it.
//
// The home directory is the machine's, not the program's, so it is asked for at run time and
// never written down (`.claude/rules/00-core.md`, «Ничего личного в исходниках»). Without it the
// measurement says what to set and measures nothing.
use std::path::{Path, PathBuf};

use crate::infrastructure::outside_repository::refuse_inside_the_repository;

const HOME_VARIABLE: &str = "CROSSKIT_STITCH_HOME";
/// The session whose candidate task the report describes member by member.
const SEED_VARIABLE: &str = "CROSSKIT_STITCH_SEED";
/// Where the owner's list of the seed task goes: a file outside the repository, because it
/// names real conversations.
const LIST_VARIABLE: &str = "CROSSKIT_STITCH_LIST";

pub(super) struct Layout {
    pub(super) home: String,
    pub(super) seed: Option<String>,
    pub(super) list: Option<PathBuf>,
}

impl Layout {
    pub(super) fn from_environment() -> Result<Self, String> {
        let Some(home) = std::env::var_os(HOME_VARIABLE) else {
            return Err(format!(
                "{HOME_VARIABLE} не задан: назовите домашний каталог, который мерить, например \
                 {HOME_VARIABLE}=$HOME. Ничего не измерено."
            ));
        };
        let home = std::fs::canonicalize(&home)
            .map_err(|error| format!("{HOME_VARIABLE}: {error}. Ничего не измерено."))?;
        let home = home
            .into_os_string()
            .into_string()
            .map_err(|_| format!("{HOME_VARIABLE} не в UTF-8. Ничего не измерено."))?;
        let list = std::env::var_os(LIST_VARIABLE).map(PathBuf::from);
        if let Some(list) = &list {
            refuse_inside_the_repository(list, LIST_VARIABLE)?;
        }
        Ok(Self {
            home,
            seed: std::env::var(SEED_VARIABLE).ok(),
            list,
        })
    }

    pub(super) fn claude_projects(&self) -> PathBuf {
        Path::new(&self.home).join(".claude/projects")
    }

    /// Codex keeps live sessions and archived ones apart; both are sessions.
    pub(super) fn codex_session_roots(&self) -> [PathBuf; 2] {
        let codex = Path::new(&self.home).join(".codex");
        [codex.join("sessions"), codex.join("archived_sessions")]
    }
}
