// What one walk of one tree saw, and the draft of contexts it leads to.
//
// This module holds the evidence and the answer; the rules that turn one into the other are
// in `context_grouping`, and the shape of the answer is in `proposed_context`. Nothing here
// reads a file, reaches the network or decides by likelihood: a directory is proposed
// because a named thing was found in it, and the sentence saying which thing travels with
// the proposal. The operator decides; this only prepares the decision.
use super::context_grouping;
use super::project_signals::{HomePlace, ProjectMarker, RepositorySignal};
use super::proposed_context::{ProposedContext, UnattributedSubtree};
use super::source_path::NormalizedPath;

/// How much there is under a directory. Depth is counted in files, because a file is what
/// the walk can vouch for having seen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubtreeVolume {
    files: u64,
    bytes: u64,
    deepest_levels: u32,
}

impl SubtreeVolume {
    pub fn new(files: u64, bytes: u64, deepest_levels: u32) -> Self {
        Self {
            files,
            bytes,
            deepest_levels,
        }
    }

    pub fn files(&self) -> u64 {
        self.files
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// How many directory levels below this one the deepest file lies. A file lying directly
    /// in the directory makes this one.
    pub fn deepest_levels(&self) -> u32 {
        self.deepest_levels
    }

    pub fn is_empty(&self) -> bool {
        self.files == 0
    }
}

/// One directory the walk has something to say about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeEvidence {
    path: NormalizedPath,
    volume: SubtreeVolume,
    repository: Option<RepositorySignal>,
    markers: Vec<ProjectMarker>,
    home_place: Option<HomePlace>,
}

impl SubtreeEvidence {
    pub fn new(
        path: NormalizedPath,
        volume: SubtreeVolume,
        repository: Option<RepositorySignal>,
        markers: Vec<ProjectMarker>,
        home_place: Option<HomePlace>,
    ) -> Self {
        Self {
            path,
            volume,
            repository,
            markers,
            home_place,
        }
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }

    pub(super) fn volume(&self) -> SubtreeVolume {
        self.volume
    }

    pub(super) fn repository(&self) -> Option<&RepositorySignal> {
        self.repository.as_ref()
    }

    pub(super) fn markers(&self) -> &[ProjectMarker] {
        &self.markers
    }

    pub(super) fn home_place(&self) -> Option<HomePlace> {
        self.home_place
    }

    /// Whether anything at all names this directory as the root of something.
    pub(super) fn is_a_candidate(&self) -> bool {
        self.repository.is_some() || !self.markers.is_empty()
    }
}

/// What the walk could not do, counted rather than guessed around.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WalkOutcome {
    pub symlinks: u64,
    /// Directories skipped whole because they carry a marker saying a tool made them.
    pub generated_directories: u64,
    pub special_entries: u64,
    pub unrepresentable_names: u64,
    pub unreadable_git_configs: u64,
    /// Directories the system would not let this process open (`EPERM`): on macOS the places
    /// behind privacy protection — the Trash, Photos, parts of `Library` — when the terminal
    /// has no Full Disk Access. Counted, because naming a hundred of them reads as breakage and
    /// none of them is a place anybody keeps a project.
    pub closed_by_the_system: u64,
    /// Every other path the walk could not inspect, by name.
    pub failures: Vec<String>,
}

/// Everything one walk of one root observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEvidence {
    pub root: NormalizedPath,
    pub whole: SubtreeVolume,
    /// Files lying directly in the root, belonging to none of its directories.
    pub loose_at_the_root: SubtreeVolume,
    /// Every directory with something to say, sorted by path, each named once.
    pub subtrees: Vec<SubtreeEvidence>,
    pub walk: WalkOutcome,
}

/// A draft of contexts for one tree, with what every line of it stands on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextProposal {
    root: NormalizedPath,
    whole: SubtreeVolume,
    contexts: Vec<ProposedContext>,
    unattributed: Vec<UnattributedSubtree>,
    /// The walked root is itself a repository. It is never proposed — one context over the
    /// whole root is the decision the operator asked to have prepared, not made — so it is
    /// said out loud instead.
    root_repository: Option<RepositorySignal>,
    walk: WalkOutcome,
}

impl ContextProposal {
    pub fn root(&self) -> &NormalizedPath {
        &self.root
    }

    pub fn whole(&self) -> SubtreeVolume {
        self.whole
    }

    pub fn contexts(&self) -> &[ProposedContext] {
        &self.contexts
    }

    pub fn unattributed(&self) -> &[UnattributedSubtree] {
        &self.unattributed
    }

    pub fn root_repository(&self) -> Option<&RepositorySignal> {
        self.root_repository.as_ref()
    }

    pub fn walk(&self) -> &WalkOutcome {
        &self.walk
    }

    /// Whether the tree held any file at all. An empty root is answered, not proposed for.
    pub fn is_empty(&self) -> bool {
        self.whole.is_empty()
    }

    /// The draft itself: contexts grouped by the evidence they stand on, and everything the
    /// evidence did not reach listed beside them.
    pub fn assemble(evidence: TreeEvidence) -> Self {
        let TreeEvidence {
            root,
            whole,
            loose_at_the_root,
            subtrees,
            walk,
        } = evidence;

        let root_repository = subtrees
            .iter()
            .find(|subtree| *subtree.path() == root)
            .and_then(|subtree| subtree.repository.clone());
        let (contexts, unattributed) = context_grouping::draft(&subtrees, &root, loose_at_the_root);

        Self {
            root,
            whole,
            contexts,
            unattributed,
            root_repository,
            walk,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextProposal, SubtreeEvidence, SubtreeVolume, TreeEvidence, WalkOutcome};
    use crate::domain::{
        ContextBasis, Evidence, GitMarker, HomePlace, NormalizedPath, NotAttributed, ProjectMarker,
        RemoteHost, RepositoryRemote, RepositorySignal,
    };

    const HOME: &str = "/home/alice";

    fn path(path: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(path).unwrap()
    }

    fn volume(files: u64) -> SubtreeVolume {
        SubtreeVolume::new(files, files * 100, 2)
    }

    fn host(name: &str) -> RemoteHost {
        RemoteHost::of_remote_url(&format!("https://{name}/repo")).unwrap()
    }

    fn repository(remote: RepositoryRemote) -> Option<RepositorySignal> {
        Some(RepositorySignal::new(GitMarker::Directory, remote, None))
    }

    fn subtree(
        at: &str,
        files: u64,
        repository: Option<RepositorySignal>,
        markers: &[&str],
    ) -> SubtreeEvidence {
        SubtreeEvidence::new(
            path(at),
            volume(files),
            repository,
            markers
                .iter()
                .map(|name| ProjectMarker::of_file_name(name).unwrap())
                .collect(),
            HomePlace::of_home_child(path(at).file_name())
                .filter(|_| path(at).parent().as_ref() == Some(&path(HOME))),
        )
    }

    /// `subtrees` must be sorted by path, which is the order the walk produces them in.
    fn assemble(subtrees: Vec<SubtreeEvidence>, loose: SubtreeVolume) -> ContextProposal {
        let mut subtrees = subtrees;
        subtrees.sort_by(|left, right| left.path().as_str().cmp(right.path().as_str()));
        let files = subtrees
            .iter()
            .filter(|subtree| subtree.path().parent().as_ref() == Some(&path(HOME)))
            .map(|subtree| subtree.volume().files())
            .sum::<u64>()
            + loose.files();
        ContextProposal::assemble(TreeEvidence {
            root: path(HOME),
            whole: SubtreeVolume::new(files, files * 100, 4),
            loose_at_the_root: loose,
            subtrees,
            walk: WalkOutcome::default(),
        })
    }

    fn context_names(proposal: &ContextProposal) -> Vec<String> {
        proposal
            .contexts()
            .iter()
            .map(|context| context.name().as_str().to_string())
            .collect()
    }

    fn includes_of(proposal: &ContextProposal, name: &str) -> Vec<String> {
        proposal
            .contexts()
            .iter()
            .find(|context| context.name().as_str() == name)
            .map(|context| {
                context
                    .includes()
                    .iter()
                    .map(|include| include.path().as_str().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn repositories_are_grouped_by_the_host_their_remote_names() {
        let proposal = assemble(
            vec![
                subtree("/home/alice/Projects", 30, None, &[]),
                subtree(
                    "/home/alice/Projects/work-one",
                    10,
                    repository(RepositoryRemote::Host(host("git.example.org"))),
                    &[],
                ),
                subtree(
                    "/home/alice/Projects/work-two",
                    10,
                    repository(RepositoryRemote::Host(host("git.example.org"))),
                    &[],
                ),
                subtree(
                    "/home/alice/Projects/mine",
                    10,
                    repository(RepositoryRemote::Host(host("github.com"))),
                    &[],
                ),
            ],
            SubtreeVolume::default(),
        );

        assert_eq!(context_names(&proposal), ["git-example-org", "github-com"]);
        assert_eq!(
            includes_of(&proposal, "git-example-org"),
            [
                "/home/alice/Projects/work-one",
                "/home/alice/Projects/work-two"
            ]
        );
        assert_eq!(
            includes_of(&proposal, "github-com"),
            ["/home/alice/Projects/mine"]
        );
        assert!(
            proposal.unattributed().is_empty(),
            "{:?}",
            proposal.unattributed()
        );
    }

    #[test]
    fn a_repository_inside_a_repository_is_covered_by_the_one_above_it() {
        let proposal = assemble(
            vec![
                subtree("/home/alice/Projects", 10, None, &[]),
                subtree(
                    "/home/alice/Projects/outer",
                    10,
                    repository(RepositoryRemote::Host(host("github.com"))),
                    &[],
                ),
                subtree(
                    "/home/alice/Projects/outer/vendor/inner",
                    4,
                    repository(RepositoryRemote::Host(host("git.example.org"))),
                    &[],
                ),
            ],
            SubtreeVolume::default(),
        );

        assert_eq!(context_names(&proposal), ["github-com"]);
        assert_eq!(
            includes_of(&proposal, "github-com"),
            ["/home/alice/Projects/outer"]
        );
        let include = &proposal.contexts()[0].includes()[0];
        assert!(
            include
                .evidence()
                .contains(&Evidence::NestedRepositories(1)),
            "the proposal must say what it covers: {:?}",
            include.evidence()
        );
    }

    #[test]
    fn a_repository_whose_remotes_name_several_hosts_is_left_to_the_operator() {
        let hosts = vec![host("git.example.org"), host("github.com")];
        let proposal = assemble(
            vec![
                subtree("/home/alice/Projects", 12, None, &[]),
                subtree(
                    "/home/alice/Projects/split",
                    12,
                    repository(RepositoryRemote::SeveralHosts(hosts.clone())),
                    &[],
                ),
            ],
            SubtreeVolume::default(),
        );

        assert!(proposal.contexts().is_empty());
        let left = proposal.unattributed();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path().as_str(), "/home/alice/Projects/split");
        assert_eq!(*left[0].reason(), NotAttributed::SeveralRemoteHosts(hosts));
    }

    #[test]
    fn a_project_without_a_repository_and_a_repository_without_a_remote_are_separate_groups() {
        let proposal = assemble(
            vec![
                subtree("/home/alice/Projects", 20, None, &[]),
                subtree(
                    "/home/alice/Projects/local",
                    10,
                    repository(RepositoryRemote::NoRemote),
                    &[],
                ),
                subtree("/home/alice/Projects/plain", 10, None, &["Cargo.toml"]),
            ],
            SubtreeVolume::default(),
        );

        assert_eq!(
            context_names(&proposal),
            ["without-remote", "without-repository"]
        );
        assert_eq!(
            includes_of(&proposal, "without-remote"),
            ["/home/alice/Projects/local"]
        );
        assert_eq!(
            includes_of(&proposal, "without-repository"),
            ["/home/alice/Projects/plain"]
        );
    }

    #[test]
    fn personal_documents_are_offered_whole_only_while_no_project_sits_inside_them() {
        let proposal = assemble(
            vec![
                subtree("/home/alice/Documents", 40, None, &[]),
                subtree("/home/alice/Desktop", 5, None, &[]),
                subtree(
                    "/home/alice/Desktop/experiment",
                    5,
                    repository(RepositoryRemote::Host(host("github.com"))),
                    &[],
                ),
                subtree("/home/alice/Downloads", 900, None, &[]),
                subtree("/home/alice/.cargo", 5000, None, &[]),
            ],
            SubtreeVolume::default(),
        );

        assert_eq!(
            includes_of(&proposal, "personal"),
            ["/home/alice/Documents"]
        );
        let reasons: Vec<(&str, &NotAttributed)> = proposal
            .unattributed()
            .iter()
            .map(|left| (left.path().as_str(), left.reason()))
            .collect();
        assert_eq!(
            reasons,
            [
                ("/home/alice/.cargo", &NotAttributed::ToolStore),
                ("/home/alice/Downloads", &NotAttributed::MixedInbox),
            ],
            "the Desktop holds a repository, so nothing of it is proposed whole"
        );
    }

    #[test]
    fn files_beside_the_projects_are_reported_without_being_counted_twice() {
        let proposal = assemble(
            vec![
                subtree("/home/alice/Projects", 25, None, &[]),
                subtree(
                    "/home/alice/Projects/one",
                    10,
                    repository(RepositoryRemote::Host(host("github.com"))),
                    &[],
                ),
            ],
            SubtreeVolume::new(3, 300, 1),
        );

        let left = proposal.unattributed();
        assert_eq!(
            left.iter()
                .map(|subtree| (
                    subtree.path().as_str(),
                    subtree.remaining_files(),
                    subtree.reason()
                ))
                .collect::<Vec<_>>(),
            [
                ("/home/alice/Projects", 15, &NotAttributed::BesideProjects),
                ("/home/alice", 3, &NotAttributed::DirectlyInTheRoot),
            ]
        );
        assert_eq!(
            left[0].volume().files(),
            25,
            "the whole subtree is still reported"
        );
        assert!(!left[0].covers_the_whole_subtree());
    }

    #[test]
    fn two_hosts_that_spell_one_name_still_become_two_contexts() {
        let proposal = assemble(
            vec![
                subtree("/home/alice/Projects", 20, None, &[]),
                subtree(
                    "/home/alice/Projects/a",
                    10,
                    repository(RepositoryRemote::Host(host("git.example.org"))),
                    &[],
                ),
                subtree(
                    "/home/alice/Projects/b",
                    10,
                    repository(RepositoryRemote::Host(host("git-example-org"))),
                    &[],
                ),
            ],
            SubtreeVolume::default(),
        );

        assert_eq!(
            context_names(&proposal),
            ["git-example-org", "git-example-org-2"]
        );
        assert_eq!(
            *proposal.contexts()[1].basis(),
            ContextBasis::RemoteHost(host("git.example.org"))
        );
    }

    #[test]
    fn a_root_that_is_itself_a_repository_is_reported_and_never_proposed() {
        let proposal = assemble(
            vec![
                subtree(
                    HOME,
                    10,
                    repository(RepositoryRemote::Host(host("github.com"))),
                    &[],
                ),
                subtree("/home/alice/Projects", 10, None, &[]),
            ],
            SubtreeVolume::default(),
        );

        assert!(proposal.contexts().is_empty());
        assert!(proposal.root_repository().is_some());
        assert_eq!(
            proposal
                .unattributed()
                .iter()
                .map(|left| left.path().as_str())
                .collect::<Vec<_>>(),
            ["/home/alice/Projects"]
        );
    }

    #[test]
    fn a_path_a_coverage_rule_cannot_spell_is_left_to_the_operator() {
        let proposal = assemble(
            vec![
                subtree("/home/alice/Projects", 10, None, &[]),
                subtree(
                    "/home/alice/Projects/star*name",
                    10,
                    repository(RepositoryRemote::Host(host("github.com"))),
                    &[],
                ),
            ],
            SubtreeVolume::default(),
        );

        assert!(proposal.contexts().is_empty());
        assert_eq!(
            *proposal.unattributed()[0].reason(),
            NotAttributed::PathIsNotExpressible
        );
    }

    #[test]
    fn an_empty_root_proposes_nothing() {
        let proposal = assemble(Vec::new(), SubtreeVolume::default());

        assert!(proposal.is_empty());
        assert!(proposal.contexts().is_empty());
        assert!(proposal.unattributed().is_empty());
    }
}
