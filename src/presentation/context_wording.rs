// How a proposal is put into words: one sentence per piece of evidence, one per reason a
// subtree was left alone.
//
// The domain says what was found; the wording is here, so a rule and the sentence that
// explains it never drift apart in two places at once. Nothing in this file decides
// anything.
use crate::domain::{
    ContextBasis, Evidence, GitMarker, HomePlace, NotAttributed, ProjectMarker, ProposedInclude,
    RepositoryRemote, RepositorySignal, SubtreeVolume, UnattributedSubtree,
};

use super::{format_bytes, format_number};

pub(super) fn basis(basis: &ContextBasis, includes: usize) -> String {
    match basis {
        ContextBasis::RemoteHost(host) => {
            format!("{} whose remote host is {host}", repositories(includes))
        }
        ContextBasis::RepositoriesWithoutARemote => format!(
            "{} with no remote host to stand under\n\
             # (whether this is work or personal does not follow from anything on disk)",
            repositories(includes)
        ),
        ContextBasis::ProjectsWithoutARepository if includes == 1 => {
            "1 directory a build manifest names, with no repository in it".to_string()
        }
        ContextBasis::ProjectsWithoutARepository => {
            format!("{includes} directories a build manifest names, with no repository in them")
        }
        ContextBasis::PersonalDocuments if includes == 1 => {
            "1 directory of the standard home layout, holding a person's own documents".to_string()
        }
        ContextBasis::PersonalDocuments => format!(
            "{includes} directories of the standard home layout, holding a person's own documents"
        ),
    }
}

fn repositories(count: usize) -> String {
    counted(count as u64, "repository")
}

fn files(count: u64) -> String {
    counted(count, "file")
}

/// `1 file`, `3 files`, `2 directories`. One rule for counting things in a sentence, so
/// two commands never disagree about how many of something there were.
pub(super) fn counted(count: u64, singular: &str) -> String {
    if count == 1 {
        return format!("1 {singular}");
    }
    let plural = match singular.strip_suffix('y') {
        Some(stem) => format!("{stem}ies"),
        None => format!("{singular}s"),
    };
    format!("{} {plural}", format_number(count))
}

pub(super) fn evidence_of(include: &ProposedInclude) -> String {
    include
        .evidence()
        .iter()
        .map(evidence)
        .collect::<Vec<_>>()
        .join(", ")
}

fn evidence(evidence: &Evidence) -> String {
    match evidence {
        Evidence::Repository(repository) => repository_evidence(repository),
        Evidence::Marker(marker) => marker_evidence(*marker),
        Evidence::HomePlace(place) => home_place(*place).to_string(),
        Evidence::NestedRepositories(1) => "covers 1 repository inside it".to_string(),
        Evidence::NestedRepositories(count) => {
            format!("covers {count} repositories inside it")
        }
    }
}

pub(super) fn repository_evidence(repository: &RepositorySignal) -> String {
    match repository.marker() {
        GitMarker::NotADirectory => {
            ".git that is not a directory, so no remote was read".to_string()
        }
        GitMarker::Directory => match repository.remote() {
            RepositoryRemote::Host(host) => format!(".git, remote host {host}"),
            RepositoryRemote::SeveralHosts(hosts) => {
                format!(".git, remote hosts {}", join(hosts))
            }
            RepositoryRemote::NoRemote => ".git declaring no remote".to_string(),
            RepositoryRemote::NoHost => {
                ".git whose remotes name only paths on this machine".to_string()
            }
            RepositoryRemote::NoConfig => ".git whose configuration was not read".to_string(),
        },
    }
}

fn marker_evidence(marker: ProjectMarker) -> String {
    format!("{} ({})", marker.file_name(), marker.ecosystem())
}

fn home_place(place: HomePlace) -> &'static str {
    match place {
        HomePlace::PersonalDocuments => "a home directory for a person's own documents",
        HomePlace::ToolStore => {
            "a standard home directory for what tools and the system keep: caches, installed \
             packages, other people's code"
        }
        HomePlace::MixedInbox => {
            "a standard home directory that everything arrives in, of every origin at once"
        }
    }
}

pub(super) fn not_attributed(reason: &NotAttributed) -> String {
    match reason {
        NotAttributed::NoSignal => "nothing here named a repository or a project".to_string(),
        NotAttributed::BesideProjects => {
            "files lying beside the projects proposed inside it".to_string()
        }
        NotAttributed::ToolStore => home_place(HomePlace::ToolStore).to_string(),
        NotAttributed::MixedInbox => home_place(HomePlace::MixedInbox).to_string(),
        NotAttributed::OverlapsProjects => {
            "offering this directory whole would cover the projects inside it, and two \
             contexts may not cover one path"
                .to_string()
        }
        NotAttributed::SeveralRemoteHosts(hosts) => format!(
            "its remotes name {}, and no origin settles which of them it belongs to",
            join(hosts)
        ),
        NotAttributed::PathIsNotExpressible => {
            "the path holds `*` or `?`, which a coverage rule reads as a wildcard: no rule \
             spells this directory and only this one"
                .to_string()
        }
        NotAttributed::DirectlyInTheRoot => {
            "files lying directly in the walked root, in none of its directories".to_string()
        }
    }
}

pub(super) fn remaining(subtree: &UnattributedSubtree) -> String {
    if subtree.covers_the_whole_subtree() {
        return describe(subtree.volume());
    }
    format!(
        "{}, {} left over from a subtree of {}",
        files(subtree.remaining_files()),
        format_bytes(subtree.remaining_bytes()),
        describe(subtree.volume())
    )
}

pub(super) fn describe(volume: SubtreeVolume) -> String {
    format!(
        "{}, {}, deepest file {}",
        files(volume.files()),
        format_bytes(volume.bytes()),
        levels(volume.deepest_levels())
    )
}

fn levels(levels: u32) -> String {
    if levels == 1 {
        "1 level down".to_string()
    } else {
        format!("{levels} levels down")
    }
}

pub(super) fn join<T: std::fmt::Display>(values: &[T]) -> String {
    values
        .iter()
        .map(T::to_string)
        .collect::<Vec<_>>()
        .join(" and ")
}
