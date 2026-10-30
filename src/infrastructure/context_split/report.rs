// How the measurement prints itself: the words for every answer the rule can give, and the
// sections of the report in order.
//
// The report is printed, not written: the numbers go into `docs/context-split-measurement.md`
// by hand, beside the date and the state of the machine, and the names in it — real on the
// owner's terminal — become the pseudonyms `docs/` already uses. Printing real names here is the
// shape `coverage` set: the terminal is the owner's, the repository is not.
use std::collections::BTreeMap;

use crate::domain::NormalizedPath;
use crate::domain::context_split::{AskBecause, DecidedBy, SplitDecision};

/// The words for one answer, in the order the report sorts them.
pub(super) fn describe(decision: &SplitDecision) -> String {
    match decision {
        SplitDecision::Decided { side, by } => format!("{} · {}", side.as_str(), decided_by(*by)),
        SplitDecision::Ask(because) => format!("спросить · {}", ask_because(*because)),
    }
}

fn decided_by(by: DecidedBy) -> &'static str {
    match by {
        DecidedBy::OwnerMailDomain => "группа или хост origin несут домен почты владельца",
        DecidedBy::EnclosingRepository => "следует за внешним репозиторием",
        DecidedBy::TemporaryPlace => "временный каталог системы",
        DecidedBy::ProjectFolder => "папка транскриптов разложенного проекта",
        DecidedBy::WorkingDirectory => "рабочие каталоги в проектах одной стороны",
        DecidedBy::ConversationsInside => "беседы внутри разложены в одну сторону",
    }
}

fn ask_because(because: AskBecause) -> &'static str {
    match because {
        AskBecause::OwnIdentity => "у репозитория своя почта, не владельца",
        AskBecause::NoRemote => "нет origin с хостом",
        AskBecause::NoGroup => "origin без группы",
        AskBecause::NoOwnerDomain => "на машине нет домена почты владельца",
        AskBecause::SeveralOwnerDomains => "на машине несколько доменов почты владельца",
        AskBecause::ForeignGroup => "группа origin не несёт домен владельца",
        AskBecause::FollowsAQuestion => "следует за вопросом, заданным выше",
        AskBecause::MixedFolder => "папка каталога с несколькими проектами",
        AskBecause::UnknownFolder => "папка не кодирует известный каталог",
        AskBecause::WorkingDirectoriesDisagree => "рабочие каталоги на разных сторонах",
        AskBecause::NoConversationsInside => "внутри не работала ни одна беседа",
    }
}

/// A path as the owner reads it: under the home it starts with `~`.
pub(super) fn shown(path: &NormalizedPath, home: &NormalizedPath) -> String {
    match path.as_str().strip_prefix(home.as_str()) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.as_str().to_string(),
    }
}

/// How many of `decisions` gave each answer, printed as rows under a header.
pub(super) fn print_tally(title: &str, decisions: impl IntoIterator<Item = SplitDecision>) {
    let mut tally: BTreeMap<SplitDecision, usize> = BTreeMap::new();
    let mut total = 0;
    for decision in decisions {
        *tally.entry(decision).or_default() += 1;
        total += 1;
    }
    println!("\n{title}: {total}");
    println!("| ответ | число |");
    println!("| --- | ---: |");
    for (decision, count) in tally {
        println!("| {} | {count} |", describe(&decision));
    }
}

pub(super) fn percent(part: usize, whole: usize) -> f64 {
    match whole {
        0 => 0.0,
        whole => part as f64 * 100.0 / whole as f64,
    }
}

/// The rule as a table, printed beside the numbers it produced so that nobody reads one
/// without the other.
pub(super) fn print_rule() {
    println!("\nПравило для репозитория, по порядку; первое сработавшее отвечает:");
    println!("| # | признак | ответ |");
    println!("| --- | --- | --- |");
    for (order, signal, answer) in [
        (
            "1",
            "`[user]` репозитория называет домен, не домен владельца",
            "спросить",
        ),
        (
            "2",
            "нет `origin`, или он указывает на путь на этой машине",
            "спросить",
        ),
        ("3", "у `origin` хост без группы", "спросить"),
        (
            "4",
            "на машине нет ровно одного домена почты владельца",
            "спросить",
        ),
        (
            "5",
            "группа `origin` = домен владельца, или хост = домен или его поддомен",
            "work",
        ),
        ("6", "любая другая группа, на любом хосте", "спросить"),
        (
            "—",
            "почта в reflog",
            "не решает; даёт домен владельца для 4–5",
        ),
        ("—", "хост сам по себе", "не решает"),
        ("—", "репозиторий внутри репозитория", "следует за внешним"),
    ] {
        println!("| {order} | {signal} | {answer} |");
    }
}
