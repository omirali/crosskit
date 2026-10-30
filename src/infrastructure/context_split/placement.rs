// The domain's rules applied to what the censuses found: every repository's own answer and the
// one it actually gets, and what each transcript folder's name is the encoding of.
//
// Conversations, folders without git and Codex sessions are placed in `conversation_placement`;
// both halves only apply the rules of `domain::context_split`, and neither decides anything of
// its own.
use std::collections::BTreeMap;

use super::document_census::DocumentCensus;
use super::home_layout::HomeLayout;
use super::repository_census::FoundRepository;
use crate::domain::NormalizedPath;
use crate::domain::context_split::{
    DecidedBy, FolderPlace, Identity, Origin, OwnerDomain, RepositoryEvidence, SplitDecision,
    decide_repository, is_temporary_folder_name, transcript_folder_name,
};

/// One repository, with its own answer and the answer it gets.
#[derive(Debug, Clone)]
pub(super) struct PlacedRepository {
    pub(super) found: FoundRepository,
    pub(super) own: SplitDecision,
    /// The outermost repository holding this one, which is the one a context would cover.
    pub(super) enclosing: Option<NormalizedPath>,
    /// What it gets: its own answer, or the enclosing repository's.
    pub(super) effective: SplitDecision,
}

/// Every repository found, placed.
#[derive(Debug)]
pub(super) struct Repositories {
    placed: Vec<PlacedRepository>,
    owner: OwnerDomain,
}

impl Repositories {
    pub(super) fn place(found: Vec<FoundRepository>, owner: OwnerDomain) -> Self {
        let own: Vec<SplitDecision> = found
            .iter()
            .map(|repository| decide_repository(&evidence_of(repository), &owner))
            .collect();
        let placed = found
            .iter()
            .enumerate()
            .map(|(index, repository)| {
                let enclosing = found
                    .iter()
                    .enumerate()
                    .filter(|(_, other)| {
                        other.root != repository.root && repository.root.is_within(&other.root)
                    })
                    .min_by_key(|(_, other)| other.root.as_str().len());
                let effective = match enclosing {
                    Some((outer, _)) => follow_the_enclosing(own[outer]),
                    None => own[index],
                };
                PlacedRepository {
                    found: repository.clone(),
                    own: own[index],
                    enclosing: enclosing.map(|(_, outer)| outer.root.clone()),
                    effective,
                }
            })
            .collect();
        Self { placed, owner }
    }

    pub(super) fn all(&self) -> &[PlacedRepository] {
        &self.placed
    }

    /// The repositories no other holds: the ones a context would name.
    pub(super) fn outermost(&self) -> impl Iterator<Item = &PlacedRepository> {
        self.placed
            .iter()
            .filter(|repository| repository.enclosing.is_none())
    }

    /// The innermost repository a path lies in.
    pub(super) fn holding(&self, path: &NormalizedPath) -> Option<&PlacedRepository> {
        self.placed
            .iter()
            .filter(|repository| path.is_within(&repository.found.root))
            .max_by_key(|repository| repository.found.root.as_str().len())
    }

    pub(super) fn owner(&self) -> &OwnerDomain {
        &self.owner
    }
}

fn evidence_of(repository: &FoundRepository) -> RepositoryEvidence {
    RepositoryEvidence::new(
        repository.origin.clone().unwrap_or(Origin::Absent),
        repository.own_identity.clone(),
    )
}

/// A repository inside another gets the outer one's answer, and says why.
fn follow_the_enclosing(outer: SplitDecision) -> SplitDecision {
    match outer {
        SplitDecision::Decided { side, .. } => {
            SplitDecision::decided(side, DecidedBy::EnclosingRepository)
        }
        question => question,
    }
}

/// The one mail domain the owner acts under: the global `[user]`, each repository's own, and
/// every reflog line, with the addresses the machine made up left out.
pub(super) fn owner_domain_of(global: &[Identity], found: &[FoundRepository]) -> OwnerDomain {
    let from_repositories = found.iter().flat_map(|repository| {
        let own = repository
            .own_identity
            .iter()
            .filter_map(Identity::mail_domain)
            .cloned();
        let reflog = repository
            .reflog
            .iter()
            .flat_map(|reflog| reflog.domains().map(|(domain, _)| domain.clone()))
            .collect::<Vec<_>>();
        own.chain(reflog)
    });
    OwnerDomain::of(
        global
            .iter()
            .filter_map(Identity::mail_domain)
            .cloned()
            .chain(from_repositories),
    )
}

/// What a transcript folder's name is the encoding of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FolderKind {
    /// A repository's directory.
    Project(NormalizedPath),
    /// A directory holding repositories without being one: the home, `Projects`.
    Mixed(NormalizedPath),
    /// A document root or a folder without git.
    Document(NormalizedPath),
    /// A directory inside a temporary place.
    Temporary,
    /// Two known directories that encode to this one name: nothing is chosen between them.
    Ambiguous(Vec<NormalizedPath>),
    Unknown,
}

/// Every directory the measurement knows, by the folder name the tool would give it.
#[derive(Debug, Default)]
pub(super) struct FolderIndex {
    by_name: BTreeMap<String, FolderKind>,
}

impl FolderIndex {
    /// Repositories first, then the directories holding them, then the document folders: a
    /// name keeps the first kind it was given, and a second directory under the same name
    /// makes it ambiguous.
    pub(super) fn new(
        repositories: &Repositories,
        layout: &HomeLayout,
        documents: &DocumentCensus,
    ) -> Self {
        let mut index = Self::default();
        for repository in repositories.all() {
            index.know(FolderKind::Project(repository.found.root.clone()));
        }
        for repository in repositories.outermost() {
            let mut above = repository.found.root.parent();
            while let Some(directory) = above {
                if !directory.is_within(layout.home()) {
                    break;
                }
                above = directory.parent();
                index.know(FolderKind::Mixed(directory));
            }
        }
        for root in layout.document_roots() {
            index.know(FolderKind::Document(root));
        }
        for folder in &documents.folders {
            index.know(FolderKind::Document(folder.path.clone()));
        }
        index
    }

    fn know(&mut self, kind: FolderKind) {
        let Some(directory) = directory_of(&kind).cloned() else {
            return;
        };
        let name = transcript_folder_name(directory.as_str());
        match self.by_name.get_mut(&name) {
            None => {
                self.by_name.insert(name, kind);
            }
            Some(existing) => {
                let Some(first) = directory_of(existing).cloned() else {
                    if let FolderKind::Ambiguous(all) = existing
                        && !all.contains(&directory)
                    {
                        all.push(directory);
                    }
                    return;
                };
                if first != directory {
                    *existing = FolderKind::Ambiguous(vec![first, directory]);
                }
            }
        }
    }

    /// What this folder name is the encoding of. A known directory first; then a temporary
    /// place, recognised by its encoded prefix; then nothing.
    pub(super) fn kind_of(&self, folder: &str) -> FolderKind {
        if let Some(kind) = self.by_name.get(folder) {
            return kind.clone();
        }
        if is_temporary_folder_name(folder) {
            return FolderKind::Temporary;
        }
        FolderKind::Unknown
    }

    /// Names that two known directories share. Zero is the finding the encoding needs.
    pub(super) fn ambiguous_names(&self) -> usize {
        self.by_name
            .values()
            .filter(|kind| matches!(kind, FolderKind::Ambiguous(_)))
            .count()
    }
}

fn directory_of(kind: &FolderKind) -> Option<&NormalizedPath> {
    match kind {
        FolderKind::Project(path) | FolderKind::Mixed(path) | FolderKind::Document(path) => {
            Some(path)
        }
        FolderKind::Temporary | FolderKind::Ambiguous(_) | FolderKind::Unknown => None,
    }
}

/// What the domain rule is told about a folder of this kind.
pub(super) fn place_of(kind: &FolderKind, repositories: &Repositories) -> FolderPlace {
    match kind {
        FolderKind::Project(root) => repositories
            .holding(root)
            .map(|repository| FolderPlace::Project(repository.effective))
            .unwrap_or(FolderPlace::Unknown),
        FolderKind::Mixed(_) => FolderPlace::Mixed,
        FolderKind::Temporary => FolderPlace::Temporary,
        FolderKind::Document(_) | FolderKind::Ambiguous(_) | FolderKind::Unknown => {
            FolderPlace::Unknown
        }
    }
}
