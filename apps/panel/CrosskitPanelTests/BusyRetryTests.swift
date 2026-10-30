import XCTest

/// `busy` — ожидание, а не ошибка: короткий проход панель пережидает повтором, а не
/// показывает отказ; длинный — отдаёт `busy` дальше, и экран держит прежнюю выдачу.
final class BusyRetryTests: XCTestCase {

    /// Ответ, который `refusals` раз подряд — `busy`, а потом — число попыток.
    private final class Core: @unchecked Sendable {
        private let refusals: Int
        private(set) var asked = 0
        init(refusals: Int) { self.refusals = refusals }

        func ask() throws -> Int {
            asked += 1
            if asked <= refusals { throw FeedFailure.core(.busy) }
            return asked
        }
    }

    private let quick: [Double] = [0.01, 0.01, 0.01, 0.01]

    func testAShortPassIsWaitedOutInsteadOfShownAsARefusal() async throws {
        let core = Core(refusals: 2)
        let answer = try await BusyRetry.asking(pauses: quick) { try core.ask() }
        XCTAssertEqual(answer, 3)
    }

    func testALongPassIsStillBusyAfterThePauses() async throws {
        let core = Core(refusals: quick.count + 1)
        do {
            _ = try await BusyRetry.asking(pauses: quick) { try core.ask() }
            XCTFail("за паузами проход не кончился — ответ должен быть busy")
        } catch {
            XCTAssertEqual(error as? FeedFailure, .core(.busy))
        }
        XCTAssertEqual(core.asked, quick.count + 1)
    }

    func testAnotherRefusalIsNotRetried() async throws {
        var asked = 0
        do {
            _ = try await BusyRetry.asking(pauses: quick) { () async throws -> Int in
                asked += 1
                throw FeedFailure.core(.needsMigration)
            }
            XCTFail("отказ не busy — не ожидание")
        } catch {
            XCTAssertEqual(error as? FeedFailure, .core(.needsMigration))
        }
        XCTAssertEqual(asked, 1)
    }

    func testTheWholeWaitStaysShort() {
        XCTAssertLessThanOrEqual(BusyRetry.pauses.reduce(0, +), 4)
    }
}
