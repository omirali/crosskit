// `index review`: what waits for the owner to decide, and `index review approve <mask>`,
// which decides about a whole kind of thing at once.
//
// The list is grouped because the number is large by design. Stage 23 stopped queueing jobs
// for revisions no rule covers, so what is left is a question — and a question about
// twenty-four thousand revisions is only answerable if it is asked about a dozen kinds of
// them. Each group carries what approving it would cost, at the price stage 22 measured,
// because that cost is the whole reason this stage exists.
use crate::application::{
    AnalysisReview, AnalysisReviewService, KnowledgeReader, ReviewGroup, WorkScopeReader,
    WorkScopeService,
};
use crate::domain::{
    AnalysisRules, NormalizedPath, PARTS_WITHOUT_ASKING, PathPattern, WorkspaceName,
};
use crate::infrastructure::ladybug::LadybugGraphRepository;
use crate::infrastructure::{LoadedIndexPolicy, load_index_policy, write_index_policy};

use super::confirmation;
use super::format_number;
use super::help;
use super::run_cost::forecast;
use super::run_cost::{SECONDS_PER_CALL, duration};
use super::scope_directory::{AskedScope, resolve};
use super::search_index::visible_paths;
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] index review\n       crosskit [--context <name>] index review approve <mask> [--yes]\n       crosskit [--context <name>] index review approve --touching <directory> [--yes]\n       crosskit [--context <name>] index review approve --session <id> [--yes]";

/// How many groups of each kind are printed. A review nobody reads to the end is a review
/// nobody makes a decision from; what is left over is counted, never hidden.
const GROUPS_SHOWN: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    List,
    Approve {
        mask: String,
        assume_yes: bool,
    },
    /// Exactly the conversations that worked on files under a directory, or one conversation.
    ApproveConversations {
        which: Conversations,
        assume_yes: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Conversations {
    Touching(String),
    Session(String),
}

pub(super) fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    let request = parse(arguments)?;
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select(&loaded, context)?;
    let manager = workspace::open(&selected)?;
    let repository = manager.repository();
    let is_visible = visible_paths(&loaded, &selected);
    println!("Context: {selected}");

    match request {
        Request::List => {
            let review = AnalysisReviewService
                .execute(&repository, loaded.analysis_rules(), &is_visible)
                .map_err(|error| error.to_string())?;
            render(&review);
            Ok(())
        }
        Request::Approve { mask, assume_yes } => {
            approve(&loaded, &repository, &mask, assume_yes, &is_visible)
        }
        Request::ApproveConversations { which, assume_yes } => approve_conversations(
            &loaded,
            &selected,
            &repository,
            &which,
            assume_yes,
            &is_visible,
        ),
    }
}

/// Approves exactly the transcripts of some conversations, one mask each, after saying what
/// asking about them costs. Nothing is written before the answer.
fn approve_conversations(
    loaded: &LoadedIndexPolicy,
    context: &WorkspaceName,
    repository: &LadybugGraphRepository,
    which: &Conversations,
    assume_yes: bool,
    is_visible: &dyn Fn(&NormalizedPath) -> bool,
) -> Result<(), String> {
    let (revisions, named) = match which {
        Conversations::Touching(directory) => {
            let scope = resolve(&AskedScope::Touching(directory.clone()), loaded, context)?;
            let work = WorkScopeService::new(repository)
                .select(&scope)
                .map_err(|error| error.to_string())?;
            let named = format!(
                "the {} conversation(s) that worked on files under {}",
                work.conversations().unwrap_or(0),
                scope.directory().as_str()
            );
            (work.revision_ids().to_vec(), named)
        }
        Conversations::Session(session) => (
            repository
                .current_revisions_of_sessions(std::slice::from_ref(session))
                .map_err(|error| error.to_string())?,
            format!("conversation {session}"),
        ),
    };
    let sizes = repository
        .revision_sizes(&revisions)
        .map_err(|error| error.to_string())?;
    // A transcript is approved by the path it is present at; one a rule already covers needs
    // no second rule.
    let wanted: Vec<(String, usize, i64)> = sizes
        .iter()
        .filter_map(|size| {
            let path = size.present_paths.iter().find(|path| is_visible(path))?;
            (!loaded.analysis_rules().cover(path))
                .then(|| (path.as_str().to_string(), size.parts, size.bytes))
        })
        .collect();
    if wanted.is_empty() {
        println!(
            "Nothing to approve: {named} — {} transcript(s), every one already covered by a rule or at no path this context admits.",
            sizes.len()
        );
        return Ok(());
    }
    if let Some((unsafe_path, _, _)) = wanted
        .iter()
        .find(|(path, _, _)| path.contains(['*', '?', '[', ']', '{', '}']))
    {
        return Err(format!(
            "{unsafe_path:?} holds a character a mask reads as a pattern, so it cannot be approved as exactly this file; approve a mask with `index review approve <mask>`"
        ));
    }

    let parts: usize = wanted.iter().map(|(_, parts, _)| parts).sum();
    let bytes: i64 = wanted.iter().map(|(_, _, bytes)| bytes).sum();
    let rate = repository
        .knowledge_totals()
        .map_err(|error| error.to_string())?
        .rate();
    println!(
        "To approve: {named} — {} transcript(s), {parts} parts, {bytes} bytes; asking them is {} call(s), {}",
        wanted.len(),
        wanted.len(),
        forecast(rate, wanted.len(), bytes)
    );
    let large: Vec<usize> = wanted
        .iter()
        .map(|(_, parts, _)| *parts)
        .filter(|parts| *parts > PARTS_WITHOUT_ASKING)
        .collect();
    if !large.is_empty() {
        println!(
            "Of them {} over {PARTS_WITHOUT_ASKING} parts (the largest {}): a run holds them back until it is given --allow-large or --max-parts.",
            large.len(),
            large.iter().max().unwrap_or(&0)
        );
    }
    let budget = loaded.insight_budget().per_job_bytes();
    let over_budget = wanted
        .iter()
        .filter(|(_, _, bytes)| *bytes > budget)
        .count();
    if over_budget > 0 {
        println!(
            "Of them {over_budget} larger than one job may send ({budget} bytes, `[insight] max_input_bytes`): they will be refused, not sent, unless the limit is raised first."
        );
    }
    println!(
        "An approved conversation is asked again every time it grows: each new revision of its transcript is a new job."
    );
    let masks: Vec<&str> = wanted.iter().map(|(path, _, _)| path.as_str()).collect();
    if !confirmed(
        loaded,
        &format!("these {} transcript(s)", masks.len()),
        assume_yes,
    )? {
        println!("Nothing was written.");
        return Ok(());
    }
    let contents = with_approved_masks(loaded, &masks)?;
    let kept = write_index_policy(loaded.path(), &contents).map_err(|error| error.to_string())?;
    println!("Approved: {} transcript(s)", masks.len());
    if let Some(kept) = kept {
        println!("Previous configuration kept at {}", kept.display());
    }
    let covers = |path: &NormalizedPath| masks.contains(&path.as_str());
    let queued = AnalysisReviewService
        .backfill(repository, &covers)
        .map_err(|error| error.to_string())?;
    let next = match which {
        Conversations::Touching(directory) => {
            format!("`crosskit index jobs run --touching {directory} --limit <N>` asks only them")
        }
        Conversations::Session(_) => {
            "`crosskit index jobs run --limit <N>` asks them in the queue's order".to_string()
        }
    };
    println!(
        "Queued for analysis: {} revision(s). {next}",
        format_number(queued as u64)
    );
    Ok(())
}

fn approve(
    loaded: &LoadedIndexPolicy,
    repository: &impl crate::application::RevisionInventory,
    mask: &str,
    assume_yes: bool,
    is_visible: &dyn Fn(&crate::domain::NormalizedPath) -> bool,
) -> Result<(), String> {
    let pattern = PathPattern::parse(mask)
        .map_err(|error| format!("{mask:?} is not a mask this program accepts: {error}"))?;
    // Already covered is not a failure and not a second rule: the answer is that the
    // question was already answered, and nothing is written.
    if loaded
        .analysis_rules()
        .approved()
        .iter()
        .any(|approved| approved.as_str() == pattern.as_str())
    {
        println!("Already approved: {mask}. Nothing was written.");
        return Ok(());
    }

    let covers = |path: &crate::domain::NormalizedPath| {
        is_visible(path) && pattern.matches(path) && !loaded.analysis_rules().cover(path)
    };
    if !confirmed(loaded, mask, assume_yes)? {
        println!("Nothing was written.");
        return Ok(());
    }

    let contents = with_approved_masks(loaded, &[mask])?;
    let kept = write_index_policy(loaded.path(), &contents).map_err(|error| error.to_string())?;
    println!("Approved: {mask}");
    if let Some(kept) = kept {
        println!("Previous configuration kept at {}", kept.display());
    }

    // The revisions already exist, so the approval reaches back over them, one page and one
    // transaction at a time.
    let queued = AnalysisReviewService
        .backfill(repository, &covers)
        .map_err(|error| error.to_string())?;
    println!(
        "Queued for analysis: {} revision(s){}",
        format_number(queued as u64),
        estimate(queued)
    );
    Ok(())
}

/// The policy file with `masks` added to `[analysis] approved`.
///
/// Rewritten from what is on disk rather than regenerated from what was parsed: a
/// configuration is a thing a person wrote, with their comments and their order in it, and
/// a command that adds one line must not return a file they did not write.
fn with_approved_masks(loaded: &LoadedIndexPolicy, masks: &[&str]) -> Result<String, String> {
    let existing = std::fs::read_to_string(loaded.path()).unwrap_or_default();
    let line = format!(
        "approved = [{}]",
        quoted_masks(loaded.analysis_rules(), masks)
    );
    if let Some(start) = find_approved_line(&existing) {
        let end = existing[start..]
            .find('\n')
            .map_or(existing.len(), |offset| start + offset);
        return Ok(format!("{}{line}{}", &existing[..start], &existing[end..]));
    }
    if let Some(start) = existing.find("[analysis]") {
        let insert = existing[start..]
            .find('\n')
            .map_or(existing.len(), |offset| start + offset + 1);
        return Ok(format!(
            "{}{line}\n{}",
            &existing[..insert],
            &existing[insert..]
        ));
    }
    // A blank line before a new section, because the file is read by people.
    let separator = match existing.is_empty() {
        true => "",
        false if existing.ends_with("\n\n") => "",
        false if existing.ends_with('\n') => "\n",
        false => "\n\n",
    };
    Ok(format!("{existing}{separator}[analysis]\n{line}\n"))
}

/// Where `approved = …` stands inside `[analysis]`, when it does. Looked for after the
/// section header so a key of that name in another section is never rewritten.
fn find_approved_line(contents: &str) -> Option<usize> {
    let section = contents.find("[analysis]")?;
    let rest = &contents[section..];
    let next_section = rest[1..].find("\n[").map_or(rest.len(), |at| at + 1);
    rest[..next_section]
        .find("\napproved")
        .map(|at| section + at + 1)
}

fn quoted_masks(rules: &AnalysisRules, added: &[&str]) -> String {
    rules
        .approved()
        .iter()
        .map(PathPattern::as_str)
        .chain(added.iter().copied())
        .map(|mask| format!("{mask:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn confirmed(loaded: &LoadedIndexPolicy, what: &str, assume_yes: bool) -> Result<bool, String> {
    if assume_yes {
        return Ok(true);
    }
    confirmation::confirmed(&format!(
        "Approve {what} for analysis and write it to {}?",
        loaded.path().display()
    ))
}

fn render(review: &AnalysisReview) {
    println!(
        "Analysed by rule: {} revision(s)",
        format_number(review.covered() as u64)
    );
    println!(
        "Waiting for a decision: {} revision(s){}",
        format_number(review.waiting() as u64),
        estimate(review.waiting())
    );
    if review.waiting() == 0 {
        return;
    }
    render_groups("By kind", review.by_extension());
    render_groups("By directory", review.by_directory());
    println!();
    println!("Approve a group with `crosskit index review approve <mask>`.");
}

fn render_groups(heading: &str, groups: &[ReviewGroup]) {
    println!();
    println!("{heading}:");
    for group in groups.iter().take(GROUPS_SHOWN) {
        println!(
            "  {:>9}  {}{}",
            format_number(group.revisions() as u64),
            group.mask(),
            estimate(group.revisions())
        );
    }
    if groups.len() > GROUPS_SHOWN {
        println!(
            "  and {} more group(s)",
            format_number((groups.len() - GROUPS_SHOWN) as u64)
        );
    }
}

/// What a number of jobs would take, at the time one call of the question was measured to
/// take (`run_cost::SECONDS_PER_CALL`).
fn estimate(jobs: usize) -> String {
    if jobs == 0 {
        return String::new();
    }
    format!(" ({})", duration(jobs as f64 * SECONDS_PER_CALL))
}

fn parse(arguments: &[String]) -> Result<Request, String> {
    let assume_yes = arguments.iter().any(|one| one == "--yes");
    let rest: Vec<&String> = arguments.iter().filter(|one| *one != "--yes").collect();
    let conversations = match rest.as_slice() {
        [command, flag, value] if *command == "approve" && *flag == "--touching" => {
            Some(Conversations::Touching(value.to_string()))
        }
        [command, flag, value] if *command == "approve" && *flag == "--session" => {
            Some(Conversations::Session(value.to_string()))
        }
        _ => None,
    };
    if let Some(which) = conversations {
        return Ok(Request::ApproveConversations { which, assume_yes });
    }
    // An option without its value is not a mask that happens to start with dashes.
    if rest.iter().skip(1).any(|one| one.starts_with("--")) {
        return Err(help::misuse(USAGE));
    }
    match arguments {
        [] => Ok(Request::List),
        [command, rest @ ..] if command == "approve" => match rest {
            [mask] => Ok(Request::Approve {
                mask: mask.clone(),
                assume_yes: false,
            }),
            [mask, flag] | [flag, mask] if flag == "--yes" => Ok(Request::Approve {
                mask: mask.clone(),
                assume_yes: true,
            }),
            _ => Err(help::misuse(USAGE)),
        },
        _ => Err(help::misuse(USAGE)),
    }
}

#[cfg(test)]
mod tests {
    use super::{Request, estimate, find_approved_line, parse};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn conversations_are_approved_by_a_directory_or_by_a_session() {
        assert_eq!(
            parse(&arguments(&[
                "approve",
                "--touching",
                "/work/registry",
                "--yes"
            ]))
            .unwrap(),
            Request::ApproveConversations {
                which: super::Conversations::Touching("/work/registry".to_string()),
                assume_yes: true
            }
        );
        assert_eq!(
            parse(&arguments(&["approve", "--session", "5b0c77de"])).unwrap(),
            Request::ApproveConversations {
                which: super::Conversations::Session("5b0c77de".to_string()),
                assume_yes: false
            }
        );
        assert!(parse(&arguments(&["approve", "--touching"])).is_err());
    }

    #[test]
    fn no_arguments_list_what_waits() {
        assert_eq!(parse(&arguments(&[])).unwrap(), Request::List);
    }

    #[test]
    fn approve_takes_a_mask_and_an_optional_yes_in_either_order() {
        let expected = |assume_yes| Request::Approve {
            mask: "**/*.py".to_string(),
            assume_yes,
        };
        assert_eq!(
            parse(&arguments(&["approve", "**/*.py"])).unwrap(),
            expected(false)
        );
        assert_eq!(
            parse(&arguments(&["approve", "**/*.py", "--yes"])).unwrap(),
            expected(true)
        );
        assert_eq!(
            parse(&arguments(&["approve", "--yes", "**/*.py"])).unwrap(),
            expected(true)
        );
        assert!(parse(&arguments(&["approve"])).is_err());
    }

    /// An estimate is only useful in the unit a person plans in, and zero jobs take no time
    /// worth printing.
    #[test]
    fn an_estimate_is_rounded_to_what_somebody_plans_with() {
        assert_eq!(estimate(0), "");
        assert_eq!(estimate(2), " (~54 s)");
        assert_eq!(estimate(100), " (~45 min)");
        assert_eq!(estimate(24_246), " (~181.8 h)");
    }

    /// The key is found only inside `[analysis]`, so a key of that name in another section
    /// is never the one that gets rewritten.
    #[test]
    fn the_approved_key_is_found_only_in_its_own_section() {
        assert!(find_approved_line("[index]\napproved = []\n").is_none());
        assert!(find_approved_line("[analysis]\napproved = []\n").is_some());
        assert!(
            find_approved_line("[analysis]\nengine = \"x\"\n\n[index]\napproved = []\n").is_none()
        );
    }
}
