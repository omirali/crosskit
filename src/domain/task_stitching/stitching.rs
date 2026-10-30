// The signals between two sessions and the candidate tasks they add up to (ADR 0051, decision 2).
//
// A signal is a fact both logs record, not a guess about meaning. A candidate task is a group of
// conversations connected by signals of at least a given strength; a weak signal never makes a
// candidate by itself, it only says what a person might look at.
use std::collections::{BTreeMap, BTreeSet};

use super::session::{Origin, SessionFacts, SessionRole};

/// How long after one session ended a later one in the same directory still counts as its
/// neighbour. Two hours is a break, not a change of task — and it is the weakest signal, so a
/// wrong guess here makes no candidate.
const NEIGHBOUR_GAP_MS: i64 = 2 * 60 * 60 * 1000;
/// How many written files two sessions share before that alone ties them.
const SHARED_WRITES: usize = 2;
/// A title shorter than this is too plain to be recognised in someone's text: «План», «Тесты».
const SHORTEST_NAMEABLE_TITLE: usize = 12;
/// Branches every task passes through; sharing one says nothing.
const SHARED_BRANCHES: [&str; 6] = ["main", "master", "develop", "dev", "staging", "HEAD"];
/// How many conversations may share a file or a branch before it is common ground. Chosen by the
/// measurement of ADR 0051 on one task of the owner: below four the task lost no conversation,
/// above it a project's manifests and long-lived branches began to tie tasks together.
const COMMON_GROUND_LIMIT: usize = 4;
/// A conversation that names more other conversations than this is a review of them — a list,
/// a measurement, a comparison — and naming one there continues nothing.
const MOST_NAMED_CONVERSATIONS: usize = 2;
/// The kinds of file a handoff is written in. A source file one conversation wrote and another
/// read is shared code, not a task handed on; shared code is `SharedWrites`, a weaker signal.
const HANDOFF_EXTENSIONS: [&str; 3] = ["md", "txt", "rst"];

/// How much a signal says on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Strength {
    Weak,
    Strong,
    Fact,
}

/// One reason two sessions belong together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LinkSignal {
    /// The later session refers to a file the earlier one wrote: a handoff, a plan, a report.
    RelayFile,
    /// The owner names the earlier conversation in the later one.
    NamedConversation,
    /// Both carry a link to the same tracker task.
    SameTrackerTask,
    /// Both ran on the same branch, and it is not a branch every task shares.
    SameBranch,
    /// Both wrote the same files.
    SharedWrites,
    /// The later began soon after the earlier ended, in the same directory.
    Neighbours,
}

impl LinkSignal {
    pub(crate) const ALL: [Self; 6] = [
        Self::RelayFile,
        Self::NamedConversation,
        Self::SameTrackerTask,
        Self::SameBranch,
        Self::SharedWrites,
        Self::Neighbours,
    ];

    pub(crate) fn strength(self) -> Strength {
        match self {
            Self::RelayFile | Self::NamedConversation | Self::SameTrackerTask => Strength::Fact,
            Self::SameBranch | Self::SharedWrites => Strength::Strong,
            Self::Neighbours => Strength::Weak,
        }
    }
}

/// Two sessions and every signal between them; `earlier` started first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) earlier: usize,
    pub(crate) later: usize,
    pub(crate) signals: Vec<LinkSignal>,
}

impl Link {
    pub(crate) fn strongest(&self) -> Strength {
        self.signals
            .iter()
            .map(|signal| signal.strength())
            .max()
            .unwrap_or(Strength::Weak)
    }
}

/// Where a later conversation must refer to an earlier one for the reference to tie them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Reach {
    /// Anywhere in the later conversation.
    #[default]
    Anywhere,
    /// Only as it starts: a conversation that continues another says so in its first
    /// replies, and one that names it later consults it.
    Start,
}

/// The two choices stitching makes, both varied by the measurement.
///
/// Common ground: a handoff file ties two or three conversations, while a project's README, its
/// manifest or a long-lived local branch is touched by every task that passes through the
/// project, and a signal through it ties them all. A file or a branch shared by more
/// conversations than the limit says nothing.
///
/// Reach: where a relay file or a conversation's name must appear in the later conversation.
#[derive(Debug, Clone, Default)]
pub(crate) struct StitchRules {
    files: BTreeSet<String>,
    branches: BTreeSet<String>,
    reach: Reach,
}

impl StitchRules {
    /// Nothing is common, and a reference anywhere counts.
    #[cfg(test)]
    pub(crate) fn plain() -> Self {
        Self::default()
    }

    /// The rules the product stitches by (ADR 0052): common ground beyond four conversations,
    /// and a continuation seen only as a conversation starts.
    pub(crate) fn product(sessions: &[SessionFacts]) -> Self {
        Self::sparing(sessions, COMMON_GROUND_LIMIT).reaching(Reach::Start)
    }

    /// The same rules with references counted only within `reach`.
    pub(crate) fn reaching(mut self, reach: Reach) -> Self {
        self.reach = reach;
        self
    }

    /// The files and branches of the conversations in `sessions` shared by more than
    /// `most_conversations` of them are common ground.
    pub(crate) fn sparing(sessions: &[SessionFacts], most_conversations: usize) -> Self {
        let mut files: BTreeMap<&str, usize> = BTreeMap::new();
        let mut branches: BTreeMap<&str, usize> = BTreeMap::new();
        for session in sessions
            .iter()
            .filter(|session| session.role == SessionRole::Conversation)
        {
            let touched: BTreeSet<&str> = session
                .written
                .iter()
                .chain(&session.referenced)
                .map(String::as_str)
                .collect();
            for file in touched {
                *files.entry(file).or_default() += 1;
            }
            if let Some(branch) = session.branch.as_deref() {
                *branches.entry(branch).or_default() += 1;
            }
        }
        let common = |counts: BTreeMap<&str, usize>| {
            counts
                .into_iter()
                .filter(|(_, count)| *count > most_conversations)
                .map(|(name, _)| name.to_string())
                .collect()
        };
        Self {
            files: common(files),
            branches: common(branches),
            reach: Reach::Anywhere,
        }
    }

    #[cfg(test)]
    pub(crate) fn common_files(&self) -> usize {
        self.files.len()
    }

    #[cfg(all(test, feature = "task-stitching-measurement"))]
    pub(crate) fn common_branches(&self) -> usize {
        self.branches.len()
    }
}

/// Every signal from `earlier` to `later`, in the order of `LinkSignal::ALL`.
pub(crate) fn signals_between(
    earlier: &SessionFacts,
    later: &SessionFacts,
    rules: &StitchRules,
) -> Vec<LinkSignal> {
    LinkSignal::ALL
        .into_iter()
        .filter(|signal| holds(*signal, earlier, later, rules))
        .collect()
}

/// Every linked pair among the conversations of `sessions`. Workers and reviewers are left out:
/// they are part of the conversation that started them, not tasks of their own.
pub(crate) fn links(sessions: &[SessionFacts], rules: &StitchRules) -> Vec<Link> {
    let mut order: Vec<usize> = (0..sessions.len())
        .filter(|&i| sessions[i].role == SessionRole::Conversation)
        .collect();
    order.sort_by_key(|&i| sessions[i].started_ms.unwrap_or(i64::MAX));
    let reviews: Vec<bool> = (0..sessions.len())
        .map(|i| names_many(&sessions[i], sessions, rules))
        .collect();
    let mut found = Vec::new();
    for (position, &earlier) in order.iter().enumerate() {
        for &later in &order[position + 1..] {
            let mut signals = signals_between(&sessions[earlier], &sessions[later], rules);
            if reviews[later] {
                signals.retain(|signal| *signal != LinkSignal::NamedConversation);
            }
            if !signals.is_empty() {
                found.push(Link {
                    earlier,
                    later,
                    signals,
                });
            }
        }
    }
    found
}

/// The groups of sessions connected by links of at least `weakest` strength, largest first.
/// A session no such link reaches is not a group.
pub(crate) fn candidate_tasks(count: usize, links: &[Link], weakest: Strength) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..count).collect();
    for link in links.iter().filter(|link| link.strongest() >= weakest) {
        let (a, b) = (
            root(&mut parent, link.earlier),
            root(&mut parent, link.later),
        );
        if a != b {
            parent[b] = a;
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for session in 0..count {
        let top = root(&mut parent, session);
        groups.entry(top).or_default().push(session);
    }
    let mut groups: Vec<Vec<usize>> = groups.into_values().filter(|g| g.len() > 1).collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.len()));
    groups
}

fn root(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

/// The files `earlier` wrote that `later` refers to within the rules' reach, leaving out
/// common ground: the evidence of a relay.
pub(crate) fn relay_files<'a>(
    earlier: &'a SessionFacts,
    later: &SessionFacts,
    rules: &StitchRules,
) -> Vec<&'a String> {
    let referenced = match rules.reach {
        Reach::Anywhere => &later.referenced,
        Reach::Start => &later.start_referenced,
    };
    earlier
        .written
        .iter()
        .filter(|file| referenced.contains(*file) && !rules.files.contains(*file))
        .filter(|file| is_a_document(file))
        .collect()
}

fn is_a_document(file: &str) -> bool {
    file.rsplit_once('.')
        .is_some_and(|(_, extension)| HANDOFF_EXTENSIONS.contains(&extension))
}

/// Whether `session` names more other conversations than one that continues a task would.
fn names_many(session: &SessionFacts, sessions: &[SessionFacts], rules: &StitchRules) -> bool {
    let said = match rules.reach {
        Reach::Anywhere => &session.owner_text,
        Reach::Start => &session.start_text,
    };
    sessions
        .iter()
        .filter(|other| other.id != session.id)
        .filter_map(|other| other.title.as_deref())
        .filter(|title| title.chars().count() >= SHORTEST_NAMEABLE_TITLE && said.contains(title))
        .count()
        > MOST_NAMED_CONVERSATIONS
}

fn holds(
    signal: LinkSignal,
    earlier: &SessionFacts,
    later: &SessionFacts,
    rules: &StitchRules,
) -> bool {
    let rare = |file: &&String| !rules.files.contains(*file);
    let said = match rules.reach {
        Reach::Anywhere => &later.owner_text,
        Reach::Start => &later.start_text,
    };
    match signal {
        LinkSignal::RelayFile => !relay_files(earlier, later, rules).is_empty(),
        LinkSignal::NamedConversation => earlier.title.as_deref().is_some_and(|title| {
            title.chars().count() >= SHORTEST_NAMEABLE_TITLE && said.contains(title)
        }),
        LinkSignal::SameTrackerTask => !earlier.tracker_links.is_disjoint(&later.tracker_links),
        LinkSignal::SameBranch => match (&earlier.branch, &later.branch) {
            (Some(a), Some(b)) => {
                a == b && !SHARED_BRANCHES.contains(&a.as_str()) && !rules.branches.contains(a)
            }
            _ => false,
        },
        LinkSignal::SharedWrites => {
            earlier
                .written
                .intersection(&later.written)
                .filter(|file| rare(file))
                .count()
                >= SHARED_WRITES
        }
        LinkSignal::Neighbours => {
            // A copy's time is the moment it was imported, not the moment it was held.
            let copied =
                earlier.origin == Origin::ImportedCopy || later.origin == Origin::ImportedCopy;
            if copied {
                return false;
            }
            let same_place = earlier.place.is_some() && earlier.place == later.place;
            match (earlier.ended_ms, later.started_ms) {
                (Some(ended), Some(started)) => {
                    same_place && (0..=NEIGHBOUR_GAP_MS).contains(&(started - ended))
                }
                _ => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::task_stitching::session::{Origin, Platform};

    const HOUR: i64 = 60 * 60 * 1000;

    fn conversation(id: &str, started_hour: i64, ended_hour: i64) -> SessionFacts {
        let mut session = SessionFacts::new(id, Platform::ClaudeCode);
        session.working_directory = Some("/w/payments-api".into());
        session.place = session.working_directory.clone();
        session.saw(started_hour * HOUR);
        session.saw(ended_hour * HOUR);
        session
    }

    #[test]
    fn a_later_conversation_reading_the_handoff_an_earlier_one_wrote_is_a_relay() {
        let mut first = conversation("a", 0, 5);
        first.written.insert("payments-api/handoff.md".into());
        let mut second = conversation("b", 30, 31);
        second.referenced.insert("payments-api/handoff.md".into());
        assert_eq!(
            signals_between(&first, &second, &StitchRules::plain()),
            [LinkSignal::RelayFile]
        );
    }

    #[test]
    fn a_conversation_named_by_its_title_is_linked_and_a_plain_title_is_not() {
        let mut first = conversation("a", 0, 1);
        first.title = Some("Двойная скидка в корзине".into());
        let mut second = conversation("b", 40, 41);
        second.owner_text = "мы в чате «Двойная скидка в корзине» решили иначе".into();
        assert_eq!(
            signals_between(&first, &second, &StitchRules::plain()),
            [LinkSignal::NamedConversation]
        );

        first.title = Some("План".into());
        second.owner_text = "План готов".into();
        assert!(signals_between(&first, &second, &StitchRules::plain()).is_empty());
    }

    #[test]
    fn a_branch_every_task_shares_says_nothing_and_a_task_branch_does() {
        let mut first = conversation("a", 0, 1);
        let mut second = conversation("b", 40, 41);
        first.branch = Some("main".into());
        second.branch = Some("main".into());
        assert!(signals_between(&first, &second, &StitchRules::plain()).is_empty());
        first.branch = Some("feat/discount".into());
        second.branch = Some("feat/discount".into());
        assert_eq!(
            signals_between(&first, &second, &StitchRules::plain()),
            [LinkSignal::SameBranch]
        );
    }

    #[test]
    fn neighbours_are_weak_and_make_no_candidate_alone() {
        let first = conversation("a", 0, 1);
        let second = conversation("b", 2, 3);
        let sessions = vec![first, second];
        let found = links(&sessions, &StitchRules::plain());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].strongest(), Strength::Weak);
        assert!(candidate_tasks(2, &found, Strength::Strong).is_empty());
        assert_eq!(candidate_tasks(2, &found, Strength::Weak), [vec![0, 1]]);
    }

    #[test]
    fn a_chain_of_relays_is_one_candidate_and_a_worker_is_never_linked() {
        let mut first = conversation("a", 0, 1);
        first.written.insert("payments-api/plan.md".into());
        let mut second = conversation("b", 30, 31);
        second.referenced.insert("payments-api/plan.md".into());
        second.written.insert("payments-api/handoff.md".into());
        let mut third = conversation("c", 60, 61);
        third.referenced.insert("payments-api/handoff.md".into());
        let mut worker = conversation("w", 30, 31);
        worker.role = SessionRole::Worker;
        worker.referenced.insert("payments-api/plan.md".into());

        let sessions = vec![first, second, third, worker];
        let found = links(&sessions, &StitchRules::plain());
        assert!(
            found
                .iter()
                .all(|link| link.later != 3 && link.earlier != 3)
        );
        assert_eq!(
            candidate_tasks(4, &found, Strength::Strong),
            [vec![0, 1, 2]]
        );
    }

    #[test]
    fn an_imported_copy_with_the_same_opening_is_the_same_conversation() {
        let mut original = conversation("a", 0, 1);
        original.opening = Some("Почему скидка считается дважды?".into());
        let mut copy = conversation("b", 0, 1);
        copy.origin = Origin::ImportedCopy;
        copy.opening = original.opening.clone();
        assert!(copy.is_copy_of(&original));
        assert!(!original.is_copy_of(&copy));
        copy.opening = Some("Другое начало".into());
        assert!(!copy.is_copy_of(&original));
    }

    #[test]
    fn a_file_every_conversation_touches_is_a_journal_and_ties_nothing() {
        let mut sessions: Vec<SessionFacts> = (0..4)
            .map(|i| {
                let mut session = conversation(&i.to_string(), i * 30, i * 30 + 1);
                session.written.insert("payments-api/README.md".into());
                session.referenced.insert("payments-api/README.md".into());
                session
            })
            .collect();
        sessions[0].written.insert("payments-api/handoff.md".into());
        sessions[1]
            .referenced
            .insert("payments-api/handoff.md".into());

        let rules = StitchRules::sparing(&sessions, 2);
        assert_eq!(rules.common_files(), 1);
        let found = links(&sessions, &rules);
        assert_eq!(candidate_tasks(4, &found, Strength::Strong), [vec![0, 1]]);
        assert_eq!(
            candidate_tasks(
                4,
                &links(&sessions, &StitchRules::plain()),
                Strength::Strong
            ),
            [vec![0, 1, 2, 3]]
        );
    }

    #[test]
    fn a_copy_is_nobodys_neighbour_because_its_time_is_the_import() {
        let first = conversation("a", 0, 1);
        let mut second = conversation("b", 2, 3);
        second.origin = Origin::ImportedCopy;
        assert!(signals_between(&first, &second, &StitchRules::plain()).is_empty());
    }

    #[test]
    fn a_source_file_one_wrote_and_another_read_is_shared_code_not_a_relay() {
        let mut first = conversation("a", 0, 1);
        first.written.insert("payments-api/cart.rs".into());
        let mut second = conversation("b", 30, 31);
        second.referenced.insert("payments-api/cart.rs".into());
        assert!(signals_between(&first, &second, &StitchRules::plain()).is_empty());
    }

    #[test]
    fn a_conversation_that_lists_many_others_reviews_them_and_continues_none() {
        let titles = [
            "Двойная скидка в корзине",
            "Возвраты по частям",
            "Отчёт по продажам",
        ];
        let mut sessions: Vec<SessionFacts> = titles
            .iter()
            .enumerate()
            .map(|(i, title)| {
                let mut session = conversation(&i.to_string(), i as i64 * 30, i as i64 * 30 + 1);
                session.title = Some((*title).into());
                session
            })
            .collect();
        let mut review = conversation("review", 200, 201);
        review.owner_text = titles.join(", ");
        sessions.push(review);
        assert!(links(&sessions, &StitchRules::plain()).is_empty());

        sessions[3].owner_text = format!("продолжаем «{}»", titles[0]);
        let found = links(&sessions, &StitchRules::plain());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].signals, [LinkSignal::NamedConversation]);
    }
}
