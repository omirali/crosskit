import CrosskitKit
import SwiftUI

/// Канбан задач контекста (`docs/decision-map.md`, раздел 9; макет `design/UXMain.dc.html`).
/// Колонку задаёт ход задачи, а не рука: задача ждёт решения ровно тогда, когда её последняя
/// развилка ждёт владельца. Первая колонка — что Crosskit предлагает считать задачей.
struct KanbanPane: View {

    @Bindable var model: DesktopModel
    /// Проект, по которому отобрана доска; ничего — все.
    @State private var projectRoot: String?
    @State private var selected: String?

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if let board = model.board.value {
                ScrollView([.horizontal, .vertical]) {
                    HStack(alignment: .top, spacing: 10) {
                        column("Предлагает Crosskit", hint: "Беседы, которые похожи на одну задачу", empty: "Новых предложений нет",
                               tint: .purple, count: candidates(board).count) {
                            ForEach(Array(candidates(board).enumerated()), id: \.offset) { _, candidate in
                                CandidateCard(model: model, candidate: candidate, project: projectName(of: candidate))
                            }
                        }
                        ForEach(Self.columns, id: \.column) { spec in
                            let tasks = tasks(board).filter { $0.column == spec.column }
                            column(spec.title, hint: spec.hint, empty: spec.empty, tint: spec.tint, count: tasks.count) {
                                ForEach(tasks) { task in
                                    TaskCard(model: model, task: task, isSelected: selected == task.id) {
                                        selected = selected == task.id ? nil : task.id
                                    }
                                }
                            }
                        }
                    }
                    .padding(16)
                }
            }
        }
        .background(.background.secondary)
    }

    private struct Column {
        let column: TaskColumn
        let title: String
        let hint: String
        let empty: String
        let tint: Color
    }

    private static let columns = [
        Column(column: .waiting, title: "Ждут вашего решения", hint: "Агент остановился на развилке", empty: "Никто не ждёт — хорошо", tint: .accentColor),
        Column(column: .inWork, title: "В работе", hint: "Решения есть, работа идёт", empty: "Пусто", tint: .green),
        Column(column: .unmapped, title: "Карта не построена", hint: "Беседы есть, развилок пока не видно", empty: "Пусто", tint: .secondary),
        Column(column: .done, title: "Готово", hint: "Закрыто без открытых критичных рисков", empty: "Пусто", tint: .secondary),
    ]

    private var header: some View {
        HStack(spacing: 10) {
            Text("Задачи · \(model.context ?? "")").font(.headline).fixedSize()
            if let board = model.board.value {
                Text(Wording.count(tasks(board).count, "задача", "задачи", "задач")).foregroundStyle(.secondary).fixedSize()
            }
            Picker("Проект", selection: $projectRoot) {
                Text("Все проекты").tag(String?.none)
                ForEach(model.projects) { project in
                    Text(project.name).tag(Optional(project.root))
                }
            }
            .pickerStyle(.menu)
            .labelsHidden()
            .fixedSize()
            Spacer()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(.background)
    }

    private func tasks(_ board: TaskBoard) -> [ConfirmedTask] {
        guard let projectRoot, let project = model.projects.first(where: { $0.root == projectRoot }) else { return board.confirmed }
        return board.confirmed.filter { project.taskIDs.contains($0.id) }
    }

    private func candidates(_ board: TaskBoard) -> [CandidateTask] {
        guard let projectRoot, let project = model.projects.first(where: { $0.root == projectRoot }) else { return board.candidates }
        return board.candidates.filter { $0.conversations.first.map { project.candidateSessions.contains($0.session) } ?? false }
    }

    private func projectName(of candidate: CandidateTask) -> String? {
        guard let first = candidate.conversations.first else { return nil }
        return model.projects.first { $0.candidateSessions.contains(first.session) }?.name
    }

    private func column(_ title: String, hint: String, empty: String, tint: Color, count: Int,
                        @ViewBuilder cards: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Circle().fill(tint).frame(width: 8, height: 8)
                Text(title).fontWeight(.semibold)
                Spacer()
                Text("\(count)").font(.caption.weight(.bold)).foregroundStyle(.secondary)
                    .padding(.horizontal, 6).padding(.vertical, 1)
                    .background(.quaternary, in: .capsule)
            }
            Text(hint).font(.caption).foregroundStyle(.secondary)
            cards()
            if count == 0 {
                Text(empty).font(.caption).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 18)
                    .overlay { RoundedRectangle(cornerRadius: 12).strokeBorder(.quaternary) }
            }
        }
        .padding(10)
        .frame(width: 250, alignment: .topLeading)
        .background(tint.opacity(0.05), in: .rect(cornerRadius: 14))
    }
}

/// Карточка задачи: проект, имя, где стоит, линия, агенты и что горит. Выбранная открывает
/// карту или чаты задачи.
private struct TaskCard: View {
    let model: DesktopModel
    let task: ConfirmedTask
    let isSelected: Bool
    let toggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: toggle) {
                VStack(alignment: .leading, spacing: 7) {
                    HStack {
                        Text(model.projects.first { $0.taskIDs.contains(task.id) }?.name ?? "").lineLimit(1)
                        Spacer()
                        Text(Wording.day(task.conversations.compactMap(\.lastAtUnixMs).max().map(Date.init(unixMilliseconds:))))
                    }
                    .font(.caption).foregroundStyle(.secondary)
                    Text(task.name).font(.headline).multilineTextAlignment(.leading)
                    let line = model.statusLine(of: task)
                    Text(line.text).font(.caption).foregroundStyle(line.color).multilineTextAlignment(.leading)
                    if let progress = task.progress, progress.mapped > 0 {
                        StepDots(steps: progress.steps, head: progress.head, size: 14, shown: 7)
                    }
                    HStack(spacing: 5) {
                        ForEach(Array(Set(task.conversations.map(\.platform))).sorted { $0.rawValue < $1.rawValue }, id: \.self) {
                            ToolBadge(platform: $0, size: 16)
                        }
                        Text(Wording.count(task.conversations.count, "беседа", "беседы", "бесед")).font(.caption).foregroundStyle(.secondary)
                        Spacer()
                        badges
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if isSelected {
                HStack(spacing: 6) {
                    Button("Открыть карту") {
                        model.open(.task(task.id))
                        model.pane = .map
                    }
                    .buttonStyle(.borderedProminent)
                    .frame(maxWidth: .infinity)
                    Button("Чаты · \(task.conversations.count)") {
                        model.open(.task(task.id))
                        model.showsChats = true
                    }
                }
                .controlSize(.small)
                .padding([.horizontal, .bottom], 12)
            }
        }
        .background(.background, in: .rect(cornerRadius: 12))
        .overlay {
            RoundedRectangle(cornerRadius: 12).strokeBorder(isSelected ? AnyShapeStyle(.tint) : AnyShapeStyle(.quaternary),
                                                            lineWidth: isSelected ? 2 : 0.5)
        }
        .opacity(task.finishedAtUnixMs != nil ? 0.75 : 1)
        .contextMenu { TaskActions(model: model, task: task) }
    }

    @ViewBuilder
    private var badges: some View {
        if task.finishedAtUnixMs == nil {
            if model.live.all.contains(where: { $0.chat.target.taskID == task.id && !$0.chat.pendingPermissions.isEmpty }) {
                Badge(text: "ждёт разрешения", tint: .orange)
            }
            if let progress = task.progress {
                if progress.openCritical > 0 {
                    Badge(text: "критично", tint: .red)
                } else if progress.openRisks > 0 {
                    Badge(text: Wording.count(progress.openRisks, "риск", "риска", "рисков"), tint: .orange)
                }
            }
        }
    }
}

private struct Badge: View {
    let text: String
    let tint: Color

    var body: some View {
        Text(text).font(.caption2.weight(.semibold)).foregroundStyle(tint)
            .padding(.horizontal, 6).padding(.vertical, 2)
            .background(tint.opacity(0.12), in: .rect(cornerRadius: 5))
    }
}

/// Предложение задачи: беседы, которые сшивка связала, и почему.
private struct CandidateCard: View {
    let model: DesktopModel
    let candidate: CandidateTask
    let project: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text([project, Wording.day(candidate.conversations.first?.firstAt)].compactMap { $0 }.joined(separator: " · "))
                .font(.caption).foregroundStyle(.secondary)
            Text(candidate.conversations.first?.title ?? "Предложение").font(.headline).lineLimit(3)
            Text(Wording.count(candidate.conversations.count, "беседа", "беседы", "бесед") + (Wording.why(candidate).map { ", \($0)" } ?? ""))
                .font(.caption).foregroundStyle(.secondary)
            HStack(spacing: 6) {
                CandidateActions(model: model, candidate: candidate)
            }
            .controlSize(.small)
            .padding(.top, 2)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background, in: .rect(cornerRadius: 12))
        .overlay { RoundedRectangle(cornerRadius: 12).strokeBorder(.purple.opacity(0.35)) }
    }
}

/// Что можно решить о подтверждённой задаче: из меню её строки, карточки и карты.
struct TaskActions: View {
    let model: DesktopModel
    let task: ConfirmedTask

    var body: some View {
        Button("Переименовать…") {
            model.nameRequest = NameRequest(title: "Переименовать задачу", initial: task.name) {
                .rename(task: task.id, name: $0)
            }
        }
        if task.finishedAtUnixMs == nil {
            Button("Отметить готовой") { Task { await model.decide(.finish(task: task.id)) } }
        } else {
            Button("Вернуть в работу") { Task { await model.decide(.resume(task: task.id)) } }
        }
    }
}

/// Что можно решить о предложении задачи.
struct CandidateActions: View {
    let model: DesktopModel
    let candidate: CandidateTask

    var body: some View {
        if let first = candidate.conversations.first {
            Button("Сделать задачей") { model.confirmCandidate(candidate) }
                .buttonStyle(.borderedProminent)
                .tint(.purple)
            Button("Не задача") { Task { await model.decide(.reject(conversation: first.session)) } }
        }
    }
}
