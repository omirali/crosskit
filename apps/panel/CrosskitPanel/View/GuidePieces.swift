import AppKit

/// Экраны, на которых вместо выдачи — объяснение: пустое окно, «ничего не найдено»
/// и плашка неудавшегося открытия.
enum GuidePieces {

    // MARK: Пустое окно

    static let hintsHeight: CGFloat = 30 + 54 + 8   // заголовок + карточки + поля
    static let corpusLineHeight: CGFloat = 24

    /// Подсказки синтаксиса. Решение 11 дизайнера: подсказка опирается только на то, что
    /// источник понимает, поэтому какие именно — решает источник, а не экран.
    static func hints(_ hints: [SyntaxHint]) -> NSView {
        let view = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: RowPieces.contentWidth, height: hintsHeight))

        let head = Widgets.label("Подсказки", font: Tokens.text(11.5, .semibold), color: Tokens.fg2)
        head.setFrameOrigin(NSPoint(x: 10, y: 12))
        view.addSubview(head)

        let gap: CGFloat = 8
        let columns = CGFloat(max(hints.count, 1))
        let width = (RowPieces.contentWidth - 20 - gap * (columns - 1)) / columns
        for (index, hint) in hints.enumerated() {
            let box = hintCard(hint.syntax, hint.explanation, width: width)
            box.setFrameOrigin(NSPoint(x: 10 + (width + gap) * CGFloat(index), y: 32))
            view.addSubview(box)
        }
        return view
    }

    private static func hintCard(_ syntax: String, _ explanation: String, width: CGFloat) -> NSView {
        let box = Widgets.Fill(Tokens.fill, radius: Tokens.rowRadius)
        box.frame = NSRect(x: 0, y: 0, width: width, height: 54)

        let mono = Widgets.label(syntax, font: Tokens.mono(12), color: Tokens.fg)
        mono.frame = NSRect(x: 10, y: 10, width: width - 20, height: mono.frame.height)
        box.addSubview(mono)

        let text = Widgets.label(explanation, font: Tokens.text(11), color: Tokens.fg3)
        text.frame = NSRect(x: 10, y: 30, width: width - 20, height: text.frame.height)
        box.addSubview(text)
        return box
    }

    /// «Ищем в work: 26 репозиториев · 480 000 файлов · 468 бесед».
    /// Ни одного из трёх чисел граф сегодня не держит в таком виде.
    static func corpusLine(context: String, corpus: CorpusSummary?) -> NSView {
        var parts: [String] = []
        if let repositories = corpus?.repositories { parts.append("\(repositories) репозиториев") }
        if let files = corpus?.files { parts.append(decimal(files) + " файлов") }
        if let conversations = corpus?.conversations { parts.append("\(conversations) бесед") }

        let text = parts.isEmpty ? "Ищем в \(context)" : "Ищем в \(context): " + parts.joined(separator: " · ")
        let label = Widgets.label(text, font: Tokens.text(11.5), color: Tokens.fg3)

        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: RowPieces.contentWidth, height: corpusLineHeight))
        label.setFrameOrigin(NSPoint(x: 10, y: (corpusLineHeight - label.frame.height) / 2))
        row.addSubview(label)
        return row
    }

    private static func decimal(_ value: Int) -> String {
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.groupingSeparator = "\u{00a0}"
        return formatter.string(from: NSNumber(value: value)) ?? "\(value)"
    }

    // MARK: Ничего не найдено

    /// Следующий шаг, который панель может предложить, когда выдача пуста.
    struct Step {
        let glyph: String
        let title: String
        let subtitle: String
        let key: String
    }

    static let noResultsHeaderHeight: CGFloat = 26 + 22 + 40 + 14
    static let stepHeight: CGFloat = 44

    static func noResultsHeader(context: String, query: String, explanation: String) -> NSView {
        refusalHeader(title: "В контексте \(context) по запросу «\(query)» ничего нет", explanation: explanation)
    }

    /// Отказ источника: заголовок говорит, что случилось, а не «ничего нет». «Ничего нет» —
    /// это ответ; «база занята» — отсутствие ответа, и прочитать одно как другое значит
    /// решить, что искомого нет, когда его просто не спросили.
    static func refusalHeader(title: String, explanation: String) -> NSView {
        let view = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: noResultsHeaderHeight))

        let heading = Widgets.label(title, font: Tokens.text(16, .semibold), color: Tokens.fg)
        heading.frame = NSRect(x: 26, y: 26, width: Tokens.windowWidth - 52, height: heading.frame.height)
        view.addSubview(heading)

        let body = NSTextField(wrappingLabelWithString: explanation)
        body.font = Tokens.text(12.5)
        body.textColor = Tokens.fg2
        body.frame = NSRect(x: 26, y: 54, width: Tokens.windowWidth - 52, height: 40)
        view.addSubview(body)
        return view
    }

    static let noticeHeight: CGFloat = 26

    /// Строка о том, что панель только что сделала, — например, что команда в буфере.
    static func notice(_ text: String) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: noticeHeight))
        let label = Widgets.label(text, font: Tokens.text(11.5, .medium), color: Tokens.fg2)
        label.frame = NSRect(x: 18, y: (noticeHeight - label.frame.height) / 2,
                             width: Tokens.windowWidth - 36, height: label.frame.height)
        row.addSubview(label)
        return row
    }

    static func step(_ step: Step, selected: Bool) -> NSView {
        let row = Widgets.Fill(selected ? Tokens.selection : .clear, radius: Tokens.rowRadius)
        row.frame = NSRect(x: 0, y: 0, width: RowPieces.contentWidth, height: stepHeight)

        let glyphBox = Widgets.Fill(Tokens.fill2, radius: 7)
        glyphBox.frame = NSRect(x: 10, y: 8, width: 28, height: 28)
        let glyph = Widgets.label(step.glyph, font: Tokens.text(13), color: Tokens.fg2)
        glyph.alignment = .center
        glyph.frame = NSRect(x: 0, y: (28 - glyph.frame.height) / 2, width: 28, height: glyph.frame.height)
        glyphBox.addSubview(glyph)
        row.addSubview(glyphBox)

        let cap = keyPlate(step.key, selected: selected)
        cap.setFrameOrigin(NSPoint(x: RowPieces.contentWidth - 10 - cap.frame.width, y: 12))
        row.addSubview(cap)

        let title = Widgets.label(step.title, font: Tokens.text(13, .semibold), color: Tokens.fg)
        title.frame = NSRect(x: 50, y: 7, width: cap.frame.minX - 62, height: title.frame.height)
        row.addSubview(title)

        let subtitle = Widgets.label(step.subtitle, font: Tokens.text(11.5), color: Tokens.fg3)
        subtitle.frame = NSRect(x: 50, y: 24, width: cap.frame.minX - 62, height: subtitle.frame.height)
        row.addSubview(subtitle)
        return row
    }

    private static func keyPlate(_ text: String, selected: Bool) -> NSView {
        let label = Widgets.label(text, font: Tokens.text(11, .medium),
                                  color: selected ? .white : Tokens.fg2)
        label.alignment = .center
        let plate = Widgets.Fill(selected ? Tokens.accent : Tokens.fill2, radius: 5)
        plate.frame = NSRect(x: 0, y: 0, width: max(18, label.frame.width + 12), height: 20)
        label.frame = NSRect(x: 0, y: (20 - label.frame.height) / 2, width: plate.frame.width, height: label.frame.height)
        plate.addSubview(label)
        return plate
    }

    // MARK: Не удалось открыть

    static let failureHeight: CGFloat = 92

    /// Ошибка открытия — встроенная плашка под строкой, не модальное окно (решение 16).
    ///
    /// Открывает панель сама, поэтому и отказ знает она: источник выдачи об инструментах
    /// ничего не знает и знать не должен.
    static func openFailure(_ failure: OpenFailure) -> NSView {
        let view = Widgets.Fill(Tokens.warn.withAlphaComponent(0.07), radius: Tokens.rowRadius,
                                border: Tokens.warn)
        view.frame = NSRect(x: 0, y: 0, width: RowPieces.contentWidth - 50, height: failureHeight)

        let title = Widgets.label(failure.title, font: Tokens.text(13, .semibold), color: Tokens.fg)
        title.frame = NSRect(x: 12, y: 10, width: view.frame.width - 24, height: title.frame.height)
        view.addSubview(title)

        let body = NSTextField(wrappingLabelWithString: failure.explanation)
        body.font = Tokens.text(12)
        body.textColor = Tokens.fg2
        body.frame = NSRect(x: 12, y: 30, width: view.frame.width - 24, height: 34)
        view.addSubview(body)

        var x: CGFloat = 12
        for (key, title) in failure.actions {
            let cap = Widgets.keyCap(key)
            cap.setFrameOrigin(NSPoint(x: x, y: 66))
            view.addSubview(cap)

            let label = Widgets.label(title, font: Tokens.text(11.5), color: Tokens.fg2)
            label.setFrameOrigin(NSPoint(x: cap.frame.maxX + 5, y: 66 + (18 - label.frame.height) / 2))
            view.addSubview(label)
            x = label.frame.maxX + 14
        }
        return view
    }
}

/// Неудавшаяся попытка открыть строку. Панель строит её сама из того, что знает о строке.
struct OpenFailure {
    let rowIdentity: String
    let title: String
    let explanation: String
    let actions: [(String, String)]
}
