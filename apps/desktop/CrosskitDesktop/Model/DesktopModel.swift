import CrosskitKit
import Foundation
import Observation

/// Что открыто в окне: проект целиком или одна задача (макет `design/UXMain.dc.html`).
enum Scope: Hashable {
    /// Проект по своему корню (`Project.root`).
    case project(String)
    case task(String)
}

/// Вкладка средней части окна. Карта — главная, остальное — расширения вокруг задачи.
enum Pane: String, CaseIterable, Identifiable {
    case map, board, changes, files, browser
    var id: Self { self }
}

/// Ответ, которого ждут, пришёл или не пришёл.
enum Loading<Value> {
    case idle
    case loading
    case loaded(Value)
    case failed(ServiceError)

    var value: Value? {
        if case .loaded(let value) = self { value } else { nil }
    }
}

/// Карта задачи и дерево, нарисованное из неё.
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

/// Состояние окна. Всё, что окно показывает о задачах, приходит от службы; решения владельца
/// окно отдаёт ей же, карту достраивает командой, а беседы с агентом ведёт само (ADR 0061, 0062).
@Observable
final class DesktopModel {

    let client: ServiceClient

    private(set) var contexts: [String] = []
    var context: String?
    private(set) var board: Loading<TaskBoard> = .idle
    /// Что служба пишет о контексте, когда она его не отдаёт.
    private(set) var contextState: ContextState?
    private(set) var projects: [Project] = []

    var scope: Scope?
    var pane: Pane = .map
    var showsChats = false
    /// Узел карты, выбранный человеком или переходом из чата.
    var focus: MapAnchor?
    /// Беседа в чате справа: сессия сохранённой беседы или ключ новой (`LiveChats`).
    var chatKey: String?
    /// Реплика, к которой чат прокручен; ничего — конец беседы.
    var target: Int?

    /// Карты и реплики задач по их id: окно помнит открытые, чтобы переход не ждал службу.
    private(set) var maps: [String: Loading<LoadedMap>] = [:]
    private(set) var speeches: [String: Loading<TaskSpeech>] = [:]

    let live = LiveChats()
    let inbox = InboxState()
    let composer = ComposerState()

    var notice: Notice?
    var nameRequest: NameRequest?
    var reasonRequest: ReasonRequest?
    private(set) var build: BuildRun?
    private var building: Task<Void, Never>?

    init(client: ServiceClient = .located()) {
        self.client = client
    }

    // MARK: Служба

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
        if board.value == nil { board = .loading }
        while !Task.isCancelled {
            do throws(ServiceError) {
                take(try await client.taskBoard(context: context))
                contextState = nil
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

    private func take(_ answer: TaskBoard) {
        board = .loaded(answer)
        projects = ProjectGrouping.projects(of: answer)
        if let scope, !holds(scope) { self.scope = nil }
        if scope == nil { scope = Self.firstScope(of: answer, in: projects) }
    }

    /// Начало дня — задача, которая ждёт решения, иначе последняя в работе, иначе первый проект.
    private static func firstScope(of board: TaskBoard, in projects: [Project]) -> Scope? {
        let open = board.confirmed.filter { $0.finishedAtUnixMs == nil }
        if let waiting = open.first(where: { $0.column == .waiting }) ?? open.first { return .task(waiting.id) }
        return projects.first.map { .project($0.root) }
    }

    private func holds(_ scope: Scope) -> Bool {
        switch scope {
        case .task(let id): task(id) != nil
        case .project(let root): projects.contains { $0.root == root }
        }
    }

    /// Карта и реплики задачи. Узел, выбранный до обновления, остаётся, если он есть и в новой
    /// карте: после решения о риске окно не прыгает.
    func loadTask(_ id: String) async {
        guard let task = task(id) else { return }
        if maps[id]?.value == nil { maps[id] = .loading }
        if speeches[id]?.value == nil { speeches[id] = .loading }
        let (client, context, key) = (client, context, task.id)
        async let mapAnswer = Self.attempt { () async throws(ServiceError) in try await client.taskMap(key, context: context) }
        async let speechAnswer = Self.attempt { () async throws(ServiceError) in try await client.taskSpeech(key, context: context) }
        switch await mapAnswer {
        case .success(let answer): maps[id] = .loaded(LoadedMap(answer: answer, tree: DecisionTree(map: answer)))
        case .failure(let error): maps[id] = .failed(error)
        }
        switch await speechAnswer {
        case .success(let speech): speeches[id] = .loaded(speech)
        case .failure(let error): speeches[id] = .failed(error)
        }
        if let tree = tree(of: id), let focus, tree.source(of: focus) == nil { self.focus = nil }
    }

    /// Карта и реплики спрашиваются разом: служба отвечает на них параллельно.
    nonisolated private static func attempt<Answer: Sendable>(
        _ ask: @Sendable () async throws(ServiceError) -> Answer
    ) async -> Result<Answer, ServiceError> {
        do throws(ServiceError) {
            return .success(try await ask())
        } catch {
            return .failure(error)
        }
    }

    func reload() async {
        await loadBoard()
        if let id = taskID { await loadTask(id) }
    }

    /// Новый контекст — свои проекты и задачи: старое выбранное к нему не относится.
    func switchContext(_ name: String) {
        guard name != context else { return }
        context = name
        scope = nil
        maps = [:]
        speeches = [:]
        focus = nil
        chatKey = nil
        target = nil
        Task { await loadBoard() }
    }

    /// Отдаёт решение владельца службе. Правда после него — доска, которой служба ответила.
    func decide(_ decision: OwnerDecision) async {
        do throws(ServiceError) {
            let updated = try await client.decide(decision, context: context)
            take(updated)
            if case .confirm(_, let name) = decision, let task = updated.confirmed.first(where: { $0.name == name }) {
                open(.task(task.id))
            }
            if let id = taskID { await loadTask(id) }
        } catch {
            let failure = Wording.failure(error)
            notice = Notice(title: failure.title, explanation: failure.explanation)
        }
    }

    // MARK: Выбор

    /// Открывает задачу или проект: чат — на последней беседе, карта — без выбранного узла.
    func open(_ scope: Scope) {
        guard scope != self.scope else { return }
        self.scope = scope
        focus = nil
        target = nil
        chatKey = nil
    }

    /// Узел карты: чат открывается на реплике, которой он доказан.
    func select(_ anchor: MapAnchor, in taskID: String) {
        if scope != .task(taskID) { open(.task(taskID)) }
        focus = anchor
        if let quote = tree(of: taskID)?.source(of: anchor) {
            show(quote)
        }
    }

    /// Чат на реплике цитаты.
    func show(_ quote: DecisionTree.Quote) {
        chatKey = quote.session
        target = quote.reply
    }

    var taskID: String? {
        if case .task(let id) = scope { id } else { nil }
    }

    var project: Project? {
        switch scope {
        case .project(let root): projects.first { $0.root == root }
        case .task(let id): projects.first { $0.taskIDs.contains(id) }
        case nil: nil
        }
    }

    func task(_ id: String) -> ConfirmedTask? {
        board.value?.confirmed.first { $0.id == id }
    }

    var currentTask: ConfirmedTask? { taskID.flatMap(task) }

    func tree(of taskID: String) -> DecisionTree? {
        maps[taskID]?.value?.tree
    }

    func map(of taskID: String) -> Loading<LoadedMap> {
        maps[taskID] ?? .idle
    }

    func speech(of taskID: String) -> Loading<TaskSpeech> {
        speeches[taskID] ?? .idle
    }

    /// Задача, в которую входит беседа.
    func taskID(of session: String) -> String? {
        board.value?.confirmed.first { $0.conversations.contains { $0.session == session } }?.id
    }

    /// Беседы того, что открыто: задачи или всех задач проекта, последние сверху.
    var scopeConversations: [TaskConversation] {
        let tasks: [ConfirmedTask] = switch scope {
        case .task(let id): task(id).map { [$0] } ?? []
        case .project: (project?.taskIDs ?? []).compactMap(task)
        case nil: []
        }
        return tasks.flatMap(\.conversations).sorted { ($0.lastAtUnixMs ?? 0) > ($1.lastAtUnixMs ?? 0) }
    }

    /// Беседа в чате: выбранная, иначе последняя беседа того, что открыто.
    var shownChatKey: String? {
        if let chatKey, scopeConversations.contains(where: { $0.session == chatKey }) || live.chat(chatKey) != nil {
            return chatKey
        }
        return scopeConversations.first?.session
    }

    func conversation(_ session: String) -> TaskConversation? {
        board.value?.confirmed.lazy.flatMap(\.conversations).first { $0.session == session }
    }

    /// Каталоги, где шли беседы того, что открыто, — от последней.
    var scopeDirectories: [String] {
        var seen: Set<String> = []
        return scopeConversations.compactMap(\.workingDirectory).filter { seen.insert($0).inserted }
    }

    // MARK: Построение карты

    /// Сколько примерно стоит достроить карту выбранной задачи.
    var estimate: BuildEstimate? {
        taskID.flatMap { maps[$0]?.value }.map { BuildEstimate(map: $0.answer) }
    }

    /// Достраивает карту выбранной задачи командой и потом читает её заново.
    func startBuild() {
        guard let task = currentTask, building == nil else { return }
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
}
