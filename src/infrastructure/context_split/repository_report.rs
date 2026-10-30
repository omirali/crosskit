// Section Б of the report: what each repository's files said and what the rule made of it.
use std::collections::{BTreeMap, BTreeSet};

use super::home_layout::HomeLayout;
use super::placement::{PlacedRepository, Repositories};
use super::report::{describe, percent, print_rule, print_tally, shown};
use super::repository_census::FoundRepository;
use crate::domain::context_split::{Identity, MailDomain, Origin, OwnerDomain, SplitDecision};
use crate::domain::{GitMarker, NormalizedPath};

pub(super) fn print_repositories(
    repositories: &Repositories,
    global: &[Identity],
    layout: &HomeLayout,
) {
    let home = layout.home();
    println!("\n## Б. Признаки репозиториев\n");
    match repositories.owner() {
        OwnerDomain::One(domain) => println!("Домен почты владельца: {}", domain.as_str()),
        OwnerDomain::Unknown => println!("Домен почты владельца: ни одного"),
        OwnerDomain::Several(domains) => println!(
            "Доменов почты владельца несколько: {}",
            domains
                .iter()
                .map(MailDomain::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
    println!(
        "  глобальный [user]: {}",
        match global.iter().filter_map(Identity::mail_domain).next() {
            Some(domain) => domain.as_str().to_string(),
            None => "нет".to_string(),
        }
    );
    print_identity_sources(repositories);

    println!("\n| репозиторий | хост origin | группа | [user] | reflog | свой ответ | ответ |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    for placed in repositories.all() {
        print_repository_row(placed, home);
    }

    print_rule();
    print_tally(
        "\nВнешние репозитории по ответу",
        repositories.outermost().map(|placed| placed.effective),
    );
    print_nested(repositories, home);
    print_what_the_reflog_alone_would_say(repositories, home);
    print_what_the_host_alone_groups(repositories);
}

fn print_identity_sources(repositories: &Repositories) {
    let own = repositories
        .all()
        .iter()
        .filter(|placed| placed.found.own_identity.is_some())
        .count();
    let mut with_reflog = 0;
    let mut only_the_owner = 0;
    let mut another_domain = 0;
    let mut only_machine = 0;
    let owner = match repositories.owner() {
        OwnerDomain::One(domain) => Some(domain),
        OwnerDomain::Unknown | OwnerDomain::Several(_) => None,
    };
    for placed in repositories.all() {
        let Some(reflog) = &placed.found.reflog else {
            continue;
        };
        with_reflog += 1;
        let domains: BTreeSet<&MailDomain> = reflog.domains().map(|(domain, _)| domain).collect();
        match (domains.is_empty(), owner) {
            (true, _) => only_machine += 1,
            (false, Some(owner)) if domains.iter().all(|domain| *domain == owner) => {
                only_the_owner += 1
            }
            (false, _) => another_domain += 1,
        }
    }
    let all = repositories.all().len();
    println!("  [user] в .git/config репозитория: {own} из {all}");
    println!("  reflog есть: {with_reflog} из {all}");
    println!("    в нём только домен владельца (и адреса машины): {only_the_owner}");
    println!("    в нём другой домен: {another_domain}");
    println!("    в нём только адреса машины: {only_machine}");
    println!(
        "  reflog нет (ничего не коммитили и не переключали): {}",
        all - with_reflog
    );
}

fn print_repository_row(placed: &PlacedRepository, home: &NormalizedPath) {
    let found: &FoundRepository = &placed.found;
    let (host, group) = match &found.origin {
        None if found.marker == GitMarker::NotADirectory => (
            "`.git` — файл-указатель, не читается".to_string(),
            "—".to_string(),
        ),
        None => ("config не прочитан".to_string(), "—".to_string()),
        Some(Origin::Owned(owner)) => {
            (owner.host().as_str().to_string(), owner.group().to_string())
        }
        Some(Origin::Ungrouped(host)) => (host.as_str().to_string(), "—".to_string()),
        Some(Origin::NotAHost) => ("путь на машине".to_string(), "—".to_string()),
        Some(Origin::Absent) => ("нет origin".to_string(), "—".to_string()),
    };
    let user = match &found.own_identity {
        Some(Identity::Mail(domain)) => domain.as_str().to_string(),
        Some(Identity::MachineDefault) => "адрес машины".to_string(),
        None => "—".to_string(),
    };
    let reflog = match &found.reflog {
        None => "нет".to_string(),
        Some(reflog) => {
            let mut parts: Vec<String> = reflog
                .domains()
                .map(|(domain, lines)| format!("{}×{lines}", domain.as_str()))
                .collect();
            if reflog.machine_defaults() > 0 {
                parts.push(format!("машина×{}", reflog.machine_defaults()));
            }
            if reflog.lines_without_identity() > 0 {
                parts.push(format!("без адреса×{}", reflog.lines_without_identity()));
            }
            parts.join(", ")
        }
    };
    println!(
        "| `{}`{} | {host} | {group} | {user} | {reflog} | {} | {} |",
        shown(&found.root, home),
        if placed.enclosing.is_some() {
            " (вложен)"
        } else {
            ""
        },
        describe(&placed.own),
        describe(&placed.effective),
    );
}

/// A repository inside another cannot get an answer of its own; where its own would have been
/// different, the owner loses something the rule cannot express, and that is counted.
fn print_nested(repositories: &Repositories, home: &NormalizedPath) {
    let nested: Vec<&PlacedRepository> = repositories
        .all()
        .iter()
        .filter(|placed| placed.enclosing.is_some())
        .collect();
    let differing: Vec<&&PlacedRepository> = nested
        .iter()
        .filter(|placed| placed.own != own_of_the_enclosing(repositories, placed))
        .collect();
    println!(
        "\nВложенных: {}; из них свой ответ расходится с ответом внешнего: {}",
        nested.len(),
        differing.len()
    );
    for placed in differing {
        println!(
            "  `{}`: свой — {}, внешний — {}",
            shown(&placed.found.root, home),
            describe(&placed.own),
            describe(&own_of_the_enclosing(repositories, placed)),
        );
    }
}

fn own_of_the_enclosing(repositories: &Repositories, placed: &PlacedRepository) -> SplitDecision {
    placed
        .enclosing
        .as_ref()
        .and_then(|root| {
            repositories
                .all()
                .iter()
                .find(|other| other.found.root == *root)
        })
        .map(|outer| outer.own)
        .unwrap_or(placed.own)
}

/// The rule the stage was handed, measured against the one it uses: if the owner's domain in a
/// reflog decided a repository, which repositories would it have decided that the rule asks
/// about.
fn print_what_the_reflog_alone_would_say(repositories: &Repositories, home: &NormalizedPath) {
    let OwnerDomain::One(owner) = repositories.owner() else {
        return;
    };
    let outermost: Vec<&PlacedRepository> = repositories.outermost().collect();
    let named: Vec<&&PlacedRepository> = outermost
        .iter()
        .filter(|placed| {
            placed
                .found
                .reflog
                .as_ref()
                .is_some_and(|reflog| reflog.domains().any(|(domain, _)| domain == owner))
        })
        .collect();
    let would_be_work_by_mail_alone: Vec<&&&PlacedRepository> = named
        .iter()
        .filter(|placed| placed.own.side().is_none())
        .collect();
    println!(
        "\nЕсли бы решала почта reflog: домен владельца назван в reflog {} внешних из {}; \
         из них правило спрашивает про {} — их почта отправила бы в work без другого признака:",
        named.len(),
        outermost.len(),
        would_be_work_by_mail_alone.len(),
    );
    for placed in would_be_work_by_mail_alone {
        println!(
            "  `{}` — {}",
            shown(&placed.found.root, home),
            describe(&placed.own)
        );
    }
    let reflogs: Vec<_> = outermost
        .iter()
        .filter_map(|placed| placed.found.reflog.as_ref())
        .collect();
    let another = reflogs
        .iter()
        .filter(|reflog| reflog.domains().any(|(domain, _)| domain != owner))
        .count();
    println!(
        "  внешних с reflog: {}; домен владельца назван в {} ({:.0} %), другой домен — в {another}",
        reflogs.len(),
        named.len(),
        percent(named.len(), reflogs.len())
    );
}

/// What `contexts propose` groups by today, beside what the groups under each host are.
fn print_what_the_host_alone_groups(repositories: &Repositories) {
    let mut by_host: BTreeMap<String, BTreeMap<String, Vec<SplitDecision>>> = BTreeMap::new();
    for placed in repositories.outermost() {
        let (host, group) = match &placed.found.origin {
            Some(Origin::Owned(owner)) => {
                (owner.host().as_str().to_string(), owner.group().to_string())
            }
            Some(Origin::Ungrouped(host)) => (host.as_str().to_string(), "—".to_string()),
            Some(Origin::NotAHost) | Some(Origin::Absent) | None => {
                ("без хоста".to_string(), "—".to_string())
            }
        };
        by_host
            .entry(host)
            .or_default()
            .entry(group)
            .or_default()
            .push(placed.own);
    }
    println!(
        "\nЕсли бы решал хост (так группирует `contexts propose`): {} групп",
        by_host.len()
    );
    for (host, groups) in by_host {
        println!("  {host}: групп под ним {}", groups.len());
        for (group, decisions) in groups {
            let decided = decisions
                .iter()
                .filter(|decision| decision.side().is_some())
                .count();
            println!(
                "    {group}: репозиториев {}, разложено правилом {decided}",
                decisions.len()
            );
        }
    }
}
