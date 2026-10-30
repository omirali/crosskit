// The physical layer: one observation of a file at a normalized path.
use super::evidence::EvidenceError;
use super::source_path::NormalizedPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    RegularFile,
    Symlink,
    /// FIFOs, sockets and devices: recorded by name only, never opened.
    Special,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationStatus {
    Present,
    Missing,
}

/// Where a file was seen. Identity is `file_id + normalized path`, so the same file at the
/// same path is one location across scans, while a different file that later occupies the
/// path becomes a new location and the old one is retired as `missing`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLocation {
    id: String,
    file_id: String,
    path: NormalizedPath,
    display_name: String,
    kind: FileKind,
    size_bytes: u64,
    modified_at_unix_ms: i64,
    first_seen_at_unix_ms: i64,
    last_seen_at_unix_ms: i64,
    status: LocationStatus,
}

impl FileLocation {
    /// A fresh `present` observation: both seen timestamps equal `observed_at_unix_ms`.
    /// Persistence keeps the earliest stored `first_seen` when the location already exists.
    /// The display name is the last path segment; it is stored separately so a future
    /// Unicode normalization of `path` cannot lose the spelling the user sees.
    pub fn observed(
        file_id: impl Into<String>,
        path: NormalizedPath,
        kind: FileKind,
        size_bytes: u64,
        modified_at_unix_ms: i64,
        observed_at_unix_ms: i64,
    ) -> Result<Self, EvidenceError> {
        let file_id = required("location file id", file_id.into())?;
        let display_name = required("location display name", path.file_name().to_string())?;
        Ok(Self {
            id: Self::identity(&file_id, &path),
            file_id,
            path,
            display_name,
            kind,
            size_bytes,
            modified_at_unix_ms,
            first_seen_at_unix_ms: observed_at_unix_ms,
            last_seen_at_unix_ms: observed_at_unix_ms,
            status: LocationStatus::Present,
        })
    }

    fn identity(file_id: &str, path: &NormalizedPath) -> String {
        format!("{file_id}@{path}")
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn kind(&self) -> FileKind {
        self.kind
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn modified_at_unix_ms(&self) -> i64 {
        self.modified_at_unix_ms
    }

    pub fn first_seen_at_unix_ms(&self) -> i64 {
        self.first_seen_at_unix_ms
    }

    pub fn last_seen_at_unix_ms(&self) -> i64 {
        self.last_seen_at_unix_ms
    }

    pub fn status(&self) -> LocationStatus {
        self.status
    }
}

fn required(field: &'static str, value: String) -> Result<String, EvidenceError> {
    if value.trim().is_empty() {
        return Err(EvidenceError::MissingField(field));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{FileKind, FileLocation, LocationStatus};
    use crate::domain::{EvidenceError, NormalizedPath};

    #[test]
    fn identity_combines_file_id_and_normalized_path() {
        let location = FileLocation::observed(
            "file:unix:1:2",
            NormalizedPath::from_absolute("/Users/alice/notes.md").unwrap(),
            FileKind::RegularFile,
            42,
            1_000,
            2_000,
        )
        .unwrap();

        assert_eq!(location.id(), "file:unix:1:2@/Users/alice/notes.md");
        assert_eq!(location.display_name(), "notes.md");
        assert_eq!(location.first_seen_at_unix_ms(), 2_000);
        assert_eq!(location.last_seen_at_unix_ms(), 2_000);
        assert_eq!(location.status(), LocationStatus::Present);
    }

    #[test]
    fn a_location_requires_a_file_id() {
        let error = FileLocation::observed(
            " ",
            NormalizedPath::from_absolute("/a").unwrap(),
            FileKind::Symlink,
            0,
            0,
            0,
        )
        .unwrap_err();
        assert_eq!(error, EvidenceError::MissingField("location file id"));
    }
}
