import Foundation

/// Что служба сама пишет о своих контекстах в `status.json` рядом с сокетом. Отказ `not-served`
/// не говорит, почему контекст не отдаётся; статус говорит: граф ещё поднимается или служба
/// ждёт и знает чего.
public struct ContextState: Decodable, Sendable, Equatable {
    public let name: String
    /// `starting`, `watching`, `waiting` или `stopped`.
    public let phase: String
    /// Почему служба ждёт, её собственной фразой.
    public let waiting: String?
    public let lastPass: TimedLine?

    public struct TimedLine: Decodable, Sendable, Equatable {
        public let atMs: Int64
        public let text: String
    }
}

struct ServiceStatusFile: Decodable {
    let contexts: [ContextState]
}

extension ServiceClient {

    /// Состояние контекста по статусу службы; ничего, когда статуса нет или контекста в нём.
    public func state(of context: String) -> ContextState? {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let data = try? Data(contentsOf: statusFile),
              let status = try? decoder.decode(ServiceStatusFile.self, from: data)
        else { return nil }
        return status.contexts.first { $0.name == context }
    }
}
