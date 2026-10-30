// The stage «Сшивка бесед»: whether the owner's Claude Code and Codex sessions stitch into tasks
// by facts their logs record (ADR 0051).
//
//   CROSSKIT_STITCH_HOME=$HOME [CROSSKIT_STITCH_SEED=<session id>] \
//   [CROSSKIT_STITCH_LIST=<a file outside the repository>] \
//     cargo test --release --offline --features task-stitching-measurement \
//     --lib task_stitching:: -- --ignored --nocapture
//
// Ignored, because it reads a real home directory. It opens no database, runs no `git`, calls no
// model and writes nothing into the repository.
use std::time::Instant;

use super::claude_sessions::claude_sessions;
use super::codex_sessions::codex_sessions;
use super::labels::marks_in;
use super::layout::Layout;
use super::report::{
    CurvePoint, Task, print_candidates, print_curve, print_inventory, print_score, print_seed,
    print_signals, write_list,
};
use crate::domain::task_stitching::session::{SessionFacts, SessionRole};
use crate::domain::task_stitching::stitching::{
    Reach, StitchRules, Strength, candidate_tasks, links,
};

/// How many conversations may share a file or a branch before it counts as common ground, in
/// the order the curve is printed: `None` is no limit, the first pass's rule.
const COMMON_GROUND_LIMITS: [Option<usize>; 6] =
    [None, Some(10), Some(6), Some(4), Some(3), Some(2)];

#[test]
#[ignore = "замер сшивки: CROSSKIT_STITCH_HOME=$HOME cargo test --release --offline --features task-stitching-measurement --lib task_stitching:: -- --ignored --nocapture"]
fn whether_conversations_stitch_into_tasks_by_their_logs() {
    let started = Instant::now();
    let layout = match Layout::from_environment() {
        Ok(layout) => layout,
        Err(message) => {
            println!("{message}");
            return;
        }
    };

    let (claude, claude_unreadable) = claude_sessions(&layout.claude_projects(), &layout.home);
    let (codex, walk) = codex_sessions(&layout.codex_session_roots(), &layout.home);
    let (sessions, dropped_copies) =
        without_known_copies(claude.into_iter().chain(codex).collect());

    print_inventory(
        &sessions,
        dropped_copies,
        &[
            ("нечитаемых журналов Claude Code", claude_unreadable),
            ("нечитаемых файлов Codex", walk.unreadable),
            ("сжатых файлов Codex (не открывались)", walk.compressed),
        ],
    );
    let conversations = sessions
        .iter()
        .filter(|session| session.role == SessionRole::Conversation)
        .count();
    let seed = layout
        .seed
        .as_ref()
        .and_then(|seed| sessions.iter().position(|session| &session.id == seed));
    if layout.seed.is_some() && seed.is_none() {
        println!("\nСессии-затравки среди прочитанных нет.");
    }

    let curve: Vec<CurvePoint> = [Reach::Anywhere, Reach::Start]
        .into_iter()
        .flat_map(|reach| {
            COMMON_GROUND_LIMITS
                .into_iter()
                .map(move |limit| (reach, limit))
        })
        .map(|(reach, limit)| {
            let rules = limit
                .map_or_else(StitchRules::plain, |most| {
                    StitchRules::sparing(&sessions, most)
                })
                .reaching(reach);
            let candidates =
                candidate_tasks(sessions.len(), &links(&sessions, &rules), Strength::Strong);
            CurvePoint {
                limit,
                reach,
                common_files: rules.common_files(),
                common_branches: rules.common_branches(),
                seed_task: seed.map(|index| group_of(&candidates, index).len()),
                candidates,
            }
        })
        .collect();
    print_curve(&curve, conversations);

    let rules = StitchRules::product(&sessions);
    let found = links(&sessions, &rules);
    print_signals(&found);
    let strong = candidate_tasks(sessions.len(), &found, Strength::Strong);
    let weak = candidate_tasks(sessions.len(), &found, Strength::Weak);
    print_candidates(
        "Г. Кандидаты по фактам и сильным признакам",
        &strong,
        conversations,
    );
    print_candidates(
        "Д. Кандидаты, если добавить соседство",
        &weak,
        conversations,
    );

    if let Some(index) = seed {
        let strong_group = group_of(&strong, index);
        let weak_group = group_of(&weak, index);
        print_seed(
            "Е. Задача беседы-затравки: факты и сильные признаки",
            &sessions,
            &strong_group,
            &found,
        );
        print_seed(
            "Ж. Та же задача, если добавить соседство",
            &sessions,
            &weak_group,
            &found,
        );
        let task = Task {
            strong: &strong_group,
            weak: &weak_group,
            links: &found,
            rules: &rules,
        };
        let marks = layout.list.as_deref().map(marks_in).unwrap_or_default();
        print_score(&sessions, &task, &marks);
        if let Some(list) = &layout.list {
            let project = project_of(&sessions, &weak_group);
            println!("\nБесед в каталогах задачи: {}.", project.len());
            match write_list(list, &sessions, &project, &task, &marks) {
                Ok(()) => println!("Список для разметки записан вне репозитория."),
                Err(error) => println!("Список не записан: {error}"),
            }
        }
    }
    println!("\nЗамер занял {:.1} с.", started.elapsed().as_secs_f64());
}

/// The sessions without the imported copies whose original is also present, and how many were
/// dropped. A conversation with both is one conversation (ADR 0051, decision 1).
fn without_known_copies(sessions: Vec<SessionFacts>) -> (Vec<SessionFacts>, usize) {
    let copies: Vec<bool> = sessions
        .iter()
        .map(|copy| sessions.iter().any(|original| copy.is_copy_of(original)))
        .collect();
    let dropped = copies.iter().filter(|&&copy| copy).count();
    let kept = sessions
        .into_iter()
        .zip(copies)
        .filter_map(|(session, copy)| (!copy).then_some(session))
        .collect();
    (kept, dropped)
}

/// Every conversation held in a working directory of `group`: the project the owner marks, so
/// that what stitching missed is on the list too.
fn project_of(sessions: &[SessionFacts], group: &[usize]) -> Vec<usize> {
    let directories: Vec<&str> = group
        .iter()
        .filter_map(|&i| sessions[i].working_directory.as_deref())
        .collect();
    (0..sessions.len())
        .filter(|&i| sessions[i].role == SessionRole::Conversation)
        .filter(|&i| {
            sessions[i]
                .working_directory
                .as_deref()
                .is_some_and(|directory| directories.contains(&directory))
        })
        .collect()
}

/// The candidate holding `index`, or the session alone when no candidate does.
fn group_of(candidates: &[Vec<usize>], index: usize) -> Vec<usize> {
    candidates
        .iter()
        .find(|group| group.contains(&index))
        .cloned()
        .unwrap_or_else(|| vec![index])
}
