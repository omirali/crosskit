import Foundation

/// Шов между панелью и ядром.
///
/// Форму задаёт панель, исходя из того, что действительно стоит на экране. Что из этого
/// ядро умеет отдать сегодня, а что требует схемы v14+, разобрано по полям в
/// `apps/panel/docs/row-shape.md`; здесь то же самое коротко, в комментариях к полям.
///
/// Шов асинхронный, и это не украшение: открытие хранилища стоит 100–140 мс, а сам запрос
/// ~5 мс. Панель не имеет права ждать ответа синхронно — окно должно быть на экране через
/// один кадр, с тем, что у него есть.
///
/// Отмена — часть шва: задачу, чей ответ уже никому не нужен, панель отменяет, и источник
/// обязан бросить работу, а не досчитать её в пустоту (`CoreFeed` завершает процесс).
protocol ResultFeed: AnyObject {

    /// Контексты, между которыми можно переключаться (⌘K). Не из графа: контексты живут
    /// в конфигурации, и каждый — своя база (ADR 0008).
    var contexts: [String] { get }

    /// Сколько строк просить на страницу. У выдуманного источника — сколько помещает макет,
    /// у ядра — больше: каждая страница там стоит запуска процесса.
    var pageSize: Int { get }

    /// Подсказки пустого окна. Только то, что этот источник действительно понимает:
    /// подсказка, которая не работает, хуже, чем никакой.
    var hints: [SyntaxHint] { get }

    /// Страница выдачи по запросу. Медленный ответ на старый запрос панель выбрасывает
    /// сама — см. `FeedRequest.generation`.
    func page(for request: FeedRequest) async throws -> FeedPage
}

/// Что панель спрашивает.
struct FeedRequest {
    /// В каком контексте искать. Выдачи контекстов не смешиваются (ADR 0008).
    let context: String
    /// Пустой запрос — не «ничего не искать», а «покажи недавнее»: так устроено пустое
    /// окно макета. Сегодня ядро на пустой запрос отвечает пустой страницей.
    let query: String
    /// Фильтр ⌘1–4. Сегодня запрос ядра фильтра не принимает вовсе.
    let scope: FeedScope
    /// Курсор предыдущей страницы. Панель в него не заглядывает и своих не сочиняет.
    let after: FeedCursor?
    /// Сколько строк на страницу.
    let limit: Int
    /// Показывать ли файлы, которых нет на диске. Сегодня это `--include-missing`.
    let includeMissing: Bool
    /// Номер запроса. Ответы приходят не в том порядке, в каком спрошены: медленный ответ
    /// на предыдущую букву не должен затирать быстрый ответ на следующую.
    let generation: Int
}

/// Фильтр из строки над списком: ⌘1 Всё, ⌘2 Чаты, ⌘3 Файлы, ⌘4 Темы.
enum FeedScope: CaseIterable {
    case everything
    case conversations
    case files
    case topics

    var label: String {
        switch self {
        case .everything: return "Всё"
        case .conversations: return "Чаты"
        case .files: return "Файлы"
        case .topics: return "Темы"
        }
    }

    var key: String {
        switch self {
        case .everything: return "⌘1"
        case .conversations: return "⌘2"
        case .files: return "⌘3"
        case .topics: return "⌘4"
        }
    }
}

extension FeedScope {
    /// Какие строки оставляет фильтр. «Темы» здесь ничего не отсекают: искать по теме —
    /// дело источника, и источник, который этого не умеет, говорит об этом отказом.
    func wants(_ kind: RowKind) -> Bool {
        switch self {
        case .everything, .topics: return true
        case .conversations: return kind == .conversation
        case .files: return kind == .file
        }
    }
}

/// Непрозрачный курсор keyset-пагинации. Панель его только передаёт обратно.
struct FeedCursor: Equatable {
    let opaque: String
}

/// Страница выдачи.
struct FeedPage {
    /// Макет показывает две озаглавленные группы, а не один список, поэтому страница
    /// приходит группами. Сегодня один запрос даёт один перемешанный список совпадений.
    let sections: [FeedSection]
    /// Курсор следующей страницы; `nil` — дальше ничего.
    let next: FeedCursor?
    /// Числа под списком. `nil` — считать нечего, и строки под списком не будет:
    /// так у пустого окна, где выдачи ещё нет.
    let counters: FeedCounters?
    let index: IndexState
    /// Сколько файлов и бесед вообще в этом контексте. Нужно пустому окну: «Ищем в work:
    /// 26 репозиториев · 480 000 файлов · 468 бесед».
    let corpus: CorpusSummary?
}

/// Группа строк одного вида со своим заголовком и счётом.
struct FeedSection {
    let kind: RowKind
    let rows: [FeedRow]
    /// Сколько совпадений всего, если известно. Сегодня неизвестно: keyset-пагинация
    /// общего числа не считает, а страница его не несёт (ADR 0013, решение 9).
    let total: Int?

    var title: String {
        switch kind {
        case .conversation: return "Чаты"
        case .file: return "Файлы"
        }
    }
}

enum RowKind {
    case conversation
    case file
}

/// Строка выдачи. Видов два, и поля у них разные — это часть ответа этапа.
enum FeedRow {
    case conversation(ConversationRow)
    case file(FileRow)

    var identity: String {
        switch self {
        case .conversation(let row): return row.id
        case .file(let row): return row.id
        }
    }

}

/// Строка беседы. Почти всё в ней требует схемы v14+: беседы как сущности в графе нет,
/// транскрипт лежит там обычным файлом.
struct ConversationRow {
    /// Устойчивый ключ строки: по нему держится выделение между страницами.
    let id: String
    let cursor: FeedCursor
    /// Каким инструментом велась беседа. v14+.
    let tool: Tool
    /// Заголовок беседы. v14+.
    let title: String
    let fragment: Fragment
    /// Проект, к которому беседа относится. v14+.
    let project: String?
    /// Когда беседа последний раз шла; `nil` — транскрипт времени не записал.
    let when: String?
    /// Сколько в ней сообщений. v14+.
    let messageCount: Int?
    let topic: String?
    /// Идентификатор сессии — им открывают беседу в инструменте и его копирует ⌘C,
    /// когда открыть не удалось. v14+.
    let sessionID: String?
}

/// Строка файла. Здесь почти всё есть сегодня.
struct FileRow {
    let id: String
    let cursor: FeedCursor
    /// Полный путь — его открывает ⏎ и показывает в Finder ⌘⏎.
    let path: String
    /// Каталог и имя раздельно: макет красит их по-разному. Граф держит и путь,
    /// и `display_name`.
    let directory: String
    let name: String
    let fragment: Fragment
    /// По каким словам совпало — макет печатает их в мета-строке.
    let matched: [String]
    let presence: Presence
    /// Проект. v14+: узла проекта в графе нет, а корень контекста — не проект.
    let project: String?
    /// `nil` — источник времени не знает. Ядро в ответе на `search` время файла не отдаёт.
    let when: FileWhen?
    let topic: String?
}

/// Есть ли файл на диске. Различие «нет на диске» и «такого не было» — то, ради чего граф
/// держит историю (ADR 0013, решение 2).
enum Presence {
    case present
    case missing
}

/// Время у файла означает разное в двух случаях, и шов обязан различать: у живого это
/// время изменения, у исчезнувшего — когда его видели в последний раз.
enum FileWhen {
    case modified(String)
    case lastSeen(String)

    var text: String {
        switch self {
        case .modified(let text): return text
        case .lastSeen(let text): return "в индексе с " + text
        }
    }
}

enum Tool: CaseIterable {
    case claudeCode
    case codex
    case cursor
    case chatgpt

    var label: String {
        switch self {
        case .claudeCode: return "Claude Code"
        case .codex: return "Codex"
        case .cursor: return "Cursor"
        case .chatgpt: return "ChatGPT"
        }
    }

    /// Временная монограмма вместо логотипа — решение 1 дизайнера.
    var monogram: String {
        switch self {
        case .claudeCode: return "CC"
        case .codex: return "Cx"
        case .cursor: return "Cu"
        case .chatgpt: return "GP"
        }
    }
}

/// Фрагмент текста с отмеченными совпадениями.
///
/// Отмечены диапазонами, а не словами, и это сознательно. Ядро сегодня отдаёт список
/// совпавших слов; искать их в тексте обратно — значит подсветить не то, когда слово
/// встречается дважды или входит в другое слово. Диапазон однозначен.
struct Fragment {
    let text: String
    let matches: [Range<String.Index>]

    /// Разбор на куски для отрисовки: `true` — совпавший.
    func parts() -> [(String, Bool)] {
        guard !matches.isEmpty else { return [(text, false)] }
        var parts: [(String, Bool)] = []
        var cursor = text.startIndex
        for match in matches.sorted(by: { $0.lowerBound < $1.lowerBound }) {
            guard match.lowerBound >= cursor, match.upperBound <= text.endIndex else { continue }
            if cursor < match.lowerBound { parts.append((String(text[cursor..<match.lowerBound]), false)) }
            parts.append((String(text[match]), true))
            cursor = match.upperBound
        }
        if cursor < text.endIndex { parts.append((String(text[cursor...]), false)) }
        return parts
    }
}

/// Числа под списком. Они закреплены под ним, а не в конце прокрутки — решение 4
/// дизайнера: числа видны всегда.
struct FeedCounters {
    /// Совпадения, чьи пути политика больше не допускает. Считаются и не называются.
    let hiddenByPolicy: Int
    /// Файлы без единой живой локации, свёрнутые в число. `nil` — идёт проход, и число
    /// ещё уточняется.
    let missingCollapsed: Int?
    /// Какая это страница по счёту. Панель считает сама; сколько их всего — неизвестно.
    let page: Int
}

/// Из чего вообще состоит контекст. Нужно пустому окну.
struct CorpusSummary {
    let repositories: Int?
    let files: Int?
    let conversations: Int?
}

/// В каком состоянии индекс — правый угол шапки. Формой различаются, а не только цветом:
/// решение 15 дизайнера.
///
/// Двух состояний макета здесь нет, и это не пропуск. «Проход · 61 %» требует доли
/// сделанного, которой в графе нет вовсе; «индекс пуст» — это экран первого запуска,
/// которого этот этап не рисует. Оба разобраны в `apps/panel/docs/row-shape.md`, и заводить
/// под них ветку, которую никто не строит, значило бы рисовать то, чего не бывает.
enum IndexState {
    case fresh
    case behind(text: String)
}

/// Подсказка синтаксиса в пустом окне: что набрать и что это даст.
struct SyntaxHint {
    let syntax: String
    let explanation: String
}

/// Почему страница не пришла.
///
/// Причины — те, которые источники действительно умеют вернуть. Отказ ядра приходит
/// категорией из закрытого списка его схемы (`src/presentation/machine_answer.rs`), а не
/// текстом: что сказать человеку, решает панель.
enum FeedFailure: Error, Equatable {
    case contextNotConfigured(String)
    /// Ядро ответило отказом своей схемы.
    case core(CoreCategory)
    /// Исполняемого файла `crosskit` нет ни по `--core=`, ни в `PATH`.
    case coreNotFound
    /// Ядро ответило не тем, что панель умеет читать: другой номер схемы или не объект.
    case coreUnreadable
    /// Ядро не ответило за отведённое время, и процесс завершён.
    case coreTimedOut
    /// Фильтр, которого ядро не умеет: «Темы».
    case scopeNotAvailable(FeedScope)
}

/// Категория отказа ядра — зеркало `FailureCategory` из его схемы.
enum CoreCategory: String, Equatable {
    case usage
    case unreadableQuery = "unreadable-query"
    case configuration
    case noContext = "no-context"
    case contextRequired = "context-required"
    case unknownContext = "unknown-context"
    case notIndexed = "not-indexed"
    case busy
    case needsMigration = "needs-migration"
    case needsRecovery = "needs-recovery"
    case indexUnreadable = "index-unreadable"
    /// Фокус графа не называет ничего, что держит этот контекст.
    case notFound = "not-found"
    case failed
}
