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
// Two questions about one thing rather than about a corpus: where one named file stops on
// its way into the index, and what a context whose graph was never written says about
// itself.
use std::path::Path;

use crate::domain::{NormalizedPath, PolicyClass, WorkspaceName};
use crate::infrastructure::config::load_index_policy;
use crate::infrastructure::ladybug::LadybugDatabaseManager;
use crate::infrastructure::ladybug::coverage_queries::coverage_of_root;
use crate::infrastructure::ladybug::graph_presence_of;

use super::configured_roots::read_configured_contexts;
use super::report::grouped;

/// The variable that names the file to follow. The path belongs to the machine the
/// measurement runs on, so it is asked for at run time and not written down here: a path out
/// of somebody's home directory is not a fact about this program.
const PROBE_VARIABLE: &str = "CROSSKIT_COVERAGE_PROBE";

/// The one file Stage 33 saw on disk and could not find in the index, followed down every
/// step of the path a file takes. The answer has to be one named step.
#[test]
#[ignore = "проба по одному файлу: cargo test --release --offline --features coverage-audit --lib coverage:: -- --ignored --nocapture"]
fn where_the_counterparty_file_stops() {
    let Ok(probe) = std::env::var(PROBE_VARIABLE) else {
        println!("Файл пробы не назван: задайте {PROBE_VARIABLE} и повторите.");
        return;
    };
    println!("Проба: {probe}\n");

    let path = match NormalizedPath::from_absolute(&probe) {
        Ok(path) => path,
        Err(error) => {
            println!("путь не разобран: {error}");
            return;
        }
    };
    println!(
        "1. на диске: {}",
        match std::fs::symlink_metadata(&probe) {
            Ok(metadata) => format!("да, {} байт", grouped(metadata.len() as usize)),
            Err(error) => format!("нет: {error}"),
        }
    );

    let loaded = load_index_policy().expect("конфигурация читается");
    let contexts = read_configured_contexts().expect("корни читаются");
    let Some(owning) = contexts.iter().find(|context| {
        context
            .roots
            .iter()
            .any(|root| NormalizedPath::from_absolute(root).is_ok_and(|root| path.is_within(&root)))
    }) else {
        println!("2. контекст: ни один корень не покрывает путь");
        return;
    };
    let root = owning
        .roots
        .iter()
        .find(|root| NormalizedPath::from_absolute(root).is_ok_and(|root| path.is_within(&root)))
        .expect("корень найден выше");
    println!("2. контекст: {} , корень {root}", owning.name);

    let configured = loaded.policy();
    println!(
        "3. политика БЕЗ корня прохода (то, что отвечает `policy explain`): {:?}, правило {:?}",
        configured.decide(&path).class(),
        configured
            .decide(&path)
            .rule()
            .map(|rule| rule.pattern().as_str().to_string()),
    );

    let scanned = NormalizedPath::from_absolute(root).expect("корень абсолютен");
    let during_a_pass = configured.with_scan_root(&scanned);
    let decision = during_a_pass.decide(&path);
    println!(
        "4. политика ВО ВРЕМЯ прохода этого корня: {:?}, правило {:?}",
        decision.class(),
        decision
            .rule()
            .map(|rule| rule.pattern().as_str().to_string()),
    );
    if decision.class() == PolicyClass::Exclude {
        println!("   → файл выпадает здесь: политика прохода его не допускает.");
        return;
    }

    let workspace = WorkspaceName::parse(&owning.name).expect("имя контекста разбирается");
    let Ok(manager) = LadybugDatabaseManager::open_workspace_read_only(&workspace) else {
        println!("5. граф контекста не открывается — значит, файла в нём нет.");
        return;
    };
    match coverage_of_root(&manager, &scanned) {
        Ok(coverage) => println!(
            "5. граф под этим корнем: файлов {}, с текущей версией {}, чанков {}, проходов {}",
            grouped(coverage.files()),
            grouped(coverage.files_with_current_version()),
            grouped(coverage.chunks_of_current_versions()),
            grouped(coverage.passes()),
        ),
        Err(error) => println!("5. граф не отвечает: {error}"),
    }
    print_graph_record_of(&manager, &path);
}

fn print_graph_record_of(manager: &LadybugDatabaseManager, path: &NormalizedPath) {
    let root_of_file = path
        .parent()
        .unwrap_or_else(|| NormalizedPath::from_absolute("/").expect("корень файловой системы"));
    match coverage_of_root(manager, &root_of_file) {
        Ok(coverage) if coverage.files() == 0 => {
            println!("6. сам файл: в графе его нет ни под каким видом.");
        }
        Ok(coverage) => println!(
            "6. каталог файла в графе: файлов {}, с текущей версией {}",
            grouped(coverage.files()),
            grouped(coverage.files_with_current_version()),
        ),
        Err(error) => println!("6. граф не отвечает: {error}"),
    }
}

/// What a context whose graph was never written says about itself.
#[test]
#[ignore = "проба по непроиндексированному контексту: см. команду выше"]
fn what_an_unindexed_context_says_about_itself() {
    let contexts = read_configured_contexts().expect("корни читаются");
    for context in &contexts {
        let Ok(workspace) = WorkspaceName::parse(&context.name) else {
            continue;
        };
        let presence = graph_presence_of(&workspace);
        let opened = LadybugDatabaseManager::open_workspace_read_only(&workspace).is_ok();
        println!(
            "{}: состояние {:?}, открывается на чтение: {}, корней {}",
            context.name,
            presence,
            opened,
            context.roots.len()
        );
        for root in &context.roots {
            let exists = Path::new(root).is_dir();
            println!("  {root}: каталог на месте: {exists}");
        }
    }
}
