// The folders without git lying directly in the places the grouping rule names.
//
// A folder is listed with the number of files under it, and none of them is opened. A name
// starting with a dot is a tool's, not a folder somebody keeps documents in, and a link is not
// followed: what it points at lies somewhere else, which is where it would be found.
use std::path::Path;

use super::transcripts::entries_in;
use crate::domain::{DocumentFolderSurvey, NormalizedPath};

/// Every directory directly in each of `places`, sorted by path.
pub fn document_folders(places: &[NormalizedPath]) -> Vec<DocumentFolderSurvey> {
    let mut folders = Vec::new();
    for place in places {
        for entry in entries_in(Path::new(place.as_str())) {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if name.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(path) = place.join(&name) else {
                continue;
            };
            folders.push(DocumentFolderSurvey::new(path, files_below(&entry.path())));
        }
    }
    folders.sort_by(|first, second| first.path().as_str().cmp(second.path().as_str()));
    folders
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
