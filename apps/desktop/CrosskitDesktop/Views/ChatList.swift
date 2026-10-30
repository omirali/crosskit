import AppKit
import CrosskitKit
import SwiftUI

/// Чаты того, что открыто: у задачи — её беседы, у проекта — по задачам. Сверху — что
/// предлагает сшивка, снизу — новый чат.
struct ChatList: View {

    let model: DesktopModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.currentTask != nil ? "ЧАТЫ ЗАДАЧИ" : "ЧАТЫ ПРОЕКТА").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                Text(model.currentTask?.name ?? model.project?.name ?? "").font(.headline).lineLimit(2)
            }
            .padding(.horizontal, 14)
            .padding(.top, 14)
            .padding(.bottom, 8)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    let known = Set(model.scopeConversations.map(\.session))
                    let fresh = model.live.fresh(known: known).filter { model.taskID == nil || $0.chat.target.taskID == model.taskID }
                    if !fresh.isEmpty {
                        group("ИДУТ СЕЙЧАС")
                        ForEach(fresh, id: \.key) { key, chat in
                            row(key: key, platform: .claudeCode, title: "Новый чат" + (chat.target.taskName.map { " · \($0)" } ?? ""),
                                sub: ChatListNote.live(chat), noteColor: .orange)
                        }
                    }
                    ForEach(groups, id: \.title) { group in
                        if let title = group.title { self.group(title) }
                        let proposed = group.tasks.flatMap { task in task.proposed.map { (task, $0) } }
                        if !proposed.isEmpty {
                            DisclosureGroup {
                                ForEach(proposed, id: \.1.session) { task, conversation in
                                    ProposedRow(model: model, task: task, conversation: conversation)
                                }
                            } label: {
                                Text("Сшивка предлагает: \(proposed.count)").font(.callout.weight(.semibold)).foregroundStyle(.purple)
                            }
                            .padding(.horizontal, 6).padding(.vertical, 4)
                        }
                        ForEach(group.conversations) { conversation in
                            let note = ChatListNote.of(conversation, in: model)
                            row(key: conversation.session, platform: conversation.platform, title: conversation.title,
                                sub: note.text, noteColor: note.color)
                                .contextMenu { menu(conversation) }
                        }
                    }
                }
                .padding(.horizontal, 8)
                .padding(.bottom, 10)
            }
            Divider()
            Button {
                model.composeNew()
            } label: {
                Label("Новый чат", systemImage: "plus").frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
            .padding(12)
        }
        .background(.background.secondary)
    }

    private struct Group {
        let title: String?
        let tasks: [ConfirmedTask]
        let conversations: [TaskConversation]
    }

    private var groups: [Group] {
        if let task = model.currentTask {
            return [Group(title: nil, tasks: [task], conversations: model.scopeConversations)]
        }
        return (model.project?.taskIDs ?? []).compactMap(model.task).map { task in
            Group(title: task.name.uppercased(), tasks: [task],
                  conversations: task.conversations.sorted { ($0.lastAtUnixMs ?? 0) > ($1.lastAtUnixMs ?? 0) })
        }
    }

    private func group(_ title: String) -> some View {
        Text(title).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            .padding(.horizontal, 6).padding(.top, 8).padding(.bottom, 2)
    }

    private func row(key: String, platform: Platform, title: String, sub: String?, noteColor: Color) -> some View {
        let current = !model.composer.startsNew && model.shownChatKey == key
        return Button {
            model.composer.startsNew = false
            model.chatKey = key
            model.target = nil
        } label: {
            HStack(alignment: .top, spacing: 8) {
                ToolBadge(platform: platform, size: 20)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).fontWeight(.semibold).lineLimit(1)
                    if let conversation = model.conversation(key) {
                        Text("\(Wording.day(conversation.lastAtUnixMs.map(Date.init(unixMilliseconds:)))) · \(ChatListNote.replies(of: conversation, in: model))")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    if let sub {
                        Text(sub).font(.caption.weight(.semibold)).foregroundStyle(noteColor)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(8)
            .background(current ? AnyShapeStyle(.tint.opacity(0.14)) : AnyShapeStyle(.clear), in: .rect(cornerRadius: 9))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private func menu(_ conversation: TaskConversation) -> some View {
        Button("Скопировать команду продолжения") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(Wording.resume(session: conversation.session, platform: conversation.platform), forType: .string)
        }
        if let taskID = model.taskID(of: conversation.session) {
            Button("Убрать из задачи") {
                Task { await model.decide(.drop(task: taskID, conversation: conversation.session)) }
            }
        }
    }
}

/// Беседа, которую сшивка связывает с задачей, а владелец ещё не решил.
private struct ProposedRow: View {
    let model: DesktopModel
    let task: ConfirmedTask
    let conversation: TaskConversation

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                ToolBadge(platform: conversation.platform, size: 20)
                Text(conversation.title).fontWeight(.semibold).lineLimit(2)
            }
            Text("Сшивка предлагает в задачу" + (conversation.signals.isEmpty ? "" : ": " + conversation.signals.joined(separator: ", ")))
                .font(.caption).foregroundStyle(.purple)
            HStack {
                Button("Добавить") { Task { await model.decide(.add(task: task.id, conversation: conversation.session)) } }
                Button("Не из этой задачи") { Task { await model.decide(.drop(task: task.id, conversation: conversation.session)) } }
            }
            .controlSize(.small)
        }
        .padding(8)
        .background(.purple.opacity(0.06), in: .rect(cornerRadius: 9))
    }
}

/// Что сказать у беседы в списке: где она на карте и что в ней ждёт.
enum ChatListNote {

    static func of(_ conversation: TaskConversation, in model: DesktopModel) -> (text: String?, color: Color) {
        if let chat = model.live.chat(conversation.session) { return (live(chat), .orange) }
        guard let taskID = model.taskID(of: conversation.session) else { return (nil, .secondary) }
        if let tree = model.tree(of: taskID), let waiting = tree.waitingStep,
           waiting.branches.contains(where: { $0.quote.session == conversation.session }) {
            return ("здесь ждут вашего решения", .accentColor)
        }
        guard let mapped = model.map(of: taskID).value?.answer.conversations.first(where: { $0.session == conversation.session })
        else { return (nil, .secondary) }
        if mapped.mappedThrough == 0 { return ("ещё не на карте", .orange) }
        if mapped.mappedThrough < mapped.replies {
            return (Wording.count(mapped.replies - mapped.mappedThrough, "новая реплика не на карте", "новые реплики не на карте", "новых реплик не на карте"), .orange)
        }
        return (nil, .secondary)
    }

    static func live(_ chat: ChatModel) -> String {
        if !chat.pendingPermissions.isEmpty { return "агент ждёт разрешения" }
        switch chat.state {
        case .ready, .waiting: return "идёт в окне · ждёт вас"
        case .working: return "идёт в окне · агент работает"
        case .ended: return "закончена в окне"
        }
    }

    static func replies(of conversation: TaskConversation, in model: DesktopModel) -> String {
        guard let taskID = model.taskID(of: conversation.session),
              let count = model.map(of: taskID).value?.answer.conversations.first(where: { $0.session == conversation.session })?.replies
        else { return Wording.tool(conversation.platform) }
        return Wording.count(count, "реплика", "реплики", "реплик")
    }
}
