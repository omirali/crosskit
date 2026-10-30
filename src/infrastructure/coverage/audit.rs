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
// This file holds the whole funnel: every configured root walked and put beside what its
// context's graph holds under it. The single questions it raised have files of their own.
use std::path::Path;
use std::time::Instant;

use crate::application::SourceWalker;
use crate::domain::WorkspaceName;
use crate::infrastructure::config::load_index_policy;
use crate::infrastructure::filesystem::DescriptorRelativeWalker;
use crate::infrastructure::ladybug::coverage_queries::{
    coverage_of_root, files_in_graph, recorded_pass_roots,
};
use crate::infrastructure::ladybug::{LadybugDatabaseManager, graph_presence_of};

use super::configured_roots::{configuration_path, read_configured_contexts};
use super::funnel::RootFunnel;
use super::report::{grouped, print_funnel_header, print_funnel_row, print_root_detail};
use super::tree_census::census_of_root;

#[test]
#[ignore = "замер покрытия: cargo test --release --offline --features coverage-audit --lib coverage:: -- --ignored --nocapture"]
fn what_the_owners_contexts_actually_hold() {
    let started = Instant::now();
    let loaded = match load_index_policy() {
        Ok(loaded) => loaded,
        Err(error) => {
            println!("КОНФИГУРАЦИЯ НЕ ПРОЧИТАНА: {error}. Ничего не измерено.");
            return;
        }
    };
    let contexts = match read_configured_contexts() {
        Ok(contexts) => contexts,
        Err(error) => {
            println!("КОРНИ НЕ ПРОЧИТАНЫ: {error}. Ничего не измерено.");
            return;
        }
    };
    println!(
        "Конфигурация: {}",
        configuration_path().unwrap_or_default().display()
    );
    println!("Контекстов: {}\n", contexts.len());

    let walker = DescriptorRelativeWalker;
    let mut funnels = Vec::new();
    let mut details = Vec::new();

    for context in &contexts {
        let workspace = match WorkspaceName::parse(&context.name) {
            Ok(workspace) => workspace,
            Err(error) => {
                println!("контекст {}: имя не разобрано: {error}", context.name);
                continue;
            }
        };
        let presence = graph_presence_of(&workspace);
        println!("--- контекст {}: граф — {:?}", context.name, presence);

        let manager = LadybugDatabaseManager::open_workspace_read_only(&workspace).ok();
        if let Some(manager) = &manager {
            match files_in_graph(manager) {
                Ok(files) => println!("    File в графе всего: {}", grouped(files)),
                Err(error) => println!("    граф не отвечает: {error}"),
            }
            match recorded_pass_roots(manager) {
                Ok(roots) if roots.is_empty() => {
                    println!("    ни одного прохода reconcile не записано");
                }
                Ok(roots) => {
                    for (root, runs) in roots {
                        println!("    проходов reconcile: {} по {root}", grouped(runs));
                    }
                }
                Err(error) => println!("    прогоны не прочитаны: {error}"),
            }
        }

        for configured_root in &context.roots {
            let root = match walker.resolve_root(Path::new(configured_root)) {
                Ok(root) => root,
                Err(error) => {
                    println!("    корень {configured_root} не разрешён: {error}");
                    continue;
                }
            };
            let walking = Instant::now();
            let tree = match census_of_root(&walker, loaded.policy(), &root) {
                Ok(tree) => tree,
                Err(error) => {
                    println!("    корень {root}: обход не завершён: {error}");
                    continue;
                }
            };
            let graph = manager
                .as_ref()
                .and_then(|manager| coverage_of_root(manager, &root).ok())
                .unwrap_or_default();

            let funnel = RootFunnel::new(
                &context.name,
                root.as_str(),
                tree.sieve,
                tree.content,
                graph,
            );
            println!(
                "    {} — {} записей, {:.1} с",
                root,
                grouped(funnel.entries_on_disk()),
                walking.elapsed().as_secs_f64()
            );
            details.push((funnel.clone(), tree.failures, tree.failure_count));
            funnels.push(funnel);
        }
    }

    println!("\n## Воронка по корням\n");
    print_funnel_header();
    for funnel in &funnels {
        print_funnel_row(funnel);
    }

    println!("\n## Каждая потеря, названная");
    for (funnel, failures, failure_count) in &details {
        print_root_detail(funnel, failures, *failure_count);
    }

    print_totals(&funnels);
    println!("\nзамер занял {:.1} с", started.elapsed().as_secs_f64());
}

fn print_totals(funnels: &[RootFunnel]) {
    println!("\n## Итого\n");
    println!("| контекст | на диске | допущено | в графе | разрыв |");
    println!("| --- | ---: | ---: | ---: | ---: |");
    let mut running = ContextTotals::default();
    for funnel in funnels {
        if funnel.context() != running.context {
            running.print_and_start(funnel.context());
        }
        running.add(funnel);
    }
    running.print_and_start("");

    let everything: usize = funnels
        .iter()
        .map(|funnel| funnel.content().revisions_with_text())
        .sum();
    println!(
        "\nРевизий с текстом, если бы проход прошёл по каждому корню: {}",
        grouped(everything)
    );
    println!(
        "Ревизий с текстом, которые графы держат сейчас: {}",
        grouped(
            funnels
                .iter()
                .map(|funnel| funnel.graph().files_with_current_version())
                .sum()
        )
    );
}

/// The running sums of one context while the rows are printed in order.
#[derive(Default)]
struct ContextTotals {
    context: String,
    on_disk: usize,
    admitted: usize,
    in_graph: usize,
}

impl ContextTotals {
    fn add(&mut self, funnel: &RootFunnel) {
        self.on_disk += funnel.entries_on_disk();
        self.admitted += funnel.sieve().reaches_the_graph();
        self.in_graph += funnel.graph().files();
    }

    /// Prints the context that just ended, if there was one, and begins the next.
    fn print_and_start(&mut self, next: &str) {
        if !self.context.is_empty() {
            println!(
                "| {} | {} | {} | {} | {} |",
                self.context,
                grouped(self.on_disk),
                grouped(self.admitted),
                grouped(self.in_graph),
                self.admitted as i64 - self.in_graph as i64,
            );
        }
        *self = Self {
            context: next.to_string(),
            ..Self::default()
        };
    }
}
