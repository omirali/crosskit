// What a file was when the service started, kept to tell later whether it is still that.
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A file by when it was last written and how large it is.
///
/// Enough for a binary: a build writes a new file, and the copy the service runs keeps the
/// time of the build it was copied from. What it cannot say is whether two builds of the same
/// source differ — it calls a rebuild that changed nothing a new build, which costs one
/// unnecessary restart and never a missed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub modified_ms: i64,
    pub size: u64,
}

pub fn file_stamp(path: &Path) -> io::Result<FileStamp> {
    let metadata = fs::metadata(path)?;
    Ok(FileStamp {
        modified_ms: unix_ms(metadata.modified()?),
        size: metadata.len(),
    })
}

/// The content of a small file as a digest, for the configuration: an editor that saves the
/// same text again changes its time and not what the service would read.
pub fn content_digest(path: &Path) -> io::Result<String> {
    let digest = Sha256::digest(fs::read(path)?);
    Ok(digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn now_ms() -> i64 {
    unix_ms(SystemTime::now())
}

fn unix_ms(moment: SystemTime) -> i64 {
    moment.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
    })
}
