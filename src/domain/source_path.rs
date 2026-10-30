// Normalized absolute paths: the comparison form shared by policy rules and location
// identity.
use std::fmt;

/// Absolute path with `/` separators, without `.`, `..` or empty segments and without a
/// trailing slash. Normalization is lexical only: symlinks are not resolved and bytes are
/// kept as they are, so paths compare equal when they were produced from the same
/// canonical root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NormalizedPath(String);

impl NormalizedPath {
    pub fn from_absolute(path: &str) -> Result<Self, PathError> {
        if !path.starts_with('/') {
            return Err(PathError::NotAbsolute(path.to_string()));
        }

        let mut segments: Vec<&str> = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                name => segments.push(name),
            }
        }
        Ok(Self::from_segments(&segments))
    }

    /// Appends one directory entry name, which must be a single segment.
    pub fn join(&self, name: &str) -> Result<Self, PathError> {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(PathError::InvalidName(name.to_string()));
        }
        if self.0 == "/" {
            return Ok(Self(format!("/{name}")));
        }
        Ok(Self(format!("{}/{name}", self.0)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|segment| !segment.is_empty())
    }

    /// The last segment, or `/` for the root itself.
    pub fn file_name(&self) -> &str {
        self.segments().last().unwrap_or("/")
    }

    /// The last dot-separated part of the file name, without the dot, exactly as it is
    /// written.
    ///
    /// The one place a path is cut at its dot. Two questions are asked of the answer — which
    /// mask would cover every file of this kind, and which language the name declares — and
    /// they must not be able to disagree about where the name ends.
    ///
    /// A name whose only dot opens it has no suffix: `.gitignore` is a name, not an
    /// extension of an empty one, and reading it as one would sort every dotfile by the
    /// wrong half of itself. A trailing dot gives nothing for the same reason.
    pub fn extension(&self) -> Option<&str> {
        match self.file_name().rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => Some(extension),
            _ => None,
        }
    }

    /// The directory this path lies in, or nothing when it is the filesystem root.
    pub fn parent(&self) -> Option<Self> {
        let cut = self.0.rfind('/')?;
        if self.0 == "/" {
            return None;
        }
        if cut == 0 {
            return Some(Self("/".to_string()));
        }
        Some(Self(self.0[..cut].to_string()))
    }

    /// Whether this path is `root` or lies below it. The boundary is a whole segment, so
    /// `/data/docs` covers `/data/docs/a.md` but neither `/data/docs-old` nor
    /// `/data/documents`.
    pub fn is_within(&self, root: &NormalizedPath) -> bool {
        if self == root {
            return true;
        }
        match root.as_str() {
            "/" => true,
            root => self
                .0
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/')),
        }
    }

    fn from_segments(segments: &[&str]) -> Self {
        if segments.is_empty() {
            return Self("/".to_string());
        }
        Self(format!("/{}", segments.join("/")))
    }
}

impl fmt::Display for NormalizedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    NotAbsolute(String),
    InvalidName(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute(path) => write!(f, "path {path:?} is not absolute"),
            Self::InvalidName(name) => write!(f, "{name:?} is not a valid path segment"),
        }
    }
}

impl std::error::Error for PathError {}

#[cfg(test)]
mod tests {
    use super::{NormalizedPath, PathError};

    #[test]
    fn normalization_removes_dot_segments_and_trailing_slashes() {
        let path = NormalizedPath::from_absolute("/Users//alice/./Projects/../Documents/").unwrap();
        assert_eq!(path.as_str(), "/Users/alice/Documents");
        assert_eq!(path.file_name(), "Documents");
        assert_eq!(
            path.segments().collect::<Vec<_>>(),
            ["Users", "alice", "Documents"]
        );
    }

    #[test]
    fn the_directory_a_path_lies_in_stops_at_the_filesystem_root() {
        let parent = |path: &str| {
            NormalizedPath::from_absolute(path)
                .unwrap()
                .parent()
                .map(|parent| parent.as_str().to_string())
        };

        assert_eq!(
            parent("/Users/alice/Documents").as_deref(),
            Some("/Users/alice")
        );
        assert_eq!(parent("/Users").as_deref(), Some("/"));
        assert_eq!(parent("/"), None);
    }

    #[test]
    fn parent_of_root_stays_root() {
        assert_eq!(
            NormalizedPath::from_absolute("/../..").unwrap().as_str(),
            "/"
        );
        assert_eq!(NormalizedPath::from_absolute("/").unwrap().file_name(), "/");
    }

    #[test]
    fn containment_stops_at_segment_boundaries() {
        let root = NormalizedPath::from_absolute("/data/docs").unwrap();
        let within = |path: &str| {
            NormalizedPath::from_absolute(path)
                .unwrap()
                .is_within(&root)
        };

        assert!(within("/data/docs"));
        assert!(within("/data/docs/a.md"));
        assert!(within("/data/docs/nested/b.md"));
        assert!(!within("/data/docs-old/a.md"));
        assert!(!within("/data/documents"));
        assert!(!within("/data"));
        assert!(!within("/other/data/docs/a.md"));

        let filesystem_root = NormalizedPath::from_absolute("/").unwrap();
        assert!(
            NormalizedPath::from_absolute("/anything")
                .unwrap()
                .is_within(&filesystem_root)
        );
        assert!(filesystem_root.is_within(&filesystem_root));
    }

    #[test]
    fn relative_paths_and_invalid_names_are_rejected() {
        assert_eq!(
            NormalizedPath::from_absolute("Documents"),
            Err(PathError::NotAbsolute("Documents".to_string()))
        );
        let root = NormalizedPath::from_absolute("/").unwrap();
        assert_eq!(root.join("notes.md").unwrap().as_str(), "/notes.md");
        assert_eq!(
            root.join("a/b"),
            Err(PathError::InvalidName("a/b".to_string()))
        );
        assert_eq!(
            root.join(".."),
            Err(PathError::InvalidName("..".to_string()))
        );
    }
}
