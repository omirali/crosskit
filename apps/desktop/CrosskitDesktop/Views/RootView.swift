import CrosskitKit
import SwiftUI

/// Окно: задачи слева, карта выбранной задачи в середине, узел — в инспекторе справа.
struct RootView: View {

    @Bindable var model: DesktopModel

    var body: some View {
        NavigationSplitView {
            TaskSidebar(model: model)
                .navigationSplitViewColumnWidth(min: 230, ideal: 270, max: 340)
        } detail: {
            detail
        }
        .inspector(isPresented: $model.inspectorShown) {
            inspector
                .inspectorColumnWidth(min: 280, ideal: 340, max: 440)
        }
        .toolbar {
            if model.confirmedTask != nil {
                ToolbarItem(placement: .principal) {
                    Picker("Вид", selection: $model.mode) {
                        ForEach(MapMode.allCases) { mode in
                            Text(Wording.label(mode)).tag(mode)
                        }
                    }
                    .pickerStyle(.segmented)
                }
                ToolbarItem {
                    Toggle("Отклонённые ветки", systemImage: "arrow.triangle.branch", isOn: $model.showsRejected)
                        .help("Показывать отклонённые и отложенные ветки")
                        .disabled(model.mode != .line)
                }
            }
            ToolbarItem {
                Button("Обновить", systemImage: "arrow.clockwise") {
                    Task { await model.reload() }
                }
                .help("Спросить службу заново (⌘R)")
            }
            ToolbarItem {
                Button("Узел", systemImage: "sidebar.right") {
                    model.inspectorShown.toggle()
                }
                .help("Показать или спрятать узел")
            }
        }
        .task { await model.start() }
        .task(id: model.choice) { await model.loadMap() }
        .onChange(of: model.context) {
            model.choice = nil
            Task { await model.loadBoard() }
        }
    }

    @ViewBuilder
    private var detail: some View {
        if case .failed(let error) = model.board {
            ServiceUnavailable(error: error, state: model.contextState)
        } else if let candidate = model.candidate {
            CandidateView(candidate: candidate)
        } else if let task = model.confirmedTask {
            TaskMapScreen(model: model, task: task)
        } else if case .loaded = model.board {
            ContentUnavailableView("Выберите задачу", systemImage: "point.3.connected.trianglepath.dotted",
                                   description: Text("Слева — задачи, которые вы подтвердили, и кандидаты сшивки."))
        } else {
            ProgressView("Спрашиваю службу…")
        }
    }

    @ViewBuilder
    private var inspector: some View {
        if case .loaded(let loaded) = model.map, let task = model.confirmedTask {
            NodeInspector(tree: loaded.tree, task: task, focus: model.focus)
        } else {
            ContentUnavailableView("Узел не выбран", systemImage: "cursorarrow.click",
                                   description: Text("Выберите развилку или риск на карте."))
        }
    }
}
