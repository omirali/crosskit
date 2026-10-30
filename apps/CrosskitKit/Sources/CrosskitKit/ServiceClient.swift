import Foundation

/// Вопросы фоновой службе Crosskit. Приложение не открывает базу и не запускает ядро: всё, что
/// оно показывает, отдаёт служба (ADR 0058, решение 4).
public actor ServiceClient {

    public static let protocolVersion = 1

    let socketPath: String
    /// Статус, который служба пишет о себе (`ServiceState.swift`).
    let statusFile: URL
    private let timeout: Duration
    /// Блокирующий обмен идёт здесь, а не в общем пуле задач.
    private let queue = DispatchQueue(label: "dev.crosskit.service-client", qos: .userInitiated)

    public init(serviceDirectory: URL, timeout: Duration = .seconds(10)) {
        self.socketPath = serviceDirectory.appendingPathComponent("crosskit.sock").path
        self.statusFile = serviceDirectory.appendingPathComponent("status.json")
        self.timeout = timeout
    }

    /// Там же, где служба его открывает: `CROSSKIT_DATA_DIR`, иначе каталог данных по умолчанию.
    public static func located(environment: [String: String] = ProcessInfo.processInfo.environment) -> ServiceClient {
        let data = environment["CROSSKIT_DATA_DIR"].map { URL(fileURLWithPath: $0) }
            ?? URL.applicationSupportDirectory.appendingPathComponent("Crosskit")
        return ServiceClient(serviceDirectory: data.appendingPathComponent("service"))
    }

    /// Контексты, которые объявляет конфигурация, в её порядке.
    public func contexts() async throws(ServiceError) -> [String] {
        let answer: ContextList = try await ask("contexts", context: nil, arguments: ["--json"])
        return answer.contexts
    }

    /// Задачи контекста: подтверждённые владельцем и кандидаты сшивки.
    public func taskBoard(context: String?) async throws(ServiceError) -> TaskBoard {
        try await ask("tasks", context: context, arguments: ["--json"])
    }

    /// Карта задачи, как её показывает `tasks map <задача> --json`. Служба только показывает:
    /// построение спрашивает модель и идёт командой.
    public func taskMap(_ task: String, context: String?) async throws(ServiceError) -> TaskMapAnswer {
        try await ask("task-map", context: context, arguments: [task, "--json"])
    }

    func ask<Answer: Decodable & Sendable>(
        _ op: String, context: String?, arguments: [String]
    ) async throws(ServiceError) -> Answer {
        let request = ServiceRequest(protocol: Self.protocolVersion, op: op, context: context, arguments: arguments)
        guard let line = try? JSONEncoder().encode(request) else { throw .unreadable }
        let (socketPath, timeout) = (socketPath, timeout)
        let exchanged: Result<Data, ServiceError> = await withCheckedContinuation { continuation in
            queue.async {
                continuation.resume(returning: Result { () throws(ServiceError) -> Data in
                    try ServiceLine.exchange(line, socketPath: socketPath, timeout: timeout)
                })
            }
        }
        return try ServiceAnswer.decode(Answer.self, from: exchanged.get())
    }
}

struct ServiceRequest: Encodable {
    let `protocol`: Int
    let op: String
    let context: String?
    let arguments: [String]
}

private struct ContextList: Decodable, Sendable {
    let contexts: [String]
}
