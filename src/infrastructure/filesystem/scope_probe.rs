// What a scoped pass asks of the disk before it walks: whether a directory is there, and
// whether a directory is one the full walk skips by its marker.
//
// By path and without following links. It decides only where a walk starts, never what is
// published: the walk itself opens every directory descriptor-relative as always, and checks
// the marker of the directory it starts from on its own.
use std::fs;
use std::path::Path;

use crate::application::ScopeProbe;
use crate::domain::{NormalizedPath, generated_directory_marker};

pub struct DiskScopeProbe;

impl ScopeProbe for DiskScopeProbe {
    fn is_directory(&self, path: &NormalizedPath) -> bool {
        fs::symlink_metadata(path.as_str()).is_ok_and(|metadata| metadata.is_dir())
    }

    fn is_generated_directory(&self, path: &NormalizedPath) -> bool {
        let directory = Path::new(path.as_str());
        generated_directory_marker(|name| {
            fs::symlink_metadata(directory.join(name)).is_ok_and(|metadata| metadata.is_file())
        })
        .is_some()
    }
}
