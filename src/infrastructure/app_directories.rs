// Crosskit's local directory: where the graph database and the policy file live.
use std::env;
use std::fmt;
use std::path::{Path, PathBuf};

/// The variable that names the data directory outright.
pub const DATA_DIRECTORY_VARIABLE: &str = "CROSSKIT_DATA_DIR";

/// The default directory's last component, under this name and under the one the product had
/// until 2026-10-02.
#[cfg(target_os = "macos")]
const DIRECTORY_NAME: &str = "Crosskit";
#[cfg(target_os = "macos")]
const EARLIER_DIRECTORY_NAME: &str = "Crosscode";
#[cfg(not(target_os = "macos"))]
const DIRECTORY_NAME: &str = "crosskit";
#[cfg(not(target_os = "macos"))]
const EARLIER_DIRECTORY_NAME: &str = "crosscode";

/// `CROSSKIT_DATA_DIR` overrides the platform default: `~/Library/Application
/// Support/Crosskit` on macOS, `$XDG_DATA_HOME/crosskit` or `~/.local/share/crosskit`
/// elsewhere.
///
/// A default directory that is absent beside one under the earlier name is refused rather
/// than created: starting empty would look like a lost index, and moving two gigabytes is a
/// step the owner takes by asking for it.
pub fn data_directory() -> Result<PathBuf, DirectoryError> {
    if let Some(path) = env::var_os(DATA_DIRECTORY_VARIABLE) {
        return Ok(PathBuf::from(path));
    }
    let rename = earlier_name_rename_at(&home()?);
    if !rename.to.exists() && rename.from.exists() {
        return Err(DirectoryError::NotMovedFromEarlierName {
            from: rename.from,
            to: rename.to,
        });
    }
    Ok(rename.to)
}

/// The default directory under the earlier name and the one it is renamed to — siblings in
/// one parent, which is why a rename between them is a single `rename(2)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlierNameRename {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// The rename of the default data directory, or why there is no default directory to rename.
pub fn earlier_name_rename() -> Result<EarlierNameRename, DirectoryError> {
    if env::var_os(DATA_DIRECTORY_VARIABLE).is_some() {
        return Err(DirectoryError::NamedByVariable);
    }
    Ok(earlier_name_rename_at(&home()?))
}

fn earlier_name_rename_at(home: &Path) -> EarlierNameRename {
    let parent = platform_data_parent(home);
    EarlierNameRename {
        from: parent.join(EARLIER_DIRECTORY_NAME),
        to: parent.join(DIRECTORY_NAME),
    }
}

/// The home directory of the person running the program: what `~/` in a path they wrote
/// stands for.
pub fn home_directory() -> Result<PathBuf, DirectoryError> {
    home()
}

fn home() -> Result<PathBuf, DirectoryError> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(DirectoryError::HomeNotSet)
}

#[cfg(target_os = "macos")]
fn platform_data_parent(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support")
}

#[cfg(not(target_os = "macos"))]
fn platform_data_parent(home: &Path) -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("share"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryError {
    HomeNotSet,
    /// The default directory does not exist and the one under the earlier name does.
    NotMovedFromEarlierName {
        from: PathBuf,
        to: PathBuf,
    },
    /// `CROSSKIT_DATA_DIR` names the directory, so there is no default one to rename.
    NamedByVariable,
}

impl fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeNotSet => write!(f, "HOME is not set; set {DATA_DIRECTORY_VARIABLE}"),
            Self::NotMovedFromEarlierName { from, to } => write!(
                f,
                "the data is still in {} under the product's earlier name and {} does not exist; \
                 `crosskit migrate --from-crosscode` moves it there in one rename",
                from.display(),
                to.display()
            ),
            Self::NamedByVariable => write!(
                f,
                "{DATA_DIRECTORY_VARIABLE} names the data directory, so there is no default \
                 directory to move; unset it, or keep using the directory it names"
            ),
        }
    }
}

impl std::error::Error for DirectoryError {}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::earlier_name_rename_at;

    /// The two directories are siblings: that is what makes the move one rename on one volume
    /// rather than a copy.
    #[test]
    fn the_earlier_directory_and_the_new_one_share_a_parent() {
        let rename = earlier_name_rename_at(Path::new("/Users/alice"));
        assert_eq!(rename.from.parent(), rename.to.parent());
        assert_ne!(rename.from, rename.to);
    }
}
