// Section Е of the report: the rule's answers against the owner's own labels — or, until the
// owner has written them, the template to write them into.
//
// The labels are a file outside the repository named by `CROSSKIT_SPLIT_TRUTH`. Without it the
// comparison says what to set and compares nothing; it prints the lines the owner would label,
// each with `?` where the label goes, and never a guess in its place: a template filled in by
// the rule would be the rule checking itself.
use std::collections::BTreeMap;

use super::conversation_placement::{PlacedConversation, PlacedFolder};
use super::home_layout::{HomeLayout, TRUTH_VARIABLE};
use super::placement::{FolderKind, Repositories};
use super::report::shown;
use super::transcript_census::TranscriptFolder;
use crate::domain::NormalizedPath;
use crate::domain::context_split::{Agreement, ContextSide, OwnerTruth, SplitDecision, agreement};

/// One thing the owner can label, and the rule's answer about it.
struct Checked<'a> {
    kind: &'static str,
    path: &'a NormalizedPath,
    /// Where the owner's label is looked for when the path itself has none: the first working
    /// directory of a conversation, so that labelling a project labels its conversations.
    fallback: Option<&'a NormalizedPath>,
    decision: SplitDecision,
}

const PROJECT: &str = "проект";
const FOLDER: &str = "папка без git";
const CONVERSATION: &str = "беседа";

fn checked<'a>(
    repositories: &'a Repositories,
    conversations: &'a [PlacedConversation],
    folders: &'a [PlacedFolder],
) -> Vec<Checked<'a>> {
    let projects = repositories.outermost().map(|placed| Checked {
        kind: PROJECT,
        path: &placed.found.root,
        fallback: None,
        decision: placed.effective,
    });
    let folders = folders.iter().map(|placed| Checked {
        kind: FOLDER,
        path: &placed.folder.path,
        fallback: None,
        decision: placed.decision,
    });
    let conversations = conversations.iter().map(|placed| Checked {
        kind: CONVERSATION,
        path: &placed.record.file,
        fallback: placed.record.first_working_directory(),
        decision: placed.decision,
    });
    projects.chain(folders).chain(conversations).collect()
}

pub(super) fn check_against_the_owner(
    layout: &HomeLayout,
    repositories: &Repositories,
    transcripts: &[TranscriptFolder],
    conversations: &[PlacedConversation],
    folders: &[PlacedFolder],
) {
    println!("\n## Е. Сверка с владельцем\n");
    let items = checked(repositories, conversations, folders);
    let Some(file) = std::env::var_os(TRUTH_VARIABLE) else {
        println!(
            "{TRUTH_VARIABLE} не задан: сверять не с чем, и ничего не сверено. Ниже — заготовка: \
             замените `?` на work, personal или skip, сохраните вне репозитория и повторите с \
             {TRUTH_VARIABLE}=<файл>."
        );
        print_template(layout, repositories, transcripts, conversations, folders);
        return;
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) => {
            println!(
                "{TRUTH_VARIABLE}={}: {error}. Ничего не сверено.",
                file.to_string_lossy()
            );
            return;
        }
    };
    let (truth, unreadable) = OwnerTruth::parse(&text, layout.home());
    println!(
        "Меток в эталоне: {}; строк не прочитано: {unreadable:?}",
        truth.len()
    );

    let mut by_kind: BTreeMap<&str, BTreeMap<Agreement, usize>> = BTreeMap::new();
    let mut named = Vec::new();
    for item in &items {
        let owner = truth
            .side_of(item.path)
            .or_else(|| item.fallback.and_then(|path| truth.side_of(path)));
        let verdict = agreement(&item.decision, owner);
        *by_kind
            .entry(item.kind)
            .or_default()
            .entry(verdict)
            .or_default() += 1;
        if matches!(verdict, Agreement::CrossedSides | Agreement::SkipDisagrees) {
            named.push((verdict, item));
        }
    }
    println!(
        "\n| что | сошлось | **уверенная ошибка** | расхождение о skip | спросили → work | \
         спросили → personal | спросили → skip | эталон молчит |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for (kind, tally) in &by_kind {
        let count = |verdict: Agreement| tally.get(&verdict).copied().unwrap_or_default();
        println!(
            "| {kind} | {} | **{}** | {} | {} | {} | {} | {} |",
            count(Agreement::Agrees),
            count(Agreement::CrossedSides),
            count(Agreement::SkipDisagrees),
            count(Agreement::Asked(ContextSide::Work)),
            count(Agreement::Asked(ContextSide::Personal)),
            count(Agreement::Asked(ContextSide::Skip)),
            count(Agreement::OwnerSilent),
        );
    }
    for (verdict, item) in named {
        let what = match verdict {
            Agreement::CrossedSides => "уверенная ошибка",
            _ => "расхождение о skip",
        };
        println!(
            "  {what}: {} `{}` — правило: {}",
            item.kind,
            shown(item.path, layout.home()),
            super::report::describe(&item.decision)
        );
    }
}

/// The lines the owner labels: every outermost repository, every folder without git, every
/// transcript folder (a label on it covers its conversations), and the conversations of a
/// folder that names no single project, one by one.
fn print_template(
    layout: &HomeLayout,
    repositories: &Repositories,
    transcripts: &[TranscriptFolder],
    conversations: &[PlacedConversation],
    folders: &[PlacedFolder],
) {
    let home = layout.home();
    println!("\n```text");
    println!("# путь  метка — work | personal | skip; `?` — ещё не решено");
    println!("# {PROJECT}ы");
    for placed in repositories.outermost() {
        println!("{} ?", shown(&placed.found.root, home));
    }
    println!("# {FOLDER}");
    for placed in folders {
        println!("{} ?", shown(&placed.folder.path, home));
    }
    println!("# папки транскриптов: метка покрывает все беседы в папке");
    for folder in transcripts {
        println!(
            "{} ?",
            shown(
                &layout
                    .transcripts()
                    .join(&folder.name)
                    .unwrap_or_else(|_| layout.transcripts()),
                home
            )
        );
    }
    println!("# беседы папок, которые не называют одного проекта");
    for placed in conversations
        .iter()
        .filter(|placed| !matches!(placed.folder_kind, FolderKind::Project(_)))
    {
        println!("{} ?", shown(&placed.record.file, home));
    }
    println!("```");
}
