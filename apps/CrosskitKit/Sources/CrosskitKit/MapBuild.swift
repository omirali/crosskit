import Foundation

/// Сколько примерно стоит достроить карту задачи, по замеру вопроса `decision-map-v4`
/// (ADR 0057): 4,27 $ за 383 реплики и 0,63 $ за 59, то есть около 1,1 цента за реплику, и
/// 0,43 $ за сводку двух бесед. Это оценка лимита подписки, не счёт: цену называет сам `claude`.
public struct BuildEstimate: Sendable, Equatable {
    public static let usdPerReply = 0.011
    public static let usdPerJoin = 0.43

    /// Реплик, которых ещё не покрывает ни одна карта.
    public let unmappedReplies: Int
    /// Бесед, у которых такие реплики есть.
    public let conversations: Int
    public let usd: Double

    public init(map: TaskMapAnswer) {
        let behind = map.conversations.filter { $0.mappedThrough < $0.replies }
        unmappedReplies = behind.map { $0.replies - $0.mappedThrough }.reduce(0, +)
        conversations = behind.count
        usd = unmappedReplies == 0 ? 0 : Double(unmappedReplies) * Self.usdPerReply + Self.usdPerJoin
    }
}

/// Построение карты задачи из окна: тот же `crosskit tasks map <задача> --build`, что из
/// терминала. Модель спрашивает команда, а не служба (ADR 0055), поэтому окно запускает её
/// процессом и читает её ход — строки, которые она пишет в stderr.
public enum MapBuild {

    /// Почему построение не закончилось.
    public struct Failure: Error, Sendable, Equatable {
        public let status: Int32
        /// Последние строки, которые команда сказала: в них её причина.
        public let said: [String]
    }

    /// Программа, на которой работает служба: её копию держит каталог данных (ADR 0049).
    public static func program(environment: [String: String] = ProcessInfo.processInfo.environment) -> URL? {
        let data = environment["CROSSKIT_DATA_DIR"].map { URL(fileURLWithPath: $0) }
            ?? URL.applicationSupportDirectory.appendingPathComponent("Crosskit")
        let program = data.appendingPathComponent("bin/crosskit")
        return FileManager.default.isExecutableFile(atPath: program.path) ? program : nil
    }

    /// Аргументы оболочки. Команда идёт через `zsh -l`: так она находит `claude` по тому же
    /// PATH, что в терминале владельца, а у приложения из Finder PATH — только системный.
    /// `"$@"` передаёт аргументы как есть, и имя задачи не может стать командой.
    static func shellArguments(program: URL, task: String, context: String?) -> [String] {
        let command = (context.map { ["--context", $0] } ?? []) + ["tasks", "map", task, "--build"]
        return ["-l", "-c", "exec \"$@\"", "crosskit", program.path] + command
    }

    /// Запускает построение. Поток отдаёт строки хода и кончается, когда команда закончила;
    /// отмена потока останавливает команду.
    public static func run(program: URL, task: String, context: String?) -> AsyncThrowingStream<String, Error> {
        AsyncThrowingStream { continuation in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/zsh")
            process.arguments = shellArguments(program: program, task: task, context: context)
            process.standardOutput = FileHandle.nullDevice
            let errors = Pipe()
            process.standardError = errors
            let reading = Task {
                var said: [String] = []
                do {
                    for try await line in errors.fileHandleForReading.bytes.lines {
                        said = Array((said + [line]).suffix(8))
                        continuation.yield(line)
                    }
                } catch {}
                process.waitUntilExit()
                if process.terminationStatus == 0 {
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
}
