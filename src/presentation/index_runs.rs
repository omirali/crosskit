// `index runs prune`: parses the retention request and renders the typed report.
//
// The report is counters only. No run id, root or path is ever printed, so the output is
// safe to paste anywhere.
use crate::application::RunRetentionService;
use crate::domain::{RetentionMode, RunRetention, RunRetentionReport};
use crate::infrastructure::load_index_policy;

use super::help;
use super::workspace;

pub(super) const USAGE: &str =
    "Usage: crosskit [--context <name>] index runs prune --keep <N> [--apply]";

/// Run history belongs to one context's graph, so the command names the context first and
/// only then opens it.
pub(super) fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    let (retention, mode) = parse_prune(arguments)?;
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let selected = workspace::select(&loaded, context)?;
    let manager = workspace::open(&selected)?;
    let repository = manager.repository();
    let report = RunRetentionService::new(&repository)
        .execute(retention, mode)
        .map_err(|error| error.to_string())?;
    render(&report, retention);
    Ok(())
}

/// `--keep <N>` (or `--keep=<N>`) is required and must be an integer not below zero;
/// `--apply` switches from preview to removal. Anything else is a usage error.
fn parse_prune(arguments: &[String]) -> Result<(RunRetention, RetentionMode), String> {
    let Some((command, options)) = arguments.split_first() else {
        return Err(help::misuse(USAGE));
    };
    if command != "prune" {
        return Err(help::misuse(USAGE));
    }

    let mut keep = None;
    let mut mode = RetentionMode::Preview;
    let mut options = options.iter();
    while let Some(option) = options.next() {
        let value = match option.as_str() {
            "--apply" => {
                mode = RetentionMode::Apply;
                continue;
            }
            "--keep" => options
                .next()
                .ok_or_else(|| format!("--keep requires a value. {USAGE}"))?,
            other => match other.strip_prefix("--keep=") {
                Some(value) => value,
                None => return Err(format!("unknown argument {other:?}. {USAGE}")),
            },
        };
        if keep.is_some() {
            return Err(format!("--keep was given more than once. {USAGE}"));
        }
        keep = Some(parse_keep(value)?);
    }

    let keep = keep.ok_or_else(|| format!("--keep <N> is required. {USAGE}"))?;
    Ok((RunRetention::keep_newest(keep), mode))
}

fn parse_keep(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("--keep expects a number of 0 or more, received {value:?}. {USAGE}"))
}

fn render(report: &RunRetentionReport, retention: RunRetention) {
    match report.mode() {
        RetentionMode::Preview => println!(
            "Reconcile run retention preview (keep {} per root): nothing was changed.",
            retention.keep()
        ),
        RetentionMode::Apply => println!(
            "Reconcile run retention applied (keep {} per root).",
            retention.keep()
        ),
    }
    println!("Eligible closed runs: {}", report.eligible_closed_runs());
    println!(
        "Protected running runs: {}",
        report.protected_running_runs()
    );
    println!(
        "Protected legacy OBSERVED runs: {}",
        report.protected_legacy_runs()
    );
    println!("Retained runs: {}", report.retained_runs());
    match report.mode() {
        RetentionMode::Preview => println!("Would remove runs: {}", report.removed_runs()),
        RetentionMode::Apply => println!("Removed runs: {}", report.removed_runs()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_prune;
    use crate::domain::{RetentionMode, RunRetention};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn keep_without_apply_is_a_preview() {
        assert_eq!(
            parse_prune(&arguments(&["prune", "--keep", "3"])).unwrap(),
            (RunRetention::keep_newest(3), RetentionMode::Preview)
        );
        assert_eq!(
            parse_prune(&arguments(&["prune", "--keep=0"])).unwrap(),
            (RunRetention::keep_newest(0), RetentionMode::Preview)
        );
    }

    #[test]
    fn apply_may_come_before_or_after_keep() {
        assert_eq!(
            parse_prune(&arguments(&["prune", "--apply", "--keep", "2"])).unwrap(),
            (RunRetention::keep_newest(2), RetentionMode::Apply)
        );
        assert_eq!(
            parse_prune(&arguments(&["prune", "--keep", "2", "--apply"])).unwrap(),
            (RunRetention::keep_newest(2), RetentionMode::Apply)
        );
    }

    #[test]
    fn invalid_requests_are_rejected() {
        for invalid in [
            vec!["prune"],
            vec!["prune", "--apply"],
            vec!["prune", "--keep"],
            vec!["prune", "--keep", "-1"],
            vec!["prune", "--keep", "many"],
            vec!["prune", "--keep", "1", "--keep", "2"],
            vec!["prune", "--keep", "1", "--force"],
            vec!["list", "--keep", "1"],
            vec![],
        ] {
            assert!(
                parse_prune(&arguments(&invalid)).is_err(),
                "{invalid:?} must be rejected"
            );
        }
    }
}
