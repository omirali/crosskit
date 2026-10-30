import CrosskitKit
import Foundation
import Observation

/// Что открыть в окне беседы: задачу, каталог и — для продолжения — сессию (ADR 0062).
struct ChatTarget: Codable, Hashable {
    let taskID: String
    let taskName: String
    let context: String?
    /// Где работает агент.
    let directory: String
    /// Продолжить эту беседу; ничего — начать новую с вводной задачи.
    let resume: String?
    /// Вводная задачи для новой беседы.
    let brief: String?
}

/// Одна строка ленты беседы.
enum ChatItem: Identifiable {
    case user(id: UUID, text: String)
    case assistant(id: UUID, text: String, streaming: Bool)
    case thinking(id: UUID, text: String)
    case tool(id: String, name: String, input: String, result: String?, failed: Bool)
    case permission(PermissionRequest, answer: Bool?)
    case note(id: UUID, text: String)

    var id: String {
        switch self {
        case .user(let id, _), .assistant(let id, _, _), .thinking(let id, _), .note(let id, _): id.uuidString
        case .tool(let id, _, _, _, _): "tool-" + id
        case .permission(let request, _): "permission-" + request.id
        }
    }
}

/// Беседа в окне: лента из потока `claude` и то, что человек в неё говорит. Окно не прячет ни
/// одного шага агента и ничего не разрешает за человека.
@Observable
final class ChatModel {

    enum State: Equatable {
        /// Ещё не начата: человек выбирает, с какими разрешениями.
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
    /// Вошла ли беседа в задачу: служба должна сначала её прочитать.
    private(set) var inTask: Bool
    var permissionMode = "default"
    var draft = ""

    private var chat: ChatSession?
    private var listening: Task<Void, Never>?
    private let client = ServiceClient.located()

    init(target: ChatTarget) {
        self.target = target
        self.inTask = target.resume != nil
    }

    func begin() {
        guard state == .ready else { return }
        let start = ChatSession.Start(directory: URL(fileURLWithPath: target.directory), resume: target.resume,
                                      brief: target.resume == nil ? target.brief : nil,
                                      permissionMode: permissionMode == "default" ? nil : permissionMode)
        let chat = ChatSession(start: start)
        self.chat = chat
        state = .waiting
        if target.resume == nil, target.brief != nil {
            note("Агент получил вводную задачи: основную линию, отказы и открытые риски.")
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

    func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, state == .waiting, let chat else { return }
        draft = ""
        items.append(.user(id: UUID(), text: text))
        state = .working
        Task { await chat.send(text) }
    }

    func answer(_ request: PermissionRequest, allow: Bool) {
        guard let index = items.firstIndex(where: { $0.id == "permission-" + request.id }) else { return }
        items[index] = .permission(request, answer: allow)
        Task { await chat?.answer(request, allow: allow) }
    }

    func stop() {
        Task { await chat?.interrupt() }
    }

    func end() {
        Task { await chat?.close() }
    }

    // MARK: Поток

    private func take(_ event: ChatEvent) {
        switch event {
        case .started(let session, let directory, let model):
            self.session = session
            note("Беседа \(session.prefix(8)) · \(directory)" + (model.map { " · \($0)" } ?? ""))
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
            items.append(.permission(request, answer: nil))
        case .unansweredRequest:
            break
        case .finished(let result):
            costUSD += result.costUSD ?? 0
            if result.isError { note("Ход закончился ошибкой: \(result.text ?? "без причины")") }
            state = .waiting
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
        guard !inTask, let session else { return }
        do throws(ServiceError) {
            _ = try await client.decide(.add(task: target.taskID, conversation: session), context: target.context)
            inTask = true
            note("Беседа вошла в задачу «\(target.taskName)».")
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
