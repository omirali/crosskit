// Stage 31: the whole path from a transcript to a ranked list of conversations, run on the
// owner's real corpus, with no line written to the graph.
//
//   cargo test --release --offline --features insight-layer-measurement \
//     --lib chats::measurement -- --ignored --nocapture
//
// Ignored like every other measurement in this repository: it reads the owner's own machine
// through their own configuration, and no ordinary `cargo test` may go there. Nothing here
// opens a database, touches the search index or calls a model; `CROSSKIT_CHATS_ROOT` points
// it at another tree.
use std::time::Instant;

use super::association_graph::AssociationGraph;
use super::chat_corpus::ChatCorpus;
use super::corpus_report::{
    CONTENT_LIMIT_BYTES, print_bytes_verdict, print_corpus, print_largest_verdict,
    print_name_verdict, print_parse_verdict,
};
use super::graph_report::{print_header, print_peak, print_row};
use super::question_report::{
    print_every_question, print_graph_row, print_neighbours, print_rules_table,
};
use super::question_set::QuestionSet;
use super::term_index::TermIndex;
use crate::domain::conversation_rank::ScoringRule;
use crate::domain::term_window::WindowShape;
use crate::infrastructure::config::load_index_policy;
use crate::infrastructure::search::term_stream::TermReader;

/// Window lengths swept, in terms.
const WINDOW_LENGTHS: [usize; 3] = [20, 40, 80];
/// Thresholds swept: a term counts if it is used in at least this many conversations.
const THRESHOLDS: [u32; 3] = [1, 2, 3];
/// The first letters Stage 29's crude rule kept of each word.
const STAGE_29_LETTERS: usize = 5;
/// How many of the questions get their terms' neighbours printed. Three is enough to show
/// what the graph learned and short enough to read.
const NEIGHBOUR_QUESTIONS: usize = 3;
/// Distinct terms one window may contribute. Fixed across the sweep, so the sweep says what
/// the window length does and not what the ceiling does.
const WINDOW_CEILING: usize = 20;

#[test]
#[ignore = "измерение смыслового слоя: cargo test --release --offline --features insight-layer-measurement --lib chats::measurement -- --ignored --nocapture"]
fn the_insight_layer_on_the_owners_corpus() {
    let set = match QuestionSet::read() {
        Ok(set) => set,
        Err(message) => {
            println!("{message}");
            return;
        }
    };
    if set.questions().len() != 15
        || set.questions().iter().any(|question| {
            question.answers().is_empty()
                || question
                    .answers()
                    .iter()
                    .any(|answer| !answer.starts_with('/'))
        })
    {
        println!(
            "ВНИМАНИЕ: в наборе не пятнадцать вопросов или у вопроса нет ответа абсолютным \
             путём — числа ниже сняты не по тому набору, по которому их сравнивают.\n"
        );
    }
    if !set.confirmed() {
        println!(
            "ВНИМАНИЕ: набор вопросов не подтверждён владельцем (`confirmed: no`). Каждое \
             число ниже снято по НЕПОДТВЕРЖДЁННЫМ ответам и окончательным называться не \
             может.\n"
        );
    }

    let started = Instant::now();
    let corpus = match ChatCorpus::read() {
        Ok(corpus) => corpus,
        Err(error) => {
            println!("КОРПУС НЕ ПРОЧИТАН: {error}");
            println!("Измерение идёт к настоящей конфигурации этой машины. Ничего не измерено.");
            return;
        }
    };
    println!("прочитано за {:.1} с\n", started.elapsed().as_secs_f64());

    print_corpus(&corpus);
    print_parse_verdict(&corpus);
    print_bytes_verdict(&corpus);
    print_name_verdict(&corpus);
    print_largest_verdict(&corpus, content_limit_bytes());

    let started = Instant::now();
    let mut index = TermIndex::of(&corpus, TermReader::with_built_in_packs());
    let mut crude = TermIndex::of(
        &corpus,
        TermReader::with_the_first_characters(STAGE_29_LETTERS),
    );
    println!(
        "ТЕРМИНЫ: словарь {} терминов, {} вхождений, за {:.1} с (пакеты ru/en, одно чтение \
         на слово)\n",
        index.vocabulary().len(),
        index.term_occurrences(),
        started.elapsed().as_secs_f64()
    );
    println!(
        "ТЕРМИНЫ ЭТАПА 29 (первые {STAGE_29_LETTERS} букв, без морфологии): словарь {} \
         терминов — правило,\n  которое надо обыграть, прочитанное тем же расщеплением слов.\n",
        crude.vocabulary().len()
    );

    print_rules_table(&corpus, &mut index, &mut crude, &set, &THRESHOLDS);

    let shapes: Vec<WindowShape> = WINDOW_LENGTHS
        .iter()
        .map(|length| {
            WindowShape::new(*length, WINDOW_CEILING)
                .expect("окно и потолок этого измерения допускают пары")
        })
        .collect();
    print_header(shapes[0].ceiling());
    let mut graph_rows = Vec::new();
    for shape in shapes {
        for least in THRESHOLDS {
            let graph = AssociationGraph::build(&index, shape, least);
            print_row(&graph);
            graph_rows.push(graph);
        }
    }
    print_peak(peak_resident_bytes());

    println!("Е. ГЛАВНОЕ ЧИСЛО — ранжирование, в которое вошёл граф со-встречаемости");
    println!(
        "  вопрос расширяется {NEIGHBOURS} сильнейшими соседями каждого термина, сосед весит \
         долю от\n  силы связи; сам термин весит 1.",
        NEIGHBOURS = super::question_report::NEIGHBOURS
    );
    for graph in &graph_rows {
        print_graph_row(&corpus, &mut index, &set, graph);
    }
    println!();
    if let Some(best) = graph_rows.iter().find(|graph| {
        graph.shape().length() == WINDOW_LENGTHS[0] && graph.least_conversations() == THRESHOLDS[2]
    }) {
        print_neighbours(&mut index, &set, best, NEIGHBOUR_QUESTIONS);
    }

    // Three detailed runs, and the first is Stage 29's own rule: the gap between its
    // published 0.67 and what it scores here has to be explainable question by question, or
    // the reproduction is not one.
    print_every_question(
        &corpus,
        &mut crude,
        &set,
        ScoringRule::the_name_alone(),
        1,
        "правило этапа 29: имя по первым 5 буквам, без морфологии",
    );
    print_every_question(
        &corpus,
        &mut index,
        &set,
        ScoringRule::the_name_alone(),
        1,
        "одно имя через морфологию индекса",
    );
    print_every_question(
        &corpus,
        &mut index,
        &set,
        ScoringRule::three_two_one_at_most_once(),
        1,
        "имя×3 + владелец×2 + модель×1, счёт «сказано или нет»",
    );
}

/// The content limit the owner's configuration is actually running with, so verdict Г
/// compares against the limit in force rather than against a number written in this file.
fn content_limit_bytes() -> u64 {
    load_index_policy()
        .map(|loaded| loaded.policy().max_content_bytes())
        .unwrap_or(CONTENT_LIMIT_BYTES)
}

/// Peak resident set size of this process, or nothing when the platform does not report one.
///
/// `ru_maxrss` is in bytes on macOS and in kilobytes on Linux, the same rule the memory
/// benchmarks of this repository already write down in one place. It is a high-water mark for
/// the whole process, which is why the report says so where it prints it.
fn peak_resident_bytes() -> Option<u64> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // Safe: `getrusage` fills the struct it is handed and reads nothing else.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    let peak = u64::try_from(usage.ru_maxrss).ok()?;
    Some(if cfg!(target_os = "macos") {
        peak
    } else {
        peak * 1024
    })
}
