import Foundation

/// Команда, которую человек сам набрал в терминале окна: идёт через его login-оболочку в
/// каталоге задачи, вывод и ошибки — строками по мере появления. Это не эмулятор терминала:
/// программа, которая ждёт ввода с клавиатуры, его не получит.
public enum ShellCommand {

    public struct Line: Sendable, Equatable {
        public let text: String
        public let isError: Bool
    }

    /// Поток строк; кончается вместе с командой, а её код выхода — последней строкой, если он
    /// не ноль. Отмена потока останавливает команду.
    public static func run(_ command: String, in directory: URL) -> AsyncStream<Line> {
        AsyncStream { continuation in
            let process = Process()
            process.executableURL = LoginShell.shell
            process.arguments = ["-l", "-c", command]
            process.currentDirectoryURL = directory
            process.standardInput = FileHandle.nullDevice
            let output = Pipe(), errors = Pipe()
            process.standardOutput = output
            process.standardError = errors
            let reading = Task.detached {
                await withTaskGroup(of: Void.self) { group in
                    for (pipe, isError) in [(output, false), (errors, true)] {
                        group.addTask {
                            do {
                                for try await line in pipe.fileHandleForReading.bytes.lines {
                                    continuation.yield(Line(text: line, isError: isError))
                                }
                            } catch {}
                        }
                    }
                }
                process.waitUntilExit()
                if process.terminationStatus != 0 {
                    continuation.yield(Line(text: "код выхода \(process.terminationStatus)", isError: true))
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in
                if process.isRunning { process.terminate() }
                reading.cancel()
            }
            do {
                try process.run()
            } catch {
                continuation.yield(Line(text: "\(error)", isError: true))
                continuation.finish()
            }
        }
    }
}
