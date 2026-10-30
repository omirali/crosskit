import CrosskitKit
import Foundation
import Observation

/// С чем начать беседу с агентом: где, что продолжить и что агент узнает до первого слова
/// (ADR 0062).
struct ChatTarget: Hashable {
    /// Задача, в которую беседа войдёт; ничего — беседа проекта без задачи.
    let taskID: String?
    let taskName: String?
    let context: String?
    /// Где работает агент.
    let directory: String
    /// Продолжить эту беседу; ничего — начать новую.
    let resume: String?
    /// Карта задачи для агента: основная линия, отказы, риски.
    let brief: String?
    /// `opus`, `sonnet`, `haiku`; ничего — как в настройках Claude Code.
    let model: String?
    /// `default`, `acceptEdits` или `plan`.
    let permissionMode: String
}

/// Одна строка ленты живой беседы.
enum ChatItem: Identifiable {
    case user(id: UUID, text: String)
    case assistant(id: UUID, text: String, streaming: Bool)
    case thinking(id: UUID, text: String)
    case tool(id: String, name: String, input: String, result: String?, failed: Bool)
    case permission(PermissionRequest, answer: PermissionAnswer?)
    case note(id: UUID, text: String)

    var id: String {
        switch self {
        case .user(let id, _), .assistant(let id, _, _), .thinking(let id, _), .note(let id, _): id.uuidString
        case .tool(let id, _, _, _, _): "tool-" + id
        case .permission(let request, _): "permission-" + request.id
        }
    }
}

/// Как человек ответил на запрос разрешения.
enum PermissionAnswer: Equatable {
    case allowed
    /// Разрешено, и правки файлов в этом чате больше не спрашиваются.
    case allowedEdits
    case denied
}

/// Что в беседе требует человека: окно говорит об этом и тогда, когда смотрят в другое место.
enum ChatAttention {
    case permission(PermissionRequest)
    case turnFinished(String?)
}

/// Беседа с агентом: лента из потока `claude` и то, что человек в неё говорит. Окно не прячет
/// ни одного шага агента и ничего не разрешает за человека — кроме правок, которые он сам
/// разрешил «в этом чате».
@Observable
final class ChatModel {

    enum State: Equatable {
        /// Процесс ещё не запущен: запустится с первым словом.
        case ready
        /// Агент отвечает.
        case working
        /// Ход закончен, агент ждёт слова.
        case waiting
        /// Беседа закончена.
        case ended(String?)
    }

    let target: ChatTarget
    private(set) var items: [ChatItem] = []
    private(set) var state: State = .ready
    private(set) var session: String?
    private(set) var costUSD = 0.0
    /// Сколько ходов агент закончил: каждый — повод сказать человеку.
    private(set) var turns = 0
    /// Вошла ли беседа в задачу: служба должна сначала её прочитать.
    private(set) var inTask: Bool
    /// Правки файлов разрешены человеком до конца беседы.
    private(set) var allowsEdits = false
    /// Сколько реплик сохранённой беседы было, когда окно её продолжило: дальше лента своя.
    let repliesBefore: Int
    var onAttention: ((ChatAttention) -> Void)?

    private var chat: ChatSession?
    private var listening: Task<Void, Never>?
    private let client = ServiceClient.located()

    static let editTools: Set<String> = ["Edit", "MultiEdit", "Write", "NotebookEdit"]

    init(target: ChatTarget, repliesBefore: Int = 0) {
        self.target = target
        self.repliesBefore = repliesBefore
        self.inTask = target.resume != nil || target.taskID == nil
    }

    /// Слово человека; первое запускает `claude`, который до него молчит.
    func send(_ text: String) {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        if state == .ready { begin() }
        guard state == .waiting, let chat else { return }
        items.append(.user(id: UUID(), text: text))
        state = .working
        Task { await chat.send(text) }
    }

    private func begin() {
        let start = ChatSession.Start(directory: URL(fileURLWithPath: target.directory), resume: target.resume,
                                      brief: target.brief,
                                      permissionMode: target.permissionMode == "default" ? nil : target.permissionMode,
                                      model: target.model)
        let chat = ChatSession(start: start)
        self.chat = chat
        state = .waiting
        if target.brief != nil {
            note("Агент получил карту задачи как вводную: основную линию, отказы и открытые риски.")
        }
        listening = Task {
            do {
                for try await event in await chat.events() {
                    take(event)
                }
                state = .ended(nil)
            } catch let failure as ChatSession.Failure {
                state = .ended(failure.said.last ?? "claude закончил с кодом \(failure.status)")
            } catch {
                state = .ended("\(error)")
            }
        }
    }

    func answer(_ request: PermissionRequest, _ answer: PermissionAnswer) {
        guard let index = items.firstIndex(where: { $0.id == "permission-" + request.id }) else { return }
        items[index] = .permission(request, answer: answer)
        if answer == .allowedEdits { allowsEdits = true }
        Task { await chat?.answer(request, allow: answer != .denied) }
    }

    var pendingPermissions: [PermissionRequest] {
        items.compactMap { item in
            if case .permission(let request, nil) = item { request } else { nil }
        }
    }

    func stop() {
        Task { await chat?.interrupt() }
    }

    func end() {
        listening?.cancel()
        Task { await chat?.close() }
    }

    /// Последний текст агента — что показать в уведомлении о конце хода.
    var lastText: String? {
        for item in items.reversed() {
            if case .assistant(_, let text, _) = item { return text }
        }
        return nil
    }

    // MARK: Поток

    private func take(_ event: ChatEvent) {
        switch event {
        case .started(let session, _, _):
            self.session = session
        case .text(let piece):
            if case .assistant(let id, let text, true)? = items.last {
                items[items.count - 1] = .assistant(id: id, text: text + piece, streaming: true)
            } else {
                items.append(.assistant(id: UUID(), text: piece, streaming: true))
            }
            state = .working
        case .thinking(let piece):
            if case .thinking(let id, let text)? = items.last {
                items[items.count - 1] = .thinking(id: id, text: text + piece)
            } else {
                items.append(.thinking(id: UUID(), text: piece))
            }
        case .message(let blocks):
            for block in blocks { settle(block) }
        case .toolResult(let id, let text, let failed):
            if let index = items.firstIndex(where: { $0.id == "tool-" + id }),
               case .tool(_, let name, let input, _, _) = items[index] {
                items[index] = .tool(id: id, name: name, input: input, result: text, failed: failed)
            }
        case .permission(let request):
            if allowsEdits, Self.editTools.contains(request.tool) {
                items.append(.permission(request, answer: .allowedEdits))
                Task { await chat?.answer(request, allow: true) }
            } else {
                items.append(.permission(request, answer: nil))
                onAttention?(.permission(request))
            }
        case .unansweredRequest:
            break
        case .finished(let result):
            costUSD += result.costUSD ?? 0
            if result.isError { note("Ход закончился ошибкой: \(result.text ?? "без причины")") }
            state = .waiting
            turns += 1
            onAttention?(.turnFinished(lastText))
            Task { await joinTask() }
        }
    }

    /// Сообщение целиком: текст, который шёл кусками, становится окончательным, а вызовы
    /// инструментов встают в ленту.
    private func settle(_ block: ChatBlock) {
        switch block {
        case .text(let text):
            if case .assistant(let id, _, true)? = items.last {
                items[items.count - 1] = .assistant(id: id, text: text, streaming: false)
            } else {
                items.append(.assistant(id: UUID(), text: text, streaming: false))
            }
        case .thinking:
            break
        case .toolUse(let id, let name, let input):
            if !items.contains(where: { $0.id == "tool-" + id }) {
                items.append(.tool(id: id, name: name, input: input, result: nil, failed: false))
            }
        }
    }

    /// Новая беседа входит в задачу, как только служба её прочитала: до этого книга задач её
    /// не знает и отказывает, и окно спросит снова после следующего хода.
    private func joinTask() async {
        guard !inTask, let session, let taskID = target.taskID else { return }
        do throws(ServiceError) {
            _ = try await client.decide(.add(task: taskID, conversation: session), context: target.context)
            inTask = true
            note("Беседа вошла в задачу «\(target.taskName ?? "")».")
        } catch .declined(let declined) where declined.kind == "unknown-conversation" {
            return
        } catch {
            note("Беседа не вошла в задачу: \(Wording.failure(error).explanation)")
            inTask = true
        }
    }

    private func note(_ text: String) {
        items.append(.note(id: UUID(), text: text))
    }
}

/// Живые беседы окна. Продолжение сохранённой беседы живёт под её сессией, новая — под своим
/// ключом, пока служба её не прочитала.
@Observable
final class LiveChats {

    private(set) var chats: [String: ChatModel] = [:]
    /// Ключи в порядке начала: новые беседы в списке — сверху.
    private(set) var order: [String] = []

    /// Беседа по ключу или по сессии, которую `claude` ей назвал.
    func chat(_ key: String) -> ChatModel? {
        chats[key] ?? chats.values.first { $0.session == key }
    }

    func start(_ key: String, _ chat: ChatModel) {
        chats[key]?.end()
        chats[key] = chat
        order.removeAll { $0 == key }
        order.insert(key, at: 0)
    }

    /// Новые беседы, которых ещё нет среди сохранённых.
    func fresh(known: Set<String>) -> [(key: String, chat: ChatModel)] {
        order.compactMap { key in
            guard let chat = chats[key], chat.target.resume == nil, !known.contains(chat.session ?? key) else { return nil }
            return (key, chat)
        }
    }

    var all: [(key: String, chat: ChatModel)] {
        order.compactMap { key in chats[key].map { (key, $0) } }
    }

    static func newKey() -> String { "new-" + UUID().uuidString }
}
