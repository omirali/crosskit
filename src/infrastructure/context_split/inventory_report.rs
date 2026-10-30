// Section А of the report: what the machine holds, counted before anything is decided.
use std::collections::BTreeMap;

use super::document_census::DocumentCensus;
use super::placement::Repositories;
use super::transcript_census::TranscriptFolder;

/// Everything section А counts, gathered by the measurement.
pub(super) struct Inventory<'a> {
    pub(super) repositories: &'a Repositories,
    pub(super) generated_directories: u64,
    pub(super) walk_failures: usize,
    pub(super) unread_git_files: usize,
    pub(super) transcripts: &'a [TranscriptFolder],
    pub(super) codex_sessions: usize,
    pub(super) documents: &'a DocumentCensus,
}

pub(super) fn print_inventory(inventory: &Inventory) {
    let all = inventory.repositories.all().len();
    let outermost = inventory.repositories.outermost().count();
    let conversations: usize = inventory
        .transcripts
        .iter()
        .map(|folder| folder.conversations.len())
        .sum();
    let nested: usize = inventory
        .transcripts
        .iter()
        .map(|folder| folder.nested_transcripts)
        .sum();
    println!("\n## А. Инвентарь\n");
    println!("| вид | число |");
    println!("| --- | ---: |");
    println!("| git-репозиториев, с вложенными | {all} |");
    println!("| из них внешних — тех, что назвал бы контекст | {outermost} |");
    println!("| вложенных в другой репозиторий | {} |", all - outermost);
    println!(
        "| папок транскриптов Claude Code | {} |",
        inventory.transcripts.len()
    );
    println!("| бесед в них (`.jsonl` верхнего уровня) | {conversations} |");
    println!("| транскриптов ниже — субагенты, не беседы | {nested} |");
    println!("| сессий Codex | {} |", inventory.codex_sessions);
    let mut folders_by_root: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    for folder in &inventory.documents.folders {
        let entry = folders_by_root.entry(folder.root.as_str()).or_default();
        entry.0 += 1;
        entry.1 += folder.files;
    }
    for (root, loose) in &inventory.documents.loose_files {
        let (folders, files) = folders_by_root
            .get(root.as_str())
            .copied()
            .unwrap_or_default();
        println!(
            "| `{root}`: папок без git / файлов в них / файлов россыпью | {folders} / {files} / {loose} |"
        );
    }
    println!(
        "\nОбход: каталогов, пропущенных по маркеру, {}; отказов обхода {}; файлов .git, не прочитанных, {}.",
        inventory.generated_directories, inventory.walk_failures, inventory.unread_git_files
    );
}
