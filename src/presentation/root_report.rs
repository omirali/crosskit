// What one root of a context is, for the two commands that report it.
//
// `status` prints every root every day; `doctor` prints only the ones that hold nothing,
// because it is read when something is already wrong. They ask the same question and they
// must not answer it differently — a root `status` calls walked and `doctor` calls empty
// would be worse than neither saying anything.
use crate::application::{FailedPass, RootCoverage, RootCoverageReader};
use crate::domain::{ContextRoot, NormalizedPath};
use crate::infrastructure::utc_stamp;

/// What is known about one `include` of a context.
pub(super) enum RootState {
    /// The include is a pattern: it covers files without naming a directory, so no pass can
    /// start here.
    NotADirectory,
    /// The graph could not be asked about it.
    Unanswered(String),
    /// What the graph holds under it, and the path it was asked about.
    Covered(NormalizedPath, RootCoverage),
}

pub(super) fn state_of(root: &ContextRoot, repository: &impl RootCoverageReader) -> RootState {
    let Some(directory) = root.directory() else {
        return RootState::NotADirectory;
    };
    let Ok(path) = NormalizedPath::from_absolute(directory) else {
        return RootState::Unanswered(format!("{directory} is not a path this build can read"));
    };
    match repository.coverage_of_root(&path) {
        Ok(coverage) => RootState::Covered(path, coverage),
        Err(error) => RootState::Unanswered(error.to_string()),
    }
}

/// How an attempt that did not walk is said, in one place: two commands print it and a
/// second wording would be a second claim about the same record.
pub(super) fn describe_failure(failure: &FailedPass) -> String {
    format!(
        "({}) on {}",
        failure.category(),
        utc_stamp(failure.at_unix_ms())
    )
}
