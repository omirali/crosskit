import AppKit

/// Мелкие элементы макета. Раскладка везде на явных прямоугольниках: у макета задан
/// каждый отступ в точках, и повторить его прямо честнее, чем через констрейнты.
enum Widgets {

    /// Контейнер с началом координат сверху слева — как в макете.
    class Flipped: NSView {
        override var isFlipped: Bool { true }
    }

    /// Область, которая откликается на клик: кнопка масштаба, пункт легенды. Без `NSButton`:
    /// его внешний вид — системный, а у панели свой. Первый клик по неактивной панели тоже
    /// действует.
    final class Clickable: Flipped {
        private let action: () -> Void

        init(frame: NSRect, action: @escaping () -> Void) {
            self.action = action
            super.init(frame: frame)
        }

        required init?(coder: NSCoder) { fatalError("панель собирается кодом, не из nib") }

        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

        override func mouseDown(with event: NSEvent) {
            action()
        }

        override func resetCursorRects() {
            addCursorRect(bounds, cursor: .pointingHand)
        }
    }

    /// Цель для `NSPopUpButton`: действие замыканием, без класса-наблюдателя на каждый выбор.
    final class Chooser: NSObject {
        private let action: (Int) -> Void

        init(action: @escaping (Int) -> Void) {
            self.action = action
        }

        @objc func chosen(_ sender: NSPopUpButton) {
            action(sender.indexOfSelectedItem)
        }
    }

    /// Слой с заливкой и радиусом, который сам переживает смену темы.
    final class Fill: Flipped {
        private let fill: NSColor
        private let radius: CGFloat
        private let border: NSColor?

        init(_ fill: NSColor, radius: CGFloat = 0, border: NSColor? = nil) {
            self.fill = fill
            self.radius = radius
            self.border = border
            super.init(frame: .zero)
            wantsLayer = true
        }

        required init?(coder: NSCoder) { fatalError("панель собирается кодом, не из nib") }

        override var wantsUpdateLayer: Bool { true }

        override func updateLayer() {
            guard let layer else { return }
            layer.cornerRadius = radius
            layer.cornerCurve = .continuous
            // CGColor темы не знает — цвет надо пересчитывать на каждой смене оформления.
            layer.backgroundColor = fill.cgColor
            layer.borderWidth = border == nil ? 0 : 1
            layer.borderColor = border?.cgColor
        }
    }

    /// Подпись, уже подогнанная под свой текст. Ширину считает сам `NSTextField`:
    /// у его ячейки есть внутренний отступ, и ширина строки, посчитанная по шрифту,
    /// на него не приходится — подпись обрезается.
    static func label(_ text: String, font: NSFont, color: NSColor) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = font
        field.textColor = color
        field.lineBreakMode = .byTruncatingTail
        field.cell?.usesSingleLineMode = true
        field.sizeToFit()
        return field
    }

    static func label(_ text: NSAttributedString) -> NSTextField {
        let field = NSTextField(labelWithAttributedString: text)
        field.lineBreakMode = .byTruncatingTail
        field.cell?.usesSingleLineMode = true
        field.sizeToFit()
        return field
    }

    /// Клавиша в строке подсказок: min-width 18, height 18, radius 5, фон `--fill2`.
    static func keyCap(_ glyph: String) -> NSView {
        let text = label(glyph, font: Tokens.text(11, .medium), color: Tokens.fg2)
        text.alignment = .center

        let width = max(18, text.frame.width + 10)
        let cap = Fill(Tokens.fill2, radius: Tokens.keyCapRadius)
        cap.frame = NSRect(x: 0, y: 0, width: width, height: 18)

        text.frame = NSRect(x: 0, y: 2, width: width, height: 14)
        cap.addSubview(text)
        return cap
    }

    /// Монограмма инструмента: временная плашка из раздела «05 Решения» макета.
    static func monogram(_ text: String, color: NSColor, side: CGFloat, radius: CGFloat, fontSize: CGFloat) -> NSView {
        let box = Fill(color, radius: radius)
        box.frame = NSRect(x: 0, y: 0, width: side, height: side)

        let glyph = label(text, font: Tokens.text(fontSize, .bold), color: .white)
        glyph.alignment = .center
        let height = glyph.frame.height
        glyph.frame = NSRect(x: 0, y: (side - height) / 2, width: side, height: height)
        box.addSubview(glyph)
        return box
    }
}
