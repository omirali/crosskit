// The rules that turn evidence into a draft: which directories are worth a context of their
// own, which context each of them belongs in, and what is left over.
//
// Nothing here is decided by likelihood. A directory is proposed because a named thing was
// found in it, and where no rule reaches, the draft says so instead of choosing. The one
// constraint every rule serves is that two contexts may not cover one path (ADR 0008), so a
// draft that could not be loaded cannot be printed either.
use super::context_proposal::{SubtreeEvidence, SubtreeVolume};
use super::git_remote::RepositoryRemote;
use super::project_signals::{HomePlace, ProjectMarker, RepositorySignal};
use super::proposed_context::{
    ContextBasis, Evidence, NotAttributed, ProposedContext, ProposedInclude, UnattributedSubtree,
    unique_name,
};
use super::source_path::NormalizedPath;
use super::workspace::WorkspaceName;

/// The draft for one walked tree: the contexts and everything they do not cover.
///
/// `subtrees` must be sorted by path and name every directory once, which is what lets a
/// candidate be compared against the ones already kept rather than against all of them.
pub(super) fn draft(
    subtrees: &[SubtreeEvidence],
    root: &NormalizedPath,
    loose_at_the_root: SubtreeVolume,
) -> (Vec<ProposedContext>, Vec<UnattributedSubtree>) {
    let (placed, undecided) = split_by_decidability(maximal_candidates(subtrees, root));
    let personal = personal_documents(subtrees, root, &placed);
    let contexts = group_into_contexts(placed, personal);

    let named: Vec<&NormalizedPath> = contexts
        .iter()
        .flat_map(|context| context.includes().iter().map(ProposedInclude::path))
        .chain(undecided.iter().map(|candidate| &candidate.path))
        .collect();
    let left = unattributed(subtrees, root, loose_at_the_root, &named, &undecided);
    (contexts, left)
}

/// One directory that carries enough to be a context of its own.
#[derive(Debug, Clone)]
struct Candidate {
    path: NormalizedPath,
    volume: SubtreeVolume,
    repository: Option<RepositorySignal>,
    markers: Vec<ProjectMarker>,
    home_place: Option<HomePlace>,
    nested_repositories: usize,
}

impl Candidate {
    fn of(evidence: &SubtreeEvidence) -> Self {
        Self {
            path: evidence.path().clone(),
            volume: evidence.volume(),
            repository: evidence.repository().cloned(),
            markers: evidence.markers().to_vec(),
            home_place: evidence.home_place(),
            nested_repositories: 0,
        }
    }

    /// The proposal this candidate becomes, carrying every piece of evidence behind it. A
    /// directory offered for its place in the home layout has exactly that as its evidence,
    /// which is why the place is only named when nothing stronger was found.
    fn into_include(self) -> ProposedInclude {
        let mut evidence = Vec::new();
        if let Some(repository) = self.repository {
            evidence.push(Evidence::Repository(repository));
        }
        let mut markers = self.markers;
        markers.sort();
        markers.dedup();
        evidence.extend(markers.into_iter().map(Evidence::Marker));
        if let (true, Some(place)) = (evidence.is_empty(), self.home_place) {
            evidence.push(Evidence::HomePlace(place));
        }
        if self.nested_repositories > 0 {
            evidence.push(Evidence::NestedRepositories(self.nested_repositories));
        }
        ProposedInclude::new(self.path, self.volume, evidence)
    }
}

/// A candidate no rule places, with the reason no rule places it.
struct Undecided {
    path: NormalizedPath,
    volume: SubtreeVolume,
    reason: NotAttributed,
}

/// The candidates no other candidate contains. A repository inside a repository is covered
/// by the one above it, and the one above it is told how many it covers: two contexts may
/// not cover one path, so a draft that offered both would not load.
fn maximal_candidates(subtrees: &[SubtreeEvidence], root: &NormalizedPath) -> Vec<Candidate> {
    let mut maximal: Vec<Candidate> = Vec::new();
    for subtree in subtrees
        .iter()
        .filter(|subtree| subtree.path() != root && subtree.is_a_candidate())
    {
        let covering = maximal
            .iter_mut()
            .find(|kept| subtree.path().is_within(&kept.path));
        match covering {
            Some(covering) => {
                if subtree.repository().is_some() {
                    covering.nested_repositories += 1;
                }
            }
            None => maximal.push(Candidate::of(subtree)),
        }
    }
    maximal
}

/// Splits the candidates into the ones a rule places and the ones no rule places. Nothing is
/// placed by preference: a repository whose remotes disagree stays undecided, because which
/// of its hosts the work belongs to is not written anywhere on this disk.
fn split_by_decidability(
    candidates: Vec<Candidate>,
) -> (Vec<(ContextBasis, Candidate)>, Vec<Undecided>) {
    let mut placed = Vec::new();
    let mut undecided = Vec::new();
    for candidate in candidates {
        if candidate.path.as_str().contains(['*', '?']) {
            undecided.push(Undecided {
                path: candidate.path,
                volume: candidate.volume,
                reason: NotAttributed::PathIsNotExpressible,
            });
            continue;
        }
        let basis = match candidate.repository.as_ref().map(RepositorySignal::remote) {
            Some(RepositoryRemote::Host(host)) => ContextBasis::RemoteHost(host.clone()),
            Some(RepositoryRemote::SeveralHosts(hosts)) => {
                undecided.push(Undecided {
                    reason: NotAttributed::SeveralRemoteHosts(hosts.clone()),
                    path: candidate.path,
                    volume: candidate.volume,
                });
                continue;
            }
            Some(_) => ContextBasis::RepositoriesWithoutARemote,
            None => ContextBasis::ProjectsWithoutARepository,
        };
        placed.push((basis, candidate));
    }
    (placed, undecided)
}

/// The home directories of a person's own documents, offered whole — and only while no
/// project inside them would have to be carved out of the offer. A cut-out of a subtree is
/// not expressible, so a directory that holds one is left to the operator instead.
fn personal_documents(
    subtrees: &[SubtreeEvidence],
    root: &NormalizedPath,
    placed: &[(ContextBasis, Candidate)],
) -> Vec<Candidate> {
    subtrees
        .iter()
        .filter(|subtree| {
            subtree.home_place() == Some(HomePlace::PersonalDocuments)
                && subtree.path().parent().as_ref() == Some(root)
                && !subtree.is_a_candidate()
                && !subtree.volume().is_empty()
                && !placed
                    .iter()
                    .any(|(_, candidate)| candidate.path.is_within(subtree.path()))
        })
        .map(Candidate::of)
        .collect()
}

fn group_into_contexts(
    placed: Vec<(ContextBasis, Candidate)>,
    personal: Vec<Candidate>,
) -> Vec<ProposedContext> {
    let mut groups: Vec<(ContextBasis, Vec<Candidate>)> = Vec::new();
    let personal = personal
        .into_iter()
        .map(|candidate| (ContextBasis::PersonalDocuments, candidate));
    for (basis, candidate) in placed.into_iter().chain(personal) {
        match groups.iter_mut().find(|(existing, _)| *existing == basis) {
            Some((_, members)) => members.push(candidate),
            None => groups.push((basis, vec![candidate])),
        }
    }
    groups.sort_by_key(|(basis, _)| basis.rank());

    let mut taken: Vec<WorkspaceName> = Vec::new();
    groups
        .into_iter()
        .map(|(basis, members)| {
            let mut includes: Vec<ProposedInclude> =
                members.into_iter().map(Candidate::into_include).collect();
            includes.sort_by(|left, right| left.path().as_str().cmp(right.path().as_str()));
            ProposedContext::new(
                unique_name(&basis.suggested_name(), &mut taken),
                basis,
                includes,
            )
        })
        .collect()
}

/// Everything the draft does not cover and why. A top-level subtree that holds proposals is
/// reported by what is left beside them, so nothing is counted in two places.
fn unattributed(
    subtrees: &[SubtreeEvidence],
    root: &NormalizedPath,
    loose_at_the_root: SubtreeVolume,
    named: &[&NormalizedPath],
    undecided: &[Undecided],
) -> Vec<UnattributedSubtree> {
    let mut left = Vec::new();
    if !loose_at_the_root.is_empty() {
        left.push(UnattributedSubtree::new(
            root.clone(),
            loose_at_the_root,
            loose_at_the_root.files(),
            loose_at_the_root.bytes(),
            NotAttributed::DirectlyInTheRoot,
        ));
    }

    for subtree in subtrees
        .iter()
        .filter(|subtree| subtree.path().parent().as_ref() == Some(root))
    {
        if named.iter().any(|path| subtree.path() == *path) {
            continue;
        }
        let inside: Vec<&NormalizedPath> = named
            .iter()
            .copied()
            .filter(|path| path.is_within(subtree.path()))
            .collect();
        let covered = inside
            .iter()
            .map(|path| volume_of(subtrees, path))
            .fold((0_u64, 0_u64), |total, volume| {
                (total.0 + volume.files(), total.1 + volume.bytes())
            });
        let remaining_files = subtree.volume().files().saturating_sub(covered.0);
        if remaining_files == 0 {
            continue;
        }
        left.push(UnattributedSubtree::new(
            subtree.path().clone(),
            subtree.volume(),
            remaining_files,
            subtree.volume().bytes().saturating_sub(covered.1),
            reason_for(subtree, !inside.is_empty()),
        ));
    }

    for candidate in undecided {
        left.push(UnattributedSubtree::new(
            candidate.path.clone(),
            candidate.volume,
            candidate.volume.files(),
            candidate.volume.bytes(),
            candidate.reason.clone(),
        ));
    }

    left.sort_by(|first, second| {
        second
            .remaining_files()
            .cmp(&first.remaining_files())
            .then_with(|| first.path().as_str().cmp(second.path().as_str()))
    });
    left
}

/// Every named path is a directory the walk described, so a path with no volume would mean
/// the draft named something the walk never saw; an absent volume is zero rather than a
/// reason to stop, and the arithmetic above saturates for the same reason.
fn volume_of(subtrees: &[SubtreeEvidence], path: &NormalizedPath) -> SubtreeVolume {
    subtrees
        .iter()
        .find(|subtree| subtree.path() == path)
        .map(SubtreeEvidence::volume)
        .unwrap_or_default()
}

fn reason_for(subtree: &SubtreeEvidence, holds_proposals: bool) -> NotAttributed {
    match subtree.home_place() {
        Some(HomePlace::ToolStore) => NotAttributed::ToolStore,
        Some(HomePlace::MixedInbox) => NotAttributed::MixedInbox,
        Some(HomePlace::PersonalDocuments) if holds_proposals => NotAttributed::OverlapsProjects,
        _ if holds_proposals => NotAttributed::BesideProjects,
        _ => NotAttributed::NoSignal,
    }
}
