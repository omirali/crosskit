import Foundation

/// Ответ `tasks map <задача> --json` (ADR 0055, 0056): карты бесед задачи и, если она собрана,
/// сводка задачи из них.
public struct TaskMapAnswer: Decodable, Sendable, Equatable {
    public let context: String
    public let task: String
    /// Версия вопроса, которым построены карты бесед.
    public let question: String
    public let joined: JoinedMap?
    /// В порядке времени, как их отдаёт ядро.
    public let conversations: [MapConversation]
}

public struct JoinedMap: Decodable, Sendable, Equatable {
    public let question: String
    /// Карты бесед выросли после сводки.
    public let stale: Bool
    public let mainLine: [JoinedItem]
    public let risks: [JoinedItem]
    public let findings: [JoinedItem]
}

public struct JoinedItem: Decodable, Sendable, Equatable {
    public let title: String
    /// Состояние ветки у шага основной линии, статус у риска.
    public let state: String
    public let level: RiskLevel?
    public let sources: [JoinedSource]
    /// Слово владельца о риске, сильнее `state` (ADR 0061).
    public let decided: Decided?
}

/// Что владелец решил о риске, когда и почему.
public struct Decided: Decodable, Sendable, Equatable, Hashable {
    /// `accepted` или `resolved`.
    public let verdict: RiskStatus
    public let reason: String
    public let atUnixMs: Int64

    public var at: Date { Date(unixMilliseconds: atUnixMs) }
}

/// Узел карты беседы, на котором стоит пункт сводки: `c0.f3` — развилка 3, `c1.r5` — риск 5.
public struct JoinedSource: Decodable, Sendable, Equatable {
    public let session: String
    public let node: String
    public let reply: Int?

    /// Номер узла в карте его беседы и чей он, по коду.
    var place: (kind: Character, index: Int)? {
        guard let dot = node.firstIndex(of: "."), node.index(after: dot) < node.endIndex else { return nil }
        let kind = node[node.index(after: dot)]
        guard let index = Int(node[node.index(dot, offsetBy: 2)...]) else { return nil }
        return (kind, index)
    }
}

public struct MapConversation: Decodable, Sendable, Equatable, Identifiable {
    public let session: String
    public let name: String?
    public let firstAtUnixMs: Int64?
    public let replies: Int
    /// Последняя реплика, которую покрывает карта.
    public let mappedThrough: Int
    public let forks: [MapFork]
    public let risks: [MapRisk]
    /// Узлов, которые доказала цитата, и отброшенных.
    public let kept: Int
    public let dropped: Int

    public var id: String { session }
    public var title: String { name ?? session }
    public var firstAt: Date? { firstAtUnixMs.map(Date.init(unixMilliseconds:)) }
}

public struct MapFork: Decodable, Sendable, Equatable {
    public let question: String
    public let reply: Int
    /// Реплику сказал владелец. Ядро до службы `task-map` этого не отдавало.
    public let owner: Bool?
    public let quote: String
    public let branches: [MapBranch]
}

public struct MapBranch: Decodable, Sendable, Equatable {
    public let option: String
    public let state: BranchState
    public let reason: String
    public let reply: Int
    public let owner: Bool?
    public let quote: String
}

public struct MapRisk: Decodable, Sendable, Equatable {
    public let description: String
    public let level: RiskLevel
    public let status: RiskStatus
    public let inTask: Bool
    public let reply: Int
    public let owner: Bool?
    public let quote: String
    public let decided: Decided?
}

/// Что стало с веткой на развилке (`docs/decision-map.md`, раздел 3).
public enum BranchState: String, Decodable, Sendable, CaseIterable {
    case chosen
    case rejected
    case deferred
    case open
    case agentDefault = "agent_default"
    case abandoned

    /// Ветка, которая ведёт основную линию дальше: выбор владельца или агента.
    public var leads: Bool { self == .chosen || self == .agentDefault }
}

/// Меньше — серьёзнее: по возрастанию идут от `critical` к `info`.
public enum RiskLevel: String, Decodable, Sendable, CaseIterable, Comparable {
    case critical, high, medium, info

    public static func < (lhs: RiskLevel, rhs: RiskLevel) -> Bool {
        lhs.rank < rhs.rank
    }

    private var rank: Int {
        switch self {
        case .critical: 0
        case .high: 1
        case .medium: 2
        case .info: 3
        }
    }
}

public enum RiskStatus: String, Decodable, Sendable, CaseIterable {
    case open, accepted, resolved
}
