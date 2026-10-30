// Stage 38: whether the owner's machine splits into a work and a personal context by itself.
//
//   CROSSKIT_SPLIT_HOME=$HOME [CROSSKIT_SPLIT_TRUTH=<labels outside the repository>] \
//     cargo test --release --offline --features context-split-measurement \
//     --lib context_split:: -- --ignored --nocapture
//
// Ignored, because it reads a real home directory. It writes nothing anywhere, opens no
// database, runs no `git` and calls no model: the request a model would get for the remainder is
// assembled and weighed, and that is as far as it goes.
use std::time::Instant;

use super::codex_census::census_of_codex_sessions;
use super::conversation_placement::{place_codex_sessions, place_conversations, place_folders};
use super::conversation_report::{print_codex, print_conversations, print_folders};
use super::document_census::census_of_documents;
use super::home_layout::HomeLayout;
use super::inventory_report::{Inventory, print_inventory};
use super::model_remainder::remainder_of;
use super::owner_truth_check::check_against_the_owner;
use super::placement::{FolderIndex, Repositories, owner_domain_of};
use super::repository_census::census_of_repositories;
use super::repository_report::print_repositories;
use super::transcript_census::census_of_transcripts;
use crate::domain::context_split::{Identity, identity_in_git_config};
use crate::infrastructure::filesystem::DescriptorRelativeWalker;

#[test]
#[ignore = "замер раскладки: CROSSKIT_SPLIT_HOME=$HOME cargo test --release --offline --features context-split-measurement --lib context_split:: -- --ignored --nocapture"]
fn whether_the_machine_splits_into_work_and_personal_by_itself() {
    let started = Instant::now();
    let layout = match HomeLayout::from_environment() {
        Ok(layout) => layout,
        Err(message) => {
            println!("{message}");
            return;
        }
    };

    let walker = DescriptorRelativeWalker;
    let found = census_of_repositories(&walker, &layout.project_roots());
    let global = global_identities(&layout);
    let owner = owner_domain_of(&global, &found.repositories);
    let repositories = Repositories::place(found.repositories, owner);
    let documents = census_of_documents(&layout);
    let transcripts = census_of_transcripts(&layout.transcripts());
    let codex = census_of_codex_sessions(&layout.codex_sessions());

    let index = FolderIndex::new(&repositories, &layout, &documents);
    let conversations = place_conversations(&transcripts, &index, &repositories);
    let folders = place_folders(&documents.folders, &conversations, &codex);
    let codex_places = place_codex_sessions(&codex, &repositories, &index, &documents.folders);

    print_inventory(&Inventory {
        repositories: &repositories,
        generated_directories: found.generated_directories,
        walk_failures: found.walk_failures,
        unread_git_files: found.unread_files,
        transcripts: &transcripts,
        codex_sessions: codex.len(),
        documents: &documents,
    });
    print_repositories(&repositories, &global, &layout);
    print_conversations(&transcripts, &conversations, &index, layout.home());
    print_folders(&folders, layout.home());

    let (remainder, untitled) = remainder_of(&repositories, &folders, &conversations);
    println!("\n## Д. Остаток для модели — собран, не отправлен\n");
    println!("| что | число |");
    println!("| --- | ---: |");
    println!("| вопросов | {} |", remainder.questions());
    println!(
        "| образцов (уже разложенных имён) | {} |",
        remainder.examples()
    );
    println!("| байт запроса | {} |", remainder.bytes());
    println!("| бесед без имени — спросить по имени нечем | {untitled} |");
    println!(
        "\nЗапрос целиком, как он ушёл бы:\n```text\n{}```",
        remainder.text()
    );

    check_against_the_owner(
        &layout,
        &repositories,
        &transcripts,
        &conversations,
        &folders,
    );
    print_codex(&codex_places);
    println!("\nзамер занял {:.1} с", started.elapsed().as_secs_f64());
}

/// The identity git would use where a repository sets none: `[user]` of the two global files.
fn global_identities(layout: &HomeLayout) -> Vec<Identity> {
    layout
        .global_git_configs()
        .iter()
        .filter_map(|path| std::fs::read_to_string(path.as_str()).ok())
        .filter_map(|text| identity_in_git_config(&text))
        .collect()
}
