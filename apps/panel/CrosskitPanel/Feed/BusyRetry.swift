import Foundation

/// `busy` — ожидание, а не ошибка: проход службы или `index watch` пишет граф, и ядро в этот
/// момент не открывает его на чтение (ADR 0049).
enum BusyRetry {

    /// Паузы перед повторами — 3,75 с в сумме. На копии графа владельца без правок каждое окно
    /// `busy` было короче 2,4 с, и такие повторы закрывают их все; длинное окно — под правками —
    /// панель переживает с прежней выдачей на экране (`PanelContent.apply(failure:)`).
    static let pauses: [Double] = [0.25, 0.5, 1.0, 2.0]

    /// `ask`, повторённый после каждой паузы, пока он отвечает `busy`. Отмена — новая буква —
    /// прерывает и паузу: `Task.sleep` бросает `CancellationError`.
    static func asking<Answer>(pauses: [Double] = pauses,
                               _ ask: () async throws -> Answer) async throws -> Answer {
        var remaining = pauses[...]
        while true {
            do {
                return try await ask()
            } catch FeedFailure.core(.busy) where !remaining.isEmpty {
                try await Task.sleep(nanoseconds: UInt64(remaining.removeFirst() * 1_000_000_000))
            }
        }
    }
}
