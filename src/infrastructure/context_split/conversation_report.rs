// Sections В, Г and Ж of the report: conversations behind their projects, folders without git
// behind their conversations, and Codex sessions by their working directory.
use std::collections::BTreeMap;
use std::path::Path;

use super::conversation_placement::{CodexPlace, PlacedConversation, PlacedFolder};
use super::placement::{FolderIndex, FolderKind};
use super::report::{describe, percent, print_tally, shown};
use super::transcript_census::TranscriptFolder;
use crate::domain::NormalizedPath;
use crate::domain::context_split::{AskBecause, ContextSide, SplitDecision};

pub(super) fn print_conversations(
    folders: &[TranscriptFolder],
    conversations: &[PlacedConversation],
    index: &FolderIndex,
    home: &NormalizedPath,
) {
    println!("\n## В. Беседы за проектом\n");
    println!("| папка транскриптов | что кодирует | бесед | ниже, субагентов |");
    println!("| --- | --- | ---: | ---: |");
    for folder in folders {
        println!(
            "| `{}` | {} | {} | {} |",
            folder.name,
            kind_words(&index.kind_of(&folder.name), home),
            folder.conversations.len(),
            folder.nested_transcripts
        );
    }
    println!(
        "\nИмён папок, которые дают два известных каталога сразу: {}",
        index.ambiguous_names()
    );
    println!(
        "Транскриптов, которые не удалось прочитать: {}",
        conversations
            .iter()
            .filter(|conversation| !conversation.record.readable)
            .count()
    );
    print_naive_decoding(folders);

    let total = conversations.len();
    print_tally(
        "\nБеседы по правилу",
        conversations
            .iter()
            .map(|conversation| conversation.decision),
    );
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for conversation in conversations {
        *by_kind
            .entry(kind_class(&conversation.folder_kind))
            .or_default() += 1;
    }
    println!("\nБеседы по тому, что кодирует их папка:");
    for (kind, count) in &by_kind {
        println!("  {kind}: {count} ({:.1} %)", percent(*count, total));
    }

    print_working_directory_checks(conversations);
    print_the_shell_only_class(conversations);
}

fn kind_words(kind: &FolderKind, home: &NormalizedPath) -> String {
    match kind {
        FolderKind::Project(root) => format!("проект `{}`", shown(root, home)),
        FolderKind::Mixed(root) => format!("каталог с проектами `{}`", shown(root, home)),
        FolderKind::Document(root) => format!("папка документов `{}`", shown(root, home)),
        FolderKind::Temporary => "временный каталог".to_string(),
        FolderKind::Ambiguous(all) => format!("неоднозначно: {} каталога", all.len()),
        FolderKind::Unknown => "ничего известного".to_string(),
    }
}

fn kind_class(kind: &FolderKind) -> &'static str {
    match kind {
        FolderKind::Project(_) => "папка проекта",
        FolderKind::Mixed(_) => "смешанная папка",
        FolderKind::Document(_) => "папка документов",
        FolderKind::Temporary => "временный каталог",
        FolderKind::Ambiguous(_) => "неоднозначная",
        FolderKind::Unknown => "не кодирует известного",
    }
}

/// Why the measurement never decodes: every folder name read back with each dash as a `/`, and
/// whether that directory exists.
fn print_naive_decoding(folders: &[TranscriptFolder]) {
    let broken = folders
        .iter()
        .filter(|folder| !Path::new(&folder.name.replace('-', "/")).is_dir())
        .count();
    println!(
        "Папок, чьё имя, прочитанное обратно «каждое `-` — это `/`», не называет существующий \
         каталог: {broken} из {}",
        folders.len()
    );
}

fn print_working_directory_checks(conversations: &[PlacedConversation]) {
    let mut names = 0;
    let mut differs = 0;
    let mut none = 0;
    for conversation in conversations {
        match conversation.first_directory_names_the_folder {
            Some(true) => names += 1,
            Some(false) => differs += 1,
            None => none += 1,
        }
    }
    let moved: Vec<&PlacedConversation> = conversations
        .iter()
        .filter(|conversation| conversation.left_the_folders_project)
        .collect();
    let moved_across = moved
        .iter()
        .filter(|conversation| {
            conversation.decision == SplitDecision::Ask(AskBecause::WorkingDirectoriesDisagree)
        })
        .count();
    let visited_undecided = conversations
        .iter()
        .filter(|conversation| {
            conversation.decision.side().is_some()
                && conversation.directories.visited_an_undecided_project()
        })
        .count();
    let outside: usize = conversations
        .iter()
        .map(|conversation| conversation.directories.outside_projects())
        .sum();
    let moving: Vec<usize> = conversations
        .iter()
        .map(|conversation| conversation.record.working_directories.len())
        .filter(|directories| *directories > 1)
        .collect();
    println!("\nПроверка папки рабочим каталогом изнутри файла:");
    println!(
        "  бесед, менявших cwd по ходу: {}; наибольшее число разных cwd в одной: {}",
        moving.len(),
        moving.iter().max().copied().unwrap_or_default()
    );
    println!("  первый cwd кодируется в имя своей папки: {names}");
    println!("  первый cwd кодируется в другое имя: {differs}");
    println!("  cwd в файле нет вовсе: {none}");
    println!("  беседа уходила из проекта своей папки: {}", moved.len());
    println!("    из них в проект другой стороны (ответ стал вопросом): {moved_across}");
    println!("  разложенных бесед, заходивших в неразложенный проект: {visited_undecided}");
    println!("  рабочих каталогов вне всякого проекта, по всем беседам: {outside}");
}

/// Stage 37's class: conversations that made no file call, which the road through touches does
/// not see at all.
fn print_the_shell_only_class(conversations: &[PlacedConversation]) {
    let untouched: Vec<&PlacedConversation> = conversations
        .iter()
        .filter(|conversation| conversation.record.touched.is_empty())
        .collect();
    println!(
        "\nБесед без единого файлового вызова (класс этапа 37): {} из {}",
        untouched.len(),
        conversations.len()
    );
    print_tally(
        "  их ответ по правилу",
        untouched.iter().map(|conversation| conversation.decision),
    );
    print_tally(
        "  их ответ по одному cwd, папка не читается",
        untouched
            .iter()
            .map(|conversation| conversation.by_directories_alone),
    );
}

pub(super) fn print_folders(folders: &[PlacedFolder], home: &NormalizedPath) {
    println!("\n## Г. Папки без git\n");
    println!(
        "| папка | файлов | бесед Claude с cwd внутри | сессий Codex внутри | ответ | касания бесед по их стороне |"
    );
    println!("| --- | ---: | ---: | ---: | --- | --- |");
    for placed in folders {
        let touched = placed
            .touched_from
            .iter()
            .map(|(side, count)| {
                format!(
                    "{}×{count}",
                    side.map(ContextSide::as_str).unwrap_or("вопрос")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "| `{}` | {} | {} | {} | {} | {} |",
            shown(&placed.folder.path, home),
            placed.folder.files,
            placed.conversations_inside,
            placed.codex_sessions_inside,
            describe(&placed.decision),
            if touched.is_empty() {
                "—".to_string()
            } else {
                touched
            },
        );
    }
    print_tally(
        "\nПапки без git по правилу",
        folders.iter().map(|placed| placed.decision),
    );
    let touched = folders
        .iter()
        .filter(|placed| !placed.touched_from.is_empty())
        .count();
    println!(
        "Папок, в которых беседы трогали файлы (не правило, а то, что могло бы им стать): {touched}"
    );
}

pub(super) fn print_codex(places: &BTreeMap<CodexPlace, usize>) {
    let total: usize = places.values().sum();
    println!("\n## Ж. Codex: сессии по рабочему каталогу\n");
    println!("Сессий: {total}");
    println!("| где cwd | сессий |");
    println!("| --- | ---: |");
    for (place, count) in places {
        let words = match place {
            CodexPlace::Unnamed => "первая строка cwd не называет".to_string(),
            CodexPlace::Temporary => "временный каталог".to_string(),
            CodexPlace::Project(decision) => format!("проект · {}", describe(decision)),
            CodexPlace::Mixed => "каталог с проектами".to_string(),
            CodexPlace::Document => "папка документов".to_string(),
            CodexPlace::Elsewhere => "в другом месте".to_string(),
        };
        println!("| {words} | {count} |");
    }
    let in_projects: usize = places
        .iter()
        .filter(|(place, _)| matches!(place, CodexPlace::Project(_)))
        .map(|(_, count)| *count)
        .sum();
    let decided: usize = places
        .iter()
        .filter(|(place, _)| matches!(place, CodexPlace::Project(decision) if decision.side().is_some()))
        .map(|(_, count)| *count)
        .sum();
    println!(
        "В известных проектах: {in_projects} из {total} ({:.0} %); из них в разложенных: {decided}",
        percent(in_projects, total)
    );
}
