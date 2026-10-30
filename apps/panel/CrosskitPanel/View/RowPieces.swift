import AppKit

/// Строки выдачи и заголовки групп.
///
/// Видов строк два, и поля у них разные: у беседы — инструмент, заголовок и число
/// сообщений, у файла — каталог, имя, состояние на диске и по каким словам совпало.
/// Общего у них три вещи: фрагмент с подсветкой, метка темы и высота.
enum RowPieces {

    static let rowHeight: CGFloat = 77          // 9 + (18 + 3 + 17 + 3 + 18) + 9
    static let sectionHeaderHeight: CGFloat = 30 // 10 сверху + 16 + 4 снизу
    static let horizontalInset: CGFloat = 8

    static var contentWidth: CGFloat { Tokens.windowWidth - horizontalInset * 2 }

    /// Заголовок группы: «Чаты 3 из 16 … Ещё 13».
    ///
    /// «3 из 16» требует общего числа совпадений, а keyset-пагинация его не считает
    /// (ADR 0013, решение 9). Когда числа нет, строка честно показывает только «3».
    static func sectionHeader(title: String, shown: Int, total: Int?, updating: Bool) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: contentWidth, height: sectionHeaderHeight))

        let name = Widgets.label(title, font: Tokens.text(11.5, .semibold), color: Tokens.fg2)
        name.setFrameOrigin(NSPoint(x: 10, y: 10))
        row.addSubview(name)

        var countText = total.map { "\(shown) из \($0)" } ?? "\(shown)"
        if updating { countText += " · обновляется" }
        let count = Widgets.label(countText, font: Tokens.text(11.5), color: Tokens.fg3)
        count.setFrameOrigin(NSPoint(x: name.frame.maxX + 8, y: 10))
        row.addSubview(count)

        if let total, total > shown, !updating {
            let more = Widgets.label("Ещё \(total - shown)", font: Tokens.text(11.5, .medium), color: Tokens.accent)
            more.setFrameOrigin(NSPoint(x: contentWidth - 10 - more.frame.width, y: 10))
            row.addSubview(more)
        }
        return row
    }

    static func row(_ feedRow: FeedRow, selected: Bool) -> NSView {
        switch feedRow {
        case .conversation(let conversation): return conversationRow(conversation, selected: selected)
        case .file(let file): return fileRow(file, selected: selected)
        }
    }

    // MARK: Беседа

    private static func conversationRow(_ item: ConversationRow, selected: Bool) -> NSView {
        let row = background(selected: selected)
        let mono = Widgets.monogram(item.tool.monogram, color: item.tool.color, side: 28, radius: 7, fontSize: 10.5)
        mono.setFrameOrigin(NSPoint(x: 10, y: 10))
        row.addSubview(mono)

        let textRight = selected
            ? place(action: primaryAction("Скопировать команду"), in: row)
            : contentWidth - 10
        fill(row, title: plain(item.title), fragment: item.fragment, meta: conversationMeta(item),
             topic: item.topic, textRight: textRight)
        return row
    }

    private static func conversationMeta(_ item: ConversationRow) -> NSAttributedString {
        let line = NSMutableAttributedString(
            string: item.tool.label,
            attributes: [.font: Tokens.text(11.5, .medium), .foregroundColor: Tokens.fg2])
        var rest = ""
        if let project = item.project { rest += " · " + project }
        if let when = item.when { rest += " · " + when }
        if let count = item.messageCount { rest += " · " + messages(count) }
        line.append(NSAttributedString(string: rest,
                                       attributes: [.font: Tokens.text(11.5), .foregroundColor: Tokens.fg3]))
        return line
    }

    private static func messages(_ count: Int) -> String {
        let hundredth = count % 100
        let tenth = count % 10
        if hundredth >= 11 && hundredth <= 14 { return "\(count) сообщений" }
        if tenth == 1 { return "\(count) сообщение" }
        if tenth >= 2 && tenth <= 4 { return "\(count) сообщения" }
        return "\(count) сообщений"
    }

    // MARK: Файл

    private static func fileRow(_ item: FileRow, selected: Bool) -> NSView {
        let row = background(selected: selected)
        let icon = fileIcon(presence: item.presence)
        icon.setFrameOrigin(NSPoint(x: 10, y: 10))
        row.addSubview(icon)

        let textRight: CGFloat
        switch (selected, item.presence) {
        case (false, _):
            textRight = contentWidth - 10
        case (true, .present):
            textRight = place(action: primaryAction("Открыть"), in: row)
        case (true, .missing):
            // Первичного действия у исчезнувшего файла нет — решение 6 дизайнера.
            textRight = place(action: unavailableAction(), in: row)
        }

        let title = NSMutableAttributedString(
            string: item.directory,
            attributes: [.font: Tokens.text(13), .foregroundColor: Tokens.fg3])
        title.append(NSAttributedString(string: item.name,
                                        attributes: [.font: Tokens.text(13, .semibold), .foregroundColor: Tokens.fg]))

        fill(row, title: title, fragment: item.fragment, meta: fileMeta(item),
             topic: item.topic, textRight: textRight, leadingBadge: item.presence == .missing ? "нет на диске" : nil)
        return row
    }

    private static func fileMeta(_ item: FileRow) -> NSAttributedString {
        let line = NSMutableAttributedString(
            string: "совпало: ",
            attributes: [.font: Tokens.text(11.5), .foregroundColor: Tokens.fg3])
        line.append(NSAttributedString(string: item.matched.joined(separator: ", "),
                                       attributes: [.font: Tokens.text(11.5, .medium), .foregroundColor: Tokens.fg2]))
        var rest = ""
        if let project = item.project { rest += " · " + project }
        if let when = item.when { rest += " · " + when.text }
        line.append(NSAttributedString(string: rest,
                                       attributes: [.font: Tokens.text(11.5), .foregroundColor: Tokens.fg3]))
        return line
    }

    private static func fileIcon(presence: Presence) -> NSView {
        let box = Widgets.Fill(presence == .present ? Tokens.fill2 : .clear, radius: 7,
                               border: presence == .present ? nil : Tokens.warn)
        box.frame = NSRect(x: 0, y: 0, width: 28, height: 28)

        let sheet = Widgets.Fill(.clear, radius: 2, border: presence == .present ? Tokens.fg3 : Tokens.warn)
        sheet.frame = NSRect(x: 9, y: 7.5, width: 10, height: 13)
        box.addSubview(sheet)
        return box
    }

    // MARK: Общее

    private static func background(selected: Bool) -> Widgets.Fill {
        let row = Widgets.Fill(selected ? Tokens.selection : .clear, radius: Tokens.rowRadius)
        row.frame = NSRect(x: 0, y: 0, width: contentWidth, height: rowHeight)
        return row
    }

    /// Три строки текста и метка темы. Возвращаемая правая граница текста зависит от того,
    /// стоит ли справа первичное действие.
    private static func fill(_ row: NSView, title: NSAttributedString, fragment: Fragment,
                             meta: NSAttributedString, topic: String?, textRight: CGFloat,
                             leadingBadge: String? = nil) {
        let left: CGFloat = 50
        let width = textRight - left

        let titleLabel = Widgets.label(title)
        titleLabel.frame = NSRect(x: left, y: 9, width: width, height: 18)
        row.addSubview(titleLabel)

        let snippet = Widgets.label(fragmentText(fragment))
        snippet.frame = NSRect(x: left, y: 30, width: width, height: 17)
        row.addSubview(snippet)

        let metaRow = metaLine(meta, topic: topic, badge: leadingBadge)
        metaRow.setFrameOrigin(NSPoint(x: left, y: 50))
        row.addSubview(metaRow)
    }

    private static func fragmentText(_ fragment: Fragment) -> NSAttributedString {
        // Строку обрезает стиль абзаца, а не `lineBreakMode` поля: у атрибутированной
        // строки свой стиль, и без него фрагмент режется посреди слова без многоточия.
        let paragraph = NSMutableParagraphStyle()
        paragraph.lineBreakMode = .byTruncatingTail

        let out = NSMutableAttributedString()
        for (text, matched) in fragment.parts() {
            var attributes: [NSAttributedString.Key: Any] = [
                .font: Tokens.text(12.5),
                .foregroundColor: matched ? Tokens.fg : Tokens.fg2,
                .paragraphStyle: paragraph
            ]
            // У макета подсветка со скруглением 3 pt; текстовая система даёт прямоугольник.
            if matched { attributes[.backgroundColor] = Tokens.mark }
            out.append(NSAttributedString(string: text, attributes: attributes))
        }
        return out
    }

    private static func metaLine(_ meta: NSAttributedString, topic: String?, badge: String?) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: 0, height: 18))
        var x: CGFloat = 0

        if let badge {
            let plate = missingBadge(badge)
            plate.setFrameOrigin(NSPoint(x: 0, y: 0))
            row.addSubview(plate)
            x = plate.frame.maxX + 6
        }

        let label = Widgets.label(meta)
        label.setFrameOrigin(NSPoint(x: x, y: (18 - label.frame.height) / 2))
        row.addSubview(label)
        x = label.frame.maxX

        if let topic {
            let pill = topicPill(topic)
            pill.setFrameOrigin(NSPoint(x: x + 8, y: 0))
            row.addSubview(pill)
            x = pill.frame.maxX
        }
        row.setFrameSize(NSSize(width: x, height: 18))
        return row
    }

    private static func missingBadge(_ text: String) -> NSView {
        let label = Widgets.label(text, font: Tokens.text(11, .medium), color: Tokens.warn)
        let plate = Widgets.Fill(.clear, radius: 5, border: Tokens.warn)
        plate.frame = NSRect(x: 0, y: 0, width: label.frame.width + 12, height: 18)
        label.setFrameOrigin(NSPoint(x: 6, y: (18 - label.frame.height) / 2))
        plate.addSubview(label)
        return plate
    }

    private static func topicPill(_ topic: String) -> NSView {
        let font = Tokens.text(11)
        let prefix = Widgets.label("тема", font: font, color: Tokens.fg3)
        let name = Widgets.label(topic, font: font, color: Tokens.fg2)

        let pill = Widgets.Fill(.clear, radius: 9, border: Tokens.separatorSoft)
        pill.frame = NSRect(x: 0, y: 0,
                            width: 6 + 9 + 5 + prefix.frame.width + 5 + name.frame.width + 7,
                            height: 18)

        let marker = Widgets.Fill(.clear, radius: 3, border: Tokens.topicMark)
        marker.frame = NSRect(x: 6, y: 6, width: 9, height: 6)
        pill.addSubview(marker)

        prefix.setFrameOrigin(NSPoint(x: 20, y: (18 - prefix.frame.height) / 2))
        pill.addSubview(prefix)
        name.setFrameOrigin(NSPoint(x: prefix.frame.maxX + 5, y: (18 - name.frame.height) / 2))
        pill.addSubview(name)
        return pill
    }

    private static func place(action: NSView, in row: NSView) -> CGFloat {
        action.setFrameOrigin(NSPoint(x: contentWidth - 10 - action.frame.width, y: 10))
        row.addSubview(action)

        let hint = Widgets.label("показать здесь", font: Tokens.text(11), color: Tokens.fg3)
        let cap = Widgets.keyCap("⌘⏎")
        hint.setFrameOrigin(NSPoint(x: contentWidth - 10 - hint.frame.width, y: 44))
        cap.setFrameOrigin(NSPoint(x: hint.frame.minX - 5 - cap.frame.width, y: 42))
        row.addSubview(cap)
        row.addSubview(hint)
        return min(action.frame.minX, cap.frame.minX) - 12
    }

    private static func primaryAction(_ title: String) -> NSView {
        let label = Widgets.label(title, font: Tokens.text(12, .semibold), color: .white)
        let button = Widgets.Fill(Tokens.accent, radius: 7)
        button.frame = NSRect(x: 0, y: 0, width: 10 + label.frame.width + 8 + 18 + 5, height: 26)

        label.setFrameOrigin(NSPoint(x: 10, y: (26 - label.frame.height) / 2))
        button.addSubview(label)

        let capText = Widgets.label("⏎", font: Tokens.text(11), color: .white)
        capText.alignment = .center
        let cap = Widgets.Fill(NSColor(white: 1, alpha: 0.22), radius: 5)
        cap.frame = NSRect(x: label.frame.maxX + 8, y: 4, width: 18, height: 18)
        capText.frame = NSRect(x: 0, y: (18 - capText.frame.height) / 2, width: 18, height: capText.frame.height)
        cap.addSubview(capText)
        button.addSubview(cap)
        return button
    }

    private static func unavailableAction() -> NSView {
        let label = Widgets.label("Открыть недоступно", font: Tokens.text(12, .medium), color: Tokens.fg3)
        let plate = Widgets.Fill(.clear, radius: 7, border: Tokens.separatorSoft)
        plate.frame = NSRect(x: 0, y: 0, width: label.frame.width + 20, height: 26)
        label.setFrameOrigin(NSPoint(x: 10, y: (26 - label.frame.height) / 2))
        plate.addSubview(label)
        return plate
    }

    private static func plain(_ text: String) -> NSAttributedString {
        NSAttributedString(string: text,
                           attributes: [.font: Tokens.text(13.5, .semibold), .foregroundColor: Tokens.fg])
    }
}

extension Tool {
    /// Цвет монограммы. Живёт в слое отрисовки: `Tokens` о видах строк не знает,
    /// а `Tool` — о цветах.
    var color: NSColor {
        switch self {
        case .claudeCode: return Tokens.toolClaude
        case .codex: return Tokens.toolCodex
        case .cursor: return Tokens.toolCursor
        case .chatgpt: return Tokens.toolChatGPT
        }
    }
}
