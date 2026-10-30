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
    /// Где задача стоит по своим картам (ADR 0061). Служба старее этого не отдаёт.
    public let progress: TaskProgress?
    /// Когда владелец отметил задачу готовой.
    public let finishedAtUnixMs: Int64?
    let riskDecisions: [RiskDecisionLine]?

    /// Слово владельца о рисках задачи, по месту цитаты каждого.
    public var decisions: [RiskDecisionLine] { riskDecisions ?? [] }

    /// Колонка доски задач (`docs/decision-map.md`, раздел 9).
    public var column: TaskColumn {
        if finishedAtUnixMs != nil { return .done }
        guard let progress, progress.mapped > 0 else { return .unmapped }
        return progress.head == .open ? .waiting : .inWork
    }
}

/// Где задача стоит по сохранённым картам.
public struct TaskProgress: Decodable, Sendable, Equatable {
    public let conversations: Int
    public let mapped: Int
    public let steps: Int
    /// Что стало с последним шагом: `open` — задача ждёт решения владельца.
    public let head: BranchState?
    public let openRisks: Int
    public let openCritical: Int
    public let decidedRisks: Int
}

/// Решение владельца о риске и место риска: беседа и реплика, где стоит его цитата.
public struct RiskDecisionLine: Decodable, Sendable, Equatable {
    public let session: String
    public let reply: Int
    /// `accepted` или `resolved`.
    public let verdict: RiskStatus
    public let reason: String
    public let atUnixMs: Int64
}

/// Колонки доски, слева направо.
public enum TaskColumn: CaseIterable, Sendable {
    /// Подтверждена, но не размечена ни одна беседа.
    case unmapped
    case inWork
    /// HEAD основной линии ждёт решения владельца.
    case waiting
    case done
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
