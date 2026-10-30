import CrosskitKit
import SwiftUI

/// Доска задач контекста (`docs/decision-map.md`, раздел 9): кандидаты сшивки, затем задачи по
/// тому, где они стоят. Колонку задаёт ход задачи, а не рука: задача ждёт решения ровно тогда,
/// когда HEAD её основной линии ждёт решения владельца.
struct TaskBoardView: View {

    @Bindable var model: DesktopModel
    let board: TaskBoard

    var body: some View {
        ScrollView(.horizontal) {
            HStack(alignment: .top, spacing: 14) {
                column("Входящие", count: board.candidates.count) {
                    ForEach(Array(board.candidates.enumerated()), id: \.offset) { index, candidate in
                        CandidateCard(candidate: candidate) { model.choice = .candidate(index: index) }
                            .contextMenu { CandidateActions(model: model, candidate: candidate) }
                    }
                }
                ForEach(TaskColumn.allCases, id: \.self) { kind in
                    let tasks = board.confirmed.filter { $0.column == kind }
                    column(Wording.title(kind), count: tasks.count) {
                        ForEach(tasks) { task in
                            TaskCard(task: task) { model.choice = .confirmed(id: task.id) }
                                .contextMenu { TaskActions(model: model, task: task) }
                        }
                    }
                }
            }
            .padding(20)
        }
        .navigationTitle("Доска")
    }

    private func column(_ title: String, count: Int, @ViewBuilder cards: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(title).font(.headline)
                Text("\(count)").font(.callout).foregroundStyle(.secondary)
            }
            .padding(.horizontal, 4)
            ScrollView {
                LazyVStack(spacing: 10) { cards() }
                    .padding(.bottom, 8)
            }
        }
        .padding(10)
        .frame(width: 260, alignment: .topLeading)
        .frame(maxHeight: .infinity, alignment: .top)
        .background(.background.secondary, in: .rect(cornerRadius: 14))
    }
}

/// Карточка задачи: имя, беседы, мини-линия и счётчики рисков.
private struct TaskCard: View {
    let task: ConfirmedTask
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            VStack(alignment: .leading, spacing: 8) {
                Text(task.name).font(.headline).lineLimit(2).multilineTextAlignment(.leading)
                HStack(spacing: 4) {
                    ForEach(Array(Set(task.conversations.map(\.platform))).sorted { $0.rawValue < $1.rawValue }, id: \.self) {
                        ToolBadge(platform: $0, size: 16)
                    }
                    Text(Wording.count(task.conversations.count, "беседа", "беседы", "бесед"))
                        .font(.caption).foregroundStyle(.secondary)
                }
                if let progress = task.progress, progress.mapped > 0 {
                    MiniLine(steps: progress.steps, head: progress.head)
                    HStack(spacing: 6) {
                        if progress.openCritical > 0 {
                            Counter(text: "critical \(progress.openCritical)", tint: .red)
                        }
                        Counter(text: "рисков \(progress.openRisks)", tint: progress.openRisks > 0 ? .orange : .secondary)
                        if progress.decidedRisks > 0 {
                            Counter(text: "решено \(progress.decidedRisks)", tint: .secondary)
                        }
                    }
                    if progress.mapped < progress.conversations {
                        Text("Размечено \(progress.mapped) из \(progress.conversations)").font(.caption2).foregroundStyle(.secondary)
                    }
                } else {
                    Text("Карта не построена").font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.background, in: .rect(cornerRadius: 10))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }
}

/// Основная линия на карточке: точка на шаг, последняя — HEAD цветом своего состояния.
private struct MiniLine: View {
    let steps: Int
    let head: BranchState?
    private static let shown = 10

    var body: some View {
        HStack(spacing: 0) {
            Circle().fill(.primary).frame(width: 6, height: 6)
            ForEach(0..<min(steps, Self.shown), id: \.self) { index in
                Rectangle().fill(.secondary.opacity(0.5)).frame(width: 10, height: 1.5)
                Circle()
                    .fill(index == min(steps, Self.shown) - 1 ? Palette.color(head ?? .chosen) : Color.secondary)
                    .frame(width: 7, height: 7)
            }
            if steps > Self.shown {
                Text(" +\(steps - Self.shown)").font(.caption2).foregroundStyle(.secondary)
            }
        }
        .accessibilityLabel("Основная линия: \(steps) шагов")
    }
}

private struct Counter: View {
    let text: String
    let tint: Color

    var body: some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 6).padding(.vertical, 2)
            .foregroundStyle(tint)
            .background(tint.opacity(0.12), in: .capsule)
    }
}

private struct CandidateCard: View {
    let candidate: CandidateTask
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            VStack(alignment: .leading, spacing: 6) {
                Text(candidate.conversations.first?.title ?? "Кандидат").font(.headline).lineLimit(2)
                    .multilineTextAlignment(.leading)
                HStack(spacing: 4) {
                    ForEach(candidate.conversations.prefix(6)) { ToolBadge(platform: $0.platform, size: 16) }
                    Text(Wording.count(candidate.conversations.count, "беседа", "беседы", "бесед"))
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.background, in: .rect(cornerRadius: 10))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }
}

/// Что можно решить о подтверждённой задаче: из меню её строки, карточки и экрана карты.
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

/// Что можно решить о кандидате.
struct CandidateActions: View {
    let model: DesktopModel
    let candidate: CandidateTask

    var body: some View {
        if let first = candidate.conversations.first {
            Button("Подтвердить как задачу…") {
                model.nameRequest = NameRequest(title: "Подтвердить задачу", initial: first.name ?? "") {
                    .confirm(conversation: first.session, name: $0)
                }
            }
            Button("Не задача") { Task { await model.decide(.reject(conversation: first.session)) } }
        }
    }
}
