// Replacing a small file of decisions whole: written beside itself and renamed over the old one,
// so a reader sees the file before the change or after it, never half of it.
use std::fs;
use std::io::Write;
use std::path::Path;

/// Replaces `path` with `bytes`, durably.
pub(super) fn replace_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let staged = path.with_extension("new");
    let mut written = fs::File::create(&staged)?;
    written.write_all(bytes)?;
    written.sync_all()?;
    fs::rename(&staged, path)
}
