// Точность поиска по контексту `chats`, измеренная на вопросах с известным ответом.
//
// Это не тест, а измерение: оно идёт к настоящему контексту на машине владельца, через ту
// же команду `crosskit search`, которой он пользуется руками. Поэтому оно под `#[ignore]`,
// как бенчмарки, и запускается отдельно:
//
//   cargo test --release --offline --test search_quality_chats -- --ignored --nocapture
//
// Почему измерение вообще существует. Полнота у поиска есть — правильный ответ почти всегда
// в пятёрке, — а точности наверху нет, и первый живой прогон владельца это показал
// (docs/search-quality-summary.md). Чинить такое на глаз нельзя: нужен набор вопросов, у
// которых ответ известен, и число до правки.
//
// Две колонки в один прогон. Первая — как программа отвечает сегодня. Вторая — как она
// отвечала бы, если бы стоп-слова вычёркивались из вопроса: измерение само вычёркивает их
// перед вызовом, и это в точности то, что делал бы фильтр в анализаторе. Так гипотезу
// удалось померить, не тронув ни анализатор, ни индекс, ни его отпечаток.
use std::path::{Path, PathBuf};
use std::process::Command;

/// Контекст, про который набор. Он назван в имени файла набора, и пути внутри — его пути.
const CONTEXT: &str = "chats";

/// Сколько результатов на странице: ровно столько, сколько человек читает глазами, и
/// столько же, сколько считает precision@5.
const PAGE: usize = 5;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/search-quality")
        .join(name)
}

/// Один вопрос: как он задан, где лежит ответ и чем этот файл правильный.
struct Question {
    text: String,
    answers: Vec<String>,
    why: String,
}

/// Каталог владельца, где лежат набор вопросов и отчёт по нему. В репозитории их нет: в наборе
/// настоящие пути, вопросы и имена бесед владельца.
const SEARCH_QUALITY_VARIABLE: &str = "CROSSKIT_SEARCH_QUALITY_DIR";

/// Строка набора: `вопрос | путь;путь | почему`. Разбор на три части, а не на все `|`,
/// потому что пояснение — последнее поле и в нём попадаются свои палки. Без переменной — `None`:
/// замер говорит, что задать, и ничего не делает.
fn question_set() -> Option<(bool, Vec<Question>)> {
    let directory = std::env::var_os(SEARCH_QUALITY_VARIABLE)?;
    let text = std::fs::read_to_string(PathBuf::from(directory).join("chats-questions.txt"))
        .expect("набор вопросов к контексту chats в каталоге владельца");
    let mut confirmed = false;
    let mut questions = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("confirmed:") {
            confirmed = value.trim() == "yes";
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let text = parts.next().expect("вопрос").trim().to_string();
        let answers = parts
            .next()
            .expect("ожидаемые файлы")
            .split(';')
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty())
            .collect();
        let why = parts.next().unwrap_or_default().trim().to_string();
        questions.push(Question { text, answers, why });
    }
    Some((confirmed, questions))
}

/// Тот же вопрос, адресованный только телу реплик.
///
/// Этап 32 сделал две вещи разом: выбросил из индекса вывод инструментов и завёл поле имени
/// беседы. Обе двигают число, и без разделения нельзя сказать, какая. Поле в запросе — то
/// самое разделение: `text:слово` спрашивает только тело, то есть индекс, из которого вывод
/// инструментов уже ушёл, но именем беседы ещё не пользуются.
fn asking_the_body_only(question: &str) -> String {
    question
        .split_whitespace()
        .map(|word| {
            let bare: String = word
                .chars()
                .filter(|letter| letter.is_alphanumeric() || *letter == '-' || *letter == '_')
                .collect();
            match bare.is_empty() {
                true => String::new(),
                false => format!("text:{bare}"),
            }
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Слова, которые вторая колонка вычёркивает из вопроса.
fn stop_words() -> Vec<String> {
    std::fs::read_to_string(fixture("candidate-stop-words.txt"))
        .expect("список стоп-слов кандидата")
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| line.split_whitespace())
        .map(|word| word.to_lowercase())
        .collect()
}

/// Вопрос без стоп-слов. Вопрос, в котором не осталось ничего, идёт как есть: пустая
/// выдача на «а что там?» — это отказ отвечать, а не улучшение точности.
fn without_stop_words(question: &str, stop: &[String]) -> String {
    let kept: Vec<&str> = question
        .split_whitespace()
        .filter(|word| {
            let bare: String = word
                .chars()
                .filter(|letter| letter.is_alphanumeric() || *letter == '-')
                .collect::<String>()
                .to_lowercase();
            !stop.contains(&bare)
        })
        .collect();
    if kept.is_empty() {
        return question.to_string();
    }
    kept.join(" ")
}

/// Один результат страницы: путь и то, по чему он совпал.
struct Found {
    path: String,
    matched: String,
}

/// Страница выдачи настоящей команды. Читается тот же текст, который видит человек.
fn search(query: &str) -> Vec<Found> {
    let output = Command::new(env!("CARGO_BIN_EXE_crosskit"))
        .args([
            "--context",
            CONTEXT,
            "search",
            query,
            "--limit",
            &PAGE.to_string(),
        ])
        .output()
        .expect("запустить crosskit");
    assert!(
        output.status.success(),
        "контекст {CONTEXT:?} не отвечает — измерение идёт к настоящей конфигурации этой \
         машины, а не к песочнице: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut found: Vec<Found> = Vec::new();
    for line in stdout.lines() {
        if let Some(path) = result_line(line) {
            found.push(Found {
                path,
                matched: String::new(),
            });
        } else if let Some(matched) = line.trim().strip_prefix("matched: ")
            && let Some(last) = found.last_mut()
        {
            last.matched = matched.to_string();
        }
    }
    found
}

/// `3. [present] /путь` — строка результата, и только она. Номер и состояние отбрасываются:
/// номер известен по порядку, а состояние к точности отношения не имеет.
fn result_line(line: &str) -> Option<String> {
    let (number, rest) = line.split_once(". ")?;
    number.parse::<usize>().ok()?;
    let (_, path) = rest.strip_prefix('[')?.split_once("] ")?;
    Some(path.to_string())
}

/// Место первого результата, попавшего в ожидаемый файл, считая с единицы.
fn rank_of(found: &[Found], answers: &[String]) -> Option<usize> {
    found
        .iter()
        .position(|result| answers.contains(&result.path))
        .map(|index| index + 1)
}

/// Сколько из пяти попали в ожидаемый файл.
fn hits(found: &[Found], answers: &[String]) -> usize {
    found
        .iter()
        .filter(|result| answers.contains(&result.path))
        .count()
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// Наибольший precision@5, какой этот набор вообще допускает.
///
/// precision@5 считает долю из пяти мест, занятых правильными файлами. У вопроса, к которому
/// правильный ответ один, таких мест не может быть больше одного, то есть 0.20 — и никакое
/// ранжирование этого не поднимет. Пока строкой выдачи был чанк, одна и та же правильная
/// беседа занимала по четыре места из пяти и число выходило выше потолка: этап 32 напечатал
/// 0.47 там, где ответить лучше 0.25 было нельзя. Поэтому потолок печатается рядом: без него
/// читатель видит падение 0.47 → 0.27 и не видит, что первое было измерением повторов.
fn precision_at_five_ceiling(questions: &[Question]) -> f64 {
    let per_question: Vec<f64> = questions
        .iter()
        .map(|question| question.answers.len().min(PAGE) as f64 / PAGE as f64)
        .collect();
    mean(&per_question)
}

/// Сколько различных файлов названо в пятёрке.
///
/// До этапа 33 строка была чанком, и одна беседа могла занять всю страницу: этап 29 намерил
/// 2.4 различных файла на пять мест. После свёртки это число равно длине страницы по
/// построению, и печатается оно не ради него самого, а ради того, чтобы было видно, что
/// свёртка действительно произошла.
fn distinct_files(found: &[Found]) -> usize {
    let mut seen: Vec<&str> = Vec::new();
    for result in found {
        if !seen.contains(&result.path.as_str()) {
            seen.push(&result.path);
        }
    }
    seen.len()
}

/// Одна колонка чисел по всему набору.
struct Column {
    ranks: Vec<Option<usize>>,
    precision_at_one: Vec<f64>,
    precision_at_five: Vec<f64>,
    files_in_page: Vec<f64>,
}

impl Column {
    fn empty() -> Self {
        Self {
            ranks: Vec::new(),
            precision_at_one: Vec::new(),
            precision_at_five: Vec::new(),
            files_in_page: Vec::new(),
        }
    }

    fn add(&mut self, found: &[Found], answers: &[String]) {
        let rank = rank_of(found, answers);
        self.ranks.push(rank);
        self.precision_at_one
            .push(if rank == Some(1) { 1.0 } else { 0.0 });
        self.precision_at_five
            .push(hits(found, answers) as f64 / PAGE as f64);
        self.files_in_page.push(distinct_files(found) as f64);
    }

    fn in_page(&self) -> usize {
        self.ranks.iter().filter(|rank| rank.is_some()).count()
    }
}

fn place(rank: Option<usize>) -> String {
    rank.map_or_else(|| "—".to_string(), |place| place.to_string())
}

#[test]
#[ignore = "измерение качества поиска: cargo test --release --offline --test search_quality_chats -- --ignored --nocapture"]
fn precision_of_the_chats_context() {
    let Some((confirmed, questions)) = question_set() else {
        println!(
            "{SEARCH_QUALITY_VARIABLE} не задан: назовите каталог владельца, где лежит \
             chats-questions.txt. Ничего не измерено."
        );
        return;
    };
    let stop = stop_words();
    if !confirmed {
        println!(
            "ВНИМАНИЕ: набор не подтверждён владельцем (`confirmed: no`). Ожидаемые ответы \
             расставил автор набора по выдаче, и числа ниже — предварительные.\n"
        );
    }

    let mut now = Column::empty();
    let mut candidate = Column::empty();
    let mut body_only = Column::empty();

    for (ordinal, question) in questions.iter().enumerate() {
        let asked = without_stop_words(&question.text, &stop);
        let found_now = search(&question.text);
        let found_candidate = if asked == question.text {
            None
        } else {
            Some(search(&asked))
        };

        now.add(&found_now, &question.answers);
        body_only.add(
            &search(&asking_the_body_only(&question.text)),
            &question.answers,
        );
        candidate.add(
            found_candidate.as_deref().unwrap_or(&found_now),
            &question.answers,
        );

        println!("{}. {}", ordinal + 1, question.text);
        if !question.why.is_empty() {
            println!("   ответ правильный тем, что: {}", question.why);
        }
        if asked != question.text {
            println!("   без стоп-слов: {asked}");
        }
        for (index, result) in found_now.iter().enumerate() {
            let hit = if question.answers.contains(&result.path) {
                "→"
            } else {
                " "
            };
            println!(
                "   {hit} {}. {:<44} по: {}",
                index + 1,
                file_name(&result.path),
                result.matched
            );
        }
        println!(
            "   место ответа: сейчас {}, со стоп-словами {}\n",
            place(now.ranks[ordinal]),
            place(candidate.ranks[ordinal])
        );
    }

    let total = questions.len();
    println!(
        "{:<20}{:>10}{:>18}{:>14}",
        "", "сейчас", "со стоп-словами", "только тело"
    );
    println!(
        "{:<20}{:>10.2}{:>18.2}{:>14.2}",
        "precision@1",
        mean(&now.precision_at_one),
        mean(&candidate.precision_at_one),
        mean(&body_only.precision_at_one)
    );
    println!(
        "{:<20}{:>10.2}{:>18.2}{:>14.2}   ← выше {:.2} этот набор не даёт",
        "precision@5",
        mean(&now.precision_at_five),
        mean(&candidate.precision_at_five),
        mean(&body_only.precision_at_five),
        precision_at_five_ceiling(&questions)
    );
    println!(
        "{:<20}{:>10}{:>18}{:>14}",
        "ответ в пятёрке",
        format!("{}/{total}", now.in_page()),
        format!("{}/{total}", candidate.in_page()),
        format!("{}/{total}", body_only.in_page())
    );
    println!(
        "{:<20}{:>10.2}{:>18.2}{:>14.2}",
        "файлов в пятёрке",
        mean(&now.files_in_page),
        mean(&candidate.files_in_page),
        mean(&body_only.files_in_page)
    );
    println!(
        "\n«только тело» — тот же индекс, спрошенный через `text:`, то есть без поля имени \
         беседы.\nРазница с колонкой «сейчас» — вклад веса на имени (этап 33): 0.60 против \
         0.80.\n«Файлов в пятёрке» — 5 с этапа 33 по построению: строка выдачи стала \
         документом,\nи страница из пяти строк — это пять разных файлов (было 3.0).\n\n\
         precision@5 сравнивать с числом до этапа 33 нельзя: у вопроса с одним правильным \
         ответом\nбольше 0.20 не бывает, и 0.47 этапа 32 — это одна и та же беседа, занявшая \
         четыре места\nиз пяти. Осмысленное сравнение — с потолком выше."
    );
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
