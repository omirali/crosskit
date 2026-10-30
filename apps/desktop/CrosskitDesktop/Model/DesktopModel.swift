import CrosskitKit
import Observation

/// Выбранное в списке задач.
enum TaskChoice: Hashable {
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

/// Состояние окна. Всё, что окно показывает, приходит от службы; окно ничего не строит и ничего
/// не пишет (этап D-1 — только чтение).
@Observable
final class DesktopModel {

    private let client: ServiceClient

    private(set) var contexts: [String] = []
    var context: String?
    private(set) var board: Loading<TaskBoard> = .idle
    /// Что служба пишет о контексте, когда она его не отдаёт.
    private(set) var contextState: ContextState?
    var choice: TaskChoice?
    private(set) var map: Loading<LoadedMap> = .idle
    var mode: MapMode = .line
    var showsRejected = true
    var focus: NodeFocus?
    var inspectorShown = true

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
                let loaded = try await client.taskBoard(context: context)
                board = .loaded(loaded)
                contextState = nil
                if choice == nil, let first = loaded.confirmed.first {
                    choice = .confirmed(id: first.id)
                }
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

    /// Карта выбранной подтверждённой задачи; у кандидата карты нет.
    func loadMap() async {
        guard case .confirmed(let id) = choice else {
            map = .idle
            return
        }
        map = .loading
        do throws(ServiceError) {
            let answer = try await client.taskMap(id, context: context)
            let tree = DecisionTree(map: answer)
            map = .loaded(LoadedMap(answer: answer, tree: tree))
            focus = tree.steps.first.map { .step($0.id) }
        } catch {
            map = .failed(error)
        }
    }

    func reload() async {
        await loadBoard()
        await loadMap()
    }

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
