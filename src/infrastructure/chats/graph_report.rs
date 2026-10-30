// Verdict Д: what the association graph costs at each window size and each threshold.
//
// The peak memory of the process is deliberately reported once and not per row. `ru_maxrss`
// is a high-water mark and never comes down, so a number taken after the fifth combination is
// a ceiling over everything that ran before it, not the cost of the fifth. What a row can
// honestly say is how large its own tables are, and that is counted rather than measured.
use super::association_graph::AssociationGraph;
use super::report_format::{grouped, megabytes};

pub(crate) fn print_header(ceiling: usize) {
    println!("Д. СЛОВАРЬ И РЁБРА");
    println!(
        "  N — терминов в окне, K — термин учитывается, если встречается не менее чем в K\n\
         \x20 беседах. Потолок окна M = {ceiling} различных термина: без него один вставленный\n\
         \x20 лог даёт столько же рёбер, сколько весь остальной корпус."
    );
    println!();
    println!(
        "  {:>3} {:>3} {:>9} {:>10} {:>12} {:>16} {:>8} {:>10}",
        "N", "K", "словарь", "окон", "рёбер", "пар, если реплика", "время", "таблицы"
    );
}

pub(crate) fn print_row(graph: &AssociationGraph) {
    println!(
        "  {:>3} {:>3} {:>9} {:>10} {:>12} {:>16} {:>7.1}с {:>10}",
        graph.shape().length(),
        graph.least_conversations(),
        grouped(graph.vocabulary() as u64),
        grouped(graph.windows()),
        grouped(graph.edges() as u64),
        grouped(graph.pairs_if_the_reply_were_the_unit()),
        graph.built_in().as_secs_f64(),
        megabytes(graph.held_bytes())
    );
}

/// The one memory number this measurement can stand behind, and what it is not.
pub(crate) fn print_peak(peak_bytes: Option<u64>) {
    match peak_bytes {
        Some(bytes) => println!(
            "\n  пик памяти процесса за весь прогон: {} — это потолок всего прогона, а не\n\
             \x20 цена одного сочетания: ru_maxrss монотонен и не опускается.",
            megabytes(bytes)
        ),
        None => {
            println!("\n  пик памяти процесса не снят: ru_maxrss не прочитался на этой платформе.")
        }
    }
    println!();
}
