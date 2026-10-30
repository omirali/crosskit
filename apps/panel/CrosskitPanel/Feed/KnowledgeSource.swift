import Foundation

/// Откуда карта знаний берёт свой граф. Отдельно от выдачи и окрестности: это третий вопрос к
/// ядру. Выдуманный источник карты не рисует — фактов у него нет, а придумывать их значило бы
/// рисовать то, чего ни один файл не говорит.
protocol KnowledgeSource: AnyObject {
    /// Знание контекста целиком, не больше `limit` сущностей ядра.
    func knowledge(context: String) async throws -> KnowledgeAnswer.Map
}

extension CoreFeed: KnowledgeSource {

    /// Процесс `crosskit --context <имя> knowledge map --json` — тот же мост, что у выдачи и
    /// графа (ADR 0044), с той же отменой и тем же таймаутом.
    func knowledge(context: String) async throws -> KnowledgeAnswer.Map {
        guard let core else { throw FeedFailure.coreNotFound }
        let outcome = try await core.run(["--context", context, "knowledge", "map", "--json"])
        return try KnowledgeAnswer.map(from: outcome.stdout)
    }
}
