// The owner's own labels, and the one distinction the stage exists to draw.
//
// A context is a confidentiality boundary. A work file sent confidently into the personal
// context — or the other way — is a leak the owner does not see; a question is a minute of the
// owner's time. So the comparison never folds the two into one accuracy number: a confident
// mistake, a question, and a disagreement about what is nobody's work are counted apart.
//
// The file lives outside the repository and is named by an environment variable: its lines
// are the owner's real paths.
use super::side::{ContextSide, SplitDecision};
use crate::domain::NormalizedPath;

/// The owner's labels, one per path; a label covers everything under its path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OwnerTruth {
    labels: Vec<(NormalizedPath, ContextSide)>,
}

/// What the placeholder of an unlabelled line is. A template the owner has not finished is not
/// a file full of mistakes.
const NOT_YET_LABELLED: &str = "?";

/// What may stand between a path and its label, so that `path → work` reads as it is written.
const SEPARATORS: [&str; 3] = ["→", "->", "="];

impl OwnerTruth {
    /// `<path> <label>` a line, the label being the last word: `work`, `personal` or `skip`.
    /// An arrow or `=` may stand between them; `~/` is the home directory; a line starting with
    /// `#` is a comment; `?` in place of a label is a line not decided yet. Returns the labels
    /// and the numbers of the lines that could not be read, which the report names.
    pub(crate) fn parse(text: &str, home: &NormalizedPath) -> (Self, Vec<usize>) {
        let mut truth = Self::default();
        let mut unreadable = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match read_line(line, home) {
                Some(Some(label)) => truth.labels.push(label),
                Some(None) => {}
                None => unreadable.push(index + 1),
            }
        }
        (truth, unreadable)
    }

    /// The label of the longest labelled path that is `path` or holds it.
    pub(crate) fn side_of(&self, path: &NormalizedPath) -> Option<ContextSide> {
        self.labels
            .iter()
            .filter(|(labelled, _)| path.is_within(labelled))
            .max_by_key(|(labelled, _)| labelled.as_str().len())
            .map(|(_, side)| *side)
    }

    pub(crate) fn len(&self) -> usize {
        self.labels.len()
    }
}

/// One line: a label, a line left for later (`Some(None)`), or a line that is not one (`None`).
fn read_line(line: &str, home: &NormalizedPath) -> Option<Option<(NormalizedPath, ContextSide)>> {
    let (path, label) = line.rsplit_once(char::is_whitespace)?;
    let mut path = path.trim();
    for separator in SEPARATORS {
        path = path.strip_suffix(separator).unwrap_or(path).trim_end();
    }
    let path = expand_home(path, home)?;
    if label == NOT_YET_LABELLED {
        return Some(None);
    }
    Some(Some((path, ContextSide::of_label(label)?)))
}

fn expand_home(path: &str, home: &NormalizedPath) -> Option<NormalizedPath> {
    let expanded = match path.strip_prefix('~') {
        Some("") => home.as_str().to_string(),
        Some(rest) if rest.starts_with('/') => format!("{}{rest}", home.as_str()),
        Some(_) => return None,
        None => path.to_string(),
    };
    NormalizedPath::from_absolute(&expanded).ok()
}

/// How the rule's answer about one thing stands against the owner's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Agreement {
    Agrees,
    /// The rule said `work` and the owner `personal`, or the other way: the failure that leaks.
    CrossedSides,
    /// One of the two said `skip` and the other a side. Nothing crosses the boundary, but a
    /// context lost or gained something nobody wanted in it.
    SkipDisagrees,
    /// The rule asked, and the owner's answer is this.
    Asked(ContextSide),
    /// The owner's file says nothing about it.
    OwnerSilent,
}

pub(crate) fn agreement(decision: &SplitDecision, truth: Option<ContextSide>) -> Agreement {
    let Some(truth) = truth else {
        return Agreement::OwnerSilent;
    };
    let Some(side) = decision.side() else {
        return Agreement::Asked(truth);
    };
    match (side, truth) {
        (said, meant) if said == meant => Agreement::Agrees,
        (ContextSide::Skip, _) | (_, ContextSide::Skip) => Agreement::SkipDisagrees,
        _ => Agreement::CrossedSides,
    }
}

#[cfg(test)]
mod tests {
    use super::{Agreement, OwnerTruth, agreement};
    use crate::domain::NormalizedPath;
    use crate::domain::context_split::{AskBecause, ContextSide, DecidedBy, SplitDecision};

    fn path(value: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(value).expect("absolute")
    }

    fn home() -> NormalizedPath {
        path("/Users/alice")
    }

    #[test]
    fn a_label_covers_its_path_and_the_longest_label_wins() {
        let (truth, unreadable) = OwnerTruth::parse(
            "# the owner's labels\n\
             ~/Projects work\n\
             ~/Projects/garden-planner → personal\n\
             /Users/alice/Downloads/My Scans = skip\n\
             ~/Projects/payments-api ?\n",
            &home(),
        );

        assert!(unreadable.is_empty(), "{unreadable:?}");
        assert_eq!(truth.len(), 3, "a line left as `?` is not a label");
        assert_eq!(
            truth.side_of(&path("/Users/alice/Projects/payments-api/src/main.rs")),
            Some(ContextSide::Work)
        );
        assert_eq!(
            truth.side_of(&path("/Users/alice/Projects/garden-planner/notes.md")),
            Some(ContextSide::Personal)
        );
        assert_eq!(
            truth.side_of(&path("/Users/alice/Downloads/My Scans/one.pdf")),
            Some(ContextSide::Skip),
            "a path with a space is read up to its label"
        );
        assert_eq!(truth.side_of(&path("/Users/alice/Projects-old/x")), None);
    }

    #[test]
    fn a_line_that_is_not_a_label_is_reported_by_its_number() {
        let (truth, unreadable) = OwnerTruth::parse(
            "~/Projects work\nrelative/path work\n~/Projects/x maybe\n~other/x work\nlonely\n",
            &home(),
        );

        assert_eq!(truth.len(), 1);
        assert_eq!(unreadable, [2, 3, 4, 5]);
    }

    /// The comparison the stage is judged by: a confident mistake across the boundary is its own
    /// count, and it is never merged with a question or with a quarrel about what to skip.
    #[test]
    fn a_confident_mistake_a_question_and_a_skip_are_counted_apart() {
        let said_work = SplitDecision::decided(ContextSide::Work, DecidedBy::OwnerMailDomain);
        let said_skip = SplitDecision::decided(ContextSide::Skip, DecidedBy::TemporaryPlace);
        let asked = SplitDecision::Ask(AskBecause::ForeignGroup);

        assert_eq!(
            agreement(&said_work, Some(ContextSide::Work)),
            Agreement::Agrees
        );
        assert_eq!(
            agreement(&said_work, Some(ContextSide::Personal)),
            Agreement::CrossedSides
        );
        assert_eq!(
            agreement(&said_work, Some(ContextSide::Skip)),
            Agreement::SkipDisagrees
        );
        assert_eq!(
            agreement(&said_skip, Some(ContextSide::Personal)),
            Agreement::SkipDisagrees
        );
        assert_eq!(
            agreement(&asked, Some(ContextSide::Personal)),
            Agreement::Asked(ContextSide::Personal)
        );
        assert_eq!(agreement(&said_work, None), Agreement::OwnerSilent);
        assert_eq!(agreement(&asked, None), Agreement::OwnerSilent);
    }
}
