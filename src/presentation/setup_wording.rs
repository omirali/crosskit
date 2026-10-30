// How `setup` puts groups into words: why a group is one, what each of its members is, and
// why something is in no group.
//
// One file for the dialog, the configuration it writes and the summary it ends with, so a
// reason reads the same in all three. Nothing here decides anything.
use crate::domain::{GroupReason, LeftOutReason, NoSignal, NormalizedPath};

use super::context_wording::counted;

/// A path as the person reads it: under the home directory it starts with `~`.
pub(super) fn shown(path: &NormalizedPath, home: Option<&NormalizedPath>) -> String {
    match home.and_then(|home| path.as_str().strip_prefix(home.as_str())) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.as_str().to_string(),
    }
}

/// Why the members of a group are asked about together.
pub(super) fn group_reason(reason: &GroupReason) -> String {
    match reason {
        GroupReason::OriginGroup(owner) => {
            format!("`origin` under group {} on {}", owner.group(), owner.host())
        }
        GroupReason::NoSignal(why) => {
            format!("nothing ties it to another project: {}", no_signal(why))
        }
    }
}

fn no_signal(why: &NoSignal) -> String {
    match why {
        NoSignal::NotARepository => "a project with no repository".to_string(),
        NoSignal::GitEntryIsAPointer => {
            "its `.git` is a pointer to a repository elsewhere, not followed".to_string()
        }
        NoSignal::ConfigNotRead => "its `.git/config` could not be read".to_string(),
        NoSignal::NoOrigin => "no remote called `origin`".to_string(),
        NoSignal::OriginIsAPath => "`origin` is a path on this machine".to_string(),
        NoSignal::OriginWithoutGroup(host) => {
            format!("`origin` lies directly under {host}, with no group")
        }
    }
}

/// The mark on a group whose `origin` group carries the domain of the person's mail.
pub(super) const ORGANIZATION_MARK: &str = "looks like your organization";

/// Why something is in no group, said about that one thing.
pub(super) fn left_out_reason(reason: &LeftOutReason, home: Option<&NormalizedPath>) -> String {
    match reason {
        LeftOutReason::Temporary => "started in a temporary place: nobody's work".to_string(),
        LeftOutReason::NoConversations => {
            "holds no conversation, so there is nothing to check its name by".to_string()
        }
        LeftOutReason::NoWorkingDirectory => {
            "its conversations record no working directory".to_string()
        }
        LeftOutReason::NameDisagrees { started_in } => format!(
            "a conversation in it started in {}, whose folder would have another name; \
             the tool's naming and this one disagree, so nothing is placed by it",
            shown(started_in, home)
        ),
        LeftOutReason::SeveralDirectories => {
            "its conversations started in several directories that share one folder name"
                .to_string()
        }
        LeftOutReason::MixedFolder { directory } => format!(
            "started in {}, which holds projects without being one: its conversations could \
             be about any of them",
            shown(directory, home)
        ),
        LeftOutReason::UnknownDirectory { directory } => format!(
            "started in {}, which is none of what was found",
            shown(directory, home)
        ),
        LeftOutReason::HoldsProjects => {
            "holds a project, and a context over it would cover the project too".to_string()
        }
        LeftOutReason::SeveralGroupsWorkedHere => {
            "conversations of several groups worked in it".to_string()
        }
        LeftOutReason::PathIsNotExpressible => {
            "its path holds `*` or `?`, which a rule would read as a wildcard".to_string()
        }
        LeftOutReason::ToolsOwnPlace { projects } => format!(
            "{} inside a directory a tool keeps for itself",
            counted(*projects as u64, "project")
        ),
    }
}

/// The heading a left-out thing is counted under in the summary. Several reasons share one
/// heading where the person would act on them the same way.
pub(super) fn left_out_heading(reason: &LeftOutReason) -> &'static str {
    match reason {
        LeftOutReason::Temporary => "temporary places",
        LeftOutReason::MixedFolder { .. } => "mixed folders",
        LeftOutReason::NameDisagrees { .. } | LeftOutReason::SeveralDirectories => {
            "folder name and working directory disagree"
        }
        LeftOutReason::NoConversations | LeftOutReason::NoWorkingDirectory => {
            "nothing to check a folder by"
        }
        LeftOutReason::UnknownDirectory { .. } => "started outside what was found",
        LeftOutReason::HoldsProjects
        | LeftOutReason::SeveralGroupsWorkedHere
        | LeftOutReason::PathIsNotExpressible => "not expressible as one context's folder",
        LeftOutReason::ToolsOwnPlace { .. } => "inside a tool's own directory",
    }
}

/// `2 projects, 1 folder without git, 5 conversations`, leaving out what there is none of.
pub(super) fn contents(
    projects: usize,
    folders: usize,
    transcripts: usize,
    conversations: usize,
) -> String {
    let mut parts = Vec::new();
    if projects > 0 {
        parts.push(counted(projects as u64, "project"));
    }
    if folders > 0 {
        parts.push(format!("{} without git", counted(folders as u64, "folder")));
    }
    if transcripts > 0 {
        parts.push(counted(transcripts as u64, "transcript folder"));
    }
    parts.push(counted(conversations as u64, "conversation"));
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::shown;
    use crate::domain::NormalizedPath;

    fn path(value: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(value).unwrap()
    }

    #[test]
    fn a_path_under_the_home_is_shown_from_the_tilde_and_only_by_whole_segments() {
        let home = path("/Users/alice");
        assert_eq!(
            shown(&path("/Users/alice/Projects/x"), Some(&home)),
            "~/Projects/x"
        );
        assert_eq!(shown(&home, Some(&home)), "~");
        assert_eq!(
            shown(&path("/Users/alice2/x"), Some(&home)),
            "/Users/alice2/x"
        );
        assert_eq!(shown(&path("/opt/x"), None), "/opt/x");
    }
}
