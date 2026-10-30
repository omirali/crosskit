import CrosskitKit
import SwiftUI

/// Проекты и их задачи; внизу — контекст. Точка у задачи говорит, где она стоит: синяя — ждёт
/// вашего решения, красная — с открытым критичным риском, зелёная — в работе, серая — карты нет.
struct ProjectSidebar: View {

    @Bindable var model: DesktopModel
    @State private var query = ""

    var body: some View {
        List(selection: Binding(get: { model.scope }, set: { if let scope = $0 { model.open(scope) } })) {
            if query.count >= 2 {
                SearchResults(model: model, query: query) { query = "" }
            } else {
                ForEach(model.projects) { project in
                    Section {
                        projectRow(project)
                        ForEach(project.taskIDs, id: \.self) { id in
                            if let task = model.task(id) {
                                taskRow(task)
                            }
                        }
                        if !project.candidateSessions.isEmpty {
                            Button {
                                model.pane = .board
                            } label: {
                                Label(Wording.count(project.candidateSessions.count, "предложение задачи", "предложения задач", "предложений задач"),
                                      systemImage: "sparkles")
                                    .font(.callout).foregroundStyle(.purple)
                            }
                            .buttonStyle(.plain)
                            .padding(.leading, 18)
                        }
                    }
                }
            }
        }
        .searchable(text: $query, placement: .sidebar, prompt: "Задачи, чаты, реплики")
        .safeAreaInset(edge: .bottom) { ContextSwitch(model: model) }
    }

    private func projectRow(_ project: Project) -> some View {
        HStack(spacing: 6) {
            Label(project.name, systemImage: project.root.isEmpty ? "tray" : "folder")
                .fontWeight(.semibold)
            Spacer()
            Text("\(project.taskIDs.count)").font(.caption).foregroundStyle(.secondary)
        }
        .help(project.root.isEmpty ? "Задачи, у бесед которых нет каталога на диске" : Wording.path(project.root))
        .tag(Scope.project(project.root))
    }

    private func taskRow(_ task: ConfirmedTask) -> some View {
        HStack(spacing: 8) {
            Circle().fill(Palette.dot(task)).frame(width: 7, height: 7)
            Text(task.name).lineLimit(1)
            Spacer(minLength: 4)
            if model.live.all.contains(where: { $0.chat.target.taskID == task.id && !$0.chat.pendingPermissions.isEmpty }) {
                Image(systemName: "hand.raised.fill").font(.caption2).foregroundStyle(.orange)
                    .help("Агент ждёт разрешения")
            }
        }
        .padding(.leading, 18)
        .tag(Scope.task(task.id))
        .contextMenu { TaskActions(model: model, task: task) }
        .accessibilityLabel("\(task.name), \(Palette.dotMeaning(task))")
    }
}

/// Найденное в контексте: задачи, чаты и реплики открытых задач.
private struct SearchResults: View {
    let model: DesktopModel
    let query: String
    let done: () -> Void

    private struct Hit: Identifiable {
        let id: String
        let kind: String
        let text: String
        let pick: @MainActor () -> Void
    }

    var body: some View {
        let hits = self.hits
        Section("Найдено") {
            ForEach(hits.prefix(30)) { hit in
                Button {
                    hit.pick()
                    done()
                } label: {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(hit.kind).font(.caption).foregroundStyle(.secondary)
                        Text(hit.text).lineLimit(2)
                    }
                }
                .buttonStyle(.plain)
            }
            if hits.isEmpty {
                Text("Ничего не нашлось в контексте «\(model.context ?? "")». Реплики ищутся в задачах, которые вы открывали.")
                    .font(.callout).foregroundStyle(.secondary)
            }
        }
    }

    private var hits: [Hit] {
        let needle = query.lowercased()
        guard let board = model.board.value else { return [] }
        var hits: [Hit] = []
        for task in board.confirmed where task.name.lowercased().contains(needle) {
            hits.append(Hit(id: "t" + task.id, kind: "Задача", text: task.name) { model.open(.task(task.id)) })
        }
        for task in board.confirmed {
            for conversation in task.conversations where conversation.title.lowercased().contains(needle) {
                hits.append(Hit(id: "c" + conversation.session, kind: "Чат · \(Wording.tool(conversation.platform)) · \(task.name)",
                                text: conversation.title) {
                    model.open(.task(task.id))
                    model.chatKey = conversation.session
                })
            }
            guard let speech = model.speech(of: task.id).value else { continue }
            for conversation in speech.conversations {
                for reply in conversation.replies where reply.text.lowercased().contains(needle) {
                    hits.append(Hit(id: "r\(conversation.session)-\(reply.n)",
                                    kind: "Реплика \(reply.n) · \(conversation.name ?? task.name)",
                                    text: Self.around(needle, in: reply.text)) {
                        model.open(.task(task.id))
                        model.chatKey = conversation.session
                        model.target = reply.n
                    })
                }
            }
        }
        return hits
    }

    /// Кусок реплики вокруг найденного.
    private static func around(_ needle: String, in text: String) -> String {
        guard let range = text.lowercased().range(of: needle) else { return String(text.prefix(120)) }
        let start = text.index(range.lowerBound, offsetBy: -40, limitedBy: text.startIndex) ?? text.startIndex
        let end = text.index(range.upperBound, offsetBy: 80, limitedBy: text.endIndex) ?? text.endIndex
        return (start > text.startIndex ? "…" : "") + text[start..<end] + (end < text.endIndex ? "…" : "")
    }
}

/// Контекст — отдельное пространство: свои проекты, чаты и карты.
private struct ContextSwitch: View {
    let model: DesktopModel

    var body: some View {
        HStack(spacing: 8) {
            Menu {
                Section("Контексты · у каждого свои проекты, чаты и карты") {
                    ForEach(Array(model.contexts.enumerated()), id: \.element) { index, name in
                        if index < 9 {
                            choice(name).keyboardShortcut(KeyEquivalent(Character("\(index + 1)")), modifiers: .command)
                        } else {
                            choice(name)
                        }
                    }
                }
            } label: {
                HStack(spacing: 8) {
                    Text(String((model.context ?? "?").prefix(1)).uppercased())
                        .font(.caption.weight(.bold)).foregroundStyle(.white)
                        .frame(width: 18, height: 18)
                        .background(.tint, in: .rect(cornerRadius: 5))
                    Text(model.context ?? "Контекст").fontWeight(.semibold)
                }
            }
            .menuStyle(.button)
            .buttonStyle(.bordered)
            .help("Контекст: отдельное пространство со своими проектами, чатами и картами")
            Spacer()
            Button("Обновить", systemImage: "arrow.clockwise") {
                Task { await model.reload() }
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .help("Спросить службу заново (⌘R)")
        }
        .padding(10)
    }

    private func choice(_ name: String) -> some View {
        Button {
            model.switchContext(name)
        } label: {
            if name == model.context {
                Label(name, systemImage: "checkmark")
            } else {
                Text(name)
            }
        }
    }
}
