import Foundation

/// Решение владельца, которое окно отдаёт службе (ADR 0053, 0061). Служба принимает его тем же
/// кодом, что `crosskit tasks …`, и отвечает доской после решения или отказом-данными.
public enum OwnerDecision: Sendable, Equatable {
    case confirm(conversation: String, name: String)
    case reject(conversation: String)
    case add(task: String, conversation: String)
    case drop(task: String, conversation: String)
    case rename(task: String, name: String)
    case risk(task: String, session: String, reply: Int, verdict: RiskStatus, reason: String)
    case reopenRisk(task: String, session: String, reply: Int)
    case finish(task: String)
    case resume(task: String)

    /// Аргументы команды `tasks`, как их принимают и CLI, и служба.
    var arguments: [String] {
        let words: [String] = switch self {
        case .confirm(let conversation, let name): ["confirm", conversation, "--name", name]
        case .reject(let conversation): ["reject", conversation]
        case .add(let task, let conversation): ["add", task, conversation]
        case .drop(let task, let conversation): ["drop", task, conversation]
        case .rename(let task, let name): ["rename", task, name]
        case .risk(let task, let session, let reply, let verdict, let reason):
            [verdict == .resolved ? "resolve" : "accept", task, session, String(reply), "--reason", reason]
        case .reopenRisk(let task, let session, let reply): ["reopen", task, session, String(reply)]
        case .finish(let task): ["finish", task]
        case .resume(let task): ["resume", task]
        }
        return words + ["--json"]
    }
}

/// Решение, которое книга задач не приняла: вид и о чём. Слова для человека — у окна.
public struct Declined: Decodable, Sendable, Equatable {
    /// `unknown-conversation`, `name-taken`, `open-critical-risks`, `empty-reason`…
    public let kind: String
    public let task: String?
    public let name: String?
    public let session: String?
    public let reference: String?
    public let count: Int?
    public let reply: Int?
}

extension ServiceClient {

    /// Отдаёт решение службе и возвращает доску после него.
    public func decide(_ decision: OwnerDecision, context: String?) async throws(ServiceError) -> TaskBoard {
        try await ask("tasks", context: context, arguments: decision.arguments)
    }
}
