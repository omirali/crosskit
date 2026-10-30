// The two lists a root has — what the policy admits on disk now, and what the graph holds
// under that root — put side by side.
//
// The funnel says how large the gap is. This says which files are in it and what each side
// calls them, which is the difference between "22 127 files are missing" and a reason
// somebody can act on.
use std::collections::{BTreeMap, BTreeSet};

/// What the graph calls a location it holds.
pub(super) const PRESENT: &str = "present";
pub(super) const MISSING: &str = "missing";

/// One root's two lists, reconciled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PathDiff {
    never_indexed: Vec<String>,
    wrongly_missing: Vec<String>,
    gone_from_disk: Vec<String>,
    agreed: usize,
}

impl PathDiff {
    /// `admitted` is what the policy would index if a pass ran now; `known` is what the
    /// graph holds, each path with its status.
    pub(super) fn between(admitted: &BTreeSet<String>, known: &[(String, String)]) -> Self {
        let known_by_path: BTreeMap<&str, &str> = known
            .iter()
            .map(|(path, status)| (path.as_str(), status.as_str()))
            .collect();

        let mut diff = Self::default();
        for path in admitted {
            match known_by_path.get(path.as_str()) {
                None => diff.never_indexed.push(path.clone()),
                // On disk, admitted by the policy, and the graph says it is gone. This is
                // the false `missing` Stage 21 named: an ordinary pass does not heal it,
                // so search will not show the file until the context is reindexed.
                Some(&MISSING) => diff.wrongly_missing.push(path.clone()),
                Some(_) => diff.agreed += 1,
            }
        }
        for (path, status) in known {
            if status == PRESENT && !admitted.contains(path) {
                diff.gone_from_disk.push(path.clone());
            }
        }
        diff
    }

    /// Admitted on disk, and the graph has never heard of the path.
    pub(super) fn never_indexed(&self) -> &[String] {
        &self.never_indexed
    }

    /// Admitted on disk, and the graph holds it as `missing`.
    pub(super) fn wrongly_missing(&self) -> &[String] {
        &self.wrongly_missing
    }

    /// The graph holds it as `present`, and the policy no longer admits that path — either
    /// the file is gone or a rule now covers it. Either way a pass would have retired it
    /// and none has run.
    pub(super) fn gone_from_disk(&self) -> &[String] {
        &self.gone_from_disk
    }

    pub(super) fn agreed(&self) -> usize {
        self.agreed
    }
}

#[cfg(test)]
mod tests {
    use super::{MISSING, PRESENT, PathDiff};
    use std::collections::BTreeSet;

    fn admitted(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    fn known(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|(path, status)| (path.to_string(), status.to_string()))
            .collect()
    }

    #[test]
    fn a_file_the_graph_never_heard_of_is_named_as_never_indexed() {
        let diff = PathDiff::between(
            &admitted(&["/tree/a.md", "/tree/b.md"]),
            &known(&[("/tree/a.md", PRESENT)]),
        );

        assert_eq!(diff.never_indexed(), ["/tree/b.md"]);
        assert_eq!(diff.agreed(), 1);
        assert!(diff.wrongly_missing().is_empty());
        assert!(diff.gone_from_disk().is_empty());
    }

    /// The defect Stage 21 named and did not heal: the file is on disk and the policy
    /// admits it, and the graph says it is gone. It has to be told apart from a file that
    /// really did disappear, or the count says nothing about what to do.
    #[test]
    fn a_file_on_disk_the_graph_calls_missing_is_named_apart() {
        let diff = PathDiff::between(
            &admitted(&["/tree/a.md"]),
            &known(&[("/tree/a.md", MISSING), ("/tree/gone.md", PRESENT)]),
        );

        assert_eq!(diff.wrongly_missing(), ["/tree/a.md"]);
        assert_eq!(diff.gone_from_disk(), ["/tree/gone.md"]);
        assert_eq!(diff.agreed(), 0);
    }

    /// A path the graph already calls missing and the policy no longer admits is not a
    /// finding: both sides say the same thing about it.
    #[test]
    fn a_path_both_sides_call_gone_is_not_a_finding() {
        let diff = PathDiff::between(&admitted(&[]), &known(&[("/tree/gone.md", MISSING)]));

        assert_eq!(diff, PathDiff::default());
    }
}
