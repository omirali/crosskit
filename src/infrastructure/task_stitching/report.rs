// The words of the measurement: numbers on stdout, and the owner's list of the seed task in a
// file outside the repository.
//
// Stdout carries no conversation's name or words, so it can be pasted into a document of the
// repository as it is. The list names conversations and is written only where the command says.
use std::collections::BTreeMap;
use std::path::Path;

use super::labels::{Mark, Score};
use crate::domain::task_stitching::session::{Origin, Platform, SessionFacts, SessionRole};
use crate::domain::task_stitching::stitching::{
    Link, LinkSignal, Reach, StitchRules, Strength, relay_files,
};
use crate::infrastructure::utc_stamp::utc_stamp;

/// How many candidate sizes the report names.
const LARGEST_CANDIDATES: usize = 10;
/// How much of a conversation's name or opening the list shows.
const LIST_CHARACTERS: usize = 100;

pub(super) fn print_inventory(
    sessions: &[SessionFacts],
    dropped_copies: usize,
    notes: &[(&str, usize)],
) {
    let mut kinds: BTreeMap<(&str, &str, &str), usize> = BTreeMap::new();
    for session in sessions {
        *kinds
            .entry((
                platform(session.platform),
                origin(session.origin),
                role(session.role),
            ))
            .or_default() += 1;
    }
    println!("## А. Что прочитано\n");
    println!("| платформа | откуда | что это | сессий |");
    println!("| --- | --- | --- | ---: |");
    for ((platform, origin, role), count) in kinds {
        println!("| {platform} | {origin} | {role} | {count} |");
    }
    println!("\n| заметка | число |");
    println!("| --- | ---: |");
    println!("| копий, у которых есть оригинал (выброшены) | {dropped_copies} |");
    for (note, count) in notes {
        println!("| {note} | {count} |");
    }
}

/// One limit of common ground and what stitching makes of the sessions under it.
pub(super) struct CurvePoint {
    pub(super) limit: Option<usize>,
    pub(super) reach: Reach,
    pub(super) common_files: usize,
    pub(super) common_branches: usize,
    pub(super) candidates: Vec<Vec<usize>>,
    pub(super) seed_task: Option<usize>,
}

pub(super) fn print_curve(curve: &[CurvePoint], conversations: usize) {
    println!("\n## Б. Общие файлы и ветки: сколько бесед может их делить\n");
    println!(
        "Файл или ветка, которых касается больше бесед, чем предел, — общий журнал проекта и \
         ничего не связывает. Кандидаты — по фактам и сильным признакам.\n"
    );
    println!(
        "| где ищется продолжение | предел | общих файлов | общих веток | кандидатов | \
         бесед в них из {conversations} | крупнейший | задача затравки |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for point in curve {
        let covered: usize = point.candidates.iter().map(Vec::len).sum();
        let largest = point.candidates.first().map_or(0, Vec::len);
        println!(
            "| {} | {} | {} | {} | {} | {covered} | {largest} | {} |",
            reach(point.reach),
            point
                .limit
                .map_or_else(|| "нет".to_string(), |limit| limit.to_string()),
            point.common_files,
            point.common_branches,
            point.candidates.len(),
            point
                .seed_task
                .map_or_else(|| "—".to_string(), |size| size.to_string()),
        );
    }
}

pub(super) fn print_signals(links: &[Link]) {
    println!("\n## В. Признаки между беседами по правилам продукта\n");
    println!("| признак | сила | пар бесед |");
    println!("| --- | --- | ---: |");
    for signal in LinkSignal::ALL {
        let pairs = links
            .iter()
            .filter(|link| link.signals.contains(&signal))
            .count();
        println!(
            "| {} | {} | {pairs} |",
            label(signal),
            strength(signal.strength())
        );
    }
}

pub(super) fn print_candidates(title: &str, candidates: &[Vec<usize>], conversations: usize) {
    let covered: usize = candidates.iter().map(Vec::len).sum();
    let sizes: Vec<String> = candidates
        .iter()
        .take(LARGEST_CANDIDATES)
        .map(|group| group.len().to_string())
        .collect();
    println!("\n## {title}\n");
    println!("| | |");
    println!("| --- | --- |");
    println!("| кандидатов в задачи | {} |", candidates.len());
    println!("| бесед в них | {covered} из {conversations} |");
    println!("| размеры крупнейших | {} |", sizes.join(", "));
}

/// The seed's candidate: how many conversations, of which origin, over what span, and which
/// signals tied them.
pub(super) fn print_seed(title: &str, sessions: &[SessionFacts], group: &[usize], links: &[Link]) {
    let members: std::collections::BTreeSet<usize> = group.iter().copied().collect();
    let inside: Vec<&Link> = links
        .iter()
        .filter(|link| members.contains(&link.earlier) && members.contains(&link.later))
        .collect();
    let first = group.iter().filter_map(|&i| sessions[i].started_ms).min();
    let last = group.iter().filter_map(|&i| sessions[i].ended_ms).max();
    let copies = group
        .iter()
        .filter(|&&i| sessions[i].origin == Origin::ImportedCopy)
        .count();
    let directories: std::collections::BTreeSet<&str> = group
        .iter()
        .filter_map(|&i| sessions[i].working_directory.as_deref())
        .collect();
    println!("\n## {title}\n");
    println!("| | |");
    println!("| --- | --- |");
    println!("| бесед | {} |", group.len());
    println!("| из них копий в Codex | {copies} |");
    println!("| рабочих каталогов | {} |", directories.len());
    println!(
        "| с — по | {} — {} |",
        first.map(utc_stamp).unwrap_or_default(),
        last.map(utc_stamp).unwrap_or_default()
    );
    for signal in LinkSignal::ALL {
        let pairs = inside
            .iter()
            .filter(|link| link.signals.contains(&signal))
            .count();
        println!("| пар по признаку «{}» | {pairs} |", label(signal));
    }
}

/// The owner's list: every conversation of the project, earliest first, with whether stitching
/// put it in the seed's task and by what, and the owner's mark carried over from the last run.
pub(super) fn write_list(
    path: &Path,
    sessions: &[SessionFacts],
    project: &[usize],
    task: &Task<'_>,
    marks: &BTreeMap<String, Mark>,
) -> std::io::Result<()> {
    let mut members: Vec<usize> = project.to_vec();
    members.sort_by_key(|&i| sessions[i].started_ms.unwrap_or(i64::MAX));
    let mut text = String::from(
        "# Беседы проекта: разметка задачи\n\n\
         Отметьте в последней колонке, относится ли беседа к задаче: да / нет. Разметка \
         сохраняется при следующем прогоне, и по ней считаются точность и полнота сшивки.\n\
         «сильный» — сшивка поставила беседу в задачу по факту или сильному признаку; \
         «соседство» — только по соседству во времени; «нет» — сшивка её в задачу не взяла.\n\n\
         | начало | платформа | откуда | сшивка | признаки | имя или начало | id | в задаче? |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for i in members {
        let session = &sessions[i];
        let tie = if task.strong.contains(&i) {
            "сильный"
        } else if task.weak.contains(&i) {
            "соседство"
        } else {
            "нет"
        };
        let mut signals: Vec<&str> = task
            .links
            .iter()
            .filter(|link| link.earlier == i || link.later == i)
            .filter(|link| task.weak.contains(&link.earlier) && task.weak.contains(&link.later))
            .flat_map(|link| link.signals.iter().map(|s| label(*s)))
            .collect();
        signals.sort_unstable();
        signals.dedup();
        text.push_str(&format!(
            "| {} | {} | {} | {tie} | {} | {} | {} | {} |\n",
            session.started_ms.map(utc_stamp).unwrap_or_default(),
            platform(session.platform),
            origin(session.origin),
            signals.join(", "),
            name_of(session),
            session.id,
            marks.get(&session.id).map_or("", |mark| mark.as_str()),
        ));
    }
    text.push_str(&ties_of(sessions, task.weak, task.links, task.rules));
    std::fs::write(path, text)
}

/// The seed's task as stitching found it: on strong signals, with neighbours, and why.
pub(super) struct Task<'a> {
    pub(super) strong: &'a [usize],
    pub(super) weak: &'a [usize],
    pub(super) links: &'a [Link],
    pub(super) rules: &'a StitchRules,
}

/// Stitching against the owner's marks: what it found right and wrong, and what it missed, for
/// the task on strong signals, with neighbours, and for originals and copies apart.
pub(super) fn print_score(
    sessions: &[SessionFacts],
    task: &Task<'_>,
    marks: &BTreeMap<String, Mark>,
) {
    let in_task = marks.values().filter(|mark| **mark == Mark::InTask).count();
    println!("\n## З. Сшивка против разметки владельца\n");
    if marks.is_empty() {
        println!("Разметки нет: заполните последнюю колонку списка и запустите замер снова.");
        return;
    }
    println!(
        "Размечено бесед: {}, из них в задаче: {in_task}.\n",
        marks.len()
    );
    println!("| что | верно | неверно | не размечено | пропущено |");
    println!("| --- | ---: | ---: | ---: | ---: |");
    let ids = |group: &[usize], keep: &dyn Fn(&SessionFacts) -> bool| -> Vec<&str> {
        group
            .iter()
            .map(|&i| &sessions[i])
            .filter(|session| keep(session))
            .map(|session| session.id.as_str())
            .collect()
    };
    let every = |_: &SessionFacts| true;
    let rows: [(&str, Vec<&str>); 2] = [
        ("сильные признаки", ids(task.strong, &every)),
        ("с соседством", ids(task.weak, &every)),
    ];
    for (name, found) in rows {
        let score = Score::of(&found, marks);
        println!(
            "| {name} | {} | {} | {} | {} |",
            score.right, score.wrong, score.unmarked, score.missed
        );
    }
    for (name, wanted) in [
        ("оригиналы", Origin::Original),
        ("копии в Codex", Origin::ImportedCopy),
    ] {
        let of_origin: BTreeMap<String, Mark> = marks
            .iter()
            .filter(|(id, _)| sessions.iter().any(|s| &s.id == *id && s.origin == wanted))
            .map(|(id, mark)| (id.clone(), *mark))
            .collect();
        let score = Score::of(&ids(task.weak, &|s| s.origin == wanted), &of_origin);
        println!(
            "| {name}, с соседством | {} | {} | {} | {} |",
            score.right, score.wrong, score.unmarked, score.missed
        );
    }
}

/// Every link inside the candidate with what tied it, earliest first: the pairs the owner
/// checks when a conversation of another task is in the list.
fn ties_of(
    sessions: &[SessionFacts],
    group: &[usize],
    links: &[Link],
    rules: &StitchRules,
) -> String {
    let mut text = String::from(
        "\n## Связи внутри кандидата\n\n| раньше | позже | признаки | файлы-эстафеты |\n| --- | --- | --- | --- |\n",
    );
    for link in links
        .iter()
        .filter(|link| group.contains(&link.earlier) && group.contains(&link.later))
    {
        let (earlier, later) = (&sessions[link.earlier], &sessions[link.later]);
        let signals: Vec<&str> = link.signals.iter().map(|signal| label(*signal)).collect();
        let files: Vec<&str> = relay_files(earlier, later, rules)
            .into_iter()
            .map(String::as_str)
            .collect();
        text.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            name_of(earlier),
            name_of(later),
            signals.join(", "),
            files.join(", ")
        ));
    }
    text
}

fn name_of(session: &SessionFacts) -> String {
    session
        .title
        .as_deref()
        .or(session.opening.as_deref())
        .unwrap_or("—")
        .chars()
        .take(LIST_CHARACTERS)
        .map(|c| if c == '|' || c == '\n' { ' ' } else { c })
        .collect()
}

fn label(signal: LinkSignal) -> &'static str {
    match signal {
        LinkSignal::RelayFile => "файл-эстафета",
        LinkSignal::NamedConversation => "беседа названа",
        LinkSignal::SameTrackerTask => "одна задача трекера",
        LinkSignal::SameBranch => "одна ветка",
        LinkSignal::SharedWrites => "общие файлы",
        LinkSignal::Neighbours => "соседство",
    }
}

fn platform(platform: Platform) -> &'static str {
    match platform {
        Platform::ClaudeCode => "Claude Code",
        Platform::Codex => "Codex",
    }
}

fn origin(origin: Origin) -> &'static str {
    match origin {
        Origin::Original => "оригинал",
        Origin::ImportedCopy => "копия в Codex",
    }
}

fn role(role: SessionRole) -> &'static str {
    match role {
        SessionRole::Conversation => "беседа",
        SessionRole::Worker => "исполнитель",
        SessionRole::AutoReview => "ревьюер команд",
    }
}

fn reach(reach: Reach) -> &'static str {
    match reach {
        Reach::Anywhere => "во всей беседе",
        Reach::Start => "в начале беседы",
    }
}

fn strength(strength: Strength) -> &'static str {
    match strength {
        Strength::Fact => "факт",
        Strength::Strong => "сильный",
        Strength::Weak => "слабый",
    }
}
