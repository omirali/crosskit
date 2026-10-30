import CrosskitKit
import Foundation
import Observation

/// Выбранное в списке задач.
enum TaskChoice: Hashable {
    /// Доска всех задач контекста.
    case board
    case confirmed(id: String)
    case candidate(index: Int)
}

/// Как показана карта задачи.
enum MapMode: String, CaseIterable, Identifiable {
    case line, conversations, risks
    var id: Self { self }
}

/// Узел, который показывает инспектор.
enum NodeFocus: Hashable {
    case step(Int)
    case risk(String)
}

/// Ответ, которого ждут, пришёл или не пришёл.
enum Loading<Value> {
    case idle
    case loading
    case loaded(Value)
    case failed(ServiceError)
}

/// Карта выбранной задачи и дерево, нарисованное из неё.
struct LoadedMap {
    let answer: TaskMapAnswer
    let tree: DecisionTree
}

/// Что окно сообщает человеку после решения или построения, которое не удалось.
struct Notice: Identifiable {
    let id = UUID()
    let title: String
    let explanation: String
}

/// Построение карты, которое окно запустило: ход команды и чем кончилось.
struct BuildRun {
    var lines: [String] = []
    var finished = false
    var failure: String?
}

/// Состояние окна. Всё, что окно показывает, приходит от службы. Решения владельца окно
/// отдаёт службе, а построение карты запускает командой: модель спрашивает она (ADR 0061).
@Observable
final class DesktopModel {

    private let client: ServiceClient

    private(set) var contexts: [String] = []
    var context: String?
    private(set) var board: Loading<TaskBoard> = .idle
    /// Что служба пишет о контексте, когда она его не отдаёт.
    private(set) var contextState: ContextState?
    var choice: TaskChoice? = .board
    private(set) var map: Loading<LoadedMap> = .idle
    var mode: MapMode = .line
    var showsRejected = true
    var focus: NodeFocus?
    var inspectorShown = true
    var notice: Notice?
    /// Лист, который окно сейчас показывает для решения: имя задачи или причина о риске.
    var nameRequest: NameRequest?
    var reasonRequest: ReasonRequest?
    private(set) var build: BuildRun?
    private var building: Task<Void, Never>?

    init(client: ServiceClient = .located()) {
        self.client = client
    }

    /// Контексты, затем задачи первого из них.
    func start() async {
        do throws(ServiceError) {
            contexts = try await client.contexts()
            if context == nil { context = contexts.first }
            await loadBoard()
        } catch {
            board = .failed(error)
        }
    }

    /// Задачи контекста. Пока служба контекст не отдаёт — поднимает граф или ждёт, — окно
    /// показывает её собственное объяснение и спрашивает снова, пока его не закроют.
    func loadBoard() async {
        board = .loading
        while !Task.isCancelled {
            do throws(ServiceError) {
                board = .loaded(try await client.taskBoard(context: context))
                contextState = nil
                if choice == nil { choice = .board }
                return
            } catch .refused("not-served") {
                board = .failed(.refused("not-served"))
                contextState = await client.state(of: context ?? contexts.first ?? "")
                try? await Task.sleep(for: Self.retryAfter)
            } catch {
                board = .failed(error)
                return
            }
        }
    }

    private static let retryAfter: Duration = .seconds(5)

    /// Карта выбранной подтверждённой задачи; у кандидата и доски карты нет. Узел в инспекторе
    /// остаётся тем же, если он есть и в новой карте: после решения о риске окно не прыгает.
    func loadMap() async {
        guard case .confirmed(let id) = choice else {
            map = .idle
            return
        }
        if case .loaded(let loaded) = map, loaded.answer.task != confirmedTask?.name {
            map = .loading
        } else if case .idle = map {
            map = .loading
        }
        do throws(ServiceError) {
            let answer = try await client.taskMap(id, context: context)
            let tree = DecisionTree(map: answer)
            map = .loaded(LoadedMap(answer: answer, tree: tree))
            if !Self.holds(tree, focus) {
                focus = tree.steps.first.map { .step($0.id) }
            }
        } catch {
            map = .failed(error)
        }
    }

    private static func holds(_ tree: DecisionTree, _ focus: NodeFocus?) -> Bool {
        switch focus {
        case .step(let id): tree.steps.indices.contains(id)
        case .risk(let id): tree.risks.contains { $0.id == id }
        case nil: false
        }
    }

    func reload() async {
        await loadBoard()
        await loadMap()
    }

    /// Отдаёт решение владельца службе. Правда после него — доска, которой служба ответила.
    func decide(_ decision: OwnerDecision) async {
        do throws(ServiceError) {
            let updated = try await client.decide(decision, context: context)
            board = .loaded(updated)
            switch decision {
            case .confirm(_, let name):
                choice = updated.confirmed.first { $0.name == name }.map { .confirmed(id: $0.id) } ?? .board
            case .reject:
                choice = .board
            default:
                break
            }
            await loadMap()
        } catch {
            let failure = Wording.failure(error)
            notice = Notice(title: failure.title, explanation: failure.explanation)
        }
    }

    // MARK: Построение карты

    /// Сколько примерно стоит достроить карту выбранной задачи.
    var estimate: BuildEstimate? {
        guard case .loaded(let loaded) = map else { return nil }
        return BuildEstimate(map: loaded.answer)
    }

    /// Достраивает карту выбранной задачи командой и потом читает её заново.
    func startBuild() {
        guard let task = confirmedTask, building == nil else { return }
        guard let program = MapBuild.program() else {
            notice = Notice(title: "Нечем строить",
                            explanation: "Окно строит карту программой службы, а служба не установлена: crosskit service install.")
            return
        }
        build = BuildRun()
        let context = context
        building = Task {
            do {
                for try await line in MapBuild.run(program: program, task: task.id, context: context) {
                    build?.lines.append(line)
                }
                build?.finished = true
                await reload()
            } catch let failure as MapBuild.Failure {
                build?.failure = failure.said.last ?? "команда закончилась с кодом \(failure.status)"
                build?.finished = true
            } catch {
                build?.failure = "\(error)"
                build?.finished = true
            }
            building = nil
        }
    }

    /// Останавливает построение. Окна, которые команда уже разметила, остаются в карте:
    /// следующее построение начнёт после них (ADR 0055).
    func cancelBuild() {
        building?.cancel()
        building = nil
        build = nil
        Task { await reload() }
    }

    func closeBuild() {
        build = nil
    }

    // MARK: Выбранное

    var confirmedTask: ConfirmedTask? {
        guard case .confirmed(let id) = choice, case .loaded(let board) = board else { return nil }
        return board.confirmed.first { $0.id == id }
    }

    var candidate: CandidateTask? {
        guard case .candidate(let index) = choice, case .loaded(let board) = board,
              board.candidates.indices.contains(index) else { return nil }
        return board.candidates[index]
    }
}
