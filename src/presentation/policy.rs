use crate::application::{PolicyExplainService, PolicyExplanation};
use crate::domain::{IndexPolicy, NormalizedPath, PolicyClass, RuleOrigin};
use crate::infrastructure::{LoadedIndexPolicy, load_index_policy};

use super::help;
pub(super) const USAGE: &str = "Usage: crosskit policy explain <path>";

pub fn run(arguments: &[String]) -> Result<(), String> {
    let [command, path] = arguments else {
        return Err(help::misuse(USAGE));
    };
    if command != "explain" {
        return Err(help::misuse(USAGE));
    }

    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    // `absolute` only prepends the working directory; the path is never opened or stat-ed.
    let absolute = std::path::absolute(path).map_err(|error| format!("resolve {path}: {error}"))?;
    let normalized = NormalizedPath::from_absolute(&absolute.to_string_lossy())
        .map_err(|error| error.to_string())?;
    let policy = policy_of_a_pass(&loaded, &normalized);
    let explanation = PolicyExplainService::new(&policy)
        .execute(normalized.as_str())
        .map_err(|error| error.to_string())?;
    render(&explanation, &loaded, &normalized);
    Ok(())
}

/// The policy a pass over this path's own root would apply, which is the only policy whose
/// answer is worth anything to somebody asking why a file is or is not in the index.
///
/// `[workspace.<name>] include` is the boundary of a context, not an Include rule: a root
/// gets Include only inside the pass that walks it (`IndexPolicy::with_scan_root`). Asking
/// the configured policy alone therefore answered `exclude — no rule covers this path` for
/// **every working file the owner has**, and it answered it about files that were in the
/// index and findable by search. Found at Stage 29, measured against the graph at Stage 34,
/// and fixed here with the thing Stage 35 added: a context can be asked for its roots.
///
/// The same reasoning is written out one file away, in `search_index::visible_paths`, which
/// has always got it right — "a path no rule matches is not hidden: the scan that indexed it
/// was granted its own root". Those two are the only places in the program that ask the
/// policy outside a pass, and only this one was wrong.
///
/// A path under no context, or under a context whose include is a mask, gets the configured
/// policy unchanged: nothing would grant it anything, and that is the true answer.
fn policy_of_a_pass(loaded: &LoadedIndexPolicy, path: &NormalizedPath) -> IndexPolicy {
    let Some(workspace) = loaded.workspaces().workspace_of(path) else {
        return loaded.policy().clone();
    };
    let root = loaded
        .workspaces()
        .roots_of(workspace)
        .into_iter()
        .filter_map(|root| {
            NormalizedPath::from_absolute(root.directory()?)
                .ok()
                .filter(|root| path.is_within(root))
        })
        .next();
    match root {
        Some(root) => loaded.policy().with_scan_root(&root),
        None => loaded.policy().clone(),
    }
}

fn render(explanation: &PolicyExplanation, loaded: &LoadedIndexPolicy, path: &NormalizedPath) {
    println!("Path: {}", explanation.path());
    // Which context owns the path is asked of the map, not of the policy: a path may be
    // allowed by the policy and still belong to no context, and then it is not indexed.
    match loaded.workspaces().workspace_of(path) {
        Some(workspace) => println!("Context: {workspace}"),
        None => println!("Context: none; a path no context covers is not indexed"),
    }
    println!("Decision: {}", class_label(explanation.class()));
    match explanation.matched_rule() {
        Some(rule) => println!(
            "Matched rule: {} {} ({})",
            class_label(rule.class()),
            rule.pattern(),
            origin_label(rule.origin())
        ),
        None => println!("Matched rule: none"),
    }
    println!("Reason: {}", reason(explanation));
    println!("Policy file: {}", policy_file_label(loaded));
}

fn reason(explanation: &PolicyExplanation) -> &'static str {
    match (explanation.matched_rule(), explanation.class()) {
        (None, _) => {
            "no rule covers this path, and no root of a context contains it; paths nothing \
             grants are excluded by default"
        }
        (Some(_), PolicyClass::Exclude) => {
            "an Exclude rule covers this path; Exclude is stronger than any other rule"
        }
        (Some(_), PolicyClass::Metadata) => {
            "a Metadata rule covers this path and no Exclude rule does; only name, size, type and dates are indexed"
        }
        (Some(_), PolicyClass::Include) => {
            "an Include rule covers this path and no Metadata or Exclude rule does; metadata and content are indexed"
        }
    }
}

fn class_label(class: PolicyClass) -> &'static str {
    match class {
        PolicyClass::Include => "include",
        PolicyClass::Metadata => "metadata",
        PolicyClass::Exclude => "exclude",
    }
}

fn origin_label(origin: RuleOrigin) -> &'static str {
    match origin {
        RuleOrigin::BuiltIn => "built-in default",
        RuleOrigin::Configuration => "configuration",
        RuleOrigin::ScanRoot => "a root of this context, as a pass over it would grant",
        // `policy explain` answers from the configuration alone, without walking, so it
        // never meets a marker and never prints this. A run that did walk does.
        RuleOrigin::GeneratedDirectory => "marker in the directory",
    }
}

pub(super) fn policy_file_label(loaded: &LoadedIndexPolicy) -> String {
    if loaded.found() {
        return loaded.path().display().to_string();
    }
    format!(
        "{} (not found; built-in defaults apply)",
        loaded.path().display()
    )
}
