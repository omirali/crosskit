import AppKit

/// Низ окна: строка счётчиков и строка клавиш.
enum FooterPieces {

    static let countersHeight: CGFloat = 36   // 7 + 22 + 7
    static let keyBarHeight: CGFloat = 30

    /// Счётчики и «Показать ещё».
    ///
    /// Закреплены под списком, а не в конце прокрутки — решение 4 дизайнера: числа видны
    /// всегда. Числа приходят из шва: сколько совпадений скрыла политика и сколько файлов
    /// свёрнуто как отсутствующие, знает тот, кто строил страницу, а не панель.
    ///
    /// «стр. 1 из 2» макета здесь не будет: keyset-пагинация общего числа страниц не знает
    /// и знать не может (ADR 0013, решение 9). Панель печатает номер страницы, который
    /// досчитала сама.
    static func counters(_ counters: FeedCounters, hasMore: Bool, pageSize: Int) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: countersHeight))
        let font = Tokens.text(11.5)
        var x: CGFloat = 18

        if hasMore {
            let label = Widgets.label("Показать ещё \(pageSize)", font: Tokens.text(11.5, .medium), color: Tokens.fg)
            let chip = Widgets.Fill(Tokens.fill2, radius: 6)
            chip.frame = NSRect(x: x, y: 7, width: label.frame.width + 18, height: 22)
            label.setFrameOrigin(NSPoint(x: 9, y: (22 - label.frame.height) / 2))
            chip.addSubview(label)
            row.addSubview(chip)
            x = chip.frame.maxX + 10

            let hint = Widgets.label("↓ в конце списка", font: font, color: Tokens.fg3)
            hint.setFrameOrigin(NSPoint(x: x, y: (countersHeight - hint.frame.height) / 2))
            row.addSubview(hint)
            x = hint.frame.maxX + 10
        }

        let page = Widgets.label("стр. \(counters.page)", font: font, color: Tokens.fg3)
        page.setFrameOrigin(NSPoint(x: x, y: (countersHeight - page.frame.height) / 2))
        row.addSubview(page)

        var right = Tokens.windowWidth - 18
        if let missing = counters.missingCollapsed {
            let group = counter(glyph: .circle(Tokens.warn), text: "Нет на диске, свёрнуто:", value: "\(missing)")
            group.setFrameOrigin(NSPoint(x: right - group.frame.width, y: (countersHeight - group.frame.height) / 2))
            row.addSubview(group)
            right = group.frame.minX - 14
        } else {
            let group = counter(glyph: .circle(Tokens.warn), text: "Нет на диске:", value: "уточняется")
            group.setFrameOrigin(NSPoint(x: right - group.frame.width, y: (countersHeight - group.frame.height) / 2))
            row.addSubview(group)
            right = group.frame.minX - 14
        }

        let policy = counter(glyph: .square(Tokens.fg3), text: "Скрыто политикой приватности:",
                             value: "\(counters.hiddenByPolicy)")
        policy.setFrameOrigin(NSPoint(x: right - policy.frame.width, y: (countersHeight - policy.frame.height) / 2))
        row.addSubview(policy)
        return row
    }

    private enum CounterGlyph {
        case square(NSColor)
        case circle(NSColor)
    }

    private static func counter(glyph: CounterGlyph, text: String, value: String) -> NSView {
        let font = Tokens.text(11.5)
        let label = Widgets.label(text, font: font, color: Tokens.fg3)
        let number = Widgets.label(value, font: Tokens.text(11.5, .semibold), color: Tokens.fg)

        let mark: Widgets.Fill
        switch glyph {
        case .square(let color): mark = Widgets.Fill(.clear, radius: 2, border: color)
        case .circle(let color): mark = Widgets.Fill(.clear, radius: 4, border: color)
        }
        mark.frame = NSRect(x: 0, y: 0, width: 8, height: 8)

        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0,
                                                width: 8 + 6 + label.frame.width + 5 + number.frame.width,
                                                height: max(label.frame.height, 8)))
        mark.setFrameOrigin(NSPoint(x: 0, y: (row.frame.height - 8) / 2))
        row.addSubview(mark)
        label.setFrameOrigin(NSPoint(x: 14, y: 0))
        row.addSubview(label)
        number.setFrameOrigin(NSPoint(x: label.frame.maxX + 5, y: 0))
        row.addSubview(number)
        return row
    }

    /// Строка клавиш. Меняется вместе с состоянием, как в макете: список клавиш —
    /// это описание того, что сейчас можно сделать, а не украшение.
    static func keyBar(_ keys: [(String, String)], escape: String = "Закрыть") -> NSView {
        let bar = Widgets.Fill(Tokens.fill)
        bar.frame = NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: keyBarHeight)

        let font = Tokens.text(11)
        var x: CGFloat = 16
        for (glyph, title) in keys {
            let cap = Widgets.keyCap(glyph)
            cap.setFrameOrigin(NSPoint(x: x, y: 6))
            bar.addSubview(cap)

            let label = Widgets.label(title, font: font, color: Tokens.fg3)
            label.setFrameOrigin(NSPoint(x: cap.frame.maxX + 5, y: (keyBarHeight - label.frame.height) / 2))
            bar.addSubview(label)
            x = label.frame.maxX + 14
        }

        let escLabel = Widgets.label(escape, font: font, color: Tokens.fg3)
        escLabel.setFrameOrigin(NSPoint(x: Tokens.windowWidth - 16 - escLabel.frame.width,
                                        y: (keyBarHeight - escLabel.frame.height) / 2))
        bar.addSubview(escLabel)

        let escCap = Widgets.keyCap("esc")
        escCap.setFrameOrigin(NSPoint(x: escLabel.frame.minX - 5 - escCap.frame.width, y: 6))
        bar.addSubview(escCap)
        return bar
    }
}
