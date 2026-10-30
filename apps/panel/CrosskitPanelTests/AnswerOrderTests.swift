import XCTest

/// Устаревший ответ отбрасывается: медленная старая буква не затирает быструю новую.
final class AnswerOrderTests: XCTestCase {

    func testALateAnswerToAnOlderQueryIsDropped() {
        var order = AnswerOrder()
        let older = order.issue()
        let newer = order.issue()
        XCTAssertTrue(order.accept(newer))
        XCTAssertFalse(order.accept(older), "ответ на старую букву пришёл после новой")
        XCTAssertFalse(order.awaiting)
    }

    func testAnswersInOrderAreAllTaken() {
        var order = AnswerOrder()
        let first = order.issue()
        let second = order.issue()
        XCTAssertTrue(order.accept(first))
        XCTAssertTrue(order.awaiting, "второй запрос ещё ждёт")
        XCTAssertTrue(order.accept(second))
        XCTAssertFalse(order.awaiting)
    }

    func testTheSameAnswerIsNotAppliedTwice() {
        var order = AnswerOrder()
        let only = order.issue()
        XCTAssertTrue(order.accept(only))
        XCTAssertFalse(order.accept(only))
    }
}
