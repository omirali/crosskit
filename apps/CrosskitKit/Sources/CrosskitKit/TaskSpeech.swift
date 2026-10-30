import Foundation

/// Ответ службы `task-speech`: реплики всех бесед задачи, как их собрал граф. Окно показывает
/// по ним беседу задачи с номерами реплик — теми же, на которые ссылается карта.
public struct TaskSpeech: Decodable, Sendable, Equatable {
    public let task: String
    public let taskId: String
    public let conversations: [SpokenConversation]

    public func conversation(_ session: String) -> SpokenConversation? {
        conversations.first { $0.session == session }
    }
}

public struct SpokenConversation: Decodable, Sendable, Equatable, Identifiable {
    public let session: String
    public let name: String?
    public let firstAtUnixMs: Int64?
    public let replies: [SpokenReply]

    public var id: String { session }
}

public struct SpokenReply: Decodable, Sendable, Equatable, Identifiable {
    /// С единицы, в порядке, в котором реплики сказаны.
    public let n: Int
    public let owner: Bool
    public let text: String

    public var id: Int { n }
}

extension ServiceClient {

    /// Реплики бесед задачи. Срок тот же, что у карты: служба собирает их из графа.
    public func taskSpeech(_ task: String, context: String?) async throws(ServiceError) -> TaskSpeech {
        try await ask("task-speech", context: context, arguments: [task, "--json"], timeout: Self.mapTimeout)
    }
}
