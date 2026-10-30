import Foundation

/// Выдуманный источник выдачи: маленький корпус в памяти, который ищет по-настоящему.
///
/// Выдуман он весь — ни одного обращения к ядру, к графу и к индексу здесь нет. Но ищет он
/// не по сценарию, а по запросу: набор в строке действительно сужает выдачу, страницы
/// действительно листаются курсором, счётчики действительно считают то, что не показано.
/// Иначе шов остался бы недоказанным — панель показывала бы заранее подготовленные экраны.
final class InventedFeed: ResultFeed {

    /// Сколько миллисекунд источник «думает». Настоящий ответ стоит ~5 мс поверх открытия
    /// хранилища; ключом запуска это растягивается, чтобы состояние набора можно было снять.
    private let thinkingMilliseconds: Int

    let contexts = ["work", "personal", "chats"]
    let pageSize = 3
    let hints = SyntaxHint.mockup

    init(thinkingMilliseconds: Int) {
        self.thinkingMilliseconds = thinkingMilliseconds
    }

    func page(for request: FeedRequest) async throws -> FeedPage {
        if thinkingMilliseconds > 0 {
            try? await Task.sleep(nanoseconds: UInt64(thinkingMilliseconds) * 1_000_000)
        }
        guard contexts.contains(request.context) else {
            throw FeedFailure.contextNotConfigured(request.context)
        }

        let terms = Self.terms(of: request.query)
        guard !terms.isEmpty else { return recentPage(context: request.context) }

        let corpus = Self.corpus.filter { $0.contexts.contains(request.context) }
        let matched = corpus.compactMap { item in item.match(terms).map { (item, $0) } }
        let admitted = matched.filter { $0.0.admittedByPolicy }
        let hiddenByPolicy = matched.count - admitted.count

        var conversations: [Item] = []
        var files: [Item] = []
        var missingCollapsed = 0
        for (item, _) in admitted {
            switch item.kind {
            case .conversation:
                conversations.append(item)
            case .file:
                if item.presence == .missing && !request.includeMissing && files.contains(where: { $0.presence == .missing }) {
                    // Первые missing показаны с пометкой, остальные свёрнуты в число —
                    // решение 5 дизайнера.
                    missingCollapsed += 1
                } else {
                    files.append(item)
                }
            }
        }

        let ranges = Dictionary(uniqueKeysWithValues: matched.map { ($0.0.id, $0.1) })
        let start = Self.offset(of: request.after)
        var sections: [FeedSection] = []
        if request.scope.wants(.conversation), !conversations.isEmpty {
            sections.append(section(.conversation, conversations, ranges: ranges, start: start, limit: request.limit))
        }
        if request.scope.wants(.file), !files.isEmpty {
            sections.append(section(.file, files, ranges: ranges, start: start, limit: request.limit))
        }

        let shown = sections.reduce(0) { $0 + $1.rows.count }
        let available = sections.reduce(0) { $0 + ($1.total ?? 0) }
        return FeedPage(
            sections: sections,
            next: shown > 0 && start + shown < available ? FeedCursor(opaque: "смещение:\(start + shown)") : nil,
            counters: FeedCounters(hiddenByPolicy: hiddenByPolicy,
                                   missingCollapsed: missingCollapsed,
                                   page: start / max(request.limit, 1) + 1),
            index: .fresh,
            corpus: nil)
    }

    /// Пустой запрос — не «ничего», а «недавнее»: так устроено пустое окно макета.
    private func recentPage(context: String) -> FeedPage {
        let recent = Self.corpus
            .filter { $0.contexts.contains(context) && $0.kind == .conversation && $0.admittedByPolicy }
            .prefix(3)
        let rows = recent.map { $0.row(matches: [], cursor: FeedCursor(opaque: "недавнее:" + $0.id)) }
        return FeedPage(
            sections: [FeedSection(kind: .conversation, rows: Array(rows), total: nil)],
            next: nil,
            counters: nil,
            index: .fresh,
            corpus: CorpusSummary(repositories: 26, files: 480_000, conversations: 468))
    }

    private func section(_ kind: RowKind, _ items: [Item], ranges: [String: [Range<String.Index>]],
                         start: Int, limit: Int) -> FeedSection {
        let window = items.dropFirst(start).prefix(limit)
        let rows = window.map { item in
            item.row(matches: ranges[item.id] ?? [], cursor: FeedCursor(opaque: "строка:" + item.id))
        }
        return FeedSection(kind: kind, rows: Array(rows), total: items.count)
    }

    private static func offset(of cursor: FeedCursor?) -> Int {
        guard let cursor, cursor.opaque.hasPrefix("смещение:") else { return 0 }
        return Int(cursor.opaque.dropFirst("смещение:".count)) ?? 0
    }

    /// Слова запроса. Правило грубое — совпадение по началу слова от трёх букв, — и это
    /// не морфология: настоящую даёт ядро (ADR 0013, решение 3).
    private static func terms(of query: String) -> [String] {
        query.lowercased()
            .split(whereSeparator: { !$0.isLetter && !$0.isNumber })
            .map(String.init)
            .filter { $0.count >= 3 }
    }
}

/// Одна запись выдуманного корпуса.
private struct Item {
    let id: String
    /// В каких контекстах она лежит. Выдуманная раскладка: `work` — дерево целиком,
    /// `chats` — только беседы, `personal` — настроен и не пройден. Нужна затем, чтобы
    /// переключение контекста меняло выдачу, а не только надпись в чипе.
    var contexts: [String] { kind == .conversation ? ["work", "chats"] : ["work"] }
    let kind: RowKind
    let tool: Tool
    let title: String
    let directory: String
    let snippet: String
    let project: String
    let when: String
    let messageCount: Int?
    let topic: String?
    let sessionID: String?
    let presence: Presence
    /// Политика допускает путь. Недопущенные не показываются никогда — только считаются.
    let admittedByPolicy: Bool

    /// Диапазоны совпадений во фрагменте, или `nil`, если запись не подошла.
    func match(_ terms: [String]) -> [Range<String.Index>]? {
        let haystack = (title + " " + directory + " " + snippet + " " + (topic ?? "")).lowercased()
        let stems = terms.map { String($0.prefix(max(4, $0.count - 2))) }
        guard stems.contains(where: { haystack.contains($0) }) else { return nil }

        var ranges: [Range<String.Index>] = []
        let lowered = snippet.lowercased()
        for term in terms {
            // Ищется не слово целиком, а его начало: «политика» должна найти «политики».
            // Это не морфология и ею не притворяется — настоящую даёт ядро.
            let stem = String(term.prefix(max(4, term.count - 2)))
            var from = lowered.startIndex
            while let found = lowered.range(of: stem, range: from..<lowered.endIndex) {
                // Подсвечивается слово целиком, а не совпавший префикс: обрезанное
                // «маркер» внутри «маркером» читается как опечатка.
                let end = lowered[found.upperBound...].firstIndex(where: { !$0.isLetter && !$0.isNumber })
                    ?? lowered.endIndex
                ranges.append(found.lowerBound..<end)
                from = end
            }
        }
        return ranges
    }

    func row(matches: [Range<String.Index>], cursor: FeedCursor) -> FeedRow {
        let fragment = Fragment(text: snippet, matches: matches)
        switch kind {
        case .conversation:
            return .conversation(ConversationRow(
                id: id, cursor: cursor, tool: tool, title: title, fragment: fragment,
                project: project, when: when, messageCount: messageCount, topic: topic,
                sessionID: sessionID))
        case .file:
            return .file(FileRow(
                id: id, cursor: cursor, path: directory + title, directory: directory, name: title,
                fragment: fragment,
                matched: matchedWords(matches), presence: presence, project: project,
                when: presence == .present ? .modified(when) : .lastSeen(when),
                topic: topic))
        }
    }

    private func matchedWords(_ matches: [Range<String.Index>]) -> [String] {
        var seen: [String] = []
        for range in matches {
            let word = String(snippet[range]).lowercased()
            if !seen.contains(word) { seen.append(word) }
        }
        return seen
    }
}

extension SyntaxHint {
    /// Подсказки макета. `проект:` и `тема:` — предположение дизайнера (решение 11), которое
    /// ядро сегодня не понимает; поэтому они живут здесь, у выдуманного источника.
    static let mockup = [
        SyntaxHint(syntax: "проект:crosscode", explanation: "только беседы и файлы проекта"),
        SyntaxHint(syntax: "тема:платежи", explanation: "по метке, которую поставила модель"),
        SyntaxHint(syntax: "\"точная фраза\"", explanation: "совпадение целиком, по порядку")
    ]
}

private extension InventedFeed {

    /// Корпус взят из макета `design/CCWindow.dc.html`, включая записи, которых на экране
    /// не бывает: их дело — быть посчитанными в строке под списком.
    static let corpus: [Item] = [
        Item(id: "c1", kind: .conversation, tool: .claudeCode,
             title: "Почему сверка помечала 727 локаций missing", directory: "",
             snippet: "…каталог с маркером пропускался на уровне обхода, и сверка читала молчание…",
             project: "crosscode", when: "3 дня назад", messageCount: 42, topic: "индексация",
             sessionID: "9c21f4a0…7b3", presence: .present, admittedByPolicy: true),
        Item(id: "c2", kind: .conversation, tool: .cursor,
             title: "Walker: .ccignore и вложенные репозитории", directory: "",
             snippet: "…если маркер лежит глубже корня, правило политики наследуется вниз по дереву…",
             project: "crosscode", when: "5 дней назад", messageCount: 23, topic: "индексация",
             sessionID: "7f3ab1c4…c21", presence: .present, admittedByPolicy: true),
        Item(id: "c3", kind: .conversation, tool: .codex,
             title: "Policy engine: порядок правил", directory: "",
             snippet: "…маркер обхода превращаем в правило политики с приоритетом выше пользовательских…",
             project: "crosscode", when: "2 недели назад", messageCount: 11, topic: "политика доступа",
             sessionID: "41ee90b2…8da", presence: .present, admittedByPolicy: true),
        Item(id: "c4", kind: .conversation, tool: .chatgpt,
             title: "Как назвать сущность пропуска в ADR", directory: "",
             snippet: "…«маркер» звучит как файл, а в ADR нам нужна политика и причина пропуска…",
             project: "crosscode", when: "3 недели назад", messageCount: 8, topic: "документация",
             sessionID: nil, presence: .present, admittedByPolicy: true),
        Item(id: "c5", kind: .conversation, tool: .codex,
             title: "Retry policy for payment webhook", directory: "",
             snippet: "…exponential backoff, но идемпотентность ключа мы так и не закрыли…",
             project: "payments-api", when: "2 недели назад", messageCount: 17, topic: "платежи",
             sessionID: "5b0c77de…41f", presence: .present, admittedByPolicy: true),
        Item(id: "c6", kind: .conversation, tool: .claudeCode,
             title: "Ключи идемпотентности для webhook-ретраев", directory: "",
             snippet: "…ключ идемпотентности живёт 24 часа, повтор после этого примет событие как новое…",
             project: "payments-api", when: "месяц назад", messageCount: 29, topic: "платежи",
             sessionID: "a1d3e8c9…002", presence: .present, admittedByPolicy: true),
        Item(id: "c7", kind: .conversation, tool: .cursor,
             title: "Ретраи webhook: очередь или cron", directory: "",
             snippet: "…webhook падает на таймауте, и ретрай из cron дублирует списание…",
             project: "payments-api", when: "12 сен", messageCount: 34, topic: "платежи",
             sessionID: "7f3ab1c4…c21", presence: .present, admittedByPolicy: true),
        Item(id: "h1", kind: .conversation, tool: .claudeCode,
             title: "Беседа, которую политика не допускает", directory: "",
             snippet: "…маркер политики закрыл этот путь, и назвать его нельзя…",
             project: "—", when: "—", messageCount: nil, topic: nil,
             sessionID: nil, presence: .present, admittedByPolicy: false),
        Item(id: "h2", kind: .conversation, tool: .codex,
             title: "Вторая закрытая политикой", directory: "",
             snippet: "…маркер политики закрыл и этот…", project: "—", when: "—",
             messageCount: nil, topic: nil, sessionID: nil, presence: .present,
             admittedByPolicy: false),
        Item(id: "f1", kind: .file, tool: .claudeCode,
             title: "0027-marker-as-policy-rule.md", directory: "~/Projects/crosscode/docs/adr/",
             snippet: "Пропуск обхода — это правило политики, а не отдельная сущность, и маркер тут ни при чём",
             project: "crosscode", when: "3 дня назад", messageCount: nil, topic: "индексация",
             sessionID: nil, presence: .present, admittedByPolicy: true),
        Item(id: "f2", kind: .file, tool: .claudeCode,
             title: "walker.rs", directory: "~/Projects/crosscode/src/index/",
             snippet: "// маркер обхода — частный случай правила политики, см. ADR-0027",
             project: "crosscode", when: "3 дня назад", messageCount: nil, topic: "индексация",
             sessionID: nil, presence: .present, admittedByPolicy: true),
        Item(id: "f3", kind: .file, tool: .claudeCode,
             title: "marker-draft.md", directory: "~/Projects/crosscode/notes/",
             snippet: "Черновик: маркер как файл .ccskip в корне каталога",
             project: "crosscode", when: "14 авг", messageCount: nil, topic: "индексация",
             sessionID: nil, presence: .missing, admittedByPolicy: true),
        Item(id: "f4", kind: .file, tool: .claudeCode,
             title: "policy-rules.rs", directory: "~/Projects/crosscode/src/policy/",
             snippet: "// правило политики читается сверху вниз, маркер — первое из них",
             project: "crosscode", when: "неделю назад", messageCount: nil, topic: "политика доступа",
             sessionID: nil, presence: .missing, admittedByPolicy: true),
        Item(id: "f5", kind: .file, tool: .claudeCode,
             title: "retry.ts", directory: "~/Projects/payments-api/src/webhooks/",
             snippet: "// ключ идемпотентности считаем от event.id, а не от payload",
             project: "payments-api", when: "вчера", messageCount: nil, topic: "платежи",
             sessionID: nil, presence: .present, admittedByPolicy: true)
    ]
}
