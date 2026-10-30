// Index scope policy: Include, Metadata and Exclude rules and the deterministic decision
// for a normalized path. Nothing here touches the filesystem.
use std::fmt;

use super::source_path::NormalizedPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyClass {
    Include,
    Metadata,
    Exclude,
}

impl PolicyClass {
    /// When rules of different classes match one path the most restrictive class wins.
    fn restrictiveness(self) -> u8 {
        match self {
            Self::Include => 0,
            Self::Metadata => 1,
            Self::Exclude => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOrigin {
    BuiltIn,
    Configuration,
    /// A root a pass walks: a directory named on the command line, or one of the
    /// `include` directories of the context. It grants Include to its subtree but, like
    /// every Include rule, never outranks Metadata or Exclude.
    ///
    /// This grant exists only inside a pass, which is what made `policy explain` answer
    /// "no rule covers this path" about every working file the owner had until Stage 36:
    /// a context's `include` is its boundary, not an Include rule.
    ScanRoot,
    /// A directory that said, by the marker in it, that a tool made it. The walk met it
    /// and turned it into an Exclude rule for the rest of the run, so that everything
    /// downstream — above all the question of which paths a run may retire — sees one kind
    /// of "we do not look here" instead of two.
    GeneratedDirectory,
}

/// Rule ordering inside one class: more literal segments, then more segments of fixed
/// depth, then more literal characters. Rules with equal specificity keep declaration
/// order, so the first declared rule wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity {
    literal_segments: usize,
    fixed_segments: usize,
    literal_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternSegment {
    /// `**`: zero or more path segments.
    AnyDepth,
    Literal(String),
    /// A single segment containing `*` (any characters) or `?` (one character).
    Glob(Vec<char>),
}

impl PatternSegment {
    fn parse(segment: &str) -> Result<Self, PatternError> {
        if segment == "**" {
            return Ok(Self::AnyDepth);
        }
        if segment == "." || segment == ".." || segment.contains("**") {
            return Err(PatternError::InvalidSegment(segment.to_string()));
        }
        if segment.contains(['*', '?']) {
            return Ok(Self::Glob(segment.chars().collect()));
        }
        Ok(Self::Literal(segment.to_string()))
    }

    /// Whether one name could satisfy both segments. Two globs are not compared: deciding
    /// it exactly is not worth the risk of deciding it wrong, and the safe answer is that
    /// they might.
    fn may_name_the_same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::AnyDepth, _) | (_, Self::AnyDepth) => true,
            (Self::Literal(left), Self::Literal(right)) => left == right,
            (Self::Literal(literal), Self::Glob(pattern))
            | (Self::Glob(pattern), Self::Literal(literal)) => {
                glob_matches(pattern, &literal.chars().collect::<Vec<_>>())
            }
            (Self::Glob(_), Self::Glob(_)) => true,
        }
    }

    fn matches_name(&self, name: &str) -> bool {
        match self {
            Self::AnyDepth => true,
            Self::Literal(literal) => literal == name,
            Self::Glob(pattern) => glob_matches(pattern, &name.chars().collect::<Vec<_>>()),
        }
    }
}

fn glob_matches(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skip| glob_matches(rest, &name[skip..])),
        Some(('?', rest)) => name
            .split_first()
            .is_some_and(|(_, remaining)| glob_matches(rest, remaining)),
        Some((literal, rest)) => name.split_first().is_some_and(|(character, remaining)| {
            character == literal && glob_matches(rest, remaining)
        }),
    }
}

/// A path pattern anchored at the filesystem root (`/...`) or at any depth (`**/...`).
/// A pattern covers every path it matches and all of their descendants, so a directory
/// rule applies to the files inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPattern {
    source: String,
    segments: Vec<PatternSegment>,
}

impl PathPattern {
    pub fn parse(pattern: &str) -> Result<Self, PatternError> {
        let body = if let Some(rest) = pattern.strip_prefix('/') {
            rest
        } else if pattern == "**" || pattern.starts_with("**/") {
            pattern
        } else {
            return Err(PatternError::NotAnchored(pattern.to_string()));
        };
        let body = body.strip_suffix('/').unwrap_or(body);

        let mut segments = Vec::new();
        for segment in body.split('/').filter(|segment| !segment.is_empty()) {
            segments.push(PatternSegment::parse(segment)?);
        }
        if body.split('/').any(str::is_empty) && !body.is_empty() {
            return Err(PatternError::InvalidSegment(String::new()));
        }
        Ok(Self {
            source: pattern.to_string(),
            segments,
        })
    }

    /// A pattern that matches exactly this path and its descendants, treating `*` and
    /// `?` in the path as ordinary characters.
    pub fn literal(path: &NormalizedPath) -> Self {
        Self {
            source: path.as_str().to_string(),
            segments: path
                .segments()
                .map(|segment| PatternSegment::Literal(segment.to_string()))
                .collect(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// The one directory this pattern names, when it names one.
    ///
    /// A pattern of literal segments covers exactly one path and everything under it, so a
    /// command that has to start walking somewhere can start there. A pattern with `*`, `?`
    /// or `**` in it covers many directories and names none: the longest literal part of it
    /// would be a different pattern — `/tree/*/src` and `/tree` do not cover the same files
    /// — so there is nothing here to return, and a caller is told that rather than handed a
    /// guess.
    pub fn as_directory(&self) -> Option<String> {
        let mut path = String::new();
        for segment in &self.segments {
            let PatternSegment::Literal(literal) = segment else {
                return None;
            };
            path.push('/');
            path.push_str(literal);
        }
        Some(match path.is_empty() {
            true => "/".to_string(),
            false => path,
        })
    }

    pub fn matches(&self, path: &NormalizedPath) -> bool {
        let names: Vec<&str> = path.segments().collect();
        matches_segments(&self.segments, &names)
    }

    /// Whether some path could match both patterns. Two contexts may not both cover one
    /// path, and the answer has to be given before any path exists, so it is taken from the
    /// patterns themselves. Where the two cannot be told apart — two globs in the same
    /// position — the answer is that they may overlap: a boundary refuses a configuration it
    /// cannot prove separate rather than deciding later by precedence.
    pub fn may_overlap(&self, other: &Self) -> bool {
        segments_may_overlap(&self.segments, &other.segments)
    }

    pub fn specificity(&self) -> Specificity {
        let mut specificity = Specificity {
            literal_segments: 0,
            fixed_segments: 0,
            literal_chars: 0,
        };
        for segment in &self.segments {
            match segment {
                PatternSegment::AnyDepth => {}
                PatternSegment::Literal(literal) => {
                    specificity.literal_segments += 1;
                    specificity.fixed_segments += 1;
                    specificity.literal_chars += literal.chars().count();
                }
                PatternSegment::Glob(pattern) => {
                    specificity.fixed_segments += 1;
                    specificity.literal_chars += pattern
                        .iter()
                        .filter(|character| !matches!(character, '*' | '?'))
                        .count();
                }
            }
        }
        specificity
    }
}

/// An exhausted pattern covers the remaining path segments: they are descendants of the
/// matched entry.
fn matches_segments(pattern: &[PatternSegment], names: &[&str]) -> bool {
    match pattern.split_first() {
        None => true,
        Some((PatternSegment::AnyDepth, rest)) => {
            (0..=names.len()).any(|skip| matches_segments(rest, &names[skip..]))
        }
        Some((segment, rest)) => names.split_first().is_some_and(|(name, remaining)| {
            segment.matches_name(name) && matches_segments(rest, remaining)
        }),
    }
}

/// An exhausted pattern covers every descendant, so it shares paths with anything the
/// other pattern can still match. `AnyDepth` either stands for no segment at all or for one
/// the other pattern must match too.
fn segments_may_overlap(left: &[PatternSegment], right: &[PatternSegment]) -> bool {
    match (left.split_first(), right.split_first()) {
        (None, _) | (_, None) => true,
        (Some((PatternSegment::AnyDepth, rest)), _) => {
            segments_may_overlap(rest, right) || segments_may_overlap(left, &right[1..])
        }
        (_, Some((PatternSegment::AnyDepth, rest))) => {
            segments_may_overlap(left, rest) || segments_may_overlap(&left[1..], right)
        }
        (Some((first, left_rest)), Some((second, right_rest))) => {
            first.may_name_the_same(second) && segments_may_overlap(left_rest, right_rest)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    NotAnchored(String),
    InvalidSegment(String),
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnchored(pattern) => {
                write!(f, "pattern {pattern:?} must start with '/', '~/' or '**/'")
            }
            Self::InvalidSegment(segment) => write!(
                f,
                "pattern segment {segment:?} is not allowed; use a name, '*', '?' or a whole '**' segment"
            ),
        }
    }
}

impl std::error::Error for PatternError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRule {
    class: PolicyClass,
    pattern: PathPattern,
    origin: RuleOrigin,
}

impl PolicyRule {
    pub fn new(class: PolicyClass, pattern: PathPattern, origin: RuleOrigin) -> Self {
        Self {
            class,
            pattern,
            origin,
        }
    }

    pub fn class(&self) -> PolicyClass {
        self.class
    }

    pub fn pattern(&self) -> &PathPattern {
        &self.pattern
    }

    pub fn origin(&self) -> RuleOrigin {
        self.origin
    }

    fn outranks(&self, other: &Self) -> bool {
        (self.class.restrictiveness(), self.pattern.specificity())
            > (other.class.restrictiveness(), other.pattern.specificity())
    }
}

/// The complete index scope policy. Decisions are deterministic: Exclude beats Metadata,
/// Metadata beats Include, the most specific rule wins inside one class and a path no rule
/// covers is excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPolicy {
    rules: Vec<PolicyRule>,
    max_content_bytes: u64,
}

impl IndexPolicy {
    pub fn new(rules: Vec<PolicyRule>, max_content_bytes: u64) -> Self {
        Self {
            rules,
            max_content_bytes,
        }
    }

    pub fn max_content_bytes(&self) -> u64 {
        self.max_content_bytes
    }

    /// The policy used while scanning `root`: the root is granted Include, and every
    /// configured Metadata or Exclude rule still applies inside it.
    pub fn with_scan_root(&self, root: &NormalizedPath) -> Self {
        let mut rules = self.rules.clone();
        rules.push(PolicyRule::new(
            PolicyClass::Include,
            PathPattern::literal(root),
            RuleOrigin::ScanRoot,
        ));
        Self::new(rules, self.max_content_bytes)
    }

    /// The same policy with `rules` added, for what a run learned from the tree itself.
    ///
    /// A walk meets a generated directory only by looking at it, so its rule cannot be in
    /// the configuration; but once met it has to be a rule and not a private note, or the
    /// steps that come after the walk would have to be told about it one by one — and the
    /// first one that was not told buried 727 files that were on disk.
    pub fn with_discovered(&self, rules: &[PolicyRule]) -> Self {
        let mut all = self.rules.clone();
        all.extend_from_slice(rules);
        Self::new(all, self.max_content_bytes)
    }

    /// An Exclude rule covering `directory` and everything under it.
    pub fn generated_directory_rule(directory: &NormalizedPath) -> PolicyRule {
        PolicyRule::new(
            PolicyClass::Exclude,
            PathPattern::literal(directory),
            RuleOrigin::GeneratedDirectory,
        )
    }

    pub fn decide(&self, path: &NormalizedPath) -> PolicyDecision<'_> {
        let mut winner: Option<&PolicyRule> = None;
        for rule in self.rules.iter().filter(|rule| rule.pattern.matches(path)) {
            if winner.is_none_or(|current| rule.outranks(current)) {
                winner = Some(rule);
            }
        }
        PolicyDecision {
            class: winner.map_or(PolicyClass::Exclude, PolicyRule::class),
            rule: winner,
        }
    }
}

/// The decision for one path together with the rule that produced it. `rule` is `None`
/// when nothing matched and the safe default applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyDecision<'a> {
    class: PolicyClass,
    rule: Option<&'a PolicyRule>,
}

impl<'a> PolicyDecision<'a> {
    pub fn class(&self) -> PolicyClass {
        self.class
    }

    pub fn rule(&self) -> Option<&'a PolicyRule> {
        self.rule
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexPolicy, PathPattern, PatternError, PolicyClass, PolicyRule, RuleOrigin};
    use crate::domain::NormalizedPath;

    fn path(path: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(path).unwrap()
    }

    fn rule(class: PolicyClass, pattern: &str) -> PolicyRule {
        PolicyRule::new(
            class,
            PathPattern::parse(pattern).unwrap(),
            RuleOrigin::Configuration,
        )
    }

    fn matches(pattern: &str, candidate: &str) -> bool {
        PathPattern::parse(pattern)
            .unwrap()
            .matches(&path(candidate))
    }

    #[test]
    fn patterns_cover_matching_paths_and_their_descendants() {
        assert!(matches("/Users/alice/Documents", "/Users/alice/Documents"));
        assert!(matches(
            "/Users/alice/Documents",
            "/Users/alice/Documents/notes/a.md"
        ));
        assert!(!matches(
            "/Users/alice/Documents",
            "/Users/alice/Documents-old/a.md"
        ));
        assert!(matches("**/.git/**", "/repo/.git"));
        assert!(matches("**/.git/**", "/repo/.git/config"));
        assert!(!matches("**/.git/**", "/repo/src/git.rs"));
        assert!(matches("**/*.key", "/etc/ssl/server.key"));
        assert!(matches("**/*.key", "/server.key"));
        assert!(!matches("**/*.key", "/etc/ssl/server.keys"));
        assert!(matches("/a/?/c", "/a/b/c/d"));
        assert!(!matches("/a/?/c", "/a/bb/c"));
        assert!(matches("/", "/anything/at/all"));
    }

    #[test]
    fn patterns_must_be_anchored_and_well_formed() {
        assert_eq!(
            PathPattern::parse("node_modules"),
            Err(PatternError::NotAnchored("node_modules".to_string()))
        );
        assert_eq!(
            PathPattern::parse("/a/**b"),
            Err(PatternError::InvalidSegment("**b".to_string()))
        );
        assert_eq!(
            PathPattern::parse("/a/../b"),
            Err(PatternError::InvalidSegment("..".to_string()))
        );
        assert_eq!(
            PathPattern::parse("/a//b"),
            Err(PatternError::InvalidSegment(String::new()))
        );
        assert!(PathPattern::parse("/Users/alice/Documents/").is_ok());
    }

    #[test]
    fn exclude_beats_metadata_and_metadata_beats_include() {
        let policy = IndexPolicy::new(
            vec![
                rule(PolicyClass::Include, "/home/alice"),
                rule(PolicyClass::Metadata, "/home/alice/Pictures"),
                rule(PolicyClass::Exclude, "**/*.key"),
                rule(PolicyClass::Include, "/home/alice/Pictures/shared"),
            ],
            100,
        );

        assert_eq!(
            policy.decide(&path("/home/alice/notes.md")).class(),
            PolicyClass::Include
        );
        assert_eq!(
            policy.decide(&path("/home/alice/Pictures/cat.jpg")).class(),
            PolicyClass::Metadata
        );
        assert_eq!(
            policy
                .decide(&path("/home/alice/Pictures/shared/dog.jpg"))
                .class(),
            PolicyClass::Metadata
        );
        assert_eq!(
            policy
                .decide(&path("/home/alice/Pictures/server.key"))
                .class(),
            PolicyClass::Exclude
        );
        assert_eq!(
            policy.decide(&path("/home/alice/server.key")).class(),
            PolicyClass::Exclude
        );
    }

    #[test]
    fn a_path_no_rule_covers_is_excluded_by_default() {
        let policy = IndexPolicy::new(vec![rule(PolicyClass::Include, "/home/alice")], 100);
        let decision = policy.decide(&path("/home/bob/notes.md"));

        assert_eq!(decision.class(), PolicyClass::Exclude);
        assert!(decision.rule().is_none());
    }

    #[test]
    fn the_most_specific_rule_of_the_winning_class_is_reported() {
        let policy = IndexPolicy::new(
            vec![
                rule(PolicyClass::Exclude, "**/.git/**"),
                rule(PolicyClass::Exclude, "/repo/.git/hooks"),
                rule(PolicyClass::Exclude, "/repo/*/hooks"),
                rule(PolicyClass::Exclude, "/repo/**/hooks"),
            ],
            100,
        );

        let decision = policy.decide(&path("/repo/.git/hooks/pre-commit"));
        assert_eq!(decision.class(), PolicyClass::Exclude);
        assert_eq!(
            decision.rule().unwrap().pattern().as_str(),
            "/repo/.git/hooks"
        );

        let fixed_depth = policy.decide(&path("/repo/other/hooks/x"));
        assert_eq!(
            fixed_depth.rule().unwrap().pattern().as_str(),
            "/repo/*/hooks"
        );
    }

    #[test]
    fn equally_specific_rules_keep_declaration_order() {
        let policy = IndexPolicy::new(
            vec![
                rule(PolicyClass::Exclude, "**/*.pem"),
                rule(PolicyClass::Exclude, "**/*.key"),
            ],
            100,
        );
        let decision = policy.decide(&path("/secrets/a.key"));
        assert_eq!(decision.rule().unwrap().pattern().as_str(), "**/*.key");
    }

    #[test]
    fn scan_root_grants_include_without_bypassing_exclude_or_metadata() {
        let configured = IndexPolicy::new(
            vec![
                rule(PolicyClass::Exclude, "**/.env"),
                rule(PolicyClass::Metadata, "/work/app/assets"),
            ],
            100,
        );
        assert_eq!(
            configured.decide(&path("/work/app/src/main.rs")).class(),
            PolicyClass::Exclude
        );

        let scanning = configured.with_scan_root(&path("/work/app"));
        let granted = scanning.decide(&path("/work/app/src/main.rs"));
        assert_eq!(granted.class(), PolicyClass::Include);
        assert_eq!(granted.rule().unwrap().origin(), RuleOrigin::ScanRoot);
        assert_eq!(
            scanning.decide(&path("/work/app/.env")).class(),
            PolicyClass::Exclude
        );
        assert_eq!(
            scanning.decide(&path("/work/app/assets/logo.png")).class(),
            PolicyClass::Metadata
        );
        assert_eq!(
            scanning.decide(&path("/work/other/file.txt")).class(),
            PolicyClass::Exclude
        );
    }
}
