// Stage 34: what is actually in the owner's contexts, measured on their own machine through
// their own configuration.
//
//   cargo test --release --offline --features coverage-audit \
//     --lib coverage:: -- --ignored --nocapture
//
// Every measurement of this stage is ignored: each reads a real tree and real graphs. None
// of them writes anywhere — no database is opened for writing, no context is indexed, no
// configuration is touched.
//
// What the graphs say on their own, without walking anything: which roots a pass ever
// reached, which files the gap between disk and graph is made of, and where a database's
// size comes from.
use std::path::Path;

use crate::application::SourceWalker;
use crate::domain::WorkspaceName;
use crate::infrastructure::config::load_index_policy;
use crate::infrastructure::filesystem::DescriptorRelativeWalker;
use crate::infrastructure::ladybug::LadybugDatabaseManager;
use crate::infrastructure::ladybug::coverage_queries::{
    GraphCoverage, coverage_of_root, files_in_graph, known_paths_under, locations_under,
    node_counts,
};

use super::configured_roots::read_configured_contexts;
use super::path_diff::PathDiff;
use super::report::grouped;
use super::tree_census::census_of_root;

/// How many of a root's locations the gap probe will read back out of the graph. Above it
/// the probe reports the root as too large to reconcile file by file rather than reading a
/// part of it and calling the rest a finding.
const PATHS_CEILING: usize = 200_000;
/// How many example paths each finding prints. The count is the measurement; the examples
/// are there so a reader can go and look at one.
const EXAMPLES: usize = 5;

/// Every configured root of every context, resolved and compared against the graph without
/// walking anything. The cheap half of the audit: it answers which roots a pass ever
/// reached, and it takes seconds rather than minutes.
#[test]
#[ignore = "быстрая сводка по графам: cargo test --release --offline --features coverage-audit --lib coverage:: -- --ignored --nocapture"]
fn which_roots_the_graph_knows_at_all() {
    let contexts = read_configured_contexts().expect("корни читаются");
    let walker = DescriptorRelativeWalker;
    println!("| контекст | корень | File в графе | с текущей версией | чанков | проходов |");
    println!("| --- | --- | ---: | ---: | ---: | ---: |");

    for context in &contexts {
        let Ok(workspace) = WorkspaceName::parse(&context.name) else {
            continue;
        };
        let Ok(manager) = LadybugDatabaseManager::open_workspace_read_only(&workspace) else {
            println!("| {} | — | граф не открыт | | | |", context.name);
            continue;
        };
        let mut covered = 0;
        for configured_root in &context.roots {
            let Ok(root) = walker.resolve_root(Path::new(configured_root)) else {
                println!(
                    "| {} | `{configured_root}` | не разрешён | | | |",
                    context.name
                );
                continue;
            };
            let coverage = coverage_of_root(&manager, &root).unwrap_or_default();
            covered += coverage.files();
            print_coverage_row(&context.name, root.as_str(), &coverage);
        }
        match files_in_graph(&manager) {
            Ok(total) => println!(
                "| **{}** | **всего** | **{}** | | | под корнями: {} |",
                context.name,
                grouped(total),
                grouped(covered)
            ),
            Err(error) => println!("| {} | всего | {error} | | | |", context.name),
        }
    }
}

fn print_coverage_row(context: &str, root: &str, coverage: &GraphCoverage) {
    println!(
        "| {context} | `{root}` | {} | {} | {} | {} |",
        grouped(coverage.files()),
        grouped(coverage.files_with_current_version()),
        grouped(coverage.chunks_of_current_versions()),
        grouped(coverage.passes()),
    );
}

/// The gap of every root turned into files with a reason each: never indexed at all, on
/// disk but held as gone, or held as present and no longer there.
#[test]
#[ignore = "разбор разрыва по файлам: см. команду выше"]
fn which_files_the_gap_is_made_of() {
    let loaded = load_index_policy().expect("конфигурация читается");
    let contexts = read_configured_contexts().expect("корни читаются");
    let walker = DescriptorRelativeWalker;

    for context in &contexts {
        let Ok(workspace) = WorkspaceName::parse(&context.name) else {
            continue;
        };
        let manager = LadybugDatabaseManager::open_workspace_read_only(&workspace).ok();
        for configured_root in &context.roots {
            let Ok(root) = walker.resolve_root(Path::new(configured_root)) else {
                continue;
            };
            let Ok(tree) = census_of_root(&walker, loaded.policy(), &root) else {
                println!("{root}: обход не завершён");
                continue;
            };
            let known = match &manager {
                None => Vec::new(),
                Some(manager) => match locations_under(manager, &root) {
                    Ok(total) if total > PATHS_CEILING => {
                        println!(
                            "\n### {root}\n{} мест в графе — больше потолка разбора {}; \
                             пофайлово не сверялось",
                            grouped(total),
                            grouped(PATHS_CEILING)
                        );
                        continue;
                    }
                    Ok(_) => known_paths_under(manager, &root, PATHS_CEILING).unwrap_or_default(),
                    Err(error) => {
                        println!("\n### {root}\nграф не отвечает: {error}");
                        continue;
                    }
                },
            };

            let diff = PathDiff::between(&tree.admitted_paths, &known);
            println!("\n### {root}");
            println!("  сошлось: {}", grouped(diff.agreed()));
            print_finding("  на диске, в графе нет вовсе", diff.never_indexed());
            print_finding(
                "  на диске и допущено, граф держит как missing",
                diff.wrongly_missing(),
            );
            print_finding(
                "  граф держит как present, политика больше не допускает",
                diff.gone_from_disk(),
            );
        }
    }
}

fn print_finding(label: &str, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    println!("{label}: {}", grouped(paths.len()));
    for path in paths.iter().take(EXAMPLES) {
        println!("      {path}");
    }
}

/// Where a context's database size comes from: how many nodes of each kind it holds, beside
/// the size of the file that holds them.
#[test]
#[ignore = "из чего состоит граф каждого контекста: см. команду выше"]
fn what_each_graph_is_made_of() {
    let contexts = read_configured_contexts().expect("корни читаются");
    let directory = crate::infrastructure::data_directory().expect("каталог данных");
    for context in &contexts {
        let Ok(workspace) = WorkspaceName::parse(&context.name) else {
            continue;
        };
        let Ok(manager) = LadybugDatabaseManager::open_workspace_read_only(&workspace) else {
            continue;
        };
        let file = directory.join(format!("graph-{}.lbug", context.name));
        let bytes = std::fs::metadata(&file).map(|data| data.len()).unwrap_or(0);
        println!(
            "\n{}: {:.1} МБ",
            context.name,
            bytes as f64 / (1024.0 * 1024.0)
        );
        match node_counts(&manager) {
            Ok(counts) => {
                for (label, count) in counts {
                    println!("  {label:<16} {}", grouped(count));
                }
            }
            Err(error) => println!("  граф не отвечает: {error}"),
        }
    }
}
