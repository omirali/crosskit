// What the answers make: a context is the roots of the groups sent to it, the remainder is a
// list of its folders and nothing else, and nothing enters a context without an answer.
use super::super::test_machine::{HOME, Machine, paths_of};
use super::{AnswerError, GroupAnswer, RemainderAnswer, RootSource, SetupAnswers};
use crate::domain::{MachineGroups, OwnerDomain, WorkspaceName, transcript_folder_name};

fn names(values: &[&str]) -> Vec<WorkspaceName> {
    values
        .iter()
        .map(|value| WorkspaceName::parse(value).unwrap())
        .collect()
}

fn at(relative: &str) -> String {
    format!("{HOME}/{relative}")
}

/// Three groups, a folder a group worked in, and a folder nobody worked in.
fn three_groups() -> MachineGroups {
    let alpha = at("Projects/alpha");
    Machine::default()
        .directory("Documents")
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .repository("Projects/beta", "git@git.example.org:other/beta.git")
        .repository("Projects/local", "")
        .transcripts(&alpha, &[&[&alpha, &at("Documents/specs")]])
        .document_folder("Documents/specs", 3)
        .document_folder("Documents/letters", 5)
        .groups(&OwnerDomain::Unknown)
}

fn roots_of(layout: &super::ContextLayout, context: &str) -> Vec<String> {
    layout
        .contexts()
        .iter()
        .find(|laid_out| laid_out.name().as_str() == context)
        .map(|laid_out| {
            laid_out
                .roots()
                .iter()
                .map(|root| root.path().as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_context_is_the_roots_of_the_groups_sent_to_it() {
    let groups = three_groups();
    let answers = SetupAnswers::new(
        names(&["client-a", "client-b", "own"]),
        RemainderAnswer::OneByOne(vec![GroupAnswer::Unanswered]),
        vec![
            GroupAnswer::Context(0),
            GroupAnswer::Context(2),
            GroupAnswer::Skip,
        ],
    )
    .unwrap();

    let layout = groups.lay_out(&answers).unwrap();

    let alpha = at("Projects/alpha");
    assert_eq!(
        roots_of(&layout, "client-a"),
        [
            alpha.clone(),
            format!("{HOME}/.claude/projects/{}", transcript_folder_name(&alpha)),
            at("Documents/specs"),
        ]
    );
    assert_eq!(roots_of(&layout, "own"), [at("Projects/beta")]);
    assert_eq!(
        layout
            .empty()
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>(),
        ["client-b"],
        "a name nothing was sent to is not written"
    );
    let first = &layout.contexts()[0];
    assert_eq!(
        (
            first.projects(),
            first.document_folders(),
            first.conversations()
        ),
        (1, 1, 1)
    );
    assert_eq!(paths_of(layout.skipped()), [at("Projects/local")]);
    assert_eq!(
        paths_of(layout.unanswered()),
        [at("Documents/letters")],
        "its one folder was left without an answer, so it stays out"
    );
}

/// The remainder is what the person agreed to by name before the first group: its folders one
/// by one, and the groups later left without an answer — never a rule that would also take
/// whatever appears beside them tomorrow.
#[test]
fn the_remainder_is_written_as_its_folders_and_the_unanswered_groups_join_it() {
    let groups = three_groups();
    let answers = SetupAnswers::new(
        names(&["client-a", "client-b", "own"]),
        RemainderAnswer::Context(2),
        vec![
            GroupAnswer::Context(0),
            GroupAnswer::Unanswered,
            GroupAnswer::Context(1),
        ],
    )
    .unwrap();

    let layout = groups.lay_out(&answers).unwrap();

    assert_eq!(
        roots_of(&layout, "own"),
        [at("Projects/beta"), at("Documents/letters")]
    );
    let own = layout
        .contexts()
        .iter()
        .find(|context| context.name().as_str() == "own")
        .unwrap();
    assert_eq!(
        own.roots()
            .iter()
            .map(|root| root.source())
            .collect::<Vec<_>>(),
        [RootSource::UnansweredGroup(1), RootSource::Remainder]
    );
    assert!(layout.unanswered().is_empty());
    assert!(layout.skipped().is_empty());
}

#[test]
fn answers_that_do_not_describe_contexts_are_refused() {
    let one = || names(&["own"]);
    assert_eq!(
        SetupAnswers::new(
            Vec::new(),
            RemainderAnswer::OneByOne(Vec::new()),
            Vec::new()
        ),
        Err(AnswerError::NoContext)
    );
    assert_eq!(
        SetupAnswers::new(
            names(&["own", "own"]),
            RemainderAnswer::Context(0),
            Vec::new()
        ),
        Err(AnswerError::NameGivenTwice(names(&["own"])[0].clone()))
    );
    assert_eq!(
        SetupAnswers::new(one(), RemainderAnswer::Context(1), Vec::new()),
        Err(AnswerError::NoSuchContext(1))
    );
    assert_eq!(
        SetupAnswers::new(
            one(),
            RemainderAnswer::OneByOne(vec![GroupAnswer::Context(3)]),
            Vec::new()
        ),
        Err(AnswerError::NoSuchContext(3))
    );
    assert_eq!(
        three_groups().lay_out(
            &SetupAnswers::new(one(), RemainderAnswer::Context(0), vec![GroupAnswer::Skip])
                .unwrap()
        ),
        Err(AnswerError::AnswersDoNotMatchGroups {
            groups: 3,
            answers: 1
        })
    );
}

/// The remainder answered `n` is asked about folder by folder: a folder sent to a context is
/// one explicit root there, by that answer, and the others stay out by theirs.
#[test]
fn a_remainder_answered_folder_by_folder_puts_each_folder_where_its_answer_says() {
    let groups = Machine::default()
        .directory("Documents")
        .repository("Projects/alpha", "git@git.example.org:acme/alpha.git")
        .document_folder("Documents/reports", 3)
        .document_folder("Documents/drafts", 4)
        .document_folder("Documents/inbox", 5)
        .groups(&OwnerDomain::Unknown);
    let folders: Vec<String> = groups
        .remainder_folders()
        .iter()
        .flat_map(paths_of)
        .collect();
    assert_eq!(
        folders,
        [
            at("Documents/drafts"),
            at("Documents/inbox"),
            at("Documents/reports")
        ]
    );

    let answers = SetupAnswers::new(
        names(&["work", "home"]),
        RemainderAnswer::OneByOne(vec![
            GroupAnswer::Context(1),
            GroupAnswer::Skip,
            GroupAnswer::Context(0),
        ]),
        vec![GroupAnswer::Context(0)],
    )
    .unwrap();
    let layout = groups.lay_out(&answers).unwrap();

    assert_eq!(
        roots_of(&layout, "work"),
        [at("Projects/alpha"), at("Documents/reports")]
    );
    assert_eq!(roots_of(&layout, "home"), [at("Documents/drafts")]);
    assert_eq!(
        layout.contexts()[0].roots()[1].source(),
        RootSource::RemainderFolder
    );
    assert_eq!(paths_of(layout.skipped()), [at("Documents/inbox")]);
    assert_eq!(
        groups.lay_out(
            &SetupAnswers::new(
                names(&["work"]),
                RemainderAnswer::OneByOne(vec![GroupAnswer::Skip]),
                vec![GroupAnswer::Skip],
            )
            .unwrap()
        ),
        Err(AnswerError::AnswersDoNotMatchFolders {
            folders: 3,
            answers: 1
        })
    );
}
