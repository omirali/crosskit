// The second half of `crosskit setup`: indexing what it has just configured.
//
// Apart from the first half because it knows nothing of drafts, questions or backups — by
// the time it runs, the configuration is a file on disk and the only question left is what
// each context holds. It indexes through the same pass as `index init`, so a first run has
// no second way of indexing.
//
// It used to have its own loop over a context's directories, and that loop used `?`. One
// directory failing therefore ended the context, and its comment explained why that was
// right: "what fails there is almost always its database, and the next directory of the same
// context would meet the same one". That was a statement about which failures are frequent,
// standing in for a decision about the one that happened — and the one that happened was the
// other kind. The database worked, two roots landed in it, the third root failed, and eight
// roots behind it were never walked for fifteen stages (`docs/coverage-audit.md`). The loop
// now lives in `index_context`, which opens the database once for the context — so the case
// that comment was really about still ends the context — and lets each root fail on its own.
use crate::application::ContextPassReport;
use crate::domain::WorkspaceName;
use crate::infrastructure::LoadedIndexPolicy;

use super::index_context::{self, Narration, Pass};
use super::workspace;

/// Indexes every directory of every context that was configured, then prints `summary` —
/// what each context holds by the person's answers — before the totals of the pass.
///
/// A context that fails does not stop the others, and now neither does a root: what each run
/// reports is a list of roots, and the last line names every one that was not walked. A first
/// run that gave up on everything because one directory could not be read would be worth less
/// than the six commands it replaces — and one that quietly gave up on nine directories was
/// worth a good deal less than that.
pub(super) fn run(
    configured: &LoadedIndexPolicy,
    contexts: &[WorkspaceName],
    summary: &str,
) -> Result<(), String> {
    println!(
        "One line per root below. The full report of a root — excluded paths, files kept as \
         metadata, what could not be read — is what `crosskit --context <name> index init \
         <directory>` prints for it."
    );
    let mut indexed_files = 0_usize;
    let mut whole = 0_usize;
    let mut partial: Vec<String> = Vec::new();

    for context in contexts {
        println!();
        println!("Indexing context {context}...");
        match index_one(configured, context) {
            Ok(report) => {
                indexed_files += report.published_files();
                match report.whole_context() {
                    true => whole += 1,
                    false => partial.push(describe_partial(context, &report)),
                }
            }
            Err(failure) => {
                eprintln!("Context {context} was not indexed at all: {failure}");
                partial.push(format!("{context}: not indexed at all"));
            }
        }
    }

    print!("{summary}");
    println!();
    println!("Contexts configured: {}", contexts.len());
    println!("Contexts indexed whole: {whole}");
    println!("Files indexed: {indexed_files}");
    if partial.is_empty() {
        println!(
            "Next: `crosskit status` shows every context, and `crosskit search <word>` \
             searches one of them."
        );
        return Ok(());
    }
    println!(
        "Next: `crosskit status` names every root of every context and says which ones hold \
         nothing; each of the roots below is indexed on its own with `crosskit --context \
         <name> index init <directory>`, or all of them again with `crosskit --context <name> \
         index init`."
    );
    Err(format!(
        "{} of {} context(s) were not indexed whole: {}",
        partial.len(),
        contexts.len(),
        partial.join("; ")
    ))
}

/// One context, through the same pass as `index init` with no directory named.
///
/// The context is taken from the file that was written rather than from the answers in
/// hand, and so are its roots: this is where a draft that declared something other than what
/// was answered would be caught.
fn index_one(
    configured: &LoadedIndexPolicy,
    name: &WorkspaceName,
) -> Result<ContextPassReport, String> {
    let workspace = workspace::select(configured, Some(name.as_str()))?;
    Ok(index_context::prepare(configured, &workspace, None)?
        .walk(Pass::Initial, Narration::OneLinePerRoot))
}

/// Which roots of one context were not walked, for the line a first run ends with.
fn describe_partial(name: &WorkspaceName, report: &ContextPassReport) -> String {
    format!(
        "{name}: {} of {} root(s) not walked ({})",
        report.roots() - report.walked(),
        report.roots(),
        report.not_walked().join(", ")
    )
}
