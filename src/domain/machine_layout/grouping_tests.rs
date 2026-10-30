// The grouping rule, reason by reason: what goes together, what goes with what, and what is
// left out with which reason.
use super::super::test_machine::{HOME, Machine, home_path, mail_at, path, paths_of};
use crate::domain::{
    GroupReason, LeftOutReason, NoSignal, OwnerDomain, RemoteHost, RemoteOwner,
    places_of_document_folders, transcript_folder_name,
};

fn transcript_path(directory: &str) -> String {
    home_path(&format!(
        ".claude/projects/{}",
        transcript_folder_name(directory)
    ))
    .as_str()
    .to_string()
}

fn at(relative: &str) -> String {
    format!("{HOME}/{relative}")
}

#[test]
fn projects_under_one_origin_group_are_one_question_and_the_rest_stand_alone() {
    let groups = Machine::default()
        .directory("Projects")
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .repository(
            "Projects/beta",
            "https://git.example.org/acme/team/beta.git",
        )
        .repository("Projects/gamma", "git@git.example.org:other/gamma.git")
        .repository("Projects/local", "")
        .repository("Projects/flat", "git@git.example.org:flat.git")
        .manifest("Projects/plain")
        .groups(&OwnerDomain::Unknown);

    let described: Vec<(GroupReason, Vec<String>)> = groups
        .groups()
        .iter()
        .map(|group| (group.reason().clone(), paths_of(group.members())))
        .collect();
    let owned = |url: &str| GroupReason::OriginGroup(RemoteOwner::of_remote_url(url).unwrap());
    assert_eq!(
        described,
        [
            (
                owned("git@git.example.org:acme/alpha.git"),
                vec![at("Projects/alpha"), at("Projects/beta")]
            ),
            (
                owned("git@git.example.org:other/gamma.git"),
                vec![at("Projects/gamma")]
            ),
            (
                GroupReason::NoSignal(NoSignal::OriginWithoutGroup(
                    RemoteHost::of_remote_url("git@git.example.org:flat.git").unwrap()
                )),
                vec![at("Projects/flat")]
            ),
            (
                GroupReason::NoSignal(NoSignal::NoOrigin),
                vec![at("Projects/local")]
            ),
            (
                GroupReason::NoSignal(NoSignal::NotARepository),
                vec![at("Projects/plain")]
            ),
        ],
        "one host holding two groups is two questions; a project with no group is its own"
    );
}

#[test]
fn a_group_carrying_the_domain_of_the_mail_is_marked_and_the_mark_is_all_it_gets() {
    let machine = Machine::default()
        .repository(
            "Projects/alpha",
            "git@git.shared.example:acme.example/alpha.git",
        )
        .repository("Projects/ops", "git@git.acme.example:devops/ops.git")
        .repository("Projects/beta", "git@git.shared.example:org-two/beta.git")
        .repository("Projects/fans", "git@github.com:acme.example-fans/app.git");

    let marked: Vec<(String, bool)> = machine
        .groups(&mail_at("acme.example"))
        .groups()
        .iter()
        .map(|group| {
            (
                group.members().projects()[0].root().file_name().to_string(),
                group.looks_like_your_organization(),
            )
        })
        .collect();
    assert_eq!(
        marked,
        [
            ("ops".to_string(), true),
            ("alpha".to_string(), true),
            ("beta".to_string(), false),
            ("fans".to_string(), false),
        ],
        "a group named after the domain, or a host under it; the same letters in a longer name \
         are somebody else's"
    );
    assert!(
        machine
            .groups(&OwnerDomain::Unknown)
            .groups()
            .iter()
            .all(|group| !group.looks_like_your_organization()),
        "without one known domain nothing is marked"
    );
}

#[test]
fn a_transcript_folder_goes_with_the_project_its_first_directory_names() {
    let alpha = at("Projects/alpha");
    let groups = Machine::default()
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .transcripts(&alpha, &[&[&alpha, &at("Projects/alpha/src")], &[&alpha]])
        .groups(&OwnerDomain::Unknown);

    let group = &groups.groups()[0];
    assert_eq!(
        paths_of(group.members()),
        [alpha.clone(), transcript_path(&alpha)]
    );
    let transcripts = &group.members().transcripts()[0];
    assert_eq!(transcripts.conversations(), 2);
    assert_eq!(transcripts.directory().as_str(), alpha);
    assert!(groups.left_out().is_empty(), "{:?}", groups.left_out());
}

/// The name is checked, never trusted: a conversation started somewhere whose folder would be
/// called otherwise means the naming rule and the folder no longer agree.
#[test]
fn a_folder_whose_conversation_started_elsewhere_is_left_out_with_where_it_started() {
    let alpha = at("Projects/alpha");
    let beta = at("Projects/beta");
    let groups = Machine::default()
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .repository("Projects/beta", "git@git.example.org:acme/beta.git")
        .transcripts(&alpha, &[&[&alpha], &[&beta]])
        .transcripts(&beta, &[&[]])
        .transcripts(&at("Projects/gamma"), &[])
        .groups(&OwnerDomain::Unknown);

    assert_eq!(groups.groups().len(), 1);
    assert!(groups.groups()[0].members().transcripts().is_empty());
    let reasons: Vec<(String, LeftOutReason)> = groups
        .left_out()
        .iter()
        .map(|left| (left.path().as_str().to_string(), left.reason().clone()))
        .collect();
    assert_eq!(
        reasons,
        [
            (
                transcript_path(&alpha),
                LeftOutReason::NameDisagrees {
                    started_in: path(&beta)
                }
            ),
            (transcript_path(&beta), LeftOutReason::NoWorkingDirectory),
            (
                transcript_path(&at("Projects/gamma")),
                LeftOutReason::NoConversations
            ),
        ]
    );
}

#[test]
fn throwaway_and_mixed_folders_are_left_out_and_a_project_in_the_walk_is_not_throwaway() {
    let projects = at("Projects");
    let alpha = at("Projects/alpha");
    let scratch = "/private/tmp/tool-501/scratchpad";
    let groups = Machine::default()
        .directory("Projects")
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .transcripts(scratch, &[&[scratch]])
        .transcripts(&projects, &[&[&projects, &alpha]])
        .groups(&OwnerDomain::Unknown);

    let reasons: Vec<(String, LeftOutReason, usize)> = groups
        .left_out()
        .iter()
        .map(|left| {
            (
                left.path().as_str().to_string(),
                left.reason().clone(),
                left.conversations(),
            )
        })
        .collect();
    assert_eq!(
        reasons,
        [
            (
                transcript_path(&projects),
                LeftOutReason::MixedFolder {
                    directory: path(&projects)
                },
                1
            ),
            (transcript_path(scratch), LeftOutReason::Temporary, 1),
        ]
    );
}

/// The synthetic home of the smoke tests lies in a temporary place, and so may any tree a
/// person walks: a project the walk found is a project wherever it lies.
#[test]
fn a_project_inside_a_temporary_place_still_takes_its_conversations() {
    let app = "/private/tmp/sandbox/app";
    let groups = Machine::walked_at("/private/tmp/sandbox")
        .manifest("app")
        .transcripts(app, &[&[app]])
        .groups(&OwnerDomain::Unknown);

    assert_eq!(groups.groups().len(), 1);
    assert_eq!(groups.groups()[0].members().conversations(), 1);
    assert!(groups.left_out().is_empty(), "{:?}", groups.left_out());
}

#[test]
fn a_repository_inside_a_project_goes_with_it_and_so_do_its_conversations() {
    let outer = at("Projects/outer");
    let inner = at("Projects/outer/vendor/inner");
    let groups = Machine::default()
        .repository("Projects/outer", "git@git.example.org:acme/outer.git")
        .repository(
            "Projects/outer/vendor/inner",
            "git@git.example.org:other/inner.git",
        )
        .transcripts(&inner, &[&[&inner]])
        .groups(&OwnerDomain::Unknown);

    assert_eq!(groups.groups().len(), 1, "the inner group gets no question");
    let group = &groups.groups()[0];
    assert!(matches!(group.reason(), GroupReason::OriginGroup(owner) if owner.group() == "acme"));
    let project = &group.members().projects()[0];
    assert_eq!(project.root().as_str(), outer);
    assert_eq!(
        project
            .nested()
            .iter()
            .map(|nested| nested.as_str())
            .collect::<Vec<_>>(),
        [inner.as_str()]
    );
    assert_eq!(group.members().conversations(), 1);
}

#[test]
fn a_folder_without_git_follows_the_group_that_worked_in_it_or_waits_in_the_remainder() {
    let alpha = at("Projects/alpha");
    let beta = at("Projects/beta");
    let specs = at("Documents/specs");
    let shared = at("Documents/shared");
    let groups = Machine::default()
        .directory("Documents")
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .repository("Projects/beta", "git@git.example.org:other/beta.git")
        .transcripts(&alpha, &[&[&alpha, &specs, &shared]])
        .transcripts(&beta, &[&[&beta, &at("Documents/shared/x")]])
        .document_folder("Documents/specs", 4)
        .document_folder("Documents/shared", 2)
        .document_folder("Documents/letters", 9)
        .groups(&OwnerDomain::Unknown);

    assert_eq!(
        paths_of(groups.groups()[0].members()),
        [alpha.clone(), transcript_path(&alpha), specs]
    );
    assert_eq!(paths_of(groups.remainder()), [at("Documents/letters")]);
    assert_eq!(
        groups
            .left_out()
            .iter()
            .map(|left| (left.path().as_str().to_string(), left.reason().clone()))
            .collect::<Vec<_>>(),
        [(shared, LeftOutReason::SeveralGroupsWorkedHere)]
    );
}

#[test]
fn projects_inside_a_tools_own_directory_are_counted_and_not_asked_about() {
    let groups = Machine::default()
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .manifest(".vscode/extensions/one")
        .manifest(".vscode/extensions/two")
        .manifest("Library/Application Support/tool/x")
        .groups(&OwnerDomain::Unknown);

    assert_eq!(groups.groups().len(), 1);
    assert_eq!(
        groups
            .left_out()
            .iter()
            .map(|left| (left.path().as_str().to_string(), left.reason().clone()))
            .collect::<Vec<_>>(),
        [
            (at(".vscode"), LeftOutReason::ToolsOwnPlace { projects: 2 }),
            (at("Library"), LeftOutReason::ToolsOwnPlace { projects: 1 }),
        ]
    );
}

#[test]
fn folders_without_git_are_looked_for_in_the_document_places_and_beside_projects() {
    let machine = Machine::default()
        .directory("Documents")
        .directory("Downloads")
        .directory("Music")
        .directory("Projects")
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .repository("Projects/alpha/inner", "")
        .repository("top", "");

    let places: Vec<String> = places_of_document_folders(&machine.evidence(), Some(&path(HOME)))
        .iter()
        .map(|place| place.as_str().to_string())
        .collect();
    assert_eq!(
        places,
        [at("Documents"), at("Downloads"), at("Projects")],
        "not the home itself, not inside a project, not an application's library"
    );
}
