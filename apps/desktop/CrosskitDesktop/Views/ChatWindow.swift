import CrosskitKit
import SwiftUI

/// Окно беседы с агентом по задаче (ADR 0062): что агент пишет — по мере того, как пишет,
/// каждый вызов инструмента с его результатом, и каждый вопрос о разрешении — человеку.
struct ChatWindow: View {

    @State private var model: ChatModel

    init(target: ChatTarget) {
        _model = State(initialValue: ChatModel(target: target))
    }

    var body: some View {
        VStack(spacing: 0) {
            ChatHeader(model: model)
            Divider()
            if model.state == .ready {
                StartPanel(model: model)
            } else {
                ChatFeed(model: model)
                Divider()
                Composer(model: model)
            }
        }
        .frame(minWidth: 560, minHeight: 560)
        .navigationTitle("Беседа · \(model.target.taskName)")
        .onDisappear { model.end() }
    }
}

private struct ChatHeader: View {
    let model: ChatModel

    var body: some View {
        HStack(spacing: 10) {
            ToolBadge(platform: .claudeCode, size: 26)
            VStack(alignment: .leading, spacing: 1) {
                Text(model.target.taskName).font(.headline).lineLimit(1)
                Text(model.target.directory.replacingOccurrences(of: NSHomeDirectory(), with: "~"))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
            }
            Spacer()
            if model.inTask {
                Label("в задаче", systemImage: "checkmark.circle.fill").font(.caption).foregroundStyle(.green)
            }
            if model.costUSD > 0 {
                Text(String(format: "%.2f $", model.costUSD)).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            }
            Text(stateLabel).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
    }

    private var stateLabel: String {
        switch model.state {
        case .ready: "не начата"
        case .working: "агент работает"
        case .waiting: "ждёт вас"
        case .ended: "закончена"
        }
    }
}

/// Перед началом: где агент работает, что он узнает о задаче и что ему можно без спроса.
private struct StartPanel: View {
    @Bindable var model: ChatModel

    var body: some View {
        Form {
            Section {
                LabeledContent("Каталог", value: model.target.directory.replacingOccurrences(of: NSHomeDirectory(), with: "~"))
                if let session = model.target.resume {
                    LabeledContent("Продолжить беседу", value: String(session.prefix(8)))
                }
                Picker("Разрешения", selection: $model.permissionMode) {
                    Text("Спрашивать, как в ваших настройках Claude Code").tag("default")
                    Text("Разрешать правки файлов без спроса").tag("acceptEdits")
                    Text("Только план, без правок").tag("plan")
                }
            } footer: {
                Text("Это настоящая беседа Claude Code: с вашими настройками, правилами и MCP. Её журнал сохраняется, служба его прочитает, и беседа войдёт в задачу. Лимит подписки тратится, как в терминале.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if let brief = model.target.brief, model.target.resume == nil {
                Section("Вводная, которую получит агент") {
                    ScrollView {
                        Text(brief).font(.caption).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(height: 220)
                }
            }
            Section {
                Button(model.target.resume == nil ? "Начать беседу" : "Продолжить беседу") { model.begin() }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .formStyle(.grouped)
    }
}

private struct ChatFeed: View {
    let model: ChatModel

    var body: some View {
        ScrollViewReader { scroll in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(model.items) { item in
                        ChatRow(item: item, model: model).id(item.id)
                    }
                    if case .ended(let reason) = model.state {
                        Text(reason.map { "Беседа закончилась: \($0)" } ?? "Беседа закончилась.")
                            .font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity)
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(14)
            }
            .onChange(of: model.items.count) { scroll.scrollTo("bottom", anchor: .bottom) }
            .onChange(of: tailLength) { scroll.scrollTo("bottom", anchor: .bottom) }
        }
    }

    /// Длина последнего куска текста: лента едет вниз и пока ответ ещё пишется.
    private var tailLength: Int {
        if case .assistant(_, let text, _)? = model.items.last { return text.count }
        return 0
    }
}

private struct ChatRow: View {
    let item: ChatItem
    let model: ChatModel

    var body: some View {
        switch item {
        case .user(_, let text):
            Text(text).textSelection(.enabled)
                .padding(10)
                .background(.tint.opacity(0.15), in: .rect(cornerRadius: 12))
                .frame(maxWidth: .infinity, alignment: .trailing)
        case .assistant(_, let text, let streaming):
            HStack(alignment: .top, spacing: 8) {
                ToolBadge(platform: .claudeCode, size: 18)
                Text(markdown(text)).textSelection(.enabled)
                if streaming { ProgressView().controlSize(.mini) }
            }
        case .thinking(_, let text):
            DisclosureGroup("Размышления") {
                Text(text).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            }
            .font(.caption).foregroundStyle(.secondary)
        case .tool(_, let name, let input, let result, let failed):
            ToolCard(name: name, input: input, result: result, failed: failed)
        case .permission(let request, let answer):
            PermissionCard(request: request, answer: answer) { model.answer(request, allow: $0) }
        case .note(_, let text):
            Text(text).font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity)
        }
    }

    /// Текст агента как markdown внутри строки: куски, которые ещё пишутся, тоже читаются.
    private func markdown(_ text: String) -> AttributedString {
        (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
    }
}

/// Вызов инструмента: что агент сделал и что получил.
private struct ToolCard: View {
    let name: String
    let input: String
    let result: String?
    let failed: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "wrench.and.screwdriver").foregroundStyle(.secondary)
                Text(name).font(.callout.weight(.semibold))
                Text(ToolInput.summary(input)).font(.system(.callout, design: .monospaced))
                    .foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                Spacer()
                if result == nil { ProgressView().controlSize(.mini) }
            }
            if let result {
                DisclosureGroup(failed ? "Ошибка" : "Результат") {
                    Text(result.count > 4000 ? String(result.prefix(4000)) + "\n…" : result)
                        .font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .font(.caption).foregroundStyle(failed ? .red : .secondary)
            }
        }
        .padding(10)
        .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 10))
    }
}

/// Вопрос агента о разрешении: решает человек.
private struct PermissionCard: View {
    let request: PermissionRequest
    let answer: Bool?
    let decide: (Bool) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Агент просит разрешения: \(request.tool)", systemImage: "hand.raised").font(.callout.weight(.semibold))
            Text(ToolInput.pretty(request.input)).font(.system(.caption, design: .monospaced))
                .textSelection(.enabled).lineLimit(12)
            switch answer {
            case nil:
                HStack {
                    Button("Разрешить") { decide(true) }.buttonStyle(.borderedProminent)
                    Button("Отклонить", role: .destructive) { decide(false) }
                }
            case true?:
                Label("Разрешено", systemImage: "checkmark").font(.caption).foregroundStyle(.green)
            case false?:
                Label("Отклонено", systemImage: "xmark").font(.caption).foregroundStyle(.red)
            }
        }
        .padding(10)
        .background(.orange.opacity(answer == nil ? 0.15 : 0.06), in: .rect(cornerRadius: 10))
    }
}

/// Вход инструмента для человека: главное одной строкой и всё — отступами.
enum ToolInput {
    static func summary(_ json: String) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any] else { return json }
        for key in ["command", "file_path", "path", "pattern", "url", "query", "description"] {
            if let value = object[key] as? String { return value }
        }
        return json
    }

    static func pretty(_ json: String) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        else { return json }
        return String(decoding: data, as: UTF8.self)
    }
}

private struct Composer: View {
    @Bindable var model: ChatModel

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField(placeholder, text: $model.draft, axis: .vertical)
                .lineLimit(1...8)
                .textFieldStyle(.plain)
                .padding(8)
                .background(.quaternary.opacity(0.4), in: .rect(cornerRadius: 10))
                .disabled(!canType)
            if model.state == .working {
                Button("Стоп", systemImage: "stop.fill") { model.stop() }
                    .help("Остановить ход агента")
            } else {
                Button("Отправить", systemImage: "arrow.up.circle.fill") { model.send() }
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(!canType || model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    .help("Отправить (⌘↩)")
            }
        }
        .labelStyle(.iconOnly)
        .font(.title3)
        .padding(10)
    }

    private var canType: Bool { model.state == .waiting }

    private var placeholder: String {
        switch model.state {
        case .working: "Агент работает…"
        case .ended: "Беседа закончена"
        default: "Слово агенту (⌘↩ — отправить)"
        }
    }
}
