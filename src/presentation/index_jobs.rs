// `index jobs`: the insight outbox by the numbers, `index jobs run`, which works a bounded
// number of them, and `index jobs withdraw`, which brings the whole queue to the rules in
// one pass. All three print counters only: no id, path, hash, text or error message of the
// engine ever reaches the terminal.
use crate::application::{
    CostRate, InsightProvider, InsightQueueService, InsightQueueSummary, InsightWithdrawalReport,
    InsightWithdrawalService, JobScope, KnowledgeReader, KnowledgeRewrite, Requeued, SelectedWork,
    WorkScope, WorkScopeService,
};
use crate::domain::{HeldJob, InsightWorkReport, LargeWork, PARTS_WITHOUT_ASKING};
use crate::infrastructure::analysis;
use crate::infrastructure::insight_run::InsightRun;
use crate::infrastructure::insight_run::{self, Question};
use crate::infrastructure::ladybug::LadybugGraphRepository;
use crate::infrastructure::load_index_policy;

use super::help;
use super::run_cost::{forecast, spend_lines};
use super::scope_directory::{AskedScope, resolve};
use super::search_index::wanted_paths;
use super::workspace;

pub(super) const USAGE: &str = "Usage: crosskit [--context <name>] index jobs\n       crosskit [--context <name>] index jobs run --limit <N> [--under <directory> | --touching <directory>] [--max-parts <N> | --allow-large]\n       crosskit [--context <name>] index jobs recheck\n       crosskit [--context <name>] index jobs requeue [--under <directory> | --touching <directory>]\n       crosskit [--context <name>] index jobs withdraw";

/// The queue belongs to the context whose graph holds it, so both forms name one like
/// every other command that opens a database.
pub(super) fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    let request = parse(arguments)?;
    let recheck_only = request == Request::Recheck;
    let requeue_only = matches!(request, Request::Requeue { .. });
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select(&loaded, context)?;
    let egress = loaded.workspaces().content_egress(&selected);
    let manager = workspace::open(&selected)?;
    let repository = manager.repository();
    println!("Context: {selected}");
    let (limit, scope, large) = match request {
        Request::Summary => {
            render_summary(
                &InsightQueueService
                    .execute(&repository)
                    .map_err(|error| error.to_string())?,
            );
            return Ok(());
        }
        // Nothing is sent and nothing is read out of a file, so no engine is decided on and
        // no key is needed: this pass only stops asking questions nobody is asking any more.
        Request::Withdraw => {
            render_withdrawal(
                &InsightWithdrawalService
                    .execute(&repository, &wanted_paths(&loaded, &selected))
                    .map_err(|error| error.to_string())?,
            );
            return Ok(());
        }
        Request::Work {
            limit,
            scope,
            large,
        } => (limit, scope, large),
        Request::Recheck => (0, None, LargeWork::Take),
        Request::Requeue { scope } => (0, scope, LargeWork::Take),
    };
    // The directory is checked and the work it selects is counted before any engine is
    // decided on: a directory outside the context is a mistake of the request, not of the run.
    let selected_work = match &scope {
        None => None,
        Some(asked) => {
            let scope = resolve(asked, &loaded, &selected)?;
            let work = WorkScopeService::new(&repository)
                .select(&scope)
                .map_err(|error| error.to_string())?;
            println!("{}", selection_line(&scope, &work));
            Some(work)
        }
    };

    // Putting answers back in the queue asks nothing and sends nothing: no engine is needed,
    // only the question this build asks.
    if requeue_only {
        let current = analysis::insight_prompt_version(loaded.engine());
        let requeued = repository
            .requeue_asked_otherwise(current, selected_work.as_ref().map(SelectedWork::job_ids))
            .map_err(|error| error.to_string())?;
        let rate = repository
            .knowledge_totals()
            .map_err(|error| error.to_string())?
            .rate();
        println!("{}", requeued_line(&requeued, current, rate));
        return Ok(());
    }

    // Which handler can exist here is decided by the context, before anything is claimed.
    let question = Question::new(loaded.vocabulary(), loaded.normalizations());
    let run = match insight_run::decide(
        &manager,
        egress,
        loaded.engine(),
        loaded.insight_budget(),
        question,
    ) {
        Ok(run) => run,
        // The engine is not usable: the jobs stay exactly where they are. Claiming them
        // would count attempts against a call that was never going to happen.
        Err(unavailable) => {
            println!(
                "Analysis: not run — {}",
                unavailable.reason("No job was claimed.")
            );
            return Ok(());
        }
    };
    // A recheck needs the same handler a run builds — the question and the cache key are the
    // run's — and then asks nothing.
    if recheck_only {
        return recheck(&run, &repository);
    }
    println!("Analysis: {}", run.label());
    // The same question search asks before it shows a path, asked before the queue spends
    // anything on one. A queue outlives the policy that filled it.
    let job_scope = selected_work
        .as_ref()
        .map_or(JobScope::Everything, |work| JobScope::Among(work.job_ids()));
    let swept = run
        .sweep_within(
            &repository,
            limit,
            &wanted_paths(&loaded, &selected),
            job_scope,
            large,
        )
        .map_err(|error| error.to_string())?;
    render_work(swept.work());
    if !swept.held().is_empty() {
        let rate = repository
            .knowledge_totals()
            .map_err(|error| error.to_string())?
            .rate();
        println!("{}", held_line(swept.held(), large, rate));
    }
    if let Some(spend) = swept.spend() {
        for line in spend_lines(spend) {
            println!("{line}");
        }
    }
    Ok(())
}

/// What the operator asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    Summary,
    Work {
        limit: usize,
        scope: Option<AskedScope>,
        large: LargeWork,
    },
    Withdraw,
    /// Apply the rules of this build to the knowledge already stored, from cached answers.
    Recheck,
    /// Put back in the queue what a model answered under an earlier question.
    Requeue {
        scope: Option<AskedScope>,
    },
}

/// The jobs left queued as too large, with what taking them would cost.
fn held_line(held: &[HeldJob], large: LargeWork, rate: Option<CostRate>) -> String {
    let most = match large {
        LargeWork::HoldOver(most) => most,
        LargeWork::Take => 0,
    };
    let parts: usize = held.iter().map(HeldJob::parts).sum();
    let bytes: i64 = held.iter().map(HeldJob::bytes).sum();
    let largest = held.iter().map(HeldJob::parts).max().unwrap_or(0);
    format!(
        "Held back: {} job(s) over {most} parts — {parts} parts, {bytes} bytes, the largest {largest} parts; {}. They stay queued: `--allow-large` takes them, `--max-parts <N>` moves the line",
        held.len(),
        forecast(rate, held.len(), bytes)
    )
}

/// What a requeue put back, and what asking it again costs.
fn requeued_line(requeued: &Requeued, current: &str, rate: Option<CostRate>) -> String {
    if requeued.jobs == 0 {
        return format!(
            "Requeued: nothing — every analysed current revision here was answered under {current}"
        );
    }
    format!(
        "Requeued: {} revision(s) a model answered under an earlier question than {current}; {} parts, {} bytes. Asking them again: {}. They are the oldest work in the queue, so the next `index jobs run` takes them first (with the same --under or --touching, only them); their facts stay until the new answers replace them",
        requeued.jobs,
        requeued.parts,
        requeued.bytes,
        forecast(rate, requeued.jobs, requeued.bytes)
    )
}

/// What the directory selected, and why the run may take fewer than that.
fn selection_line(scope: &WorkScope, work: &SelectedWork) -> String {
    let directory = scope.directory().as_str();
    let what = match work.conversations() {
        None => format!("{} revisions under {directory}", work.revisions()),
        Some(conversations) => format!(
            "{conversations} conversations that worked on files under {directory}, {} transcript revisions",
            work.revisions()
        ),
    };
    let mut line = format!("Selected: {what}; {} of them queued", work.queued());
    if work.without_job() > 0 {
        let approve = match scope {
            WorkScope::Touching(_) => format!(
                "`crosskit index review approve --touching {directory}` approves exactly these conversations, with their price before anything is written"
            ),
            WorkScope::Under(_) => {
                "`crosskit index review` shows the masks that would cover them".to_string()
            }
        };
        line.push_str(&format!(
            ". {} of them have no job: no analysis rule covers them. {approve}",
            work.without_job()
        ));
    }
    line
}

/// No arguments summarize the queue; `run --limit <N>` (or `--limit=<N>`) works at most N
/// jobs; `withdraw` takes none at all, because it is asked of the whole queue or not at all.
/// Anything else is a usage error, refused before a database is opened.
fn parse(arguments: &[String]) -> Result<Request, String> {
    let Some((command, options)) = arguments.split_first() else {
        return Ok(Request::Summary);
    };
    if command == "withdraw" && options.is_empty() {
        return Ok(Request::Withdraw);
    }
    if command == "recheck" && options.is_empty() {
        return Ok(Request::Recheck);
    }
    if command == "requeue" {
        return match options {
            [] => Ok(Request::Requeue { scope: None }),
            [flag, directory] if flag == "--under" => Ok(Request::Requeue {
                scope: Some(AskedScope::Under(directory.clone())),
            }),
            [flag, directory] if flag == "--touching" => Ok(Request::Requeue {
                scope: Some(AskedScope::Touching(directory.clone())),
            }),
            _ => Err(help::misuse(USAGE)),
        };
    }
    if command != "run" {
        return Err(help::misuse(USAGE));
    }

    let mut limit = None;
    let mut scope = None;
    let mut large = None;
    let mut options = options.iter();
    while let Some(option) = options.next() {
        let mut value = |name: &str| {
            options
                .next()
                .cloned()
                .ok_or_else(|| format!("{name} requires a value. {USAGE}"))
        };
        let asked = match option.as_str() {
            "--under" => Some(AskedScope::Under(value("--under")?)),
            "--touching" => Some(AskedScope::Touching(value("--touching")?)),
            _ => None,
        };
        let large_asked = match option.as_str() {
            "--allow-large" => Some(LargeWork::Take),
            "--max-parts" => Some(LargeWork::HoldOver(parse_parts(&value("--max-parts")?)?)),
            _ => None,
        };
        if let Some(asked) = large_asked {
            if large.is_some() {
                return Err(format!(
                    "--max-parts and --allow-large say one thing about large jobs, once. {USAGE}"
                ));
            }
            large = Some(asked);
            continue;
        }
        if let Some(asked) = asked {
            if scope.is_some() {
                return Err(format!(
                    "--under and --touching name one directory, once. {USAGE}"
                ));
            }
            scope = Some(asked);
            continue;
        }
        let given = match option.as_str() {
            "--limit" => value("--limit")?,
            other => match other.strip_prefix("--limit=") {
                Some(given) => given.to_string(),
                None => return Err(format!("unknown argument {other:?}. {USAGE}")),
            },
        };
        if limit.is_some() {
            return Err(format!("--limit was given more than once. {USAGE}"));
        }
        limit = Some(parse_limit(&given)?);
    }

    Ok(Request::Work {
        limit: limit.ok_or_else(|| format!("--limit <N> is required. {USAGE}"))?,
        scope,
        large: large.unwrap_or(LargeWork::HoldOver(PARTS_WITHOUT_ASKING)),
    })
}

fn parse_parts(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|parts| *parts > 0)
        .ok_or_else(|| {
            format!("--max-parts expects a number of 1 or more, received {value:?}. {USAGE}")
        })
}

fn parse_limit(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("--limit expects a number of 0 or more, received {value:?}. {USAGE}"))
}

/// The recheck: every analysed current revision proved again from its cached answer under the
/// rules of this build, with how many facts that leaves and why the others went.
fn recheck(
    run: &InsightRun<Box<dyn InsightProvider>>,
    repository: &LadybugGraphRepository,
) -> Result<(), String> {
    let facts_before = repository
        .knowledge_totals()
        .map_err(|error| error.to_string())?
        .facts;
    let Some((report, spend)) = run.recheck(repository).map_err(|error| error.to_string())? else {
        println!(
            "Recheck: nothing to do — this context asks no model, so no answer was ever stored"
        );
        return Ok(());
    };
    let facts_after = repository
        .knowledge_totals()
        .map_err(|error| error.to_string())?
        .facts;
    println!(
        "Rechecked: {} revisions from their cached answers ({} had none, {} could not be read); no model was asked",
        report.rechecked, report.without_cached_answer, report.failed
    );
    println!("Facts: {facts_before} → {facts_after}");
    let [knowledge, _cost] = spend_lines(&spend);
    println!("{knowledge}");
    Ok(())
}

fn render_summary(summary: &InsightQueueSummary) {
    for (state, count) in summary.counts() {
        println!("Insight jobs {}: {count}", state.label());
    }
}

/// What one sweep found. `Still queued` is the number the owner is waiting on, so it is
/// printed rather than left to be worked out from the other two.
fn render_withdrawal(report: &InsightWithdrawalReport) {
    println!("Queued jobs examined: {}", report.examined());
    println!(
        "Withdrawn (no rule asks for them any more): {}",
        report.withdrawn()
    );
    println!("Still queued: {}", report.still_queued());
}

fn render_work(report: &InsightWorkReport) {
    println!(
        "Returned to the queue: {}",
        report.recovery().returned_to_queue()
    );
    println!("Retired as dead: {}", report.recovery().retired_dead());
    println!("Claimed jobs: {}", report.claimed());
    println!("Completed jobs: {}", report.completed());
    println!("Failed jobs: {}", report.failed());
    println!(
        "Skipped jobs (the policy no longer asks): {}",
        report.skipped()
    );
}

#[cfg(test)]
mod tests {
    use super::{AskedScope, Request, parse};
    use crate::domain::{LargeWork, PARTS_WITHOUT_ASKING};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn no_arguments_summarize_the_queue() {
        assert_eq!(parse(&arguments(&[])).unwrap(), Request::Summary);
    }

    #[test]
    fn run_takes_the_limit_in_either_spelling() {
        assert_eq!(
            parse(&arguments(&["run", "--limit", "5"])).unwrap(),
            Request::Work {
                limit: 5,
                scope: None,
                large: LargeWork::HoldOver(PARTS_WITHOUT_ASKING)
            }
        );
        assert_eq!(
            parse(&arguments(&["run", "--limit=0"])).unwrap(),
            Request::Work {
                limit: 0,
                scope: None,
                large: LargeWork::HoldOver(PARTS_WITHOUT_ASKING)
            }
        );
    }

    #[test]
    fn run_takes_one_directory_by_either_question() {
        assert_eq!(
            parse(&arguments(&[
                "run",
                "--under",
                "/work/registry",
                "--limit",
                "3"
            ]))
            .unwrap(),
            Request::Work {
                limit: 3,
                scope: Some(AskedScope::Under("/work/registry".to_string())),
                large: LargeWork::HoldOver(PARTS_WITHOUT_ASKING)
            }
        );
        assert_eq!(
            parse(&arguments(&[
                "run",
                "--limit=3",
                "--touching",
                "docs",
                "--max-parts",
                "80"
            ]))
            .unwrap(),
            Request::Work {
                limit: 3,
                scope: Some(AskedScope::Touching("docs".to_string())),
                large: LargeWork::HoldOver(80)
            }
        );
        assert_eq!(
            parse(&arguments(&["run", "--allow-large", "--limit", "1"])).unwrap(),
            Request::Work {
                limit: 1,
                scope: None,
                large: LargeWork::Take
            }
        );
    }

    #[test]
    fn withdraw_takes_nothing_at_all() {
        assert_eq!(parse(&arguments(&["withdraw"])).unwrap(), Request::Withdraw);
        assert_eq!(parse(&arguments(&["recheck"])).unwrap(), Request::Recheck);
        assert_eq!(
            parse(&arguments(&["requeue"])).unwrap(),
            Request::Requeue { scope: None }
        );
        assert_eq!(
            parse(&arguments(&["requeue", "--under", "docs"])).unwrap(),
            Request::Requeue {
                scope: Some(AskedScope::Under("docs".to_string()))
            }
        );
    }

    #[test]
    fn invalid_requests_are_rejected() {
        for invalid in [
            vec!["run"],
            vec!["withdraw", "--limit", "5"],
            vec!["withdraw", "--yes"],
            vec!["recheck", "--limit", "5"],
            vec!["requeue", "--limit", "5"],
            vec!["requeue", "--under"],
            vec!["run", "--limit"],
            vec!["run", "--limit", "-1"],
            vec!["run", "--limit", "many"],
            vec!["run", "--limit", "1", "--limit", "2"],
            vec!["run", "--limit", "1", "--force"],
            vec!["run", "--limit", "1", "--under"],
            vec!["run", "--limit", "1", "--under", "/a", "--touching", "/b"],
            vec!["run", "--under", "/a"],
            vec!["run", "--limit", "1", "--max-parts", "0"],
            vec!["run", "--limit", "1", "--allow-large", "--max-parts", "5"],
            vec!["list"],
            vec!["--all"],
        ] {
            assert!(
                parse(&arguments(&invalid)).is_err(),
                "{invalid:?} must be rejected"
            );
        }
    }
}
