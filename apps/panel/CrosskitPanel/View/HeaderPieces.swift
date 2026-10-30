import AppKit

/// Шапка: чип контекста, состояние индекса и строка фильтров.
/// Строка запроса живёт в `PanelContent` — она одна переживает перестроение экрана,
/// потому что держит фокус и текст.
enum HeaderPieces {

    static let rowHeight: CGFloat = 30
    static let filterRowHeight: CGFloat = 34   // 24 строка + 10 снизу

    /// Чип контекста: имя, шеврон и ⌘K. Имя приходит из шва — контексты живут
    /// в конфигурации, каждый со своей базой.
    static func contextChip(_ context: String) -> NSView {
        let name = Widgets.label(context, font: Tokens.text(13, .semibold), color: Tokens.fg)
        let cap = Widgets.keyCap("⌘K")

        let chip = Widgets.Fill(Tokens.fill2, radius: Tokens.chipRadius)
        chip.frame = NSRect(x: 0, y: 0,
                            width: 10 + name.frame.width + 7 + 9 + 7 + cap.frame.width + 5,
                            height: rowHeight)

        name.setFrameOrigin(NSPoint(x: 10, y: (rowHeight - name.frame.height) / 2))
        chip.addSubview(name)

        let chevron = Chevron()
        chevron.frame = NSRect(x: name.frame.maxX + 7, y: 11, width: 9, height: 9)
        chip.addSubview(chevron)

        cap.setFrameOrigin(NSPoint(x: chevron.frame.maxX + 7, y: 6))
        chip.addSubview(cap)
        return chip
    }

    /// Состояние индекса. Состояния различаются формой значка, а не только цветом —
    /// решение 15 дизайнера.
    static func indexBadge(_ state: IndexState) -> NSView {
        let (text, color, glyph): (String, NSColor, NSView) = {
            switch state {
            case .fresh:
                return ("индекс свежий", Tokens.fg3, dot(Tokens.ok))
            case .behind(let text):
                return (text, Tokens.warn, triangle(Tokens.warn))
            }
        }()

        let label = Widgets.label(text, font: Tokens.text(11.5), color: color)
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0,
                                                width: 16 + label.frame.width,
                                                height: label.frame.height))
        glyph.setFrameOrigin(NSPoint(x: 0, y: (row.frame.height - glyph.frame.height) / 2))
        row.addSubview(glyph)
        label.setFrameOrigin(NSPoint(x: 16, y: 0))
        row.addSubview(label)
        return row
    }

    /// Строка фильтров ⌘1–4 и монограммы источников.
    static func filterRow(scope: FeedScope) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: filterRowHeight))
        var x: CGFloat = 16

        for candidate in FeedScope.allCases {
            let chip = filterChip(candidate, selected: candidate == scope)
            chip.setFrameOrigin(NSPoint(x: x, y: 0))
            row.addSubview(chip)
            x = chip.frame.maxX + 4
        }

        let divider = Widgets.Fill(Tokens.separatorSoft)
        divider.frame = NSRect(x: x + 8, y: 4, width: 1, height: 16)
        row.addSubview(divider)
        x = divider.frame.maxX + 8

        for tool in Tool.allCases {
            let chip = sourceChip(tool)
            chip.setFrameOrigin(NSPoint(x: x, y: 0))
            row.addSubview(chip)
            x = chip.frame.maxX + 2
        }
        return row
    }

    private static func filterChip(_ scope: FeedScope, selected: Bool) -> NSView {
        let font = Tokens.text(12, .medium)
        let label = Widgets.label(scope.label, font: font, color: selected ? Tokens.inverse : Tokens.fg2)
        let key = Widgets.label(scope.key, font: Tokens.text(10.5),
                                color: (selected ? Tokens.inverse : Tokens.fg2).withAlphaComponent(0.6))

        let chip = Widgets.Fill(selected ? Tokens.fg : .clear, radius: 6)
        chip.frame = NSRect(x: 0, y: 0, width: 8 + label.frame.width + 6 + key.frame.width + 8, height: 24)

        label.setFrameOrigin(NSPoint(x: 8, y: (24 - label.frame.height) / 2))
        chip.addSubview(label)
        key.setFrameOrigin(NSPoint(x: label.frame.maxX + 6, y: (24 - key.frame.height) / 2))
        chip.addSubview(key)
        return chip
    }

    private static func sourceChip(_ tool: Tool) -> NSView {
        let label = Widgets.label(tool.label, font: Tokens.text(12), color: Tokens.fg2)
        let chip = Widgets.Fill(.clear, radius: 6, border: Tokens.separatorSoft)
        chip.frame = NSRect(x: 0, y: 0, width: 5 + 14 + 5 + label.frame.width + 8, height: 24)

        let mono = Widgets.monogram(tool.monogram, color: tool.color, side: 14, radius: 4, fontSize: 7)
        mono.setFrameOrigin(NSPoint(x: 5, y: 5))
        chip.addSubview(mono)

        label.setFrameOrigin(NSPoint(x: 24, y: (24 - label.frame.height) / 2))
        chip.addSubview(label)
        return chip
    }

    private static func dot(_ color: NSColor) -> NSView {
        let view = Widgets.Fill(color, radius: 3.5)
        view.frame = NSRect(x: 0, y: 0, width: 7, height: 7)
        return view
    }

    private static func triangle(_ color: NSColor) -> NSView {
        let view = Triangle(color: color)
        view.frame = NSRect(x: 0, y: 0, width: 10, height: 10)
        return view
    }

    /// Кнопки масштаба холста − + ⤢ у правого края `right`, по середине строки высотой
    /// `height`: путь без жестов, тот же, что + − 0 и ⌘= ⌘- ⌘0. Возвращает левый край.
    static func zoomButtons(in row: NSView, endingAt right: CGFloat, height: CGFloat,
                            action: @escaping (CanvasZoom) -> Void) -> CGFloat {
        var right = right
        for (glyph, step) in [("⤢", CanvasZoom.fit), ("+", .zoomIn), ("−", .zoomOut)] {
            let button = Widgets.Clickable(frame: NSRect(x: right - 24, y: (height - 22) / 2,
                                                         width: 24, height: 22)) { action(step) }
            let plate = Widgets.Fill(Tokens.fill2, radius: 6)
            plate.frame = button.bounds
            button.addSubview(plate)
            let label = Widgets.label(glyph, font: Tokens.text(13, .medium), color: Tokens.fg2)
            label.setFrameOrigin(NSPoint(x: (24 - label.frame.width) / 2, y: (22 - label.frame.height) / 2))
            button.addSubview(label)
            row.addSubview(button)
            right = button.frame.minX - 4
        }
        return right
    }
}

/// Шеврон чипа контекста: в макете это инлайновый SVG, отдельного ресурса под него нет.
final class Chevron: NSView {
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath()
        path.move(to: NSPoint(x: 2, y: 6.5))
        path.line(to: NSPoint(x: 5, y: 3.5))
        path.line(to: NSPoint(x: 8, y: 6.5))
        path.lineWidth = 1.5
        path.lineCapStyle = .round
        path.lineJoinStyle = .round
        Tokens.fg2.setStroke()
        path.stroke()
    }
}

/// Значок «индекс отстал»: форма, а не цвет, отличает его от свежего.
private final class Triangle: NSView {
    private let color: NSColor

    init(color: NSColor) {
        self.color = color
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("панель собирается кодом, не из nib") }

    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath()
        path.move(to: NSPoint(x: 5, y: 9))
        path.line(to: NSPoint(x: 9.5, y: 1))
        path.line(to: NSPoint(x: 0.5, y: 1))
        path.close()
        color.setFill()
        path.fill()
    }
}
