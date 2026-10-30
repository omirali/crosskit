// The deterministic signals a directory can carry: a repository, a project marker, and the
// place a directory holds in the standard layout of a home directory.
//
// Every signal here is a name the walk met on disk, never a guess about what a file
// contains. What a signal means for the proposal is decided in `context_proposal`; this
// module only says which signals exist and how a name is recognised as one.
use super::git_remote::RepositoryRemote;
use super::git_remote::remote_owner::Origin;

/// What the walk found at the `.git` entry of a repository root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitMarker {
    /// A directory the walk descended into far enough to look for `config`.
    Directory,
    /// A file, a symlink or a special entry named `.git`: the pointer a worktree or a
    /// submodule leaves behind. It is not followed, so the repository it points at is not
    /// read and no remote is known.
    NotADirectory,
}

/// A repository root, as the walk saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySignal {
    marker: GitMarker,
    remote: RepositoryRemote,
    /// What `origin` says about who owns the repository, or nothing when no `.git/config`
    /// was read. `contexts propose` groups by the host and never looks at this; `setup`
    /// groups by it (ADR 0042).
    origin: Option<Origin>,
}

impl RepositorySignal {
    pub fn new(marker: GitMarker, remote: RepositoryRemote, origin: Option<Origin>) -> Self {
        Self {
            marker,
            remote,
            origin,
        }
    }

    pub fn marker(&self) -> GitMarker {
        self.marker
    }

    pub fn remote(&self) -> &RepositoryRemote {
        &self.remote
    }

    pub fn origin(&self) -> Option<&Origin> {
        self.origin.as_ref()
    }
}

/// A file whose presence names the directory it sits in as the root of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProjectMarker {
    file_name: &'static str,
    ecosystem: &'static str,
}

impl ProjectMarker {
    /// The marker this file name is, or nothing. Exact names only: a marker is evidence
    /// exactly because its name is fixed by the tool that reads it, and a name matched by
    /// a pattern would be a guess.
    pub fn of_file_name(file_name: &str) -> Option<Self> {
        PROJECT_MARKERS
            .iter()
            .find(|marker| marker.file_name == file_name)
            .copied()
    }

    pub fn file_name(self) -> &'static str {
        self.file_name
    }

    pub fn ecosystem(self) -> &'static str {
        self.ecosystem
    }
}

/// The manifests that a build tool requires at the root of the thing it builds. Being
/// required is what makes them evidence: each of these is where its tool starts looking.
const PROJECT_MARKERS: &[ProjectMarker] = &[
    marker("Cargo.toml", "Rust"),
    marker("package.json", "Node"),
    marker("deno.json", "Deno"),
    marker("go.mod", "Go"),
    marker("pyproject.toml", "Python"),
    marker("setup.py", "Python"),
    marker("Gemfile", "Ruby"),
    marker("composer.json", "PHP"),
    marker("pom.xml", "Maven"),
    marker("build.gradle", "Gradle"),
    marker("build.gradle.kts", "Gradle"),
    marker("CMakeLists.txt", "CMake"),
    marker("mix.exs", "Elixir"),
    marker("pubspec.yaml", "Dart"),
    marker("Package.swift", "Swift"),
];

const fn marker(file_name: &'static str, ecosystem: &'static str) -> ProjectMarker {
    ProjectMarker {
        file_name,
        ecosystem,
    }
}

/// What a directory directly inside a home directory is, when its name is one the standard
/// layout fixes.
///
/// This is the one place where a name decides something, and it holds only for the children
/// of a home directory the command was pointed at: `~/Documents` is a place in a layout the
/// operating system and its tools create, not a name somebody chose to mean something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomePlace {
    /// Where a person's own documents go.
    PersonalDocuments,
    /// State that tools and the system keep: caches, installed packages, other people's
    /// code. Indexing it would fill a context with things nobody wrote here.
    ToolStore,
    /// Whatever arrived from outside, of every origin at once.
    MixedInbox,
}

impl HomePlace {
    /// The place this name holds in the standard layout, or nothing for a name the layout
    /// does not fix.
    pub fn of_home_child(name: &str) -> Option<Self> {
        HOME_LAYOUT
            .iter()
            .find(|(child, _)| *child == name)
            .map(|(_, place)| *place)
    }
}

const HOME_LAYOUT: &[(&str, HomePlace)] = &[
    ("Documents", HomePlace::PersonalDocuments),
    ("Desktop", HomePlace::PersonalDocuments),
    ("Pictures", HomePlace::PersonalDocuments),
    ("Music", HomePlace::PersonalDocuments),
    ("Movies", HomePlace::PersonalDocuments),
    ("Videos", HomePlace::PersonalDocuments),
    ("Downloads", HomePlace::MixedInbox),
    ("Library", HomePlace::ToolStore),
    ("Applications", HomePlace::ToolStore),
    (".Trash", HomePlace::ToolStore),
    (".cache", HomePlace::ToolStore),
    (".config", HomePlace::ToolStore),
    (".local", HomePlace::ToolStore),
    (".cargo", HomePlace::ToolStore),
    (".rustup", HomePlace::ToolStore),
    (".npm", HomePlace::ToolStore),
    (".nvm", HomePlace::ToolStore),
    (".pyenv", HomePlace::ToolStore),
    (".gradle", HomePlace::ToolStore),
    (".m2", HomePlace::ToolStore),
    (".gem", HomePlace::ToolStore),
    (".docker", HomePlace::ToolStore),
    ("go", HomePlace::ToolStore),
];

#[cfg(test)]
mod tests {
    use super::{HOME_LAYOUT, HomePlace, PROJECT_MARKERS, ProjectMarker};

    #[test]
    fn a_marker_is_recognised_by_its_exact_name() {
        assert_eq!(
            ProjectMarker::of_file_name("Cargo.toml").map(ProjectMarker::ecosystem),
            Some("Rust")
        );
        assert_eq!(ProjectMarker::of_file_name("cargo.toml"), None);
        assert_eq!(ProjectMarker::of_file_name("Cargo.toml.bak"), None);
        assert_eq!(ProjectMarker::of_file_name("README.md"), None);
    }

    #[test]
    fn a_home_child_is_placed_only_by_a_name_the_layout_fixes() {
        assert_eq!(
            HomePlace::of_home_child("Downloads"),
            Some(HomePlace::MixedInbox)
        );
        assert_eq!(
            HomePlace::of_home_child(".cargo"),
            Some(HomePlace::ToolStore)
        );
        assert_eq!(HomePlace::of_home_child("Projects"), None);
        assert_eq!(HomePlace::of_home_child("documents"), None);
    }

    /// Two entries for one name would make the answer depend on the order of the table.
    #[test]
    fn neither_table_names_anything_twice() {
        for (index, marker) in PROJECT_MARKERS.iter().enumerate() {
            assert!(
                !PROJECT_MARKERS[index + 1..]
                    .iter()
                    .any(|other| other.file_name == marker.file_name),
                "{} is listed twice",
                marker.file_name
            );
        }
        for (index, (name, _)) in HOME_LAYOUT.iter().enumerate() {
            assert!(
                !HOME_LAYOUT[index + 1..]
                    .iter()
                    .any(|(other, _)| other == name),
                "{name} is listed twice"
            );
        }
    }
}
