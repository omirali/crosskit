import XCTest

/// Клавиши холста — графа окрестности и карты знаний: что двигает, что приближает и что остаётся строке запроса.
final class CanvasKeysTests: XCTestCase {

    private func key(_ code: UInt16, _ characters: String? = nil, _ modifiers: NSEvent.ModifierFlags = [],
                     listed: Bool = false) -> CanvasKey? {
        CanvasKey.of(keyCode: code, characters: characters, modifiers: modifiers, foundListed: listed)
    }

    /// → показывает то, что правее; с ⇧ — шаг втрое длиннее.
    func testArrowsPanTheVisibleArea() {
        XCTAssertEqual(key(124), .pan(dx: CanvasKey.step, dy: 0))
        XCTAssertEqual(key(123), .pan(dx: -CanvasKey.step, dy: 0))
        XCTAssertEqual(key(126), .pan(dx: 0, dy: -CanvasKey.step))
        XCTAssertEqual(key(125, nil, [.shift, .numericPad, .function]), .pan(dx: 0, dy: CanvasKey.longStep))
    }

    /// ⌥← ⌥→ двигают курсор в строке, ⌘ и ⌃ — сочетания панели и системы: холст их не берёт.
    func testModifiedArrowsStayWithTheQueryField() {
        XCTAssertNil(key(123, nil, [.option]))
        XCTAssertNil(key(124, nil, [.option]))
        XCTAssertNil(key(124, nil, [.command]))
        XCTAssertNil(key(126, nil, [.control]))
        XCTAssertNil(key(24, "=", [.command]))
    }

    /// ⌥↑ ⌥↓ выбирают элемент холста: простые стрелки теперь двигают холст, а ⇥ — «к выдаче».
    func testOptionUpAndDownChooseAnElement() {
        XCTAssertEqual(key(126, nil, [.option]), .choose(-1))
        XCTAssertEqual(key(125, nil, [.option, .numericPad, .function]), .choose(1))
        XCTAssertEqual(key(125, nil, [.option], listed: true), .choose(1))
        XCTAssertNil(key(29, "º", [.option]), "⌥0 печатает символ, а не вписывает")
    }

    /// Пока открыт список найденного, ↑ ↓ ходят по нему; ← → по-прежнему двигают карту.
    func testUpAndDownBelongToTheFoundList() {
        XCTAssertNil(key(126, listed: true))
        XCTAssertNil(key(125, listed: true))
        XCTAssertEqual(key(124, listed: true), .pan(dx: CanvasKey.step, dy: 0))
    }

    /// + и = приближают, − отдаляет, 0 вписывает; буквы идут в поиск.
    func testSignsZoomAndLettersSearch() {
        XCTAssertEqual(key(24, "+", [.shift]), .zoom(.zoomIn))
        XCTAssertEqual(key(24, "="), .zoom(.zoomIn))
        XCTAssertEqual(key(27, "-"), .zoom(.zoomOut))
        XCTAssertEqual(key(29, "0"), .zoom(.fit))
        XCTAssertNil(key(40, "к"))
        XCTAssertNil(key(18, "1"))
    }
}
