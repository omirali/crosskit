// The filesystem adapter of the index use case: a canonical root and a descriptor-relative
// walk that never follows symlinks.
use std::fs;
use std::path::Path;

use crate::application::{IndexError, SourceVisitor, SourceWalker};
use crate::domain::NormalizedPath;

pub struct DescriptorRelativeWalker;

impl SourceWalker for DescriptorRelativeWalker {
    /// Symlinks in the root path itself are resolved here, once, so the policy always sees
    /// the real directory. Below the root nothing is ever followed.
    fn resolve_root(&self, root: &Path) -> Result<NormalizedPath, IndexError> {
        let canonical = fs::canonicalize(root).map_err(|error| {
            IndexError::Source(format!("resolve root {}: {error}", root.display()))
        })?;
        if !canonical.is_dir() {
            return Err(IndexError::Source(format!(
                "root {} is not a directory",
                root.display()
            )));
        }
        NormalizedPath::from_absolute(&canonical.to_string_lossy())
            .map_err(|error| IndexError::Source(error.to_string()))
    }

    fn walk(
        &self,
        root: &NormalizedPath,
        visitor: &mut dyn SourceVisitor,
    ) -> Result<(), IndexError> {
        #[cfg(unix)]
        {
            super::unix_walk::walk(root, visitor)
        }

        #[cfg(not(unix))]
        {
            let _ = (root, visitor);
            Err(IndexError::Source(
                "secure descriptor-relative directory traversal is unavailable on this platform"
                    .to_string(),
            ))
        }
    }
}
