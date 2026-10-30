// Which revisions are analysed without being asked about, and which wait for the owner.
//
// Stage 22 measured what analysis costs: 25 952 revisions of one real tree at 10.7 seconds
// each is 77 hours of a subscription, spent on a line no command read. Stage 23 answers that
// with a rule rather than with a faster engine — documents analyse themselves, everything
// else waits to be approved, and what waits is visible.
//
// A rule is a mask, never a file. Approving a directory once must not mean approving every
// revision that appears in it afterwards; the owner decides about a kind of thing, and the
// decision keeps working.
use super::index_policy::PathPattern;
use super::source_path::NormalizedPath;

/// What this installation analyses on its own, and what it analyses because the owner said
/// so. Two lists rather than one, because they are answerable to different people: the
/// first is this program's default and is argued for in an ADR, the second is the owner's
/// and grows as they work.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalysisRules {
    automatic: Vec<PathPattern>,
    approved: Vec<PathPattern>,
}

impl AnalysisRules {
    pub fn new(automatic: Vec<PathPattern>, approved: Vec<PathPattern>) -> Self {
        Self {
            automatic,
            approved,
        }
    }

    /// Whether a revision at this path is analysed at all.
    ///
    /// The two lists are asked the same question and neither outranks the other: there is
    /// nothing to resolve between "this program analyses documents" and "the owner approved
    /// this directory", because both are a yes and there is no no to weigh them against. A
    /// path no rule covers is not refused — it is unanswered, and that is what the review
    /// list is for.
    pub fn cover(&self, path: &NormalizedPath) -> bool {
        self.automatic
            .iter()
            .chain(&self.approved)
            .any(|pattern| pattern.matches(path))
    }

    pub fn approved(&self) -> &[PathPattern] {
        &self.approved
    }
}

#[cfg(test)]
mod tests {
    use super::{AnalysisRules, NormalizedPath, PathPattern};

    fn rules(automatic: &[&str], approved: &[&str]) -> AnalysisRules {
        let parse = |patterns: &[&str]| {
            patterns
                .iter()
                .map(|pattern| PathPattern::parse(pattern).unwrap())
                .collect()
        };
        AnalysisRules::new(parse(automatic), parse(approved))
    }

    fn path(text: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(text).unwrap()
    }

    #[test]
    fn a_path_an_automatic_rule_covers_is_analysed() {
        let rules = rules(&["**/*.md"], &[]);
        assert!(rules.cover(&path("/work/notes/readme.md")));
        assert!(!rules.cover(&path("/work/src/main.rs")));
    }

    /// The owner's own list answers the same question, and a path either list covers is
    /// covered: there is no order to resolve between two kinds of yes.
    #[test]
    fn a_path_the_owner_approved_is_analysed_too() {
        let rules = rules(&["**/*.md"], &["/work/src/**"]);
        assert!(rules.cover(&path("/work/src/main.rs")));
        assert!(rules.cover(&path("/work/notes/readme.md")));
        assert!(!rules.cover(&path("/other/main.rs")));
    }

    /// The point of a mask: approving a directory covers what appears in it later, so the
    /// owner answers about a kind of thing once rather than about every new revision.
    #[test]
    fn an_approved_directory_covers_what_appears_in_it_afterwards() {
        let rules = rules(&[], &["/work/docs/**"]);
        assert!(rules.cover(&path("/work/docs/today.md")));
        assert!(rules.cover(&path("/work/docs/deeper/tomorrow.md")));
    }

    #[test]
    fn covering_nothing_is_a_configuration_and_not_an_accident() {
        assert!(!rules(&[], &[]).cover(&path("/work/notes/readme.md")));
    }
}
