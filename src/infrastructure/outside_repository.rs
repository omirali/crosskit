// The one check every measurement that writes real material makes first: the place it writes to
// is outside the repository (`.claude/rules/00-core.md`, «Ничего личного в исходниках»).
use std::path::Path;

/// Refuses `path` when it lies inside the directory the measurement runs from, which is the
/// repository. `variable` names the setting, for the sentence.
pub(super) fn refuse_inside_the_repository(path: &Path, variable: &str) -> Result<(), String> {
    let repository = std::env::current_dir().map_err(|error| error.to_string())?;
    let existing = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(Path::new("."))
    };
    let resolved = std::fs::canonicalize(existing)
        .map_err(|error| format!("{variable}: каталог {}: {error}", existing.display()))?;
    if resolved.starts_with(&repository) {
        return Err(format!(
            "{variable} внутри репозитория: там будут настоящие беседы, им место вне его. \
             Ничего не измерено."
        ));
    }
    Ok(())
}
