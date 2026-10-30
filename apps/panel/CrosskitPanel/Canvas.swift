import AppKit

/// Холст панели — граф окрестности (⇥) или карта знаний (⌘M) — и то, чем его двигают.
///
/// Механизм один на оба экрана (ADR 0046, ADR 0048): окно (`PanelWindow.sendEvent`) ловит
/// клавиши и жесты раньше вида под курсором и строки запроса, клавишу разбирает `CanvasKey`,
/// жест — `GraphPage.gesture`, а двигает холст общая камера страницы (`camera.js`). Экран
/// говорит только, какой холст открыт и есть ли на нём список, по которому ходят ↑ ↓.

/// Шаг масштаба: кнопки − + ⤢, клавиши + − 0 и ⌘= ⌘- ⌘0.
enum CanvasZoom: String {
    case zoomIn = "in"
    case zoomOut = "out"
    case fit
}

/// Клавиши холста без ⌘: стрелки двигают видимую зону, + − приближают, 0 вписывает, ⌥↑ ⌥↓
/// выбирают элемент.
///
/// Разбор — чистая функция от того, что в событии, чтобы правило проверялось тестом, а не
/// пальцем. Клавиши разбирает окно панели раньше строки запроса: иначе «+» и «0» напечатались
/// бы в поиск, а ← → сдвинули бы курсор. Это цена — на холсте эти символы не набираются; курсор
/// в строке двигают ⌥← / ⌥→, их разбор не трогает.
///
/// Выбор элемента — ⌥↑ / ⌥↓, а не ⇥ / ⇧⇥: ⇥ на графе уже значит «к выдаче», и отнять его
/// значило бы сломать привычный выход.
enum CanvasKey: Equatable {
    /// Сдвиг видимой зоны: доли её ширины и высоты, знак — куда смотрит человек.
    case pan(dx: Double, dy: Double)
    case zoom(CanvasZoom)
    /// Предыдущий (−1) или следующий (+1) элемент холста.
    case choose(Int)

    /// Короткий шаг стрелки и длинный — с ⇧.
    static let step = 0.15
    static let longStep = 0.5

    /// Стрелки по коду клавиши — он один в любой раскладке.
    private static let left: UInt16 = 123
    private static let right: UInt16 = 124
    private static let down: UInt16 = 125
    private static let up: UInt16 = 126

    /// Строка клавиш холста — одна на оба экрана; экран добавляет к ней своё.
    static let hints: [(String, String)] = [("← → ↑ ↓", "Сдвиг"), ("+ −", "Масштаб"), ("0", "Вписать")]

    /// Что значит нажатие на холсте, или `nil` — тогда оно идёт строке запроса как обычно.
    /// ↑ ↓ при списке найденного идут ему (`foundListed`): выбор в списке важнее сдвига.
    static func of(keyCode: UInt16, characters: String?, modifiers: NSEvent.ModifierFlags,
                   foundListed: Bool) -> CanvasKey? {
        guard modifiers.isDisjoint(with: [.command, .control]) else { return nil }
        if modifiers.contains(.option) {
            switch keyCode {
            case up: return .choose(-1)
            case down: return .choose(1)
            default: return nil
            }
        }
        let length = modifiers.contains(.shift) ? longStep : step
        switch keyCode {
        case left: return .pan(dx: -length, dy: 0)
        case right: return .pan(dx: length, dy: 0)
        case up: return foundListed ? nil : .pan(dx: 0, dy: -length)
        case down: return foundListed ? nil : .pan(dx: 0, dy: length)
        default: break
        }
        // «=» и «-» стоят на тех же клавишах и в русской раскладке, «+» — это ⇧=.
        switch characters {
        case "+", "=": return .zoom(.zoomIn)
        case "-", "−": return .zoom(.zoomOut)
        case "0": return .zoom(.fit)
        default: return nil
        }
    }
}

/// Вызовы общей камеры (`camera.js`): у обеих страниц они одинаковы, различается только объект.
extension GraphPageCall {

    static func zoom(_ step: CanvasZoom, on page: Page) -> GraphPageCall {
        struct Zoom: Encodable { let step: String }
        return GraphPageCall(page: page, function: .zoom, payload: encode(Zoom(step: step.rawValue)))
    }

    /// Сдвиг видимой зоны стрелкой — долями её ширины и высоты, с короткой анимацией.
    static func step(dx: Double, dy: Double, on page: Page) -> GraphPageCall {
        struct Step: Encodable { let dx: Double; let dy: Double }
        return GraphPageCall(page: page, function: .step, payload: encode(Step(dx: dx, dy: dy)))
    }

    /// Поставить элемент в середину видимой зоны, не меняя масштаба.
    static func center(on id: String, on page: Page) -> GraphPageCall {
        struct Center: Encodable { let id: String }
        return GraphPageCall(page: page, function: .center, payload: encode(Center(id: id)))
    }

    /// Сдвиг холста жестом трекпада, в точках.
    static func pan(dx: Double, dy: Double, on page: Page) -> GraphPageCall {
        struct Pan: Encodable { let dx: Double; let dy: Double }
        return GraphPageCall(page: page, function: .panBy, payload: encode(Pan(dx: dx, dy: dy)))
    }

    /// Приближение в точке холста множителем — щипок или колесо.
    static func zoomBy(_ factor: Double, at point: NSPoint, on page: Page) -> GraphPageCall {
        struct ZoomBy: Encodable { let factor: Double; let x: Double; let y: Double }
        return GraphPageCall(page: page, function: .zoomBy,
                             payload: encode(ZoomBy(factor: factor, x: Double(point.x), y: Double(point.y))))
    }
}
