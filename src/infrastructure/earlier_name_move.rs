// Moving the data directory from the product's earlier name to its current one.
//
// The directory holds every context's graph, search index and answer cache — gigabytes on a
// real machine — and everything inside it is found relative to it, so a rename is the whole
// move. It is one `rename(2)` or nothing: a copy would take minutes, need the space twice and
// leave two directories that could each be mistaken for the real one, so every case in which
// a rename cannot do it is refused instead of worked around.
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::app_directories::{DirectoryError, EarlierNameRename, earlier_name_rename};
use super::ladybug::{GraphPresence, graph_presence_at};

/// What became of the earlier directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EarlierNameMove {
    Moved(EarlierNameRename),
    /// Only the current directory exists: the move was made before.
    AlreadyMoved(EarlierNameRename),
    /// Neither exists: there is no data to move.
    NothingToMove(EarlierNameRename),
}

/// Why the directory was left where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveRefusal {
    Directory(DirectoryError),
    /// Both directories exist; which one holds the real data is not this program's guess.
    BothExist(EarlierNameRename),
    /// A process holds a graph in the earlier directory for writing.
    HeldByWriter {
        database: PathBuf,
        pid: i32,
    },
    /// Whether anybody is writing could not be asked, so nothing is moved.
    Undetermined {
        database: PathBuf,
        reason: String,
    },
    /// The earlier directory is on another volume, and a rename across volumes is a copy.
    DifferentVolumes(EarlierNameRename),
    Failed {
        rename: EarlierNameRename,
        error: String,
    },
}

/// Moves the default data directory from its earlier name, or says why it did not.
pub fn move_from_earlier_name() -> Result<EarlierNameMove, MoveRefusal> {
    let rename = earlier_name_rename().map_err(MoveRefusal::Directory)?;
    let from = match fs::symlink_metadata(&rename.from) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(if exists(&rename.to) {
                EarlierNameMove::AlreadyMoved(rename)
            } else {
                EarlierNameMove::NothingToMove(rename)
            });
        }
        Err(error) => return Err(failed(&rename, &error)),
    };
    if exists(&rename.to) {
        return Err(MoveRefusal::BothExist(rename));
    }
    refuse_held_graphs(&rename.from)?;
    let parent = parent_of(&rename)?;
    let parent_device = fs::metadata(parent)
        .map_err(|error| failed(&rename, &error))?
        .dev();
    if let Some(refusal) = volume_refusal(from.dev(), parent_device, &rename) {
        return Err(refusal);
    }
    rename_without_replacing(parent, &rename)?;
    Ok(EarlierNameMove::Moved(rename))
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn failed(rename: &EarlierNameRename, error: &io::Error) -> MoveRefusal {
    MoveRefusal::Failed {
        rename: rename.clone(),
        error: error.to_string(),
    }
}

fn parent_of(rename: &EarlierNameRename) -> Result<&Path, MoveRefusal> {
    rename.from.parent().ok_or_else(|| MoveRefusal::Failed {
        rename: rename.clone(),
        error: "the directory has no parent".to_string(),
    })
}

/// Every graph in the directory, asked who writes it without opening it.
///
/// A reader takes no lock and cannot be seen this way; a reader is a short process that
/// keeps its open descriptors through a rename, while a writer that is moved underneath
/// would go on writing a directory that no longer has the name the next command looks for.
fn refuse_held_graphs(directory: &Path) -> Result<(), MoveRefusal> {
    let entries = fs::read_dir(directory).map_err(|error| MoveRefusal::Undetermined {
        database: directory.to_path_buf(),
        reason: error.to_string(),
    })?;
    for entry in entries {
        let database = entry
            .map_err(|error| MoveRefusal::Undetermined {
                database: directory.to_path_buf(),
                reason: error.to_string(),
            })?
            .path();
        if !is_graph(&database) {
            continue;
        }
        match graph_presence_at(&database) {
            GraphPresence::HeldByWriter { pid } | GraphPresence::BeingWritten { pid } => {
                return Err(MoveRefusal::HeldByWriter { database, pid });
            }
            GraphPresence::Undetermined { reason } => {
                return Err(MoveRefusal::Undetermined { database, reason });
            }
            GraphPresence::NeverIndexed
            | GraphPresence::Ready
            | GraphPresence::StoppedUncleanly { .. } => {}
        }
    }
    Ok(())
}

/// `graph-<context>.lbug`, the name every context's database has.
fn is_graph(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("graph-") && name.ends_with(".lbug"))
}

/// The two directories share a parent, so they differ in volume only when the earlier one
/// is itself a mount point. `lstat` is what is compared: a symbolic link under the earlier
/// name lives on its parent's volume and is renamed as a link, its target untouched.
fn volume_refusal(
    from_device: u64,
    parent_device: u64,
    rename: &EarlierNameRename,
) -> Option<MoveRefusal> {
    (from_device != parent_device).then(|| MoveRefusal::DifferentVolumes(rename.clone()))
}

/// One rename relative to the shared parent, refusing to replace anything at the new name.
///
/// A plain `rename(2)` replaces an empty directory at its target, so a directory created
/// between the check above and this call would be silently swallowed; the exclusive form
/// fails instead.
fn rename_without_replacing(parent: &Path, rename: &EarlierNameRename) -> Result<(), MoveRefusal> {
    let name = |path: &Path| {
        path.file_name()
            .and_then(|name| CString::new(name.as_bytes()).ok())
            .ok_or_else(|| MoveRefusal::Failed {
                rename: rename.clone(),
                error: format!("{} has no usable name", path.display()),
            })
    };
    let (from, to) = (name(&rename.from)?, name(&rename.to)?);
    let directory = fs::File::open(parent).map_err(|error| failed(rename, &error))?;
    let descriptor = std::os::unix::io::AsRawFd::as_raw_fd(&directory);
    // Safety: both names are NUL-terminated and live until the call returns, and the
    // descriptor belongs to `directory`, which is open for its whole duration.
    let result = unsafe { exclusive_renameat(descriptor, from.as_ptr(), to.as_ptr()) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    Err(match error.raw_os_error() {
        Some(libc::EEXIST) | Some(libc::ENOTEMPTY) => MoveRefusal::BothExist(rename.clone()),
        Some(libc::EXDEV) => MoveRefusal::DifferentVolumes(rename.clone()),
        _ => failed(rename, &error),
    })
}

#[cfg(target_os = "macos")]
unsafe fn exclusive_renameat(
    directory: libc::c_int,
    from: *const libc::c_char,
    to: *const libc::c_char,
) -> libc::c_int {
    unsafe { libc::renameatx_np(directory, from, directory, to, libc::RENAME_EXCL) }
}

#[cfg(target_os = "linux")]
unsafe fn exclusive_renameat(
    directory: libc::c_int,
    from: *const libc::c_char,
    to: *const libc::c_char,
) -> libc::c_int {
    unsafe { libc::renameat2(directory, from, directory, to, libc::RENAME_NOREPLACE) }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{EarlierNameRename, MoveRefusal, is_graph, volume_refusal};

    fn rename() -> EarlierNameRename {
        EarlierNameRename {
            from: PathBuf::from("/Users/alice/Library/Application Support/Crosscode"),
            to: PathBuf::from("/Users/alice/Library/Application Support/Crosskit"),
        }
    }

    /// A rename across volumes is a copy under another name, and two gigabytes are not copied
    /// silently. A mounted volume cannot be put under that name in a test without root, so the
    /// decision is checked on its own; the rename maps `EXDEV` to the same refusal.
    #[test]
    fn an_earlier_directory_on_another_volume_is_refused() {
        assert_eq!(
            volume_refusal(7, 9, &rename()),
            Some(MoveRefusal::DifferentVolumes(rename()))
        );
        assert_eq!(volume_refusal(7, 7, &rename()), None);
    }

    #[test]
    fn only_context_graphs_are_asked_about() {
        assert!(is_graph(&PathBuf::from("/d/graph-work.lbug")));
        assert!(!is_graph(&PathBuf::from("/d/graph-work.lbug.wal")));
        assert!(!is_graph(&PathBuf::from("/d/config.toml")));
    }
}
