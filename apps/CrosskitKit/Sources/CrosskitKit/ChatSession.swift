import Foundation

/// Беседа с установленным `claude`, которую ведёт окно (ADR 0062).
///
/// Это настоящая беседа Claude Code: с настройками, правилами и MCP владельца, в рабочем каталоге
/// задачи, и её журнал сохраняется, как у беседы из терминала, — поэтому служба прочитает её и
/// сшивка найдёт её задачу. Окно не прячет ни одного шага: текст идёт кусками по мере ответа,
/// вызовы инструментов и их результаты видны, а разрешения спрашивает человек — CLI задаёт этот
/// вопрос в том же потоке (`--permission-prompt-tool stdio`), и окно отвечает за человека только
/// его словом.
public actor ChatSession {

    /// Как начать беседу.
    public struct Start: Sendable, Equatable {
        /// Где работает агент.
        public var directory: URL
        /// Продолжить эту сессию, а не начать новую.
        public var resume: String?
        /// Что агент узнаёт о задаче до первого слова: основная линия, отказы, риски.
        public var brief: String?
        /// `default`, `acceptEdits` или `plan`; ничего — как в настройках владельца.
        public var permissionMode: String?
        public var model: String?

        public init(directory: URL, resume: String? = nil, brief: String? = nil,
                    permissionMode: String? = nil, model: String? = nil) {
            self.directory = directory
            self.resume = resume
            self.brief = brief
            self.permissionMode = permissionMode
            self.model = model
        }

        /// Аргументы `claude`.
        var arguments: [String] {
            var arguments = ["-p", "--input-format", "stream-json", "--output-format", "stream-json",
                             "--verbose", "--include-partial-messages", "--permission-prompt-tool", "stdio"]
            if let resume { arguments += ["--resume", resume] }
            if let brief, !brief.isEmpty { arguments += ["--append-system-prompt", brief] }
            if let permissionMode { arguments += ["--permission-mode", permissionMode] }
            if let model { arguments += ["--model", model] }
            return arguments
        }
    }

    /// Почему беседа кончилась раньше времени.
    public struct Failure: Error, Sendable, Equatable {
        public let status: Int32
        public let said: [String]
    }

    private let program: String
    private let start: Start
    private var process: Process?
    private var input: FileHandle?

    /// `program` — имя или путь `claude`; ищется по PATH login-оболочки.
    public init(start: Start, program: String = "claude") {
        self.start = start
        self.program = program
    }

    /// Запускает `claude` и отдаёт его события, пока процесс жив. Отмена потока его
    /// останавливает.
    public func events() -> AsyncThrowingStream<ChatEvent, Error> {
        let process = LoginShell.process(running: program, with: start.arguments, in: start.directory)
        var environment = ProcessInfo.processInfo.environment
        // Окно, запущенное из беседы Claude Code, не должно выдать себя за неё.
        environment.removeValue(forKey: "CLAUDECODE")
        process.environment = environment
        let stdin = Pipe(), stdout = Pipe(), stderr = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = stderr
        self.process = process
        self.input = stdin.fileHandleForWriting

        return AsyncThrowingStream { continuation in
            // Отдельно от actor: ожидание выхода процесса блокирует, а actor в это время
            // отвечает на «отправить», «разрешить» и «остановить».
            let reading = Task.detached {
                do {
                    for try await line in stdout.fileHandleForReading.bytes.lines {
                        for event in ChatEvent.parse(Data(line.utf8)) {
                            if case .unansweredRequest(let id, let subtype) = event {
                                await self.decline(id, subtype)
                            }
                            continuation.yield(event)
                        }
                    }
                } catch {}
                process.waitUntilExit()
                let said = String(decoding: stderr.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                    .split(separator: "\n").suffix(8).map(String.init)
                if process.terminationStatus == 0 || process.terminationReason == .uncaughtSignal {
                    continuation.finish()
                } else {
                    continuation.finish(throwing: Failure(status: process.terminationStatus, said: said))
                }
            }
            continuation.onTermination = { _ in
                if process.isRunning { process.terminate() }
                reading.cancel()
            }
            do {
                try process.run()
            } catch {
                continuation.finish(throwing: Failure(status: -1, said: ["\(error)"]))
            }
        }
    }

    /// Слово человека агенту.
    public func send(_ text: String) {
        write(["type": "user", "message": ["role": "user", "content": [["type": "text", "text": text]]]])
    }

    /// Ответ на просьбу инструмента: «можно» возвращает его вход без правок, «нельзя» — с
    /// причиной, которую увидит агент.
    public func answer(_ request: PermissionRequest, allow: Bool, reason: String? = nil) {
        let decision: [String: Any] = allow
            ? ["behavior": "allow",
               "updatedInput": (try? JSONSerialization.jsonObject(with: Data(request.input.utf8))) ?? [String: Any]()]
            : ["behavior": "deny", "message": reason ?? "Владелец не разрешил."]
        write(["type": "control_response",
               "response": ["subtype": "success", "request_id": request.id, "response": decision]])
    }

    /// Останавливает ход агента; беседа остаётся и ждёт следующего слова.
    public func interrupt() {
        write(["type": "control_request", "request_id": UUID().uuidString, "request": ["subtype": "interrupt"]])
    }

    /// Заканчивает беседу: CLI дописывает журнал и выходит.
    public func close() {
        try? input?.close()
        input = nil
    }

    /// Запрос, которого окно не умеет, получает отказ: иначе CLI ждал бы ответа вечно.
    private func decline(_ id: String, _ subtype: String) {
        write(["type": "control_response",
               "response": ["subtype": "error", "request_id": id, "error": "Окно Crosskit не отвечает на \(subtype)."]])
    }

    private func write(_ message: [String: Any]) {
        guard let input, let data = try? JSONSerialization.data(withJSONObject: message) else { return }
        try? input.write(contentsOf: data + Data([0x0A]))
    }
}
