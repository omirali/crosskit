// `crosskit setup <directory>`: the first run, as one conversation.
//
// It walks the tree the way `contexts propose` does, reads beside it the folders Claude Code
// filed conversations under and the folders without git where documents are kept, and sorts
// what it found into groups — things that belong together, with the reason they do (ADR 0042).
// Then it asks: first which contexts there are, named by the person; then where everything
// left without an answer goes; then about each group, one at a time. The file those answers
// make is shown whole and written only after a yes, and every context in it is indexed.
//
// What it will not do is decide for the person. It has no names of its own for contexts, it
// puts nothing in one without an answer about the group the thing lies in, it refuses a
// configuration that already declares contexts rather than merging into it, it keeps whatever
// file it replaces, and it writes nothing at all before the last question is answered. No model
// is asked and no request leaves the machine.
use std::io;
use std::path::PathBuf;

use crate::domain::{
    ContextLayout, MachineGroups, OwnerDomain, WorkspaceName, places_of_document_folders,
};
use crate::infrastructure::machine_survey::{
    codex_sessions, document_folders, owner_mail_domain, transcript_folders, transcripts_root,
};
use crate::infrastructure::{LoadedIndexPolicy, load_index_policy, write_index_policy};

use super::context_draft::write_walk;
use super::context_wording::counted;
use super::help;
use super::setup_dialog::Dialog;
use super::setup_report::{egress_line, found, summary};
use super::{contexts, setup_draft, setup_indexing};

pub(super) const USAGE: &str = "Usage: crosskit setup <directory> [--yes]";

pub fn run(arguments: &[String]) -> Result<(), String> {
    let request = parse(arguments)?;
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    refuse_a_configured_machine(&loaded)?;
    if request.assume_yes {
        return Err(NOBODY_TO_NAME_CONTEXTS.to_string());
    }

    let home = contexts::home_directory();
    let evidence = contexts::survey(&request.root)?;
    if evidence.whole.is_empty() {
        let mut printed = format!("Nothing to propose: {} holds no files.\n", evidence.root);
        write_walk(&mut printed, &evidence.walk);
        print!("{printed}");
        println!();
        println!("Nothing was written: there is nothing here to make a context of.");
        return Ok(());
    }

    let transcripts_root = home.as_ref().and_then(transcripts_root);
    let transcripts = transcripts_root
        .as_ref()
        .map(transcript_folders)
        .unwrap_or_default();
    let documents = document_folders(&places_of_document_folders(&evidence, home.as_ref()));
    let owner = home
        .as_ref()
        .map(owner_mail_domain)
        .unwrap_or(OwnerDomain::Unknown);
    let codex = home.as_ref().map(codex_sessions).unwrap_or_default();
    let groups =
        MachineGroups::assemble(&evidence, home.as_ref(), &transcripts, &documents, &owner);

    print!("{}", found(&groups, &evidence.root, home.as_ref(), codex));
    if groups.is_empty() {
        println!();
        println!(
            "Nothing was written: nothing under {} is a project or a folder a context could \
             hold. A context is written by hand into {}, and `crosskit --context <name> index \
             init <directory>` indexes it.",
            evidence.root,
            loaded.path().display()
        );
        return Ok(());
    }

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout();
    let mut dialog = Dialog::new(&mut input, &mut output, home.as_ref());
    let Some(answers) = dialog.answers(&groups)? else {
        println!();
        println!("Nothing was written: the input ended before the last answer.");
        return Ok(());
    };
    let layout = groups
        .lay_out(&answers)
        .map_err(|error| format!("the answers do not describe contexts: {error:?}"))?;
    if layout.contexts().is_empty() {
        println!();
        println!("Nothing was written: no group was sent to a context.");
        return Ok(());
    }

    let draft = setup_draft::render(&layout, &groups, &evidence, transcripts_root.as_ref());
    println!();
    print!("{draft}");
    println!();
    let question = format!(
        "Write this to {} and index {}?",
        loaded.path().display(),
        contexts_and_directories(&layout)
    );
    if !dialog.confirmed(&question)? {
        println!("Nothing was written.");
        return Ok(());
    }

    let kept = write_index_policy(loaded.path(), &draft).map_err(|error| error.to_string())?;
    println!("Configuration written: {}", loaded.path().display());
    if let Some(kept) = kept {
        println!(
            "The configuration that was there is kept at: {}",
            kept.display()
        );
    }
    println!("{}", egress_line(&layout));

    // Read back rather than reused: what the contexts are now is what the file says, and
    // the file is the thing the operator can still edit. A draft that did not load would
    // stop here, before a single database is opened.
    let configured = load_index_policy().map_err(|error| {
        format!(
            "the configuration was written to {} but does not load: {error}",
            loaded.path().display()
        )
    })?;
    let names: Vec<WorkspaceName> = layout
        .contexts()
        .iter()
        .map(|context| context.name().clone())
        .collect();
    println!();
    setup_indexing::run(&configured, &names, &summary(&layout, &groups, codex))
}

/// Why `--yes` is refused. It used to write the proposal unasked; now there is nothing to write
/// until a person has named the contexts, and a run nobody watches has nobody to name them.
const NOBODY_TO_NAME_CONTEXTS: &str = "setup --yes cannot lay out this machine: the contexts are \
     named by a person, one by one, and there is no guess to fall back on when nobody is there to \
     name them. Run `crosskit setup <directory>` in a terminal and answer its questions.";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    root: PathBuf,
    assume_yes: bool,
}

fn parse(arguments: &[String]) -> Result<Request, String> {
    let mut root: Option<&String> = None;
    let mut assume_yes = false;
    for argument in arguments {
        match argument.as_str() {
            "--yes" if assume_yes => return Err(format!("--yes was given twice. {USAGE}")),
            "--yes" => assume_yes = true,
            other if other.starts_with("--") => {
                return Err(format!("unknown argument {other:?}. {USAGE}"));
            }
            _ if root.is_some() => {
                return Err(format!(
                    "setup configures one directory at a time, and two were given. {USAGE}"
                ));
            }
            _ => root = Some(argument),
        }
    }
    Ok(Request {
        root: PathBuf::from(root.ok_or_else(|| help::misuse(USAGE))?),
        assume_yes,
    })
}

/// A machine that already has contexts is not a machine to set up.
///
/// Neither merging nor replacing is offered, and that is the decision rather than a gap in
/// it: contexts are a confidentiality boundary, and a command that moved one — by adding a
/// rule beside it or by writing over it — would move which files may be read together, on
/// the strength of what a walk guessed from a tree. Whoever drew that boundary edits it.
fn refuse_a_configured_machine(loaded: &LoadedIndexPolicy) -> Result<(), String> {
    let declared = loaded.workspaces().names();
    if declared.is_empty() {
        return Ok(());
    }
    Err(format!(
        "this machine already has {} configured in {}; setup is the first run and does not \
         change contexts that exist, because a context is a confidentiality boundary and \
         moving one is not a thing to be done by a walk over a directory. To lay the machine \
         out anew, move that file aside — rename it, and keep it until the new contexts are \
         indexed — and run setup again. To add one context instead, edit the file and run \
         `crosskit --context <name> index init <directory>` for it. `crosskit contexts \
         propose <directory>` prints a draft by host without writing anything.",
        describe_contexts(declared),
        loaded.path().display()
    ))
}

fn describe_contexts(names: &[WorkspaceName]) -> String {
    let listed = names
        .iter()
        .map(WorkspaceName::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match names.len() {
        1 => format!("the context {listed}"),
        count => format!("{count} contexts ({listed})"),
    }
}

fn contexts_and_directories(layout: &ContextLayout) -> String {
    let directories: usize = layout
        .contexts()
        .iter()
        .map(|context| context.roots().len())
        .sum();
    format!(
        "{} in {}",
        counted(directories as u64, "directory"),
        counted(layout.contexts().len() as u64, "context")
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Request, parse};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn a_directory_is_required_and_the_flag_is_optional() {
        assert_eq!(
            parse(&arguments(&["/tree"])).unwrap(),
            Request {
                root: PathBuf::from("/tree"),
                assume_yes: false
            }
        );
        assert_eq!(
            parse(&arguments(&["--yes", "/tree"])).unwrap(),
            Request {
                root: PathBuf::from("/tree"),
                assume_yes: true
            }
        );
    }

    #[test]
    fn invalid_requests_are_refused() {
        for invalid in [
            vec![],
            vec!["--yes"],
            vec!["/tree", "/other"],
            vec!["/tree", "--force"],
            vec!["/tree", "--yes", "--yes"],
        ] {
            assert!(
                parse(&arguments(&invalid)).is_err(),
                "{invalid:?} must be refused"
            );
        }
    }
}
