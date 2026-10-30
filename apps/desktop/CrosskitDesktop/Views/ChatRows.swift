import CrosskitKit
import SwiftUI

/// Реплика сохранённой беседы: номер, кто сказал, текст и узлы карты, которые она доказала.
struct ReplyRow: View {
    let model: DesktopModel
    let taskID: String
    let reply: SpokenReply
    let anchors: [MapAnchor]
    let isTarget: Bool
    @State private var whole = false

    /// Длиннее этого реплика показывается началом: целиком — по кнопке.
    private static let shown = 1_600

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Text("\(reply.n)")
                .font(.caption.monospacedDigit()).foregroundStyle(.tertiary)
                .frame(width: 34, alignment: .trailing)
                .padding(.top, 7)
            VStack(alignment: reply.owner ? .trailing : .leading, spacing: 6) {
                let long = reply.text.count > Self.shown && !whole
                let text = long ? String(reply.text.prefix(Self.shown)) + "…" : reply.text
                if reply.owner {
                    Text(text).textSelection(.enabled)
                        .padding(.horizontal, 12).padding(.vertical, 8)
                        .background(.quaternary.opacity(0.6), in: .rect(cornerRadius: 12))
                        .frame(maxWidth: .infinity, alignment: .trailing)
                        .padding(.leading, 40)
                } else {
                    Text(Markdown.inline(text)).textSelection(.enabled).lineSpacing(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                if long {
                    Button("Показать целиком") { whole = true }.buttonStyle(.link).font(.caption)
                }
                ForEach(anchors, id: \.self) { anchor in
                    AnchorChip(model: model, taskID: taskID, anchor: anchor)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(isTarget ? AnyShapeStyle(.tint.opacity(0.1)) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 12))
            .overlay {
                if isTarget { RoundedRectangle(cornerRadius: 12).strokeBorder(.tint) }
            }
        }
    }
}

/// «на карте: шаг 2 · Где считать скидку?» — переход к узлу, который доказан этой репликой.
private struct AnchorChip: View {
    let model: DesktopModel
    let taskID: String
    let anchor: MapAnchor

    var body: some View {
        let tree = model.tree(of: taskID)
        let on = model.focus == anchor
        let (glyph, color, label): (String, Color, String) = switch anchor {
        case .step(let id):
            tree.flatMap { $0.steps.indices.contains(id) ? $0.steps[id] : nil }
                .map { (Wording.mark($0.state), Palette.color($0.state), "шаг \(id + 1) · \($0.question)") } ?? ("", .secondary, "")
        case .risk(let id):
            tree?.risks.first { $0.id == id }
                .map { ("●", Palette.color($0.level), "\(Wording.label($0.level).lowercased()) · \($0.title)") } ?? ("", .secondary, "")
        }
        Button {
            model.focus = anchor
            model.pane = .map
        } label: {
            HStack(spacing: 6) {
                Text(glyph).fontWeight(.bold).foregroundStyle(color)
                Text("на карте:").foregroundStyle(.secondary)
                Text(label).fontWeight(.semibold).lineLimit(1)
            }
            .font(.caption)
            .padding(.horizontal, 9).padding(.vertical, 3)
            .background(on ? AnyShapeStyle(.tint.opacity(0.12)) : AnyShapeStyle(.quaternary.opacity(0.6)), in: .rect(cornerRadius: 8))
            .overlay {
                if on { RoundedRectangle(cornerRadius: 8).strokeBorder(.tint) }
            }
        }
        .buttonStyle(.plain)
        .help("Показать узел на карте")
    }
}

/// Событие живой беседы.
struct LiveRow: View {
    let item: ChatItem
    let chat: ChatModel
    let model: DesktopModel

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Color.clear.frame(width: 34, height: 1)
            content.frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder
    private var content: some View {
        switch item {
        case .user(_, let text):
            Text(text).textSelection(.enabled)
                .padding(.horizontal, 12).padding(.vertical, 8)
                .background(.tint.opacity(0.14), in: .rect(cornerRadius: 12))
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.leading, 40)
        case .assistant(_, let text, let streaming):
            HStack(alignment: .top, spacing: 6) {
                Text(Markdown.inline(text)).textSelection(.enabled).lineSpacing(2)
                if streaming { ProgressView().controlSize(.mini) }
            }
            .padding(.horizontal, 8)
        case .thinking(_, let text):
            DisclosureGroup("Размышления") {
                Text(text).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            }
            .font(.caption).foregroundStyle(.secondary).padding(.horizontal, 8)
        case .tool(_, let name, let input, let result, let failed):
            ToolRow(activity: ToolActivity(tool: name, input: input, directory: chat.target.directory),
                    result: result, failed: failed)
        case .permission(let request, let answer):
            PermissionCard(request: request, answer: answer, directory: chat.target.directory) { chat.answer(request, $0) }
        case .note(_, let text):
            Text(text).font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity)
        }
    }
}

/// Вызов инструмента: что агент сделал, с чем, и по нажатию — разница или вывод.
private struct ToolRow: View {
    let activity: ToolActivity
    let result: String?
    let failed: Bool
    @State private var open: Bool?

    var body: some View {
        let expandable = !activity.lines.isEmpty || !(result ?? "").isEmpty
        // Правка раскрыта сразу: её и надо видеть; вывод команды — по нажатию.
        let isOpen = expandable && (open ?? (activity.kind == .edit && activity.lines.count <= 40))
        VStack(alignment: .leading, spacing: 6) {
            Button {
                open = !isOpen
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: Self.symbol(activity.kind)).frame(width: 20, height: 20)
                        .background(.quaternary.opacity(0.7), in: .rect(cornerRadius: 6))
                    Text(activity.verb).foregroundStyle(.secondary)
                    Text(activity.subject).font(.system(.callout, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                    if activity.added > 0 { Text("+\(activity.added)").font(.caption.monospaced()).foregroundStyle(.green) }
                    if activity.removed > 0 { Text("−\(activity.removed)").font(.caption.monospaced()).foregroundStyle(.red) }
                    note
                    if expandable { Image(systemName: isOpen ? "chevron.down" : "chevron.right").font(.caption).foregroundStyle(.tertiary) }
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if isOpen {
                if !activity.lines.isEmpty {
                    DiffView(title: activity.subject, lines: activity.lines)
                } else if let result {
                    Text(result.count > 6_000 ? String(result.prefix(6_000)) + "\n…" : result)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(failed ? Color.red : Color(nsColor: .init(white: 0.91, alpha: 1)))
                        .textSelection(.enabled)
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color(nsColor: .init(white: 0.11, alpha: 1)), in: .rect(cornerRadius: 10))
                }
            }
        }
        .padding(.horizontal, 8)
    }

    @ViewBuilder
    private var note: some View {
        if result == nil {
            ProgressView().controlSize(.mini)
        } else if failed {
            Text("ошибка").font(.caption).foregroundStyle(.red)
        } else if activity.kind == .read, let result {
            Text(Wording.count(result.split(separator: "\n").count, "строка", "строки", "строк")).font(.caption).foregroundStyle(.secondary)
        } else if activity.kind == .run {
            Text("✓").font(.caption).foregroundStyle(.green)
        }
    }

    static func symbol(_ kind: ToolActivity.Kind) -> String {
        switch kind {
        case .read: "doc.text"
        case .run: "terminal"
        case .edit: "plusminus"
        case .create: "doc.badge.plus"
        case .search: "magnifyingglass"
        case .other: "wrench.and.screwdriver"
        }
    }
}

/// Запрос разрешения: что агент хочет сделать, сама правка — и решает человек.
private struct PermissionCard: View {
    let request: PermissionRequest
    let answer: PermissionAnswer?
    let directory: String
    let decide: (PermissionAnswer) -> Void

    var body: some View {
        let activity = ToolActivity(tool: request.tool, input: request.input, directory: directory)
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(Self.title(activity)).fontWeight(.bold)
                Text(activity.subject).font(.system(.callout, design: .monospaced)).lineLimit(2)
                if activity.added > 0 { Text("+\(activity.added)").font(.caption.monospaced()).foregroundStyle(.green) }
                if activity.removed > 0 { Text("−\(activity.removed)").font(.caption.monospaced()).foregroundStyle(.red) }
            }
            if !activity.lines.isEmpty {
                DiffView(title: nil, lines: Array(activity.lines.prefix(80)))
            } else if activity.kind != .run {
                Text(ToolInput.pretty(request.input)).font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled).lineLimit(12)
            }
            switch answer {
            case nil:
                HStack(spacing: 8) {
                    Button("Разрешить") { decide(.allowed) }
                        .buttonStyle(.borderedProminent)
                        .keyboardShortcut(.return, modifiers: [.command, .shift])
                    if ChatModel.editTools.contains(request.tool) {
                        Button("Разрешать правки в этом чате") { decide(.allowedEdits) }
                    }
                    Button("Отклонить") { decide(.denied) }
                }
                .controlSize(.small)
            case .allowed?:
                Text("Разрешено").font(.callout.weight(.semibold)).foregroundStyle(.green)
            case .allowedEdits?:
                Text("Разрешено · правки в этом чате больше не спрашиваются").font(.callout.weight(.semibold)).foregroundStyle(.green)
            case .denied?:
                Text("Вы отклонили — агент предложит другой путь").font(.callout.weight(.semibold)).foregroundStyle(.red)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.orange.opacity(answer == nil ? 0.08 : 0.03), in: .rect(cornerRadius: 12))
        .overlay { RoundedRectangle(cornerRadius: 12).strokeBorder(.orange.opacity(answer == nil ? 0.6 : 0.2)) }
    }

    static func title(_ activity: ToolActivity) -> String {
        switch activity.kind {
        case .create: "Агент хочет записать файл"
        case .edit: "Агент хочет изменить файл"
        case .run: "Агент хочет запустить"
        case .read: "Агент хочет прочитать"
        case .search, .other: "Агент просит разрешения: \(activity.verb)"
        }
    }
}

/// Разница: номера строк по обе стороны, добавленное зелёным, убранное красным.
struct DiffView: View {
    let title: String?
    let lines: [DiffLine]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let title {
                Text(title).font(.caption.weight(.semibold))
                    .padding(.horizontal, 10).padding(.vertical, 5)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.quaternary.opacity(0.4))
            }
            ScrollView(.horizontal) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                        HStack(spacing: 0) {
                            Text(line.old.map(String.init) ?? "").frame(width: 36, alignment: .trailing).foregroundStyle(.tertiary)
                            Text(line.new.map(String.init) ?? "").frame(width: 36, alignment: .trailing).foregroundStyle(.tertiary)
                            Text(Self.sign(line.kind)).frame(width: 18).foregroundStyle(Self.color(line.kind))
                            Text(line.text).foregroundStyle(line.kind == .hunk ? .secondary : .primary).fixedSize()
                            Spacer(minLength: 12)
                        }
                        .padding(.vertical, 0.5)
                        .background(Self.color(line.kind).opacity(line.kind == .context || line.kind == .hunk ? 0 : 0.1))
                    }
                }
                .font(.system(.caption, design: .monospaced))
                .textSelection(.enabled)
            }
        }
        .clipShape(.rect(cornerRadius: 9))
        .overlay { RoundedRectangle(cornerRadius: 9).strokeBorder(.quaternary) }
    }

    static func sign(_ kind: DiffLine.Kind) -> String {
        switch kind {
        case .added: "+"
        case .removed: "−"
        case .context, .hunk: ""
        }
    }

    static func color(_ kind: DiffLine.Kind) -> Color {
        switch kind {
        case .added: .green
        case .removed: .red
        case .context, .hunk: .secondary
        }
    }
}

/// Вход инструмента для человека: всё — отступами.
enum ToolInput {
    static func pretty(_ json: String) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        else { return json }
        return String(decoding: data, as: UTF8.self)
    }
}

/// Текст агента как markdown внутри строки: куски, которые ещё пишутся, тоже читаются.
enum Markdown {
    static func inline(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
    }
}
