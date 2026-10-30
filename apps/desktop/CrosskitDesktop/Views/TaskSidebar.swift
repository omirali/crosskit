import CrosskitKit
import SwiftUI

/// Задачи контекста: подтверждённые владельцем и кандидаты сшивки.
struct TaskSidebar: View {

    @Bindable var model: DesktopModel

    var body: some View {
        List(selection: $model.choice) {
            if case .loaded(let board) = model.board {
                Section("Задачи") {
                    ForEach(board.confirmed) { task in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(task.name).font(.headline).lineLimit(1)
                            Text(Wording.count(task.conversations.count, "беседа", "беседы", "бесед"))
                                .font(.subheadline).foregroundStyle(.secondary)
                        }
                        .tag(TaskChoice.confirmed(id: task.id))
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
