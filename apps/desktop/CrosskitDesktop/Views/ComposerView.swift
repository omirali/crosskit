import CrosskitKit
import SwiftUI

/// Строка ввода под чатом: слово агенту, быстрый вопрос и что выбрано для следующей беседы —
/// агент, модель, разрешения. Справа — сколько стоила живая беседа.
struct ComposerView: View {

    @Bindable var model: DesktopModel

    var body: some View {
        let composer = model.composer
        let key = model.shownChatKey
        let chat = composer.startsNew ? nil : key.flatMap(model.live.chat)
        let stored = composer.startsNew ? nil : key.flatMap(model.conversation)
        VStack(alignment: .leading, spacing: 8) {
            if let quick = composer.quick {
                QuickCard(model: model, quick: quick)
            }
            HStack(alignment: .bottom, spacing: 8) {
                TextField(placeholder(chat: chat, stored: stored), text: Bindable(model.composer).draft, axis: .vertical)
                    .lineLimit(1...8)
                    .textFieldStyle(.plain)
                    .onSubmit { model.send() }
                    .disabled(chat?.state == .working && !composer.quickOn)
                    .padding(.vertical, 6)
                if chat?.state == .working, !composer.quickOn {
                    Button("Стоп", systemImage: "stop.fill") { chat?.stop() }
                        .buttonStyle(.bordered)
                        .help("Остановить ход агента")
                } else {
                    Button(composer.quickOn ? "Спросить" : chat == nil ? "Начать" : "Отправить") { model.send() }
                        .buttonStyle(.borderedProminent)
                        .tint(composer.quickOn ? .purple : .accentColor)
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(composer.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        .help("Отправить (↩ или ⌘↩)")
                }
            }
            .padding(.leading, 14)
            .padding(.trailing, 8)
            .padding(.vertical, 6)
            .background(composer.quickOn ? AnyShapeStyle(.purple.opacity(0.06)) : AnyShapeStyle(.background), in: .rect(cornerRadius: 14))
            .overlay {
                RoundedRectangle(cornerRadius: 14)
                    .strokeBorder(composer.quickOn ? AnyShapeStyle(.purple.opacity(0.45)) : AnyShapeStyle(.quaternary))
            }
            pickers(chat: chat)
            if let stored, stored.platform == .codex, chat == nil, !composer.quickOn {
                Text("Беседу Codex окно продолжить не может: начнётся новый чат с Claude Code в этой задаче, и агент получит карту как вводную. Чат «\(stored.title)» останется как есть.")
                    .font(.caption).foregroundStyle(.secondary)
                    .padding(8)
                    .background(.quaternary.opacity(0.4), in: .rect(cornerRadius: 8))
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 10)
        .padding(.bottom, 12)
    }

    private func placeholder(chat: ChatModel?, stored: TaskConversation?) -> String {
        if model.composer.quickOn { return "Быстрый вопрос — ответ исчезнет сам и не попадёт в беседы и на карту" }
        if chat?.state == .working { return "Агент работает…" }
        if chat != nil { return "Ответить агенту" }
        if stored?.platform == .claudeCode, stored?.workingDirectory != nil { return "Продолжить этот чат — агент получит карту как вводную" }
        if let task = model.currentTask { return "Новый чат в задаче «\(task.name)»" }
        return "Новый чат"
    }

    private func pickers(chat: ChatModel?) -> some View {
        let composer = model.composer
        let running = chat != nil
        return HStack(spacing: 6) {
            Toggle(isOn: Bindable(model.composer).quickOn) {
                Image(systemName: "bolt.fill")
            }
            .toggleStyle(.button)
            .tint(.purple)
            .help("Быстрый вопрос: ответ не сохранится в беседах и на карте и исчезнет сам")
            if composer.quickOn {
                Picker("Через сколько исчезнет ответ", selection: Bindable(model.composer).quickSeconds) {
                    Text("30 с").tag(30)
                    Text("2 мин").tag(120)
                    Text("10 мин").tag(600)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            } else {
                Menu {
                    Button("Claude Code") {}
                    Button("Codex — окно научится вести его позже") {}.disabled(true)
                } label: {
                    Label("Claude Code", systemImage: "asterisk")
                }
                .help("Агент новой беседы")
                Menu {
                    ForEach(ComposerState.models, id: \.name) { option in
                        Button {
                            model.composer.model = option.id
                        } label: {
                            Text(option.name + (option.id == composer.model ? " ✓" : ""))
                            Text(option.note)
                        }
                    }
                } label: {
                    Text(chat?.target.model.flatMap { id in ComposerState.models.first { $0.id == id }?.name }
                         ?? ComposerState.models.first { $0.id == composer.model }?.name ?? "Модель")
                }
                .disabled(running)
                .help(running ? "Модель выбирается до начала беседы" : "Модель новой беседы")
                Menu {
                    ForEach(ComposerState.modes, id: \.id) { mode in
                        Button {
                            model.composer.permissionMode = mode.id
                        } label: {
                            Text(mode.name + (mode.id == composer.permissionMode ? " ✓" : ""))
                            Text(mode.note)
                        }
                    }
                } label: {
                    Text(ComposerState.modes.first { $0.id == (chat?.target.permissionMode ?? composer.permissionMode) }?.short ?? "")
                }
                .disabled(running)
                .help(running ? "Разрешения выбираются до начала беседы; правки можно разрешить в самом запросе" : "Что агенту можно без спроса")
            }
            Spacer(minLength: 4)
            if let chat, chat.costUSD > 0 {
                Text("этот чат: \(Wording.usd(chat.costUSD))").font(.caption).foregroundStyle(.secondary)
                    .help("Сколько стоили ходы этой беседы в окне, по словам claude")
            }
        }
        .menuStyle(.button)
        .buttonStyle(.bordered)
        .controlSize(.small)
        .fixedSize(horizontal: false, vertical: true)
    }
}

/// Ответ быстрого вопроса: исчезает сам, если его не закрепить.
private struct QuickCard: View {
    let model: DesktopModel
    let quick: QuickRun

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.5)) { context in
            let total = Double(model.composer.quickSeconds)
            let left = quick.answeredAt.map { max(0, total - context.date.timeIntervalSince($0)) } ?? total
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "bolt.fill")
                    Text("Быстрый ответ").fontWeight(.bold)
                    Text("· Claude Code · \(QuickQuestion.model) · только чтение · не сохраняется")
                    Spacer()
                    Text(quick.kept ? "закреплён" : quick.answeredAt == nil ? "ждём ответ" : "исчезнет через \(Int(left.rounded(.up))) с")
                        .fontWeight(.semibold)
                }
                .font(.caption).foregroundStyle(.purple)
                ProgressView(value: quick.kept || quick.answeredAt == nil ? 1 : left / total)
                    .tint(.purple)
                Text(quick.question)
                    .padding(.horizontal, 11).padding(.vertical, 6)
                    .background(.purple.opacity(0.1), in: .rect(cornerRadius: 11))
                    .frame(maxWidth: .infinity, alignment: .trailing)
                if let failure = quick.failure {
                    Text(failure).foregroundStyle(.red).textSelection(.enabled)
                } else if let answer = quick.answer {
                    Text(Markdown.inline(answer)).textSelection(.enabled)
                } else {
                    Text("Думает…").foregroundStyle(.secondary)
                }
                HStack {
                    if !quick.kept, quick.answeredAt != nil {
                        Button("Не удалять") { model.composer.quick?.kept = true }
                    }
                    Button("Скрыть сейчас") { model.composer.quick = nil }
                }
                .controlSize(.small)
            }
            .padding(12)
            .background(.purple.opacity(0.05), in: .rect(cornerRadius: 14))
            .overlay { RoundedRectangle(cornerRadius: 14).strokeBorder(.purple.opacity(0.35)) }
            .onChange(of: context.date) { model.expireQuick(at: context.date) }
        }
        .accessibilityElement(children: .contain)
    }
}
