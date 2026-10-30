import Foundation

/// Источник, которому нечего отдать.
///
/// Он не для полноты набора. Пустая выдача — это не пустой список: человеку надо сказать,
/// сколько совпадений было скрыто политикой и насколько отстал индекс, иначе «ничего нет»
/// прочтётся как «ничего нет вообще», хотя что-то было и не показано.
///
/// Чего этот источник не выражает: «индекс пуст, контекст ещё не проходили». Это другое
/// сообщение и другой экран — первый запуск, который этот этап не рисует.
final class EmptyFeed: ResultFeed {

    let contexts: [String]
    let pageSize = 3
    let hints = SyntaxHint.mockup

    init(contexts: [String] = ["work", "personal", "chats"]) {
        self.contexts = contexts
    }

    func page(for request: FeedRequest) async throws -> FeedPage {
        FeedPage(
            sections: [],
            next: nil,
            counters: FeedCounters(hiddenByPolicy: 2, missingCollapsed: 0, page: 1),
            index: .behind(text: "отстал на 2 ч"),
            corpus: nil)
    }
}
