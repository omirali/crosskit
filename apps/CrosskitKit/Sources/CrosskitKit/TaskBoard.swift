import Foundation

/// Ответ `tasks --json`: задачи, которые владелец подтвердил, и кандидаты сшивки (ADR 0052, 0053).
public struct TaskBoard: Decodable, Sendable, Equatable {
    public let context: String
    public let confirmed: [ConfirmedTask]
    public let candidates: [CandidateTask]

    enum CodingKeys: String, CodingKey {
        case context, confirmed
        case candidates = "tasks"
    }
}

public struct ConfirmedTask: Decodable, Sendable, Equatable, Identifiable {
    public let id: String
    public let name: String
    public let conversations: [TaskConversation]
    /// Что сшивка связывает с задачей сейчас и о чём владелец ещё не решил.
    public let proposed: [TaskConversation]
}

public struct CandidateTask: Decodable, Sendable, Equatable {
    public let conversations: [TaskConversation]
}

public struct TaskConversation: Decodable, Sendable, Equatable, Identifiable {
    public let session: String
    public let platform: Platform
    public let name: String?
    /// Первые слова владельца — для беседы без имени.
    public let opening: String?
    public let firstAtUnixMs: Int64?
    public let lastAtUnixMs: Int64?
    /// `strong` или `neighbour`.
    public let tie: String
    public let signals: [String]

    public var id: String { session }
    public var title: String { name ?? opening ?? session }
    public var firstAt: Date? { firstAtUnixMs.map(Date.init(unixMilliseconds:)) }
}

public enum Platform: String, Decodable, Sendable {
    case claudeCode = "claude-code"
    case codex
}
