// Verdicts А–Г: what the corpus turned out to be once it was parsed.
//
// Everything printed here is counted by the same pass that builds the conversations the rest
// of the measurement ranks, so no number in this section can be about a different corpus than
// the numbers below it.
use super::chat_corpus::ChatCorpus;
use super::report_format::{grouped, kilobytes, mebibytes, median, megabytes, share};
use crate::domain::conversation::{NamedBy, SkippedLine, Speaker, UnreadableLine};

/// The content limit a default configuration applies, which is the one the owner's
/// configuration leaves in place. Written here as the number the report compares against, and
/// checked against the configuration by the measurement that prints it.
pub(crate) const CONTENT_LIMIT_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn print_corpus(corpus: &ChatCorpus) {
    let (excluded_directories, excluded_files, excluded_bytes) = corpus.excluded();
    let (other_files, other_bytes) = corpus.other_admitted();
    println!("КОРПУС");
    println!("  корень                       {}", corpus.root());
    println!(
        "  транскриптов .jsonl          {:>9}   {}",
        grouped(corpus.transcripts().len() as u64),
        megabytes(corpus.file_bytes())
    );
    println!(
        "  строк в них                  {:>9}",
        grouped(corpus.lines())
    );
    println!(
        "  отсечено политикой           {:>9} каталогов целиком — внутрь не спускались,\n\
         \x20                              {:>9} файлов поштучно ({})",
        grouped(excluded_directories),
        grouped(excluded_files),
        megabytes(excluded_bytes)
    );
    println!(
        "  прочие файлы, что политика пускает (.md, .txt — вне этого этапа)\n\
         \x20                              {:>9}   {}",
        grouped(other_files),
        megabytes(other_bytes)
    );
    for (path, reason) in corpus.unreadable_files() {
        println!("  НЕ ОТКРЫЛСЯ ФАЙЛ             {path}: {reason}");
    }
    println!();
}

pub(crate) fn print_parse_verdict(corpus: &ChatCorpus) {
    let tally = corpus.tally();
    println!("А. ЧТО РАЗОБРАЛОСЬ");
    println!(
        "  бесед                        {:>9}",
        grouped(corpus.transcripts().len() as u64)
    );
    println!(
        "  реплик                       {:>9}",
        grouped(tally.replies())
    );
    println!(
        "  строк прочитано              {:>9}",
        grouped(corpus.lines())
    );
    println!(
        "  наблюдений по строкам        {:>9}   (одна строка сообщения даёт по наблюдению \
         на блок)",
        grouped(tally.observations())
    );
    println!();
    println!("  Строки, которые НЕ разобрались — по категориям:");
    for reason in UnreadableLine::ALL {
        println!(
            "    {:<34} {:>9}",
            reason.to_string(),
            grouped(tally.unreadable(reason))
        );
    }
    println!(
        "    {:<34} {:>9}   ({} строк корпуса)",
        "всего",
        grouped(tally.unreadable_total()),
        share(tally.unreadable_total(), corpus.lines())
    );
    println!();
    println!("  Отброшено намеренно (наблюдений, не строк):");
    for reason in SkippedLine::ALL {
        println!(
            "    {:<34} {:>9}",
            reason.to_string(),
            grouped(tally.skipped(reason))
        );
    }
    println!();
}

pub(crate) fn print_bytes_verdict(corpus: &ChatCorpus) {
    let spoken = corpus.spoken_bytes();
    let files = corpus.file_bytes();
    println!("Б. ДОЛЯ ТЕКСТА РЕПЛИК В БАЙТАХ КОРПУСА");
    println!(
        "  байт транскриптов            {:>9}   {}",
        grouped(files),
        megabytes(files)
    );
    println!(
        "  байт реплик                  {:>9}   {}",
        grouped(spoken),
        megabytes(spoken)
    );
    println!("  доля                         {:>9}", share(spoken, files));
    for speaker in [Speaker::Owner, Speaker::Model] {
        let (replies, bytes) = said_by(corpus, speaker);
        println!(
            "    {:<12} {:>14}   {} в {} репликах",
            speaker.to_string(),
            share(bytes, files),
            megabytes(bytes),
            grouped(replies)
        );
    }
    let medians: Vec<u64> = corpus
        .transcripts()
        .iter()
        .map(|transcript| transcript.conversation().spoken_bytes())
        .collect();
    let file_medians: Vec<u64> = corpus
        .transcripts()
        .iter()
        .map(|transcript| transcript.file_bytes())
        .collect();
    println!(
        "  медиана диалога в беседе     {:>9}   при медиане файла {} — транскрипт толще\n\
         \x20                                          разговора в {:.0} раз",
        kilobytes(median(medians.clone())),
        megabytes(median(file_medians.clone())),
        median(file_medians) as f64 / median(medians).max(1) as f64
    );
    println!();
}

/// How much of the corpus one party said, in replies and in bytes.
fn said_by(corpus: &ChatCorpus, speaker: Speaker) -> (u64, u64) {
    let mut replies = 0;
    let mut bytes = 0;
    for transcript in corpus.transcripts() {
        for reply in transcript.conversation().replies() {
            if reply.speaker() != speaker {
                continue;
            }
            replies += 1;
            bytes += reply.text().len() as u64;
        }
    }
    (replies, bytes)
}

pub(crate) fn print_name_verdict(corpus: &ChatCorpus) {
    let named: Vec<_> = corpus
        .transcripts()
        .iter()
        .filter_map(|transcript| {
            transcript
                .conversation()
                .name()
                .map(|name| (transcript, name))
        })
        .collect();
    let by_owner = named
        .iter()
        .filter(|(_, name)| name.named_by() == NamedBy::Owner)
        .count();
    let name_lines: Vec<u64> = corpus
        .transcripts()
        .iter()
        .map(|transcript| transcript.tally().names())
        .collect();
    let renamed = corpus
        .transcripts()
        .iter()
        .filter(|transcript| transcript.tally().renames() > 0)
        .count();

    println!("В. ИМЯ БЕСЕДЫ");
    println!(
        "  имя нашлось у                {:>9} из {}",
        named.len(),
        corpus.transcripts().len()
    );
    println!(
        "    назвал владелец            {:>9}   (строка custom-title)",
        by_owner
    );
    println!(
        "    назвала модель             {:>9}   (строка ai-title)",
        named.len() - by_owner
    );
    println!(
        "  строк с именем в корпусе     {:>9}   медиана на файл {}, максимум {}",
        grouped(corpus.tally().names()),
        grouped(median(name_lines.clone())),
        grouped(name_lines.iter().copied().max().unwrap_or_default())
    );
    println!(
        "  имя менялось по ходу         {:>9} бесед, {} смен всего — правило «последняя \
         побеждает» имеет смысл ровно здесь",
        renamed,
        grouped(corpus.tally().renames())
    );
    for transcript in corpus.transcripts() {
        if transcript.conversation().name().is_none() {
            println!(
                "  БЕЗ ИМЕНИ                    {} ({})",
                transcript.file_name(),
                megabytes(transcript.file_bytes())
            );
        }
    }
    println!();
}

/// Verdict Г: the largest transcript, which is the one the index does not hold at all.
pub(crate) fn print_largest_verdict(corpus: &ChatCorpus, limit_bytes: u64) {
    let Some(largest) = corpus
        .transcripts()
        .iter()
        .max_by_key(|transcript| transcript.file_bytes())
    else {
        println!("Г. САМАЯ КРУПНАЯ БЕСЕДА: корпус пуст\n");
        return;
    };
    let spoken = largest.conversation().spoken_bytes();
    println!("Г. САМАЯ КРУПНАЯ БЕСЕДА");
    println!("  файл                         {}", largest.file_name());
    println!(
        "  имя                          {}",
        largest
            .conversation()
            .name()
            .map_or("— без имени —", |name| name.value())
    );
    println!(
        "  размер                       {} = {}   предел содержимого {}",
        megabytes(largest.file_bytes()),
        mebibytes(largest.file_bytes()),
        mebibytes(limit_bytes)
    );
    println!(
        "  в индексе                    {}",
        if largest.file_bytes() > limit_bytes {
            "НЕТ — файл крупнее предела, его содержимое не проиндексировано вовсе"
        } else {
            "да"
        }
    );
    println!(
        "  строк в файле                {}",
        grouped(largest.lines())
    );
    println!(
        "  диалог                       {} в {} репликах = {} файла",
        megabytes(spoken),
        grouped(largest.conversation().replies().len() as u64),
        share(spoken, largest.file_bytes())
    );
    if spoken > 0 {
        println!(
            "  диалог влезает в предел      в {:.1} раза",
            limit_bytes as f64 / spoken as f64
        );
    }
    println!();
}
