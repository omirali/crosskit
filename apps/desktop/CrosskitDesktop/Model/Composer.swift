import CrosskitKit
import Foundation
import Observation

/// Строка ввода под чатом и что выбрано рядом с ней: агент, модель, разрешения, быстрый вопрос.
@Observable
final class ComposerState {
    var draft = ""
    /// Модель новой беседы; ничего — как в настройках Claude Code.
    var model: String?
    /// Что агенту можно без спроса в новой беседе.
    var permissionMode = "default"
    /// Следующее слово — быстрый вопрос: ответ исчезнет сам и не попадёт ни в беседы, ни на карту.
    var quickOn = false
    /// Через сколько секунд ответ быстрого вопроса исчезнет.
    var quickSeconds = 30
    var quick: QuickRun?
    /// Следующее слово начнёт новую беседу, а не продолжит открытую.
    var startsNew = false
    /// Слово, которое ждёт, пока человек выберет каталог новой беседы.
    var waitingForDirectory: String?

    static let models: [(id: String?, name: String, note: String)] = [
        (nil, "Как в настройках", "модель из настроек Claude Code"),
        ("opus", "Opus", "самая сильная, дороже и медленнее"),
        ("sonnet", "Sonnet", "баланс"),
        ("haiku", "Haiku", "быстрая и дешёвая"),
    ]

    static let modes: [(id: String, name: String, short: String, note: String)] = [
        ("default", "Спрашивать перед правками", "Спрашивать", "каждая правка и команда ждёт «Разрешить»"),
        ("acceptEdits", "Принимать правки", "Без спроса", "правки файлов — без спроса, команды спросит"),
        ("plan", "Только план", "Только план", "агент ничего не меняет"),
    ]
}

/// Быстрый вопрос: что спросили, что ответили и когда ответ исчезнет.
struct QuickRun: Identifiable {
    let id = UUID()
    let question: String
    var answer: String?
    var failure: String?
    var costUSD: Double?
    /// Когда пришёл ответ: от него считается, сколько ему осталось.
    var answeredAt: Date?
    var kept = false
}

extension DesktopModel {

    /// Слово из строки ввода: быстрый вопрос, слово живой беседе или начало новой.
    func send() {
        let text = composer.draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        if composer.quickOn {
            ask(text)
            return
        }
        if !composer.startsNew, let key = shownChatKey, let chat = live.chat(key) {
            guard chat.state == .waiting else { return }
            chat.send(text)
            composer.draft = ""
            return
        }
        startChat(saying: text, in: nil)
    }

    /// Начинает беседу с первым словом. Сохранённую беседу Claude Code окно продолжает в её
    /// каталоге; беседу Codex продолжить нельзя, и начинается новая в той же задаче.
    func startChat(saying text: String, in directory: String?) {
        let stored = composer.startsNew ? nil : shownChatKey.flatMap(conversation)
        let resumable = stored.flatMap { $0.platform == .claudeCode && $0.workingDirectory != nil ? $0 : nil }
        let taskID = stored.flatMap { taskID(of: $0.session) } ?? self.taskID
        guard let directory = directory ?? resumable?.workingDirectory ?? taskDirectory(taskID) ?? scopeDirectories.first else {
            composer.waitingForDirectory = text
            return
        }
        let task = taskID.flatMap(task)
        let target = ChatTarget(taskID: task?.id, taskName: task?.name, context: context, directory: directory,
                                resume: resumable?.session, brief: taskID.flatMap(tree(of:)).map(TaskBrief.of),
                                model: composer.model, permissionMode: composer.permissionMode)
        let key = resumable?.session ?? LiveChats.newKey()
        let before = resumable.flatMap { stored in
            taskID.flatMap { speech(of: $0).value?.conversation(stored.session)?.replies.count }
        } ?? 0
        let chat = ChatModel(target: target, repliesBefore: before)
        chat.onAttention = { [weak self, weak chat] attention in
            guard let self, let chat else { return }
            inbox.announce(attention, in: chat, place: place(of: chat))
        }
        live.start(key, chat)
        chatKey = key
        self.target = nil
        composer.startsNew = false
        composer.draft = ""
        chat.send(text)
    }

    /// Каталог последней беседы задачи, чей журнал ещё на диске.
    func taskDirectory(_ taskID: String?) -> String? {
        taskID.flatMap(task)?.conversations
            .filter { $0.workingDirectory != nil }
            .max { ($0.lastAtUnixMs ?? 0) < ($1.lastAtUnixMs ?? 0) }?
            .workingDirectory
    }

    /// Где идёт беседа, словами: контекст · проект · задача.
    func place(of chat: ChatModel) -> String {
        let project = projects.first { $0.root == ProjectGrouping.gitRoot(chat.target.directory) }?.name
            ?? URL(fileURLWithPath: chat.target.directory).lastPathComponent
        return [context, project, chat.target.taskName].compactMap { $0 }.joined(separator: " · ")
    }

    /// Новая беседа в том, что открыто: следующее слово её начнёт.
    func composeNew() {
        composer.startsNew = true
        composer.quickOn = false
        chatKey = nil
        target = nil
    }

    // MARK: Быстрый вопрос

    private func ask(_ question: String) {
        let directory = shownChatKey.flatMap(conversation)?.workingDirectory ?? taskDirectory(taskID)
            ?? scopeDirectories.first ?? NSHomeDirectory()
        let run = QuickRun(question: question)
        composer.quick = run
        composer.draft = ""
        Task {
            do {
                let answer = try await QuickQuestion.ask(question, in: URL(fileURLWithPath: directory))
                guard composer.quick?.id == run.id else { return }
                composer.quick?.answer = answer.text
                composer.quick?.costUSD = answer.costUSD
            } catch let failure as QuickQuestion.Failure {
                composer.quick?.failure = failure.said
            } catch {
                composer.quick?.failure = "\(error)"
            }
            composer.quick?.answeredAt = .now
        }
    }

    /// Ответ, срок которого вышел, исчезает. Спрашивается раз в полсекунды, пока ответ на экране.
    func expireQuick(at now: Date) {
        guard let quick = composer.quick, !quick.kept, let answered = quick.answeredAt else { return }
        if now.timeIntervalSince(answered) >= Double(composer.quickSeconds) { composer.quick = nil }
    }
}
