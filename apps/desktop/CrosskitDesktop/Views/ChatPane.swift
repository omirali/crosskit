import CrosskitKit
import SwiftUI

/// Чат справа (макет `design/UXMain.dc.html`): беседа задачи с номерами реплик и тем, что из
/// них стало на карте, а ниже — живое продолжение, которое окно ведёт само (ADR 0062, 0063).
struct ChatPane: View {

    @Bindable var model: DesktopModel
    let terminal: TerminalModel

    var body: some View {
        VStack(spacing: 0) {
            ChatHeader(model: model)
            Divider()
            ChatFeed(model: model)
                .frame(maxHeight: .infinity)
                .overlay(alignment: .topTrailing) {
                    if model.inbox.isOpen {
                        InboxPanel(model: model)
                            .frame(width: 400)
                            .padding(8)
                            .transition(.move(edge: .trailing).combined(with: .opacity))
                    }
                }
            ComposerView(model: model)
        }
        .background(.background)
    }
}

/// Шапка чата: список чатов, какая беседа открыта и колокольчик.
private struct ChatHeader: View {
    @Bindable var model: DesktopModel

    var body: some View {
        let key = model.shownChatKey
        let stored = key.flatMap(model.conversation)
        let live = key.flatMap(model.live.chat)
        HStack(spacing: 10) {
            Toggle(isOn: $model.showsChats) {
                Label("Чаты · \(model.scopeConversations.count)", systemImage: "sidebar.squares.left")
            }
            .toggleStyle(.button)
            .help(model.showsChats ? "Скрыть список чатов" : "Показать список чатов")
            if model.composer.startsNew {
                ToolBadge(platform: .claudeCode)
                Text("Новый чат").fontWeight(.semibold)
            } else if key != nil {
                ToolBadge(platform: stored?.platform ?? .claudeCode)
                VStack(alignment: .leading, spacing: 0) {
                    Text(stored?.title ?? live?.target.taskName.map { "Новый чат · \($0)" } ?? "Новый чат")
                        .fontWeight(.semibold).lineLimit(1)
                    HStack(spacing: 0) {
                        Text("\(Wording.tool(stored?.platform)) · \(Wording.day(stored.flatMap { $0.lastAtUnixMs }.map(Date.init(unixMilliseconds:)) ?? .now))")
                        if let target = model.target {
                            Text(" · реплика \(target)").foregroundStyle(.tint).fontWeight(.semibold)
                        }
                        if let live {
                            Text(" · " + Self.state(live)).foregroundStyle(live.pendingPermissions.isEmpty ? Color.secondary : .orange)
                        }
                    }
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 4)
            if model.target != nil {
                Button("К концу") { model.target = nil }
                    .controlSize(.small)
            }
            InboxBell(model: model)
        }
        .padding(.horizontal, 12)
        .frame(height: 52)
    }

    static func state(_ chat: ChatModel) -> String {
        if !chat.pendingPermissions.isEmpty { return "ждёт разрешения" }
        switch chat.state {
        case .ready: return "не начата"
        case .working: return "агент работает"
        case .waiting: return "ждёт вас"
        case .ended: return "закончена"
        }
    }
}

private struct InboxBell: View {
    let model: DesktopModel

    var body: some View {
        let unread = model.inboxItems.filter { !model.inbox.isRead($0.id) }.count
        Button {
            withAnimation(.snappy) { model.inbox.isOpen.toggle() }
        } label: {
            Image(systemName: unread > 0 ? "bell.badge" : "bell")
                .symbolRenderingMode(unread > 0 ? .multicolor : .monochrome)
                .font(.title3)
                .overlay(alignment: .topTrailing) {
                    if unread > 0 {
                        Text("\(unread)").font(.caption2.weight(.bold)).foregroundStyle(.white)
                            .padding(.horizontal, 4).frame(minWidth: 16, minHeight: 16)
                            .background(.red, in: .capsule)
                            .offset(x: 9, y: -7)
                    }
                }
        }
        .buttonStyle(.borderless)
        .help("Центр уведомлений (⇧⌘U)")
        .accessibilityLabel("Уведомления: \(unread) непрочитанных")
    }
}

/// Лента: сохранённые реплики беседы, затем живые события окна.
private struct ChatFeed: View {
    let model: DesktopModel

    var body: some View {
        let key = model.shownChatKey
        ScrollViewReader { scroll in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    if model.composer.startsNew {
                        NewChatStart(model: model)
                    } else if let key {
                        History(model: model, key: key)
                        if let chat = model.live.chat(key) {
                            ForEach(chat.items) { item in
                                LiveRow(item: item, chat: chat, model: model).id(item.id)
                            }
                            if case .ended(let reason) = chat.state {
                                Text(reason.map { "Беседа закончилась: \($0)" } ?? "Беседа закончилась.")
                                    .font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity)
                            }
                        }
                    } else {
                        ContentUnavailableView("Чатов пока нет", systemImage: "bubble.left",
                                               description: Text("Начните чат ниже — агент получит карту задачи, если она построена."))
                    }
                    Color.clear.frame(height: 1).id("bottom")
                }
                .padding(.vertical, 14)
                .padding(.leading, 6)
                .padding(.trailing, 18)
            }
            .onChange(of: model.target, initial: true) { scrollToTarget(scroll) }
            .onChange(of: key) { scrollToTarget(scroll) }
            .onChange(of: liveLength(key)) { if model.target == nil { scroll.scrollTo("bottom", anchor: .bottom) } }
            .onChange(of: historyLoaded(key)) { scrollToTarget(scroll) }
        }
    }

    private func scrollToTarget(_ scroll: ScrollViewProxy) {
        Task { @MainActor in
            if let target = model.target {
                withAnimation { scroll.scrollTo(History.replyID(target), anchor: .center) }
            } else {
                scroll.scrollTo("bottom", anchor: .bottom)
            }
        }
    }

    /// Сколько живого текста: лента едет вниз, пока ответ пишется.
    private func liveLength(_ key: String?) -> Int {
        guard let chat = key.flatMap(model.live.chat) else { return 0 }
        if case .assistant(_, let text, _)? = chat.items.last { return chat.items.count * 10_000 + text.count }
        return chat.items.count * 10_000
    }

    private func historyLoaded(_ key: String?) -> Bool {
        guard let key, let task = model.taskID(of: key) else { return false }
        return model.speech(of: task).value != nil
    }
}

/// Сохранённая беседа: реплики, которые прочитал граф, с узлами карты у каждой.
private struct History: View {
    let model: DesktopModel
    let key: String

    static func replyID(_ n: Int) -> String { "reply-\(n)" }

    var body: some View {
        if let taskID = model.taskID(of: key) {
            let mapped = model.map(of: taskID).value?.answer.conversations.first { $0.session == key }
            let tree = model.tree(of: taskID)
            switch model.speech(of: taskID) {
            case .idle, .loading:
                ProgressView("Читаю беседу…").frame(maxWidth: .infinity).padding(.vertical, 30)
            case .failed(let error):
                Text("Реплики не прочитаны: \(Wording.failure(error).title.lowercased()). \(Wording.failure(error).explanation)")
                    .font(.callout).foregroundStyle(.secondary).padding(.leading, 42)
            case .loaded(let speech):
                if let spoken = speech.conversation(key) {
                    let cutoff = model.live.chat(key)?.repliesBefore ?? Int.max
                    let through = mapped?.mappedThrough ?? 0
                    if through == 0 {
                        Banner(text: "Этот чат ещё не прочитан Crosskit — на карте его нет.")
                    }
                    ForEach(spoken.replies.filter { $0.n <= cutoff }) { reply in
                        ReplyRow(model: model, taskID: taskID, reply: reply,
                                 anchors: tree?.anchors(session: key, reply: reply.n) ?? [],
                                 isTarget: model.target == reply.n)
                            .id(Self.replyID(reply.n))
                        if through > 0, reply.n == through, through < spoken.replies.count {
                            Banner(text: "\(Wording.count(spoken.replies.count - through, "реплика", "реплики", "реплик")) после \(through)-й ещё не прочитаны — на карте их нет")
                        }
                    }
                } else {
                    Text("Граф ещё не собрал реплики этой беседы.").font(.callout).foregroundStyle(.secondary).padding(.leading, 42)
                }
            }
        }
    }
}

private struct Banner: View {
    let text: String

    var body: some View {
        Text(text).font(.callout).foregroundStyle(.orange)
            .padding(.horizontal, 12).padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.orange.opacity(0.1), in: .rect(cornerRadius: 10))
            .padding(.leading, 42)
    }
}

/// До первого слова новой беседы: где агент будет работать и что он узнает о задаче.
private struct NewChatStart: View {
    let model: DesktopModel

    var body: some View {
        let directory = model.taskDirectory(model.taskID) ?? model.scopeDirectories.first
        let brief = model.taskID.flatMap(model.tree(of:)).map(TaskBrief.of)
        VStack(alignment: .leading, spacing: 10) {
            Text("Новый чат с Claude Code").font(.title3.weight(.semibold))
            Text(directory.map { "Агент будет работать в \(Wording.path($0))." } ?? "Каталог окно спросит при первом слове.")
                .foregroundStyle(.secondary)
            Text("Это настоящая беседа Claude Code: с вашими настройками, правилами и MCP. Её журнал сохранится, Crosskit его прочитает, и чат войдёт в задачу. Лимит подписки тратится, как в терминале.")
                .font(.callout).foregroundStyle(.secondary)
            if let brief {
                DisclosureGroup("Вводная, которую получит агент") {
                    Text(brief).font(.caption).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 4)
                }
            } else if model.taskID != nil {
                Text("Карты у задачи пока нет — агент начнёт без вводной.").font(.callout).foregroundStyle(.secondary)
            }
            Button("Отмена") { model.composer.startsNew = false }
                .controlSize(.small)
        }
        .padding(14)
        .background(.quaternary.opacity(0.35), in: .rect(cornerRadius: 12))
        .padding(.leading, 42)
    }
}
