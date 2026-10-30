// A synthetic machine for the tests of this module: a home, the projects the walk would find in
// it, and the transcript folders and folders without git the censuses would list.
//
// The names are made up and so are the organisations. Transcript folders are named by the
// product's own encoding, because what the tests check is what the grouping does with a name,
// not how a name is computed — `transcript_folder` has its own tests for that.
use crate::domain::{
    ConversationStart, DocumentFolderSurvey, GitMarker, HomePlace, MachineGroups, NormalizedPath,
    Origin, OwnerDomain, ProjectMarker, RepositoryRemote, RepositorySignal, SubtreeEvidence,
    SubtreeVolume, TranscriptFolderSurvey, TreeEvidence, WalkOutcome, transcript_folder_name,
};

pub(crate) const HOME: &str = "/Users/alice";

pub(crate) fn path(value: &str) -> NormalizedPath {
    NormalizedPath::from_absolute(value).expect("an absolute path")
}

/// `relative` under the home.
pub(crate) fn home_path(relative: &str) -> NormalizedPath {
    path(&format!("{HOME}/{relative}"))
}

pub(crate) struct Machine {
    /// The walked tree; the home unless a test walks somewhere else.
    root: NormalizedPath,
    subtrees: Vec<SubtreeEvidence>,
    transcripts: Vec<TranscriptFolderSurvey>,
    documents: Vec<DocumentFolderSurvey>,
}

impl Default for Machine {
    fn default() -> Self {
        Self::walked_at(HOME)
    }
}

impl Machine {
    /// A machine whose walk starts at `root`; paths given to the builders are relative to it.
    pub(crate) fn walked_at(root: &str) -> Self {
        Self {
            root: path(root),
            subtrees: Vec::new(),
            transcripts: Vec::new(),
            documents: Vec::new(),
        }
    }

    /// A repository whose `origin` is `url`, or which has no remote when `url` is empty.
    pub(crate) fn repository(mut self, relative: &str, url: &str) -> Self {
        let config = match url {
            "" => String::new(),
            url => format!("[remote \"origin\"]\n\turl = {url}\n"),
        };
        let signal = RepositorySignal::new(
            GitMarker::Directory,
            RepositoryRemote::of_git_config(&config),
            Some(Origin::of_git_config(&config)),
        );
        self.subtrees
            .push(evidence(&self.root, relative, Some(signal), Vec::new()));
        self
    }

    /// A project a build manifest names, with no repository.
    pub(crate) fn manifest(mut self, relative: &str) -> Self {
        let marker = ProjectMarker::of_file_name("Cargo.toml").expect("a known marker");
        self.subtrees
            .push(evidence(&self.root, relative, None, vec![marker]));
        self
    }

    /// A directory the walk describes without any signal: a child of the home.
    pub(crate) fn directory(mut self, relative: &str) -> Self {
        self.subtrees
            .push(evidence(&self.root, relative, None, Vec::new()));
        self
    }

    /// A transcript folder named after `directory`, holding one conversation per entry of
    /// `conversations`, each the working directories it records.
    pub(crate) fn transcripts(mut self, directory: &str, conversations: &[&[&str]]) -> Self {
        let name = transcript_folder_name(directory);
        self.transcripts.push(TranscriptFolderSurvey::new(
            home_path(&format!(".claude/projects/{name}")),
            conversations
                .iter()
                .map(|directories| {
                    ConversationStart::new(directories.iter().map(|value| path(value)).collect())
                })
                .collect(),
        ));
        self
    }

    pub(crate) fn document_folder(mut self, relative: &str, files: u64) -> Self {
        self.documents
            .push(DocumentFolderSurvey::new(home_path(relative), files));
        self
    }

    pub(crate) fn evidence(&self) -> TreeEvidence {
        let mut subtrees = self.subtrees.clone();
        subtrees.sort_by(|first, second| first.path().as_str().cmp(second.path().as_str()));
        TreeEvidence {
            root: self.root.clone(),
            whole: SubtreeVolume::new(1, 1, 1),
            loose_at_the_root: SubtreeVolume::default(),
            subtrees,
            walk: WalkOutcome::default(),
        }
    }

    pub(crate) fn groups(&self, owner: &OwnerDomain) -> MachineGroups {
        MachineGroups::assemble(
            &self.evidence(),
            Some(&path(HOME)),
            &self.transcripts,
            &self.documents,
            owner,
        )
    }
}

fn evidence(
    root: &NormalizedPath,
    relative: &str,
    repository: Option<RepositorySignal>,
    markers: Vec<ProjectMarker>,
) -> SubtreeEvidence {
    let at = path(&format!("{}/{relative}", root.as_str()));
    let home_place = match at.parent() == Some(path(HOME)) {
        true => HomePlace::of_home_child(at.file_name()),
        false => None,
    };
    SubtreeEvidence::new(
        at,
        SubtreeVolume::new(1, 1, 1),
        repository,
        markers,
        home_place,
    )
}

/// The owner domain of a person whose git mail is at `domain`.
pub(crate) fn mail_at(domain: &str) -> OwnerDomain {
    OwnerDomain::of(
        crate::domain::git_identity::Identity::of_address(&format!("alice@{domain}"))
            .and_then(|identity| identity.mail_domain().cloned()),
    )
}

/// The paths of one group's projects, transcript folders and folders, in that order.
pub(crate) fn paths_of(members: &crate::domain::GroupMembers) -> Vec<String> {
    members
        .projects()
        .iter()
        .map(|project| project.root().as_str().to_string())
        .chain(
            members
                .transcripts()
                .iter()
                .map(|transcript| transcript.path().as_str().to_string()),
        )
        .chain(
            members
                .documents()
                .iter()
                .map(|document| document.path().as_str().to_string()),
        )
        .collect()
}
