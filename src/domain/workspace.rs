// Contexts: the confidentiality boundary. A context owns a part of the filesystem, and a
// run works in exactly one of them. What keeps one context's files out of another is not a
// filter applied to every query but the fact that each context has a graph of its own, so
// there is nothing to reach across; this module decides which context a path belongs to and
// what a context is allowed to be called.
use std::fmt;

use super::context_root::ContextRoot;
use super::index_policy::{PathPattern, Specificity};
use super::source_path::NormalizedPath;

/// The longest a context name may be. Its name becomes part of a database file name, and a
/// file name is bounded; the limit is stated here so an over-long name is refused where it
/// is read rather than where it is written.
const MAX_NAME_CHARS: usize = 64;

/// The name of a context. It reaches the filesystem as part of a database file name, so
/// anything that is not a plain name is refused by the constructor: a name is not something
/// to be cleaned up and used anyway, because what a sanitizer silently turns a name into is
/// a different context than the one the operator wrote.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceName(String);

impl WorkspaceName {
    pub fn parse(name: &str) -> Result<Self, WorkspaceError> {
        if name.is_empty() {
            return Err(WorkspaceError::EmptyName);
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(WorkspaceError::NameTooLong {
                name: name.to_string(),
                limit: MAX_NAME_CHARS,
            });
        }
        if name == "." || name == ".." || name.starts_with('.') {
            return Err(WorkspaceError::NameNotAPlainName(name.to_string()));
        }
        let plain = name
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_' | '.'));
        if !plain {
            return Err(WorkspaceError::NameNotAPlainName(name.to_string()));
        }
        Ok(Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether the content of a context's files may leave this machine. The default is
/// refusal, and a context that says nothing has not said yes: the work context, where an
/// employer's files live, may not send a byte anywhere until its own configuration says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentEgress {
    #[default]
    Denied,
    Allowed,
}

impl ContentEgress {
    /// Proof that a context allows its content to leave, or nothing. A denying context
    /// cannot produce this value, so code that needs one to run cannot run there — the
    /// permission is a thing that exists or does not, never a flag somebody checks.
    pub fn permit(self) -> Option<ContentEgressPermit> {
        match self {
            Self::Allowed => Some(ContentEgressPermit(())),
            Self::Denied => None,
        }
    }
}

/// The proof itself. It carries nothing and cannot be constructed from outside this module:
/// the only way to hold one is to have been given a context that allows egress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentEgressPermit(());

/// One coverage rule: a pattern and the context it gives its paths to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRule {
    workspace: WorkspaceName,
    pattern: PathPattern,
}

impl WorkspaceRule {
    pub fn new(workspace: WorkspaceName, pattern: PathPattern) -> Self {
        Self { workspace, pattern }
    }

    pub fn workspace(&self) -> &WorkspaceName {
        &self.workspace
    }

    pub fn pattern(&self) -> &PathPattern {
        &self.pattern
    }
}

/// Which context every path belongs to. The most specific rule wins, exactly as it does in
/// the index policy, and a path no rule covers belongs to no context and is not indexed —
/// the same refusal by default the policy already makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMap {
    rules: Vec<WorkspaceRule>,
    names: Vec<WorkspaceName>,
    /// The contexts that said their content may leave. Every other context denies, which
    /// is why this is a list of the exceptions rather than a value per context.
    allow_egress: Vec<WorkspaceName>,
}

impl WorkspaceMap {
    /// Every context denies egress. This is the constructor most callers want, and it is
    /// the reason a context cannot gain the permission by accident: no argument of this
    /// function can grant it.
    pub fn new(rules: Vec<WorkspaceRule>) -> Result<Self, WorkspaceError> {
        Self::with_egress(rules, Vec::new())
    }

    /// The same, with the contexts that declared their content may leave this machine. A
    /// named context that covers nothing is refused: a permission written for a context
    /// that does not exist is a typo, and a typo here silently means the opposite of what
    /// was written.
    ///
    /// Two contexts may not cover one path. The check is made here, when the configuration
    /// is read, because a boundary that is resolved later by precedence is not a boundary:
    /// whoever wrote the rules has to be told which two of them meet.
    pub fn with_egress(
        rules: Vec<WorkspaceRule>,
        allow_egress: Vec<WorkspaceName>,
    ) -> Result<Self, WorkspaceError> {
        for (index, rule) in rules.iter().enumerate() {
            for other in &rules[index + 1..] {
                if rule.workspace() != other.workspace()
                    && rule.pattern().may_overlap(other.pattern())
                {
                    return Err(WorkspaceError::OverlappingCoverage {
                        workspace: rule.workspace().clone(),
                        pattern: rule.pattern().as_str().to_string(),
                        other_workspace: other.workspace().clone(),
                        other_pattern: other.pattern().as_str().to_string(),
                    });
                }
            }
        }
        let mut names: Vec<WorkspaceName> = Vec::new();
        for rule in &rules {
            if !names.contains(&rule.workspace) {
                names.push(rule.workspace.clone());
            }
        }
        for allowed in &allow_egress {
            if !names.contains(allowed) {
                return Err(WorkspaceError::EgressForUnknownWorkspace(allowed.clone()));
            }
        }
        Ok(Self {
            rules,
            names,
            allow_egress,
        })
    }

    /// Whether this context's file content may leave the machine. A context this map does
    /// not know denies, like every context that did not say otherwise.
    pub fn content_egress(&self, workspace: &WorkspaceName) -> ContentEgress {
        if self.allow_egress.contains(workspace) {
            return ContentEgress::Allowed;
        }
        ContentEgress::Denied
    }

    /// Every context the configuration declares, in the order it declares them. A report
    /// over all of them walks this; a run that works in one asks `select` instead.
    pub fn names(&self) -> &[WorkspaceName] {
        &self.names
    }

    /// Where a pass over this context starts: every `include` it declares, in the order the
    /// configuration declares them, each one saying whether it names a directory.
    ///
    /// The map already answers "whose path is this"; this is the other direction, and until
    /// Stage 35 nothing asked for it — which is why `index init` could only be given a
    /// directory, and why a context of ten roots was indexed by ten separate commands with
    /// nothing checking that all ten ran.
    ///
    /// A context this map does not know has no includes, which is the same answer as a
    /// context that declares none: there is nothing to walk either way.
    pub fn roots_of(&self, workspace: &WorkspaceName) -> Vec<ContextRoot> {
        self.rules
            .iter()
            .filter(|rule| rule.workspace() == workspace)
            .map(|rule| ContextRoot::of(rule.pattern()))
            .collect()
    }

    /// The context `path` belongs to, or none when no rule covers it.
    pub fn workspace_of(&self, path: &NormalizedPath) -> Option<&WorkspaceName> {
        let mut winner: Option<&WorkspaceRule> = None;
        for rule in self.rules.iter().filter(|rule| rule.pattern.matches(path)) {
            if winner.is_none_or(|current| outranks(rule, current)) {
                winner = Some(rule);
            }
        }
        winner.map(WorkspaceRule::workspace)
    }

    /// The context this run works in: the one it named, or the only one configured. Several
    /// configured and none named is refused, because a run belongs to exactly one context
    /// and choosing it for the operator is the one thing a boundary must never do.
    pub fn select(&self, requested: Option<&str>) -> Result<&WorkspaceName, WorkspaceError> {
        match requested {
            Some(requested) => self
                .names
                .iter()
                .find(|name| name.as_str() == requested)
                .ok_or_else(|| WorkspaceError::UnknownWorkspace {
                    requested: requested.to_string(),
                    configured: self.declared_names(),
                }),
            None => match self.names.as_slice() {
                [] => Err(WorkspaceError::NoWorkspaceConfigured),
                [only] => Ok(only),
                _ => Err(WorkspaceError::AmbiguousWorkspace {
                    configured: self.declared_names(),
                }),
            },
        }
    }

    fn declared_names(&self) -> String {
        self.names
            .iter()
            .map(WorkspaceName::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The more specific rule wins; equally specific rules keep declaration order, so the first
/// declared one stays. This is the index policy's ordering, applied to the same patterns.
fn outranks(rule: &WorkspaceRule, current: &WorkspaceRule) -> bool {
    specificity(rule) > specificity(current)
}

fn specificity(rule: &WorkspaceRule) -> Specificity {
    rule.pattern.specificity()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    EmptyName,
    NameNotAPlainName(String),
    NameTooLong {
        name: String,
        limit: usize,
    },
    OverlappingCoverage {
        workspace: WorkspaceName,
        pattern: String,
        other_workspace: WorkspaceName,
        other_pattern: String,
    },
    /// A context was allowed to send content out, but it covers no path at all.
    EgressForUnknownWorkspace(WorkspaceName),
    NoWorkspaceConfigured,
    AmbiguousWorkspace {
        configured: String,
    },
    UnknownWorkspace {
        requested: String,
        configured: String,
    },
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => f.write_str("a context name must not be empty"),
            Self::NameNotAPlainName(name) => write!(
                f,
                "context name {name:?} is not a plain name; it becomes part of a database file name, so use letters, digits, '-', '_' or '.' and do not begin with '.'"
            ),
            Self::NameTooLong { name, limit } => write!(
                f,
                "context name {name:?} is longer than {limit} characters"
            ),
            Self::OverlappingCoverage {
                workspace,
                pattern,
                other_workspace,
                other_pattern,
            } => write!(
                f,
                "contexts {workspace} and {other_workspace} both cover the same paths: {pattern:?} and {other_pattern:?}; a path belongs to exactly one context"
            ),
            Self::EgressForUnknownWorkspace(workspace) => write!(
                f,
                "context {workspace} is allowed to send content outside this machine but covers no path; give it an `include` pattern or remove the permission, because a permission written for a context that does not exist reads as the opposite of what it says"
            ),
            Self::NoWorkspaceConfigured => f.write_str(
                "no context is configured; add a [workspace.<name>] section with an `include` pattern, because a path no context covers is not indexed",
            ),
            Self::AmbiguousWorkspace { configured } => write!(
                f,
                "several contexts are configured ({configured}); name the one this run works in with --context <name>"
            ),
            Self::UnknownWorkspace {
                requested,
                configured,
            } => write!(f, "no context named {requested:?} is configured ({configured})"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

#[cfg(test)]
mod tests {
    use super::{ContentEgress, WorkspaceError, WorkspaceMap, WorkspaceName, WorkspaceRule};
    use crate::domain::{NormalizedPath, PathPattern};

    fn rule(workspace: &str, pattern: &str) -> WorkspaceRule {
        WorkspaceRule::new(
            WorkspaceName::parse(workspace).unwrap(),
            PathPattern::parse(pattern).unwrap(),
        )
    }

    fn path(path: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(path).unwrap()
    }

    /// The name reaches the filesystem as part of a database file name, so everything that
    /// is not a plain name is refused where it is read.
    #[test]
    fn a_context_name_that_is_not_a_plain_name_is_refused() {
        for refused in [
            "work/personal",
            "work\\personal",
            "..",
            ".",
            "../escape",
            ".hidden",
            "",
            "with space",
            "with:colon",
        ] {
            assert!(
                WorkspaceName::parse(refused).is_err(),
                "{refused:?} must be refused"
            );
        }
        assert!(WorkspaceName::parse(&"n".repeat(65)).is_err());
    }

    #[test]
    fn a_plain_name_is_accepted_in_any_script() {
        for accepted in ["work", "personal-2", "work_laptop", "личное", "v1.2"] {
            assert_eq!(
                WorkspaceName::parse(accepted).unwrap().as_str(),
                accepted,
                "{accepted:?} must be accepted as written"
            );
        }
    }

    #[test]
    fn the_most_specific_rule_decides_and_an_uncovered_path_belongs_to_no_context() {
        let map = WorkspaceMap::new(vec![
            rule("work", "/tree/work"),
            rule("work", "/tree/also-work"),
            rule("personal", "/tree/personal"),
        ])
        .unwrap();

        assert_eq!(
            map.workspace_of(&path("/tree/work/notes/today.md"))
                .map(WorkspaceName::as_str),
            Some("work")
        );
        assert_eq!(
            map.workspace_of(&path("/tree/personal/diary.md"))
                .map(WorkspaceName::as_str),
            Some("personal")
        );
        assert_eq!(map.workspace_of(&path("/tree/nobody/file.md")), None);
        assert_eq!(map.workspace_of(&path("/tree")), None);
    }

    /// A boundary resolved later by precedence is not a boundary: whoever wrote the rules
    /// is told which two of them meet, in the words they wrote.
    #[test]
    fn two_contexts_covering_one_path_are_refused_with_both_rules_named() {
        let error = WorkspaceMap::new(vec![
            rule("work", "/tree"),
            rule("personal", "/tree/personal"),
        ])
        .unwrap_err();

        let WorkspaceError::OverlappingCoverage {
            workspace,
            pattern,
            other_workspace,
            other_pattern,
        } = error
        else {
            panic!("expected overlapping coverage, received {error:?}");
        };
        assert_eq!(workspace.as_str(), "work");
        assert_eq!(pattern, "/tree");
        assert_eq!(other_workspace.as_str(), "personal");
        assert_eq!(other_pattern, "/tree/personal");
    }

    /// Where two patterns cannot be told apart the answer is that they may overlap: the
    /// configuration is refused rather than resolved later.
    #[test]
    fn patterns_that_cannot_be_proved_separate_are_refused() {
        assert!(
            WorkspaceMap::new(vec![rule("work", "**/notes"), rule("personal", "**/diary")])
                .is_err()
        );
        assert!(
            WorkspaceMap::new(vec![
                rule("work", "/tree/*"),
                rule("personal", "/tree/personal")
            ])
            .is_err()
        );
    }

    #[test]
    fn contexts_over_separate_subtrees_are_accepted() {
        assert!(
            WorkspaceMap::new(vec![
                rule("work", "/tree/work"),
                rule("personal", "/tree/personal"),
                rule("work", "/elsewhere/work"),
            ])
            .is_ok()
        );
    }

    /// Two rules of one context may cover each other: the boundary is between contexts.
    #[test]
    fn one_context_may_cover_its_own_paths_twice() {
        assert!(WorkspaceMap::new(vec![rule("work", "/tree"), rule("work", "/tree/docs")]).is_ok());
    }

    #[test]
    fn the_only_configured_context_is_taken_and_several_are_refused() {
        let alone = WorkspaceMap::new(vec![rule("work", "/tree/work")]).unwrap();
        assert_eq!(alone.select(None).unwrap().as_str(), "work");
        assert_eq!(alone.select(Some("work")).unwrap().as_str(), "work");
        assert!(matches!(
            alone.select(Some("personal")),
            Err(WorkspaceError::UnknownWorkspace { .. })
        ));

        let several = WorkspaceMap::new(vec![
            rule("work", "/tree/work"),
            rule("personal", "/tree/personal"),
        ])
        .unwrap();
        assert!(matches!(
            several.select(None),
            Err(WorkspaceError::AmbiguousWorkspace { .. })
        ));
        assert_eq!(
            several.select(Some("personal")).unwrap().as_str(),
            "personal"
        );

        let none = WorkspaceMap::new(Vec::new()).unwrap();
        assert!(matches!(
            none.select(None),
            Err(WorkspaceError::NoWorkspaceConfigured)
        ));
    }

    #[test]
    fn a_context_denies_content_egress_until_it_says_otherwise() {
        let work = WorkspaceName::parse("work").unwrap();
        let personal = WorkspaceName::parse("personal").unwrap();
        let map = WorkspaceMap::new(vec![
            rule("work", "/tree/work"),
            rule("personal", "/tree/personal"),
        ])
        .unwrap();

        for context in [&work, &personal] {
            assert_eq!(map.content_egress(context), ContentEgress::Denied);
            // The proof a sender needs cannot be produced for a denying context.
            assert!(map.content_egress(context).permit().is_none());
        }
    }

    #[test]
    fn only_the_contexts_that_said_so_may_send_content_out() {
        let work = WorkspaceName::parse("work").unwrap();
        let personal = WorkspaceName::parse("personal").unwrap();
        let map = WorkspaceMap::with_egress(
            vec![
                rule("work", "/tree/work"),
                rule("personal", "/tree/personal"),
            ],
            vec![personal.clone()],
        )
        .unwrap();

        assert_eq!(map.content_egress(&personal), ContentEgress::Allowed);
        assert!(map.content_egress(&personal).permit().is_some());
        assert_eq!(map.content_egress(&work), ContentEgress::Denied);
        assert!(map.content_egress(&work).permit().is_none());
        // A context nobody declared is denied like every context that said nothing.
        assert_eq!(
            map.content_egress(&WorkspaceName::parse("other").unwrap()),
            ContentEgress::Denied
        );
    }

    #[test]
    fn a_permission_for_a_context_that_covers_nothing_is_refused() {
        let error = WorkspaceMap::with_egress(
            vec![rule("work", "/tree/work")],
            vec![WorkspaceName::parse("persnal").unwrap()],
        )
        .unwrap_err();

        assert!(
            matches!(error, WorkspaceError::EgressForUnknownWorkspace(ref name) if name.as_str() == "persnal"),
            "{error:?}"
        );
    }
}
