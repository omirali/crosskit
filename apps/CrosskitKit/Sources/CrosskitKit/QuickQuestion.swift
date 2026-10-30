import Foundation

/// Быстрый вопрос агенту (макет `design/UXMain.dc.html`): ответ только читает проект, не
/// сохраняется ни в журнал, ни в беседы задачи, и потому не попадает на карту.
///
/// Это `claude -p` с `--no-session-persistence`: журнала нет, служба его не прочитает, сшивка
/// не найдёт. Инструменты — только чтение, так что вопрос не может ничего поменять.
public enum QuickQuestion {

    /// Модель по умолчанию: вопрос короткий, ответ нужен быстро.
    public static let model = "haiku"
    static let readOnlyTools = "Read,Grep,Glob"

    static func arguments(question: String, model: String) -> [String] {
        ["-p", question, "--model", model, "--no-session-persistence", "--tools", readOnlyTools,
         "--output-format", "json"]
    }

    public struct Answer: Sendable, Equatable {
        public let text: String
        public let costUSD: Double?
    }

    public struct Failure: Error, Sendable, Equatable {
        public let said: String
    }

    /// Задаёт вопрос в каталоге проекта и ждёт ответа целиком.
    public static func ask(_ question: String, in directory: URL, model: String = model,
                           program: String = "claude") async throws -> Answer {
        let arguments = arguments(question: question, model: model)
        return try await Task.detached {
            let process = LoginShell.process(running: program, with: arguments, in: directory)
            var environment = ProcessInfo.processInfo.environment
            environment.removeValue(forKey: "CLAUDECODE")
            process.environment = environment
            let output = Pipe(), errors = Pipe()
            process.standardOutput = output
            process.standardError = errors
            process.standardInput = FileHandle.nullDevice
            try process.run()
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard let answer = answer(from: data) else {
                let said = String(decoding: errors.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
                throw Failure(said: said.isEmpty ? "claude закончил с кодом \(process.terminationStatus)" : said)
            }
            return answer
        }.value
    }

    /// Итог `--output-format json`: одна строка с `result` и ценой.
    static func answer(from data: Data) -> Answer? {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let text = object["result"] as? String, object["is_error"] as? Bool != true
        else { return nil }
        return Answer(text: text, costUSD: object["total_cost_usd"] as? Double)
    }
}
