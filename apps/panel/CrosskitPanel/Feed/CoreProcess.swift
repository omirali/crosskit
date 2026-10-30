import Foundation
import QuartzCore

/// Один запуск `crosskit`: аргументы на входе, stdout на выходе.
///
/// Сначала — вопрос фоновой службе по её сокету (`ServiceSocket`, ADR 0050): она держит базы
/// и отвечает без процесса и без `busy`. Без службы — мост этапа UI-3: процесс `crosskit` на
/// каждый запрос.
///
/// Две обязанности, без которых мост ломается не по времени, а по поведению:
///
/// * **отмена завершает процесс.** Задачу, чей ответ устарел, панель отменяет, и процесс на
///   старую букву получает SIGTERM, а не досчитывает в пустоту. Иначе быстрый набор копит
///   процессы, и каждый держит открытую базу;
/// * **зависший процесс не держит панель.** Через `timeout` процесс завершается, и панель
///   говорит, что ядро не ответило, вместо вечного «обновляется».
struct CoreProcess {

    let executable: URL
    let timeout: TimeInterval
    /// Служба, которую спрашивают раньше процесса; `nil` — только процесс (тесты, замер моста).
    var service: ServiceSocket? = ServiceSocket.located()

    /// Что вышло из одного запуска, со штампами `CACurrentMediaTime` для замера.
    struct Outcome {
        let stdout: Data
        let spawnedAt: CFTimeInterval
        let exitedAt: CFTimeInterval
    }

    /// Где лежит ядро: `--core=<путь>`, иначе первый `crosskit` в `PATH`.
    ///
    /// Своего пути по умолчанию у панели нет: путь к ядру — свойство машины, а не исходников.
    static func locate(explicit: String?, path: String?) -> URL? {
        let manager = FileManager.default
        if let explicit {
            let expanded = (explicit as NSString).expandingTildeInPath
            return manager.isExecutableFile(atPath: expanded) ? URL(fileURLWithPath: expanded) : nil
        }
        for directory in (path ?? "").split(separator: ":") {
            let candidate = (String(directory) as NSString).appendingPathComponent("crosskit")
            if manager.isExecutableFile(atPath: candidate) { return URL(fileURLWithPath: candidate) }
        }
        return nil
    }

    /// Ответ службы, если она его дала, иначе запуск процесса.
    func run(_ arguments: [String]) async throws -> Outcome {
        let asked = CACurrentMediaTime()
        if let service, let served = await service.answer(arguments, timeout: timeout) {
            try Task.checkCancellation()
            return Outcome(stdout: served, spawnedAt: asked, exitedAt: CACurrentMediaTime())
        }
        return try await spawn(arguments)
    }

    /// Запуск с ожиданием, отменяемый.
    private func spawn(_ arguments: [String]) async throws -> Outcome {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        process.standardInput = FileHandle.nullDevice

        let state = RunState()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                // Две вещи кончаются порознь: процесс выходит, канал доходит до конца. Ответ
                // готов, когда случились обе; канал читается всё время, а не после выхода,
                // иначе ответ больше буфера канала остановил бы процесс на записи.
                let finished = DispatchGroup()
                finished.enter()
                finished.enter()
                process.terminationHandler = { _ in
                    state.exitedAt = CACurrentMediaTime()
                    finished.leave()
                }
                // stderr — предложение для человека; панель его не читает, но вычитывает,
                // чтобы процесс не встал на полном канале.
                stderr.fileHandleForReading.readabilityHandler = { handle in _ = handle.availableData }
                do {
                    state.spawnedAt = CACurrentMediaTime()
                    try process.run()
                } catch {
                    stderr.fileHandleForReading.readabilityHandler = nil
                    continuation.resume(throwing: FeedFailure.coreNotFound)
                    return
                }
                state.process = process
                // Отмена могла прийти между запуском и этой строкой, когда процесса ещё
                // некому было завершить.
                if state.cancelled { process.terminate() }
                DispatchQueue.global().async {
                    state.output = stdout.fileHandleForReading.readDataToEndOfFile()
                    finished.leave()
                }
                finished.notify(queue: .global()) {
                    stderr.fileHandleForReading.readabilityHandler = nil
                    if state.cancelled {
                        continuation.resume(throwing: CancellationError())
                    } else if state.timedOut {
                        continuation.resume(throwing: FeedFailure.coreTimedOut)
                    } else {
                        continuation.resume(returning: Outcome(stdout: state.output,
                                                               spawnedAt: state.spawnedAt, exitedAt: state.exitedAt))
                    }
                }
                DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
                    if process.isRunning {
                        state.timedOut = true
                        process.terminate()
                    }
                }
            }
        } onCancel: {
            state.cancelled = true
            if let running = state.process, running.isRunning { running.terminate() }
        }
    }

    /// Запуск, который ждёт на месте. Только для списка контекстов при старте: он читает одну
    /// конфигурацию, базы не открывает, и окна, которое могло бы замереть, ещё нет.
    func runBlocking(_ arguments: [String]) throws -> Data {
        if let service, let request = ServiceSocket.request(for: arguments),
           let served = service.ask(request, timeout: timeout) {
            return served
        }
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = FileHandle.nullDevice
        process.standardInput = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            throw FeedFailure.coreNotFound
        }
        let output = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return output
    }
}

/// Состояние одного запуска, которое видят три потока: обработчик выхода, отмена и таймер.
private final class RunState: @unchecked Sendable {
    private let lock = NSLock()
    private var _process: Process?
    private var _cancelled = false
    private var _timedOut = false
    private var _spawnedAt: CFTimeInterval = 0
    private var _exitedAt: CFTimeInterval = 0
    private var _output = Data()

    var process: Process? {
        get { lock.withLock { _process } }
        set { lock.withLock { _process = newValue } }
    }
    var cancelled: Bool {
        get { lock.withLock { _cancelled } }
        set { lock.withLock { _cancelled = newValue } }
    }
    var timedOut: Bool {
        get { lock.withLock { _timedOut } }
        set { lock.withLock { _timedOut = newValue } }
    }
    var spawnedAt: CFTimeInterval {
        get { lock.withLock { _spawnedAt } }
        set { lock.withLock { _spawnedAt = newValue } }
    }
    var exitedAt: CFTimeInterval {
        get { lock.withLock { _exitedAt } }
        set { lock.withLock { _exitedAt = newValue } }
    }
    var output: Data {
        get { lock.withLock { _output } }
        set { lock.withLock { _output = newValue } }
    }
}
