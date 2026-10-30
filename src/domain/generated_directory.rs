// Whether a directory was made by a tool rather than written by a person, decided from a
// marker the tool itself leaves behind.
//
// This is the half of the exclusion rule that does not age. A list of names knows only the
// ecosystems that existed when it was written, and every new tool arrives with a directory
// nobody has heard of; a marker is left by the tool that made the directory, so the rule
// keeps working for tools this program will never be told about.

/// Names that mark the directory holding one as generated.
///
/// Both are conventions their ecosystems agreed on, not guesses:
///
/// - `CACHEDIR.TAG` is the cache-directory tagging standard. Tools write it so that backup
///   and archiving programs can skip what can be made again; cargo, ruff, mypy and pytest
///   all do.
/// - `pyvenv.cfg` is written at the root of a Python virtual environment by the standard
///   library that creates it, and it exists nowhere else.
///
/// Measured on the owner's tree: between them they mark 15 directories holding 43 760 files
/// and 1.55 GB, which covers every Python environment on it — the `site-packages` results
/// that crowded out search — without the word "python" appearing in any rule.
const MARKERS: [&str; 2] = ["CACHEDIR.TAG", "pyvenv.cfg"];

/// Which marker says this directory was generated, if any. `holds` answers whether the
/// directory holds an entry with the given name; the answer is returned rather than a bare
/// `bool` so a person asking why a path was skipped can be told which marker decided it.
pub fn generated_directory_marker(holds: impl Fn(&str) -> bool) -> Option<&'static str> {
    MARKERS.into_iter().find(|marker| holds(marker))
}

#[cfg(test)]
mod tests {
    use super::{MARKERS, generated_directory_marker};

    #[test]
    fn a_directory_holding_a_marker_is_generated_and_says_which_one() {
        assert_eq!(
            generated_directory_marker(|name| name == "pyvenv.cfg"),
            Some("pyvenv.cfg")
        );
        assert_eq!(
            generated_directory_marker(|name| name == "CACHEDIR.TAG"),
            Some("CACHEDIR.TAG")
        );
    }

    #[test]
    fn a_directory_holding_none_of_them_is_not_generated() {
        assert_eq!(generated_directory_marker(|_| false), None);
        assert_eq!(
            generated_directory_marker(|name| name == "Cargo.toml" || name == "README.md"),
            None
        );
    }

    /// The names are asked for exactly as their conventions spell them. A case-folded or
    /// trimmed lookup would match `cachedir.tag`, which the standard does not define, and
    /// the rule would then depend on the filesystem's case behaviour rather than on the
    /// marker.
    #[test]
    fn the_markers_are_asked_for_by_their_exact_names() {
        assert_eq!(MARKERS, ["CACHEDIR.TAG", "pyvenv.cfg"]);
        assert_eq!(
            generated_directory_marker(|name| name == "cachedir.tag"),
            None
        );
    }
}
