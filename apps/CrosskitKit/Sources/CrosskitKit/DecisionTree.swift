import Foundation

/// Задача как дерево решений: от начала через развилки к HEAD (`docs/decision-map.md`).
///
/// Шаги основной линии берутся из сводки задачи, а ветки шага — из развилки карты беседы, на
/// которую сводка ссылается: сводка говорит, что решили, развилка — из чего выбирали и чьими
/// словами. Сводки нет — шагами становятся все развилки бесед по времени. Каждый риск висит на
/// шаге, после которого он появился, и открытый тянется до HEAD.
public struct DecisionTree: Sendable, Equatable {

    /// Где в беседе стоит узел: цитата, номер реплики и кто её сказал.
    public struct Quote: Sendable, Equatable, Hashable {
        public let text: String
        public let reply: Int
        public let owner: Bool
        public let conversation: String
        public let session: String
    }

    public struct Branch: Sendable, Equatable, Identifiable {
        public let id: Int
        public let option: String
        public let state: BranchState
        public let reason: String
        public let quote: Quote
    }

    public struct Step: Sendable, Equatable, Identifiable {
        public let id: Int
        /// Что решили — так это называет сводка. Без сводки — вопрос развилки.
        public let title: String
        /// Вопрос развилки, на которой это решали.
        public let question: String
        public let state: BranchState
        public let branches: [Branch]
        /// Ветка, которая ведёт основную линию дальше, если такая есть.
        public let lead: Branch?
        /// Все развилки бесед, из которых сведён шаг.
        public let quotes: [Quote]
    }

    public struct Risk: Sendable, Equatable, Identifiable {
        public let id: String
        public let title: String
        public let level: RiskLevel
        public let status: RiskStatus
        /// Находка вне задачи: на основной линии не висит.
        public let outside: Bool
        /// Шаг, после которого риск появился.
        public let step: Int
        public let quotes: [Quote]
        /// Слово владельца: принят или закрыт, почему и когда (ADR 0061).
        public let decided: Decided?

        /// В скольких беседах риск поднимался.
        public var raisedIn: Int { Set(quotes.map(\.session)).count }
        /// Открыт, пока ни карта, ни владелец его не закрыли.
        public var isOpen: Bool { decided == nil && status == .open }
        /// Статус с учётом слова владельца.
        public var shownStatus: RiskStatus { decided?.verdict ?? status }
        /// Открытый критичный риск, который виден на линии полосой до HEAD. Только critical:
        /// на настоящей задаче открытых high двенадцать, и столько полос закрыли бы подписи
        /// веток. High остаются метками у своего шага.
        public var pullsDown: Bool { !outside && isOpen && level == .critical }
    }

    public let task: String
    /// Первая беседа задачи — её начало.
    public let start: MapConversation?
    public let steps: [Step]
    public let risks: [Risk]
    /// Шаги сведены сводкой задачи, а не взяты из развилок бесед как есть.
    public let joined: Bool

    public init(map: TaskMapAnswer) {
        let reading = Reading(map: map)
        task = map.task
        start = map.conversations.first
        if let joinedMap = map.joined {
            joined = true
            steps = joinedMap.mainLine.enumerated().map { index, item in reading.step(index, from: item) }
            let positions = joinedMap.mainLine.map(reading.position(of:))
            risks = joinedMap.risks.enumerated().map { index, item in
                reading.risk("r\(index)", from: item, outside: false, positions: positions)
            } + joinedMap.findings.enumerated().map { index, item in
                reading.risk("f\(index)", from: item, outside: true, positions: positions)
            }
        } else {
            joined = false
            let forks = reading.forksInTime()
            steps = forks.enumerated().map { index, fork in reading.step(index, from: fork.fork, of: fork.conversation) }
            let positions = forks.map { Reading.Position(conversation: $0.conversation, reply: $0.fork.reply) }
            risks = reading.risksInTime().enumerated().map { index, risk in
                reading.risk(index, from: risk.risk, of: risk.conversation, positions: positions)
            }
        }
    }

    /// Последний шаг — где задача стоит сейчас.
    public var head: Int? { steps.indices.last }
    public var openRisks: [Risk] { risks.filter { !$0.outside && $0.isOpen } }
}

/// Чтение ответа ядра: как код узла сводки находит развилку или риск карты беседы.
private struct Reading {
    let map: TaskMapAnswer

    struct Position: Comparable {
        let conversation: Int
        let reply: Int

        static func < (lhs: Position, rhs: Position) -> Bool {
            (lhs.conversation, lhs.reply) < (rhs.conversation, rhs.reply)
        }
    }

    func conversationIndex(_ session: String) -> Int? {
        map.conversations.firstIndex { $0.session == session }
    }

    func quote(_ text: String, reply: Int, owner: Bool?, of conversation: Int) -> DecisionTree.Quote {
        let source = map.conversations[conversation]
        return DecisionTree.Quote(text: text, reply: reply, owner: owner ?? false,
                                  conversation: source.title, session: source.session)
    }

    func fork(at source: JoinedSource) -> (fork: MapFork, conversation: Int)? {
        guard let conversation = conversationIndex(source.session), let place = source.place,
              place.kind == "f", map.conversations[conversation].forks.indices.contains(place.index)
        else { return nil }
        return (map.conversations[conversation].forks[place.index], conversation)
    }

    func risk(at source: JoinedSource) -> (risk: MapRisk, conversation: Int)? {
        guard let conversation = conversationIndex(source.session), let place = source.place,
              place.kind == "r", map.conversations[conversation].risks.indices.contains(place.index)
        else { return nil }
        return (map.conversations[conversation].risks[place.index], conversation)
    }

    /// Самое раннее место пункта сводки в задаче.
    func position(of item: JoinedItem) -> Position? {
        item.sources.compactMap { source -> Position? in
            guard let conversation = conversationIndex(source.session) else { return nil }
            return Position(conversation: conversation, reply: source.reply ?? 0)
        }.min()
    }

    func step(_ index: Int, from item: JoinedItem) -> DecisionTree.Step {
        let forks = item.sources.compactMap { fork(at: $0) }
        let state = BranchState(rawValue: item.state) ?? .open
        guard let first = forks.first else {
            return DecisionTree.Step(id: index, title: item.title, question: item.title, state: state,
                                     branches: [], lead: nil, quotes: [])
        }
        let branches = branches(of: first.fork, in: first.conversation)
        let lead = branches.first { $0.state == state && state.leads } ?? branches.first { $0.state.leads }
        return DecisionTree.Step(
            id: index, title: item.title, question: first.fork.question, state: state, branches: branches, lead: lead,
            quotes: forks.map { quote($0.fork.quote, reply: $0.fork.reply, owner: $0.fork.owner, of: $0.conversation) })
    }

    func step(_ index: Int, from fork: MapFork, of conversation: Int) -> DecisionTree.Step {
        let branches = branches(of: fork, in: conversation)
        let lead = branches.first { $0.state.leads }
        return DecisionTree.Step(
            id: index, title: fork.question, question: fork.question,
            state: lead?.state ?? branches.first?.state ?? .open, branches: branches, lead: lead,
            quotes: [quote(fork.quote, reply: fork.reply, owner: fork.owner, of: conversation)])
    }

    func branches(of fork: MapFork, in conversation: Int) -> [DecisionTree.Branch] {
        fork.branches.enumerated().map { index, branch in
            DecisionTree.Branch(id: index, option: branch.option, state: branch.state, reason: branch.reason,
                                quote: quote(branch.quote, reply: branch.reply, owner: branch.owner, of: conversation))
        }
    }

    func risk(_ id: String, from item: JoinedItem, outside: Bool, positions: [Position?]) -> DecisionTree.Risk {
        let found = item.sources.compactMap { risk(at: $0) }
        let level = item.level ?? found.map(\.risk.level).min() ?? .info
        return DecisionTree.Risk(
            id: id, title: item.title, level: level, status: RiskStatus(rawValue: item.state) ?? .open,
            outside: outside, step: Self.step(at: position(of: item), among: positions),
            quotes: found.map { quote($0.risk.quote, reply: $0.risk.reply, owner: $0.risk.owner, of: $0.conversation) },
            decided: item.decided)
    }

    func risk(_ index: Int, from risk: MapRisk, of conversation: Int, positions: [Position?]) -> DecisionTree.Risk {
        DecisionTree.Risk(
            id: "r\(index)", title: risk.description, level: risk.level, status: risk.status, outside: !risk.inTask,
            step: Self.step(at: Position(conversation: conversation, reply: risk.reply), among: positions),
            quotes: [quote(risk.quote, reply: risk.reply, owner: risk.owner, of: conversation)],
            decided: risk.decided)
    }

    func forksInTime() -> [(fork: MapFork, conversation: Int)] {
        map.conversations.indices.flatMap { conversation in
            map.conversations[conversation].forks.sorted { $0.reply < $1.reply }.map { (fork: $0, conversation: conversation) }
        }
    }

    func risksInTime() -> [(risk: MapRisk, conversation: Int)] {
        map.conversations.indices.flatMap { conversation in
            map.conversations[conversation].risks.sorted { $0.reply < $1.reply }.map { (risk: $0, conversation: conversation) }
        }
    }

    /// Последний шаг, который начался не позже риска; риск раньше всех шагов — на первом.
    static func step(at position: Position?, among steps: [Position?]) -> Int {
        guard let position else { return 0 }
        return steps.indices.last { index in steps[index].map { $0 <= position } ?? false } ?? 0
    }
}
