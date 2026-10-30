// Which part of a context a pass walks.
//
// Until ADR 0050 an event was a flag and nothing more: something under some root changed, so
// every root was walked again — 22 of them on the owner's `work`, minutes of work for one
// edited file. Now an event's paths say where, and a pass after events walks the directories
// they lie in. The rule that made the flag safe stays where it matters: the paths decide only
// where to look, never what is in the graph, and every sign that events were lost — and every
// start — still costs a pass over the whole context.
use std::collections::HashSet;

use crate::domain::{IndexPolicy, NormalizedPath, PolicyClass};

/// Above this many directories a scoped pass is no cheaper to reason about than a whole one,
/// and a burst that large — a checkout, an unpacked archive — is walked whole.
pub const MOST_SCOPED_DIRECTORIES: usize = 64;

/// Above this many distinct paths between two passes the set is not kept at all: what it would
/// have said is "a great deal changed", which is a whole pass.
pub const MOST_CHANGED_PATHS: usize = 10_000;

/// The paths change hints named since the last pass, as the event source collected them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedPaths {
    paths: HashSet<NormalizedPath>,
    /// A hint arrived whose path could not be kept — not representable, or past the limit — so
    /// what changed is not known.
    unknown: bool,
}

impl ChangedPaths {
    /// Nothing is known about where: the answer every source gives that keeps no paths.
    pub fn unknown() -> Self {
        Self {
            paths: HashSet::new(),
            unknown: true,
        }
    }

    pub fn note(&mut self, path: NormalizedPath) {
        if self.unknown {
            return;
        }
        if self.paths.len() >= MOST_CHANGED_PATHS {
            self.note_unknown();
            return;
        }
        self.paths.insert(path);
    }

    pub fn note_unknown(&mut self) {
        self.unknown = true;
        self.paths.clear();
    }

    pub fn is_unknown(&self) -> bool {
        self.unknown
    }

    pub fn paths(&self) -> impl Iterator<Item = &NormalizedPath> {
        self.paths.iter()
    }
}

/// What a pass walks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassScope {
    /// Every root of the context.
    Whole,
    /// The directories these paths lie in, worked out by `directories_to_walk`.
    Changes(ChangedPaths),
}

/// What a scoped pass needs to know about the disk: whether a directory is there, and whether
/// it is one a walk skips whole by its marker.
pub trait ScopeProbe {
    fn is_directory(&self, path: &NormalizedPath) -> bool;
    fn is_generated_directory(&self, path: &NormalizedPath) -> bool;
}

/// The directories a pass over `changed` walks, or `None` when it has to walk the whole
/// context.
///
/// For each path: the directory it lies in, or the nearest one of its ancestors still on disk
/// when the change was a removal — so a file moved from one directory to another has both
/// walked, and a directory removed whole is retired by a walk of its parent. A path no root of
/// the context holds is a whole pass: the backend reports real paths, and a root configured
/// through a link would otherwise drop every change made under it. A path
/// the full walk would never reach — excluded by the policy, or under a directory skipped by
/// its marker — is dropped exactly as the full walk drops it, so a scoped walk never indexes
/// what a whole one would not. What is left is reduced to the directories no other one covers.
pub fn directories_to_walk(
    changed: &ChangedPaths,
    roots: &[NormalizedPath],
    policy: &IndexPolicy,
    probe: &dyn ScopeProbe,
) -> Option<Vec<NormalizedPath>> {
    if changed.is_unknown() {
        return None;
    }
    let mut directories: Vec<NormalizedPath> = Vec::new();
    for path in changed.paths() {
        let root = roots.iter().find(|root| path.is_within(root))?;
        let Some(directory) = walked_directory(path, root, probe) else {
            continue;
        };
        if reached_by_a_whole_walk(path, &directory, root, policy, probe) {
            directories.push(directory);
        }
    }
    let covered = covering(directories);
    (covered.len() <= MOST_SCOPED_DIRECTORIES).then_some(covered)
}

/// The directory that holds `path`, climbing to the nearest one still there.
fn walked_directory(
    path: &NormalizedPath,
    root: &NormalizedPath,
    probe: &dyn ScopeProbe,
) -> Option<NormalizedPath> {
    let mut directory = match path == root {
        true => root.clone(),
        false => path.parent()?,
    };
    while &directory != root && !probe.is_directory(&directory) {
        directory = directory.parent()?;
    }
    Some(directory)
}

/// Whether the whole walk of `root` would reach `path` at all: no directory on the way down to
/// `directory` is excluded or skipped by its marker, and `path` itself is not excluded.
fn reached_by_a_whole_walk(
    path: &NormalizedPath,
    directory: &NormalizedPath,
    root: &NormalizedPath,
    policy: &IndexPolicy,
    probe: &dyn ScopeProbe,
) -> bool {
    let walked = policy.with_scan_root(root);
    if path != root && walked.decide(path).class() == PolicyClass::Exclude {
        return false;
    }
    let mut ancestor = Some(directory.clone());
    while let Some(current) = ancestor {
        if &current != root && walked.decide(&current).class() == PolicyClass::Exclude {
            return false;
        }
        if probe.is_generated_directory(&current) {
            return false;
        }
        if &current == root {
            return true;
        }
        ancestor = current.parent();
    }
    true
}

/// The directories none of the others already covers, each once.
fn covering(mut directories: Vec<NormalizedPath>) -> Vec<NormalizedPath> {
    directories.sort_by_key(|directory| directory.as_str().len());
    let mut kept: Vec<NormalizedPath> = Vec::new();
    for directory in directories {
        if !kept.iter().any(|covering| directory.is_within(covering)) {
            kept.push(directory);
        }
    }
    kept.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    kept
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{ChangedPaths, MOST_CHANGED_PATHS, ScopeProbe, directories_to_walk};
    use crate::domain::{
        IndexPolicy, NormalizedPath, PathPattern, PolicyClass, PolicyRule, RuleOrigin,
    };

    const ROOT: &str = "/Users/alice/Projects/payments-api";

    fn path(text: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(text).unwrap()
    }

    /// A tree described by its directories and its generated directories.
    struct Tree {
        directories: HashSet<String>,
        generated: HashSet<String>,
    }

    impl Tree {
        fn of(directories: &[&str], generated: &[&str]) -> Self {
            Self {
                directories: directories.iter().map(|d| format!("{ROOT}{d}")).collect(),
                generated: generated.iter().map(|d| format!("{ROOT}{d}")).collect(),
            }
        }
    }

    impl ScopeProbe for Tree {
        fn is_directory(&self, path: &NormalizedPath) -> bool {
            path.as_str() == ROOT || self.directories.contains(path.as_str())
        }
        fn is_generated_directory(&self, path: &NormalizedPath) -> bool {
            self.generated.contains(path.as_str())
        }
    }

    fn policy() -> IndexPolicy {
        IndexPolicy::new(
            vec![
                PolicyRule::new(
                    PolicyClass::Include,
                    PathPattern::parse(&format!("{ROOT}/**")).unwrap(),
                    RuleOrigin::Configuration,
                ),
                PolicyRule::new(
                    PolicyClass::Exclude,
                    PathPattern::parse("**/node_modules/**").unwrap(),
                    RuleOrigin::BuiltIn,
                ),
                PolicyRule::new(
                    PolicyClass::Exclude,
                    PathPattern::parse("**/*.key").unwrap(),
                    RuleOrigin::BuiltIn,
                ),
            ],
            16 * 1024 * 1024,
        )
    }

    fn walked(changed: &[&str], tree: &Tree) -> Option<Vec<String>> {
        let mut paths = ChangedPaths::default();
        for changed in changed {
            paths.note(path(&format!("{ROOT}{changed}")));
        }
        directories_to_walk(&paths, &[path(ROOT)], &policy(), tree).map(|dirs| {
            dirs.iter()
                .map(|d| d.as_str().trim_start_matches(ROOT).to_string())
                .collect()
        })
    }

    #[test]
    fn a_file_moved_between_directories_has_both_sides_walked() {
        let tree = Tree::of(&["/src", "/docs"], &[]);
        assert_eq!(
            walked(&["/src/retry.ts", "/docs/retry.ts"], &tree),
            Some(vec!["/docs".to_string(), "/src".to_string()])
        );
    }

    /// A renamed directory: its old name is gone, its new name is there, and both lie in the
    /// same parent — which is walked once and retires the old name with everything under it.
    #[test]
    fn a_renamed_directory_is_walked_from_the_parent_both_names_share() {
        let tree = Tree::of(&["/src", "/src/handlers"], &[]);
        assert_eq!(
            walked(&["/src/hooks", "/src/handlers"], &tree),
            Some(vec!["/src".to_string()])
        );
    }

    /// A directory removed with everything in it: the change names paths under it, none of
    /// which is on disk, and the walk climbs to the nearest ancestor that still is.
    #[test]
    fn a_directory_removed_whole_is_retired_by_a_walk_of_the_nearest_ancestor_left() {
        let tree = Tree::of(&["/src"], &[]);
        assert_eq!(
            walked(&["/src/legacy/old/a.ts", "/src/legacy"], &tree),
            Some(vec!["/src".to_string()])
        );
    }

    /// What the whole walk would never reach is not walked by a scoped one either.
    #[test]
    fn a_path_the_policy_excludes_or_a_generated_directory_holds_walks_nothing() {
        let tree = Tree::of(
            &[
                "/web",
                "/web/node_modules",
                "/web/node_modules/left-pad",
                "/build",
            ],
            &["/build"],
        );
        assert_eq!(
            walked(&["/web/node_modules/left-pad/index.js"], &tree),
            Some(vec![])
        );
        assert_eq!(walked(&["/web/secret.key"], &tree), Some(vec![]));
        assert_eq!(walked(&["/build/out.js"], &tree), Some(vec![]));
    }

    /// Where no root holds a path, the scope cannot be trusted to have caught the change.
    #[test]
    fn a_path_outside_every_root_walks_the_whole_context() {
        let tree = Tree::of(&[], &[]);
        assert_eq!(walked(&[], &tree), Some(vec![]));
        let mut paths = ChangedPaths::default();
        paths.note(path("/private/Users/alice/Projects/payments-api/src/a.ts"));
        assert_eq!(
            directories_to_walk(&paths, &[path(ROOT)], &policy(), &tree),
            None
        );
    }

    #[test]
    fn a_directory_inside_another_is_walked_with_it() {
        let tree = Tree::of(&["/src", "/src/a", "/src/a/b"], &[]);
        assert_eq!(
            walked(&["/src/a/b/x.ts", "/src/y.ts"], &tree),
            Some(vec!["/src".to_string()])
        );
    }

    /// Not knowing where is a whole pass, and so is a burst too large to keep.
    #[test]
    fn an_unknown_change_or_too_many_paths_walk_the_whole_context() {
        let tree = Tree::of(&["/src"], &[]);
        assert_eq!(
            directories_to_walk(&ChangedPaths::unknown(), &[path(ROOT)], &policy(), &tree),
            None
        );
        let mut many = ChangedPaths::default();
        for index in 0..=MOST_CHANGED_PATHS {
            many.note(path(&format!("{ROOT}/src/f{index}.ts")));
        }
        assert!(many.is_unknown());
        let wide: Vec<String> = (0..65).map(|i| format!("/d{i}/x.ts")).collect();
        let wide_tree = Tree::of(
            &(0..65)
                .map(|i| format!("/d{i}"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            &[],
        );
        let refs: Vec<&str> = wide.iter().map(String::as_str).collect();
        assert_eq!(walked(&refs, &wide_tree), None);
    }
}
