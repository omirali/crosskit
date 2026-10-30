import XCTest

/// Процесс на старую букву завершается, а не копится; зависший — не держит панель.
/// Ядро здесь подменено сценарием оболочки, который просто спит.
final class CoreProcessTests: XCTestCase {

    private var sleeper: URL!

    override func setUpWithError() throws {
        sleeper = FileManager.default.temporaryDirectory
            .appendingPathComponent("crosskit-sleeper-\(UUID().uuidString)")
        try "#!/bin/sh\nsleep 30\n".write(to: sleeper, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: sleeper.path)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: sleeper)
    }

    func testACancelledRunEndsItsProcess() async throws {
        let core = CoreProcess(executable: sleeper, timeout: 30)
        let started = Date()
        let run = Task { try await core.run([]) }
        try await Task.sleep(nanoseconds: 200_000_000)
        run.cancel()
        do {
            _ = try await run.value
            XCTFail("отменённый запуск не должен вернуть ответ")
        } catch {
            XCTAssertTrue(error is CancellationError, "\(error)")
        }
        XCTAssertLessThan(Date().timeIntervalSince(started), 5, "процесс должен быть завершён, а не дождан")
    }

    func testAProcessThatDoesNotAnswerIsEnded() async throws {
        let core = CoreProcess(executable: sleeper, timeout: 0.3)
        let started = Date()
        do {
            _ = try await core.run([])
            XCTFail("зависший процесс не должен вернуть ответ")
        } catch {
            XCTAssertEqual(error as? FeedFailure, .coreTimedOut)
        }
        XCTAssertLessThan(Date().timeIntervalSince(started), 5)
    }

    func testAMissingCoreIsNamedAsMissing() {
        XCTAssertNil(CoreProcess.locate(explicit: "/nonexistent/crosskit", path: nil))
        XCTAssertNil(CoreProcess.locate(explicit: nil, path: "/nonexistent"))
        XCTAssertEqual(CoreProcess.locate(explicit: sleeper.path, path: nil), sleeper)
    }
}
