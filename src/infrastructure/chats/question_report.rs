// Verdicts Е–З: the fifteen questions, ranked.
//
// Two tables and one detail. The first table is the weighting alone, with no graph in it at
// all, which is what answers "how much is the name worth" without anything else moving. The
// second adds the association graph as query expansion — the only place the graph touches the
// ranking — so the difference between the two tables is the whole of what the insight layer
// bought.
//
// Both are printed against the two numbers Stage 29 left: 0.60 for the search index as it
// stands, and 0.67 for a crude match against conversation names alone.
use std::collections::HashMap;

use super::association_graph::AssociationGraph;
use super::chat_corpus::ChatCorpus;
use super::question_set::QuestionSet;
use super::ranking::{Column, PAGE, Ranking};
use super::term_index::TermIndex;
use crate::domain::conversation_rank::ScoringRule;
use crate::domain::term_window::TermId;

/// What Stage 29 measured with the whole body index, through the shipped `crosskit search`.
pub(crate) const SEARCH_TODAY: f64 = 0.60;
/// What Stage 29 measured matching conversation names alone by their first five letters, and
/// how many of the fifteen had their answer in the page. Both are published numbers taken
/// without any accounting of ties, which is exactly why this stage reproduces the rule rather
/// than comparing against them.
pub(crate) const NAMES_ALONE: f64 = 0.67;
pub(crate) const NAMES_ALONE_IN_PAGE: usize = 13;

/// How many neighbours one question term is expanded by, and how much a neighbour is worth
/// against the term itself. Both are fixed: the parameters this stage sweeps are the window
/// and the threshold, and letting the expansion move too would leave no way to say which of
/// them did anything.
pub(crate) const NEIGHBOURS: usize = 5;
pub(crate) const NEIGHBOUR_SHARE: f64 = 0.5;

/// The six weightings the stage compares.
pub(crate) fn rules() -> [(&'static str, ScoringRule); 6] {
    [
        ("гипотеза 3/2/1", ScoringRule::three_two_one()),
        ("без веса имени", ScoringRule::without_the_name()),
        ("одно имя", ScoringRule::the_name_alone()),
        (
            "3/2/1, log2(1+реплики)",
            ScoringRule::three_two_one_saturated(),
        ),
        (
            "3/2/1, сказано или нет",
            ScoringRule::three_two_one_at_most_once(),
        ),
        (
            "без имени, сказано или нет",
            ScoringRule::without_the_name_at_most_once(),
        ),
    ]
}

/// Every question of the set as terms this index knows, at one threshold.
fn every_question_as_terms(
    index: &mut TermIndex,
    set: &QuestionSet,
    least: u32,
) -> Vec<Vec<TermId>> {
    set.questions()
        .iter()
        .map(|question| index.terms_of(question.text(), least))
        .collect()
}

/// The question's own terms, each at full weight.
fn asked_plainly(terms: &[TermId]) -> Vec<(TermId, f64)> {
    terms.iter().map(|term| (*term, 1.0)).collect()
}

/// The question's own terms plus the strongest neighbours of each, at a share of full weight
/// proportional to how strong the tie is against that term's strongest tie.
fn asked_with_the_graph(
    terms: &[TermId],
    neighbours: &HashMap<TermId, Vec<(TermId, f64)>>,
) -> Vec<(TermId, f64)> {
    let mut asked = asked_plainly(terms);
    for term in terms {
        let Some(found) = neighbours.get(term) else {
            continue;
        };
        let Some(strongest) = found.first().map(|(_, weight)| *weight) else {
            continue;
        };
        if strongest <= 0.0 {
            continue;
        }
        for (neighbour, weight) in found {
            if terms.contains(neighbour) {
                continue;
            }
            asked.push((*neighbour, NEIGHBOUR_SHARE * weight / strongest));
        }
    }
    asked
}

/// One column of the tables: the whole question set under one weighting and one asking.
fn column(
    corpus: &ChatCorpus,
    index: &TermIndex,
    set: &QuestionSet,
    rule: ScoringRule,
    asked: &[Vec<(TermId, f64)>],
) -> Column {
    let ranking = Ranking::over(corpus, index);
    let mut column = Column::default();
    for (question, asked) in set.questions().iter().zip(asked) {
        column.add(&ranking.ranked(asked, rule), question.answers());
    }
    column
}

/// Verdicts Е and Ж: the weighting alone, at each threshold, with nothing from the graph.
///
/// The first row is Stage 29's own rule, reproduced over the same corpus and run through this
/// column. It is there because 0.87 against 0.67 was two rulers in one comparison: Stage 29
/// never counted what its ties were doing, and its crude rule — which joins every word sharing
/// five letters — has more of them, not fewer. Whatever the name field is worth, it has to be
/// read off two numbers taken the same way.
pub(crate) fn print_rules_table(
    corpus: &ChatCorpus,
    index: &mut TermIndex,
    crude: &mut TermIndex,
    set: &QuestionSet,
    thresholds: &[u32],
) {
    println!("Е/Ж. РАНЖИРОВАНИЕ БЕЗ ГРАФА — что даёт одна расстановка весов");
    println!(
        "  {:<30} {:>3} {:>7} {:>9} {:>7} {:>12} {:>12}",
        "правило", "K", "p@1", "p@1 без ничьи", "p@5", "в пятёрке", "ничья на 1-м"
    );

    // Stage 29's rule has no threshold of its own: it matched one line per file and nothing
    // was ever dropped for being rare, so it is run once and marked with a dash.
    let asked = every_question_as_terms(crude, set, 1);
    let asked: Vec<Vec<(TermId, f64)>> = asked.iter().map(|terms| asked_plainly(terms)).collect();
    let stage_29 = column(corpus, crude, set, ScoringRule::the_name_alone(), &asked);
    print_column("имя, первые 5 букв (этап 29)", "—", &stage_29);

    for least in thresholds {
        let asked = every_question_as_terms(index, set, *least);
        let asked: Vec<Vec<(TermId, f64)>> =
            asked.iter().map(|terms| asked_plainly(terms)).collect();
        for (name, rule) in rules() {
            let column = column(corpus, index, set, rule, &asked);
            print_column(name, &least.to_string(), &column);
        }
    }
    println!(
        "\n  первая строка — правило этапа 29, воспроизведённое здесь и прогнанное через тот же\n\
         \x20 столбец. Этап 29 опубликовал по нему p@1 {NAMES_ALONE:.2} и {NAMES_ALONE_IN_PAGE}/15\n\
         \x20 в пятёрке, не считая ничьих вовсе; весь индекс тела через `crosskit search` дал\n\
         \x20 p@1 {SEARCH_TODAY:.2}.\n"
    );
}

/// Verdict Е: the same questions with the association graph expanding them.
pub(crate) fn print_graph_row(
    corpus: &ChatCorpus,
    index: &mut TermIndex,
    set: &QuestionSet,
    graph: &AssociationGraph,
) {
    let per_question = every_question_as_terms(index, set, graph.least_conversations());
    let every_term: Vec<TermId> = {
        let mut every: Vec<TermId> = Vec::new();
        for terms in &per_question {
            for term in terms {
                if !every.contains(term) {
                    every.push(*term);
                }
            }
        }
        every
    };
    let neighbours = graph.strongest_neighbours_of(&every_term, NEIGHBOURS);
    let asked: Vec<Vec<(TermId, f64)>> = per_question
        .iter()
        .map(|terms| asked_with_the_graph(terms, &neighbours))
        .collect();

    for (name, rule) in [
        ("одно имя + граф", ScoringRule::the_name_alone()),
        (
            "3/2/1 сказано-или-нет + граф",
            ScoringRule::three_two_one_at_most_once(),
        ),
    ] {
        let column = column(corpus, index, set, rule, &asked);
        println!(
            "  {:<28} N={:<3} K={:<3} p@1 {:>5.2}  без ничьи {:>5.2}  p@5 {:>5.2}  \
             в пятёрке {:>2}/{}  ничья на 1-м {:>2}",
            name,
            graph.shape().length(),
            graph.least_conversations(),
            column.precision_at_one(),
            column.precision_at_one_outright(),
            column.precision_at_five(),
            column.in_page(),
            column.questions(),
            column.tied_at_the_top()
        );
    }
}

/// What the graph actually learned about the words of the questions, for the one setting the
/// verdict rests on. Printed rather than described, because "the graph pulled in words from
/// the same conversation" is a claim about the data and has to be read off it.
pub(crate) fn print_neighbours(
    index: &mut TermIndex,
    set: &QuestionSet,
    graph: &AssociationGraph,
    questions: usize,
) {
    println!(
        "  чему граф научился (N={}, K={}) — сильнейшие соседи терминов первых {questions} \
         вопросов:",
        graph.shape().length(),
        graph.least_conversations()
    );
    let asked = every_question_as_terms(index, set, graph.least_conversations());
    for terms in asked.iter().take(questions) {
        let neighbours = graph.strongest_neighbours_of(terms, NEIGHBOURS);
        for term in terms {
            let found = neighbours
                .get(term)
                .map(|found| {
                    found
                        .iter()
                        .map(|(neighbour, weight)| {
                            format!("{} ({weight:.4})", index.vocabulary().key(*neighbour))
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            println!("    {:<14} → {}", index.vocabulary().key(*term), found);
        }
    }
    println!();
}

fn print_column(name: &str, least: &str, column: &Column) {
    println!(
        "  {:<30} {:>3} {:>7.2} {:>9.2} {:>7.2} {:>9}/{} {:>11}/{}",
        name,
        least,
        column.precision_at_one(),
        column.precision_at_one_outright(),
        column.precision_at_five(),
        column.in_page(),
        column.questions(),
        column.tied_at_the_top(),
        column.questions()
    );
}

/// The fifteen questions one by one under the setting that did best, printed the way Stage 29
/// printed them so the two can be read side by side.
pub(crate) fn print_every_question(
    corpus: &ChatCorpus,
    index: &mut TermIndex,
    set: &QuestionSet,
    rule: ScoringRule,
    least: u32,
    label: &str,
) {
    println!("ПО ВОПРОСАМ — {label}");
    let asked = every_question_as_terms(index, set, least);
    let ranking = Ranking::over(corpus, index);
    let mut column = Column::default();
    for (ordinal, (question, terms)) in set.questions().iter().zip(&asked).enumerate() {
        let ranked = ranking.ranked(&asked_plainly(terms), rule);
        column.add(&ranked, question.answers());
        let page = &ranked[..ranked.len().min(PAGE)];

        println!("{}. {}", ordinal + 1, question.text());
        println!(
            "   термины: {}",
            terms
                .iter()
                .map(|term| index.vocabulary().key(*term))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !question.why().is_empty() {
            println!("   правильный ответ: {}", question.why());
        }
        for (place, found) in page.iter().enumerate() {
            let hit = if question
                .answers()
                .iter()
                .any(|answer| answer == found.path())
            {
                "→"
            } else {
                " "
            };
            println!(
                "   {hit} {}. {:<52} {:>10.1}",
                place + 1,
                shorten(found.name()),
                found.score()
            );
        }
        if page.is_empty() {
            println!("     — ни одна беседа не набрала ничего");
        }
        println!(
            "   место ответа: {}\n",
            column
                .place(ordinal)
                .map_or_else(|| "—".to_string(), |place| place.to_string())
        );
    }
    println!(
        "  итого: p@1 {:.2} (без ничьи {:.2}), p@5 {:.2}, ответ в пятёрке {}/{}, ничья на \
         первом месте у {} вопросов\n  (страница — {PAGE} результатов)\n",
        column.precision_at_one(),
        column.precision_at_one_outright(),
        column.precision_at_five(),
        column.in_page(),
        column.questions(),
        column.tied_at_the_top()
    );
}

/// A conversation name, cut to what a column holds. Cut on a character boundary, because a
/// name is Russian more often than not.
fn shorten(name: &str) -> String {
    const MOST: usize = 50;
    if name.chars().count() <= MOST {
        return name.to_string();
    }
    let kept: String = name.chars().take(MOST - 1).collect();
    format!("{kept}…")
}
