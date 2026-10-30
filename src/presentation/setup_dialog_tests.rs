// The conversation, driven through its input: what each question accepts, what it asks again
// about, and that a closed input answers nothing.
use std::io::Cursor;

use super::Dialog;
use crate::domain::test_machine::{HOME, Machine, path};
use crate::domain::{ContextLayout, MachineGroups, RootSource, SetupAnswers};

/// Two groups — one marked as the person's organisation — and a folder nobody worked in.
fn two_groups() -> MachineGroups {
    Machine::default()
        .directory("Documents")
        .repository("Projects/alpha", "git@git.acme.example:team/alpha.git")
        .repository("Projects/beta", "git@github.com:someone/beta.git")
        .document_folder("Documents/letters", 2)
        .groups(&crate::domain::test_machine::mail_at("acme.example"))
}

/// The answers `typed` gives, and everything the dialog printed on the way.
fn converse(groups: &MachineGroups, typed: &str) -> (Option<SetupAnswers>, String) {
    let mut input = Cursor::new(typed.as_bytes().to_vec());
    let mut output: Vec<u8> = Vec::new();
    let home = path(HOME);
    let answers = Dialog::new(&mut input, &mut output, Some(&home))
        .answers(groups)
        .unwrap();
    (answers, String::from_utf8(output).unwrap())
}

fn layout(groups: &MachineGroups, typed: &str) -> ContextLayout {
    let (answers, printed) = converse(groups, typed);
    let answers = answers.unwrap_or_else(|| panic!("no answers from:\n{printed}"));
    groups.lay_out(&answers).unwrap()
}

fn context_names(layout: &ContextLayout) -> Vec<&str> {
    layout
        .contexts()
        .iter()
        .map(|context| context.name().as_str())
        .collect()
}

#[test]
fn the_names_are_typed_one_per_line_and_an_empty_line_ends_them() {
    let groups = two_groups();
    let laid_out = layout(&groups, "client-a\nclient-b\nown\n\n3\n1\n2\n");

    assert_eq!(context_names(&laid_out), ["client-a", "client-b", "own"]);
    let own = &laid_out.contexts()[2];
    assert_eq!(
        own.roots()
            .iter()
            .map(|root| root.source())
            .collect::<Vec<_>>(),
        [RootSource::Remainder]
    );
}

#[test]
fn a_name_that_cannot_be_one_or_is_given_twice_is_asked_again() {
    let groups = two_groups();
    let (answers, printed) = converse(&groups, "\n../up\nown\nown\n\nn\n1\ns\n\n");

    assert!(
        printed.contains("At least one context is needed."),
        "{printed}"
    );
    assert!(
        printed.contains("\"../up\" cannot name a context"),
        "{printed}"
    );
    assert!(printed.contains("own is already named."), "{printed}");
    let laid_out = groups.lay_out(&answers.unwrap()).unwrap();
    assert_eq!(context_names(&laid_out), ["own"]);
}

#[test]
fn a_group_is_shown_with_its_reason_and_mark_and_accepts_only_its_own_answers() {
    let groups = two_groups();
    let (answers, printed) = converse(&groups, "work\nmine\n\nn\n7\nyes\n1\n\n\n");

    assert!(
        printed.contains("Group 1 of 2: `origin` under group team on git.acme.example — looks like your organization"),
        "{printed}"
    );
    assert!(
        printed.contains("  project        ~/Projects/alpha"),
        "{printed}"
    );
    assert!(
        printed.contains("Group 2 of 2: `origin` under group someone on github.com\n"),
        "the other group carries no mark: {printed}"
    );
    assert_eq!(
        printed
            .matches("Answer with a number from 1 to 2, s, or an empty line")
            .count(),
        2,
        "a number out of range and a word are both asked again: {printed}"
    );
    let laid_out = groups.lay_out(&answers.unwrap()).unwrap();
    assert_eq!(context_names(&laid_out), ["work"]);
    assert_eq!(
        laid_out.unanswered().projects().len(),
        1,
        "an empty answer without a remainder context goes nowhere"
    );
}

#[test]
fn the_remainder_question_lists_its_folders_and_takes_a_number_or_no() {
    let groups = two_groups();
    let (_, printed) = converse(&groups, "own\n\nmaybe\n1\n\n\n");

    assert!(
        printed.contains("Folders no group's conversations worked in:\n  folder         ~/Documents/letters  (2 files)"),
        "{printed}"
    );
    assert!(
        printed.contains("Answer with a number from 1 to 1, or n"),
        "{printed}"
    );
    assert!(printed.contains("empty: own?"), "{printed}");
}

/// `n` to the remainder is not "nowhere": each of its folders is asked about after the groups,
/// and one sent to a context goes there by that answer.
#[test]
fn a_remainder_answered_no_is_asked_about_folder_by_folder_after_the_groups() {
    let groups = two_groups();
    let (answers, printed) = converse(&groups, "work\nmine\n\nn\n1\n2\n2\n");

    let folder = printed
        .find("Folder 1 of 1: no group's conversations worked in it")
        .unwrap_or_else(|| panic!("{printed}"));
    assert!(
        printed
            .find("Group 2 of 2")
            .is_some_and(|group| group < folder),
        "the folders come after the groups:\n{printed}"
    );
    assert!(
        printed[folder..].contains("empty: no context?"),
        "{printed}"
    );
    let laid_out = groups.lay_out(&answers.unwrap()).unwrap();
    let mine = &laid_out.contexts()[1];
    assert_eq!(
        mine.roots()
            .iter()
            .map(|root| (root.path().file_name().to_string(), root.source()))
            .collect::<Vec<_>>(),
        [
            ("beta".to_string(), RootSource::Group(1)),
            ("letters".to_string(), RootSource::RemainderFolder),
        ]
    );
}

/// A closed input at any question is an answer to nothing.
#[test]
fn a_closed_input_at_any_question_answers_nothing() {
    let groups = two_groups();
    for typed in [
        "",
        "own\n",
        "own\n\n",
        "own\n\n1\n",
        "own\n\n1\n1\n",
        "own\n\nn\n1\n1\n",
    ] {
        let (answers, _) = converse(&groups, typed);
        assert_eq!(answers, None, "{typed:?} must not be taken for answers");
    }
}

#[test]
fn only_a_yes_confirms() {
    for (typed, expected) in [
        ("y\n", true),
        ("YES\n", true),
        ("n\n", false),
        ("\n", false),
        ("", false),
    ] {
        let mut input = Cursor::new(typed.as_bytes().to_vec());
        let mut output: Vec<u8> = Vec::new();
        let confirmed = Dialog::new(&mut input, &mut output, None)
            .confirmed("Write?")
            .unwrap();
        assert_eq!(confirmed, expected, "{typed:?}");
    }
}
