// Writing the policy file: the one place in the program that does.
//
// Two rules, and both are about what happens when this goes wrong. Whatever was at the
// path is copied beside it first, under a name carrying the moment — a configuration is a
// thing a person wrote, and a command that replaces one has to leave the old one reachable.
// And the new file arrives by a rename from a temporary file in the same directory, so a
// process that dies mid-write leaves either the old configuration or the new one, never
// half of one: a truncated `config.toml` is a machine on which no command runs at all.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::utc_stamp::utc_stamp;

/// What a write left behind: the path the previous configuration was kept under, when
/// there was one to keep.
pub fn write_index_policy(path: &Path, contents: &str) -> Result<Option<PathBuf>, WriteError> {
    write_index_policy_at(path, contents, now_unix_ms())
}

/// The moment a kept file is named after. A clock this program cannot read is not a reason
/// to refuse to write a configuration, so the epoch stands in: two names would then
/// collide, and the copy refuses to overwrite nothing — it simply keeps the newer one under
/// a name that says 1970, which is visibly wrong rather than silently lost.
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}

fn write_index_policy_at(
    path: &Path,
    contents: &str,
    unix_ms: i64,
) -> Result<Option<PathBuf>, WriteError> {
    let directory = path.parent().ok_or_else(|| WriteError {
        path: path.to_path_buf(),
        reason: "the policy file path has no directory".to_string(),
    })?;
    fs::create_dir_all(directory).map_err(|error| WriteError::at(directory, &error))?;

    let kept = keep_previous(path, unix_ms)?;
    let temporary = directory.join(format!(".config-{unix_ms}.toml.partial"));
    fs::write(&temporary, contents).map_err(|error| WriteError::at(&temporary, &error))?;
    if let Err(error) = fs::rename(&temporary, path) {
        // The temporary file is this function's own; leaving one behind after a failure
        // would put a file nobody wrote in the operator's data directory.
        let _ = fs::remove_file(&temporary);
        return Err(WriteError::at(path, &error));
    }
    Ok(kept)
}

/// Copies whatever is at `path` beside it. Copied rather than renamed: a rename would take
/// the configuration away before the new one exists, and the window between the two is
/// exactly when a person stops the command.
fn keep_previous(path: &Path, unix_ms: i64) -> Result<Option<PathBuf>, WriteError> {
    if !path.exists() {
        return Ok(None);
    }
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".before-setup-{}", utc_stamp(unix_ms)));
    let kept = PathBuf::from(name);
    fs::copy(path, &kept).map_err(|error| WriteError::at(&kept, &error))?;
    Ok(Some(kept))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteError {
    path: PathBuf,
    reason: String,
}

impl WriteError {
    fn at(path: &Path, error: &io::Error) -> Self {
        Self {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the configuration was not written to {}: {}",
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for WriteError {}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::write_index_policy_at;

    fn directory(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "crosskit-policy-write-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_first_write_keeps_nothing_and_leaves_only_the_configuration() {
        let directory = directory("first");
        let path = directory.join("config.toml");

        let kept = write_index_policy_at(&path, "[workspace.work]\n", 1_790_087_400_000).unwrap();

        assert_eq!(kept, None, "there was nothing to keep");
        assert_eq!(fs::read_to_string(&path).unwrap(), "[workspace.work]\n");
        let mut names: Vec<String> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["config.toml".to_string()],
            "no temporary file may survive the write"
        );
        fs::remove_dir_all(&directory).unwrap();
    }

    /// The previous configuration is a thing a person wrote, so replacing it leaves it
    /// readable under a name that says when it was replaced.
    #[test]
    fn a_replaced_configuration_is_kept_beside_the_new_one() {
        let directory = directory("kept");
        let path = directory.join("config.toml");
        fs::write(&path, "[index]\nmax_content_size_mb = 4\n").unwrap();

        let kept = write_index_policy_at(&path, "[workspace.work]\n", 1_790_087_400_000)
            .unwrap()
            .expect("the previous configuration must be kept");

        assert_eq!(
            kept.file_name().unwrap().to_string_lossy(),
            "config.toml.before-setup-20260922T143000Z"
        );
        assert_eq!(
            fs::read_to_string(&kept).unwrap(),
            "[index]\nmax_content_size_mb = 4\n",
            "kept means the same bytes"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "[workspace.work]\n");
        fs::remove_dir_all(&directory).unwrap();
    }

    /// Two runs in the same second would collide on one name; a second apart do not. What
    /// the test states is the rule — a name that repeats overwrites a configuration this
    /// command promised to keep.
    #[test]
    fn a_second_replacement_does_not_overwrite_the_first_one_kept() {
        let directory = directory("twice");
        let path = directory.join("config.toml");
        fs::write(&path, "first\n").unwrap();

        write_index_policy_at(&path, "second\n", 1_790_087_400_000).unwrap();
        write_index_policy_at(&path, "third\n", 1_790_087_401_000).unwrap();

        assert_eq!(
            fs::read_to_string(directory.join("config.toml.before-setup-20260922T143000Z"))
                .unwrap(),
            "first\n"
        );
        assert_eq!(
            fs::read_to_string(directory.join("config.toml.before-setup-20260922T143001Z"))
                .unwrap(),
            "second\n"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "third\n");
        fs::remove_dir_all(&directory).unwrap();
    }
}
