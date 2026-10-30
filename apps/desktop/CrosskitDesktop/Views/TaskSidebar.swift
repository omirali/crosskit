import CrosskitKit
import SwiftUI

/// Доска, задачи, которые владелец подтвердил, и кандидаты сшивки. Решения о задаче — в меню
/// её строки.
struct TaskSidebar: View {

    @Bindable var model: DesktopModel

    var body: some View {
        List(selection: $model.choice) {
            Label("Доска", systemImage: "rectangle.split.3x1").tag(TaskChoice.board)
            if case .loaded(let board) = model.board {
                Section("Задачи") {
                    ForEach(board.confirmed) { task in
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text(task.name).font(.headline).lineLimit(1)
                                if task.finishedAtUnixMs != nil {
                                    Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                                        .accessibilityLabel("Готово")
                                }
                                if let critical = task.progress?.openCritical, critical > 0, task.finishedAtUnixMs == nil {
                                    Text("\(critical)").font(.caption2.weight(.bold)).foregroundStyle(.white)
                                        .padding(.horizontal, 5).background(.red, in: .capsule)
                                        .help("Открытых критичных рисков: \(critical)")
                                }
                            }
                            Text(Wording.count(task.conversations.count, "беседа", "беседы", "бесед"))
                                .font(.subheadline).foregroundStyle(.secondary)
                        }
                        .tag(TaskChoice.confirmed(id: task.id))
                        .contextMenu { TaskActions(model: model, task: task) }
                    }
                }
                if !board.candidates.isEmpty {
                    Section("Кандидаты") {
                        ForEach(Array(board.candidates.enumerated()), id: \.offset) { index, candidate in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(candidate.conversations.first?.title ?? "Кандидат").lineLimit(1)
                                Text(Wording.count(candidate.conversations.count, "беседа", "беседы", "бесед"))
                                    .font(.subheadline).foregroundStyle(.secondary)
                            }
                            .tag(TaskChoice.candidate(index: index))
                            .contextMenu { CandidateActions(model: model, candidate: candidate) }
                        }
                    }
                }
            }
        }
        .safeAreaInset(edge: .bottom) {
            if model.contexts.count > 1 {
                Picker("Контекст", selection: $model.context) {
                    ForEach(model.contexts, id: \.self) { name in
                        Text(name).tag(Optional(name))
                    }
                }
                .padding(12)
            }
        }
        .navigationTitle(model.context ?? "Crosskit")
    }
}
