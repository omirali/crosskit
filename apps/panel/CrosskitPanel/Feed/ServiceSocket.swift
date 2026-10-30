import CrosskitKit
import Foundation

/// Вопрос фоновой службе по её сокету — без процесса и без открытия базы (ADR 0050).
///
/// Служба отвечает той же строкой, что напечатал бы `crosskit … --json`, поэтому разбор
/// ответа у панели один. Чего служба своим ответом не даёт — её нет, другой протокол, контекст
/// она сейчас не держит, вопрос не из её четырёх, — панель спрашивает процессом, как раньше.
struct ServiceSocket {

    /// Каталог `service/` в каталоге данных.
    let directory: String

    static let protocolVersion = 1

    /// Там же, где его ищет ядро: `CROSSKIT_DATA_DIR`, иначе каталог данных по умолчанию.
    static func located(environment: [String: String] = ProcessInfo.processInfo.environment) -> ServiceSocket {
        let data = environment["CROSSKIT_DATA_DIR"]
            ?? (NSHomeDirectory() as NSString).appendingPathComponent("Library/Application Support/Crosskit")
        return ServiceSocket(directory: (data as NSString).appendingPathComponent("service"))
    }

    /// Аргументы командной строки — в запрос протокола; `nil` — вопрос, на который служба не
    /// отвечает и который идёт процессом.
    static func request(for arguments: [String]) -> [String: Any]? {
        var rest = arguments[...]
        var context: String?
        if rest.first == "--context", rest.count > 1 {
            context = rest[rest.startIndex + 1]
            rest = rest.dropFirst(2)
        }
        guard let command = rest.first else { return nil }
        let op: String
        let commandArguments = Array(rest.dropFirst())
        switch command {
        case "search": op = "search"
        case "graph": op = "graph"
        case "contexts": op = "contexts"
        case "knowledge" where commandArguments.first == "map": op = "knowledge-map"
        default: return nil
        }
        guard commandArguments.contains("--json") else { return nil }
        return ["protocol": protocolVersion, "op": op, "context": context ?? NSNull(),
                "arguments": commandArguments]
    }

    /// Ответ службы на эти аргументы — или `nil`, и тогда спрашивают процессом.
    func answer(_ arguments: [String], timeout: TimeInterval) async -> Data? {
        guard let request = Self.request(for: arguments) else { return nil }
        return await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                continuation.resume(returning: ask(request, timeout: timeout))
            }
        }
    }

    /// Один запрос, одна строка ответа. Ждёт не дольше `timeout` на каждую сторону. Сам обмен —
    /// в `CrosskitKit`, общий с desktop (ADR 0058); сокет другого пользователя вопроса не получает.
    func ask(_ request: [String: Any], timeout: TimeInterval) -> Data? {
        let path = (directory as NSString).appendingPathComponent("crosskit.sock")
        guard let line = try? JSONSerialization.data(withJSONObject: request),
              let answer = try? ServiceLine.exchange(line, socketPath: path, timeout: .seconds(timeout))
        else {
            return nil
        }
        // Отказ протокола — «спроси сама»; ответ команды, даже отказ её схемы, — ответ.
        if let object = try? JSONSerialization.jsonObject(with: answer) as? [String: Any],
           object["refused"] != nil {
            return nil
        }
        return answer
    }
}
