import Foundation

/// Строка службы — это либо машинный ответ команды (schema 2), либо отказ: протокола
/// (`refused`), самой команды (`error`), «нет такой задачи» (`missing_task`) или решение, которое
/// книга задач не приняла (`declined`).
enum ServiceAnswer {

    static func decode<Answer: Decodable>(_ type: Answer.Type, from line: Data) throws(ServiceError) -> Answer {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        if let refusal = try? decoder.decode(Refusal.self, from: line) {
            if let refused = refusal.refused { throw .refused(refused) }
            if let error = refusal.error { throw .commandRefused(category: error.category) }
            if let task = refusal.missingTask { throw .noSuchTask(task) }
            if let declined = refusal.declined { throw .declined(declined) }
        }
        do {
            return try decoder.decode(type, from: line)
        } catch {
            throw .unreadable
        }
    }

    private struct Refusal: Decodable {
        let refused: String?
        let error: CommandRefusal?
        let missingTask: String?
        let declined: Declined?
    }

    private struct CommandRefusal: Decodable {
        let category: String
    }
}

extension Date {
    /// Миллисекунды Unix, в которых ядро отдаёт время.
    public init(unixMilliseconds: Int64) {
        self.init(timeIntervalSince1970: TimeInterval(unixMilliseconds) / 1000)
    }
}
