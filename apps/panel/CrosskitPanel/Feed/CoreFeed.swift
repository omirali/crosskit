import Foundation
import QuartzCore

/// Настоящий источник выдачи: ядро, процесс `crosskit` на каждую страницу.
///
/// Панель не открывает базу и не читает конфигурацию сама — всё, что она знает о данных,
/// приходит одним объектом схемы `--json`. Контексты — из `crosskit contexts --json` один
/// раз при старте, страница — из `crosskit --context <имя> search <запрос> --json`.
///
/// Цена моста известна заранее: каждый запрос платит запуск процесса и открытие хранилища.
/// Сколько именно на дереве владельца — замер этапа UI-3, и по нему решается, нужен ли демон.
final class CoreFeed: ResultFeed {

    let contexts: [String]
    /// Страница больше, чем у макета: каждая следующая стоит запуска процесса.
    let pageSize = 8

    /// Только то, что ядро понимает сегодня. `проект:` и `тема:` из макета ядро не разбирает,
    /// и обещать их значило бы показывать подсказку, после которой ничего не находится.
    let hints = [
        SyntaxHint(syntax: "\"точная фраза\"", explanation: "совпадение целиком, по порядку"),
        SyntaxHint(syntax: "+нужно -без", explanation: "слово обязательно или исключено"),
        SyntaxHint(syntax: "на прошлой неделе", explanation: "только то, что было в этот период")
    ]

    /// Ядро, которым отвечает этот источник; `nil`, если его не нашли.
    let core: CoreProcess?
    /// Почему ядро недоступно с самого старта; тогда этот отказ — ответ на любой запрос.
    private let startupFailure: FeedFailure?
    let meter: BridgeMeter?
    private let pageNumbers = PageNumbers()

    init(corePath: String?, environmentPath: String?, meter: BridgeMeter?) {
        self.meter = meter
        guard let executable = CoreProcess.locate(explicit: corePath, path: environmentPath) else {
            core = nil
            contexts = []
            startupFailure = .coreNotFound
            return
        }
        let core = CoreProcess(executable: executable, timeout: 10)
        self.core = core
        do {
            contexts = try CoreAnswer.contexts(from: core.runBlocking(["contexts", "--json"]))
            startupFailure = nil
        } catch let failure as FeedFailure {
            contexts = []
            startupFailure = failure
        } catch {
            contexts = []
            startupFailure = .coreUnreadable
        }
    }

    func page(for request: FeedRequest) async throws -> FeedPage {
        if let startupFailure { throw startupFailure }
        guard let core else { throw FeedFailure.coreNotFound }
        if request.scope == .topics { throw FeedFailure.scopeNotAvailable(.topics) }
        guard !request.query.trimmingCharacters(in: .whitespaces).isEmpty else {
            // Пустой запрос — подсказки, а не «недавнее»: недавнего ядро не отдаёт
            // (row-shape.md, конфликт 8), и процесс ради пустой страницы не запускается.
            return FeedPage(sections: [], next: nil, counters: nil, index: .fresh, corpus: nil)
        }

        // Под фильтром ⌘2 и ⌘3 страница ядра может не держать ни одной строки нужного вида,
        // а следующая — держать. Пустой экран тогда сказал бы «ничего нет» неправду, поэтому
        // следующие страницы дочитываются — до первой строки нужного вида, не до полной
        // страницы: добор до полной стоил 3–4 процесса и p50 413 мс на `work` (замер UI-3),
        // а остальное и так придёт по ↓. Не больше `mostRounds` процессов на одну страницу.
        var after = request.after
        var answer: CoreAnswer.Page?
        for _ in 0..<Self.mostRounds {
            let page = try await onePage(request, after: after, core: core)
            let joined = answer.map { $0.followed(by: page) } ?? page
            answer = joined
            guard let next = joined.next, joined.rows(wanted: request.scope) == 0 else { break }
            after = FeedCursor(opaque: next)
        }
        guard let answer else { throw FeedFailure.coreUnreadable }
        let number = pageNumbers.number(of: request.after)
        let page = CoreAnswer.feedPage(answer, scope: request.scope, pageNumber: number,
                                       home: NSHomeDirectory(), now: Date())
        if let next = answer.next { pageNumbers.remember(next, as: number + 1) }
        return page
    }

    /// Сколько процессов можно потратить на одну страницу под фильтром.
    private static let mostRounds = 4

    private func onePage(_ request: FeedRequest, after: FeedCursor?, core: CoreProcess) async throws -> CoreAnswer.Page {
        try await BusyRetry.asking { try await askOnce(request, after: after, core: core) }
    }

    private func askOnce(_ request: FeedRequest, after: FeedCursor?, core: CoreProcess) async throws -> CoreAnswer.Page {
        var arguments = ["--context", request.context, "search", request.query, "--json",
                         "--limit", String(request.limit)]
        if let after { arguments += ["--after", after.opaque] }
        if request.includeMissing { arguments.append("--include-missing") }

        let outcome = try await core.run(arguments)
        let decodeStarted = CACurrentMediaTime()
        let page = try CoreAnswer.page(from: outcome.stdout)
        meter?.record(generation: request.generation,
                      stages: BridgeMeter.CoreStages(spawned: outcome.spawnedAt, exited: outcome.exitedAt,
                                                     decodeStarted: decodeStarted,
                                                     decoded: CACurrentMediaTime()))
        return page
    }
}

/// Номер страницы по курсору, который к ней привёл. Сколько страниц всего, keyset не знает
/// (ADR 0013, решение 9), поэтому номер досчитывается здесь — по ходу листания.
private final class PageNumbers: @unchecked Sendable {
    private let lock = NSLock()
    private var byCursor: [String: Int] = [:]

    func number(of cursor: FeedCursor?) -> Int {
        guard let cursor else { return 1 }
        return lock.withLock { byCursor[cursor.opaque] ?? 2 }
    }

    func remember(_ cursor: String, as number: Int) {
        lock.withLock { byCursor[cursor] = number }
    }
}
