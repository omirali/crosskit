// The folders without git at the top of the document roots, and the files lying loose beside
// them.
//
// A folder is one directory directly in `Documents`, `Desktop` or `Downloads`, or directly in
// `Projects` without a repository of its own: the unit the owner would put in a context whole.
// Its files are counted and none is opened.
use std::path::Path;

use super::home_layout::HomeLayout;
use super::transcript_census::entries_in;
use crate::domain::NormalizedPath;

/// One folder without git.
#[derive(Debug, Clone)]
pub(super) struct DocumentFolder {
    pub(super) path: NormalizedPath,
    /// The root it lies in: `Documents`, `Desktop`, `Downloads` or `Projects`.
    pub(super) root: String,
    pub(super) files: u64,
}

#[derive(Debug, Default)]
pub(super) struct DocumentCensus {
    pub(super) folders: Vec<DocumentFolder>,
    /// Per document root, the files lying directly in it and in no folder.
    pub(super) loose_files: Vec<(String, u64)>,
}

pub(super) fn census_of_documents(layout: &HomeLayout) -> DocumentCensus {
    let mut census = DocumentCensus::default();
    let projects = layout.projects_directory();
    let roots = layout
        .document_roots()
        .into_iter()
        .chain(Path::new(projects.as_str()).is_dir().then_some(projects));
    for root in roots {
        let mut loose = 0;
        for entry in entries_in(Path::new(root.as_str())) {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if kind.is_file() {
                loose += 1;
                continue;
            }
            if !kind.is_dir() || entry.path().join(".git").symlink_metadata().is_ok() {
                continue;
            }
            let Ok(path) = root.join(&name) else {
                continue;
            };
            census.folders.push(DocumentFolder {
                path,
                root: root.file_name().to_string(),
                files: files_below(&entry.path()),
            });
        }
        census
            .loose_files
            .push((root.file_name().to_string(), loose));
    }
    census
        .folders
        .sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
    census
}

fn files_below(directory: &Path) -> u64 {
    let mut files = 0;
    for entry in entries_in(directory) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            files += files_below(&entry.path());
        } else if kind.is_file() {
            files += 1;
        }
    }
    files
}
