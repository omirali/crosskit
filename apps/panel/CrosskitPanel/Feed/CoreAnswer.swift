import Foundation

/// Ответ ядра на `--json` и как он становится страницей панели.
///
/// Схема объявлена в одном месте — `src/presentation/machine_answer.rs`, — и здесь её
/// зеркало, ровно на те поля, которые панель читает. Номер схемы проверяется первым: ответ
/// другого номера не читается вовсе, а не читается наполовину — одно поле, принятое за
/// другое, хуже честного «не понимаю».
///
/// Здесь нет ни процесса, ни окна: только байты на входе и страница на выходе. Поэтому этот
/// файл и проверяется тестами без запуска приложения.
enum CoreAnswer {

    /// Номер схемы, под который написана эта панель.
    static let schema = 2

    /// Страница выдачи, разобранная и ещё не отфильтрованная.
    struct Page: Decodable, Equatable {
        let context: String
        let query: String
        let results: [Row]
        let next: String?
        let notShown: NotShown
        let index: Index?
    }

    struct Row: Decodable, Equatable {
        let kind: Kind
        let cursor: String
        let path: String
        let state: State
        let fragment: String
        let matched: [String]
        let matches: Int
        let topic: String?
        let conversation: Conversation?
    }

    enum Kind: String, Decodable, Equatable {
        case conversation
        case file

        var rowKind: RowKind { self == .conversation ? .conversation : .file }
    }

    enum State: String, Decodable, Equatable {
        case present
        case missing
    }

    struct Conversation: Decodable, Equatable {
        let sessionId: String
        let name: String?
        let messageCount: Int
        let lastMessageAtUnixMs: Int64?
    }

    struct NotShown: Decodable, Equatable {
        let byPolicy: Int
        let missing: Int
        let lostByGraph: Int
    }

    struct Index: Decodable, Equatable {
        let documents: Int
        let writtenByThisBuild: Bool
    }

    // MARK: Разбор

    /// Страница — или отказ, брошенный как `FeedFailure`.
    static func page(from data: Data) throws -> Page {
        let decoder = try checked(data)
        return try decode(Page.self, from: data, with: decoder)
    }

    /// Контексты из `crosskit contexts --json` — или отказ.
    static func contexts(from data: Data) throws -> [String] {
        let decoder = try checked(data)
        return try decode(Contexts.self, from: data, with: decoder).contexts
    }

    private struct Envelope: Decodable {
        let schema: Int
        let error: Refusal?
    }

    private struct Refusal: Decodable {
        let category: String
    }

    private struct Contexts: Decodable {
        let contexts: [String]
    }

    /// Номер схемы и отказ — до того, как читать что-то ещё. Общий для всех ответов ядра:
    /// страницы, списка контекстов и графа (`GraphAnswer`).
    static func checked(_ data: Data) throws -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let envelope = try decode(Envelope.self, from: data, with: decoder)
        guard envelope.schema == schema else { throw FeedFailure.coreUnreadable }
        if let refusal = envelope.error {
            // Категории — закрытый список, но панель и ядро обновляются порознь. Незнакомая
            // категория читается как «не получилось», а не угадывается по написанию.
            throw FeedFailure.core(CoreCategory(rawValue: refusal.category) ?? .failed)
        }
        return decoder
    }

    static func decode<T: Decodable>(_ type: T.Type, from data: Data, with decoder: JSONDecoder) throws -> T {
        do {
            return try decoder.decode(type, from: data)
        } catch {
            throw FeedFailure.coreUnreadable
        }
    }

    // MARK: Страница панели

    /// Страница панели из страницы ядра.
    ///
    /// Ядро отдаёт один ранжированный список, панель показывает группы: строки раскладываются
    /// по виду, порядок внутри группы — порядок ядра. Фильтр ⌘2 и ⌘3 — это выбор группы на
    /// стороне панели: в запросе ядра фильтра нет (row-shape.md, конфликт 6).
    static func feedPage(_ page: Page, scope: FeedScope, pageNumber: Int, home: String, now: Date) -> FeedPage {
        var sections: [FeedSection] = []
        for kind in [RowKind.conversation, .file] where scope.wants(kind) {
            let ofKind = page.results.filter { $0.kind.rowKind == kind }.map { feedRow($0, home: home, now: now) }
            guard !ofKind.isEmpty else { continue }
            sections.append(FeedSection(kind: kind, rows: ofKind, total: nil))
        }
        return FeedPage(
            sections: sections,
            next: page.next.map(FeedCursor.init(opaque:)),
            counters: FeedCounters(hiddenByPolicy: page.notShown.byPolicy,
                                   missingCollapsed: page.notShown.missing,
                                   page: pageNumber),
            index: indexState(page),
            corpus: nil)
    }

    private static func indexState(_ page: Page) -> IndexState {
        guard let index = page.index else { return .fresh }
        if index.documents == 0 { return .behind(text: "индекс пуст") }
        if !index.writtenByThisBuild { return .behind(text: "написан другой сборкой") }
        if page.notShown.lostByGraph > 0 { return .behind(text: "отстал от графа") }
        return .fresh
    }

    static func feedRow(_ row: Row, home: String, now: Date) -> FeedRow {
        let fragment = Fragment(text: row.fragment, matches: ranges(of: row.matched, in: row.fragment))
        let cursor = FeedCursor(opaque: row.cursor)
        if let conversation = row.conversation {
            return .conversation(ConversationRow(
                id: row.cursor, cursor: cursor,
                // Эта сборка ядра читает транскрипты одного инструмента — Claude Code
                // (`ExtractionProfile` в ядре); поля инструмента в схеме нет.
                tool: .claudeCode,
                title: conversation.name ?? "Без названия · " + conversation.sessionId,
                fragment: fragment, project: nil,
                when: conversation.lastMessageAtUnixMs.map { relative(unixMs: $0, now: now) },
                messageCount: conversation.messageCount, topic: row.topic,
                sessionID: conversation.sessionId))
        }
        let (directory, name) = split(row.path, home: home)
        return .file(FileRow(
            id: row.cursor, cursor: cursor, path: row.path, directory: directory, name: name,
            fragment: fragment, matched: row.matched,
            presence: row.state == .present ? .present : .missing,
            project: nil, when: nil, topic: row.topic))
    }

    /// Каталог и имя раздельно; домашний каталог — `~`, как в макете.
    static func split(_ path: String, home: String) -> (String, String) {
        var shown = path
        if !home.isEmpty, shown.hasPrefix(home + "/") {
            shown = "~" + shown.dropFirst(home.count)
        }
        guard let slash = shown.lastIndex(of: "/") else { return ("", shown) }
        return (String(shown[...slash]), String(shown[shown.index(after: slash)...]))
    }

    /// Где подсветить совпавшие слова.
    ///
    /// Ядро отдаёт слова, а не диапазоны (row-shape.md, конфликт 7). Слова эти — из самого
    /// фрагмента, в его написании, поэтому обратный поиск почти всегда точен; чтобы слово не
    /// подсветилось внутри другого слова, совпадение берётся только целым словом.
    static func ranges(of words: [String], in text: String) -> [Range<String.Index>] {
        var found: [Range<String.Index>] = []
        for word in words where !word.isEmpty {
            var from = text.startIndex
            while let range = text.range(of: word, options: [.caseInsensitive], range: from..<text.endIndex) {
                if isWholeWord(range, in: text), !found.contains(where: { $0.overlaps(range) }) {
                    found.append(range)
                }
                from = range.upperBound
            }
        }
        return found.sorted { $0.lowerBound < $1.lowerBound }
    }

    private static func isWholeWord(_ range: Range<String.Index>, in text: String) -> Bool {
        let before = range.lowerBound > text.startIndex ? text[text.index(before: range.lowerBound)] : nil
        let after = range.upperBound < text.endIndex ? text[range.upperBound] : nil
        return [before, after].allSatisfy { character in
            guard let character else { return true }
            return !character.isLetter && !character.isNumber
        }
    }

    /// «сегодня», «вчера», «3 дня назад», «12 сен» — как в макете.
    static func relative(unixMs: Int64, now: Date) -> String {
        let then = Date(timeIntervalSince1970: TimeInterval(unixMs) / 1000)
        let calendar = Calendar.current
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: then),
                                           to: calendar.startOfDay(for: now)).day ?? 0
        switch days {
        case ..<1: return "сегодня"
        case 1: return "вчера"
        case 2...6: return "\(days) \(daysWord(days)) назад"
        default:
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "ru_RU")
            formatter.setLocalizedDateFormatFromTemplate(
                calendar.component(.year, from: then) == calendar.component(.year, from: now) ? "d MMM" : "d MMM yyyy")
            return formatter.string(from: then)
        }
    }

    private static func daysWord(_ days: Int) -> String {
        switch days {
        case 2...4: return "дня"
        default: return "дней"
        }
    }
}

extension CoreAnswer.Page {

    /// Строки, которые оставит фильтр.
    func rows(wanted scope: FeedScope) -> Int {
        results.filter { scope.wants($0.kind.rowKind) }.count
    }

    /// Эта страница и следующая за ней — как одна: строки подряд, курсор последней, а
    /// непоказанное — сумма, потому что на каждой странице оно своё.
    func followed(by later: CoreAnswer.Page) -> CoreAnswer.Page {
        CoreAnswer.Page(context: context, query: query, results: results + later.results, next: later.next,
             notShown: CoreAnswer.NotShown(byPolicy: notShown.byPolicy + later.notShown.byPolicy,
                                missing: notShown.missing + later.notShown.missing,
                                lostByGraph: notShown.lostByGraph + later.notShown.lostByGraph),
             index: later.index ?? index)
    }
}
