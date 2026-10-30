import CrosskitKit
import SwiftUI

/// Окно: задачи слева, доска или карта выбранной задачи в середине, узел — в инспекторе справа.
struct RootView: View {

    @Bindable var model: DesktopModel
    /// Окно спрашивает, тратить ли лимит, прежде чем строить.
    @State private var askingToBuild = false

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
        .toolbar { toolbar }
        .task { await model.start() }
        .task(id: model.choice) { await model.loadMap() }
        .onChange(of: model.context) {
            model.choice = .board
            Task { await model.loadBoard() }
        }
        .sheet(item: $model.nameRequest) { request in
            NameSheet(request: request) { decision in
                model.nameRequest = nil
                if let decision { Task { await model.decide(decision) } }
            }
        }
        .sheet(item: $model.reasonRequest) { request in
            ReasonSheet(request: request) { decision in
                model.reasonRequest = nil
                if let decision { Task { await model.decide(decision) } }
            }
        }
        .sheet(isPresented: Binding(get: { model.build != nil }, set: { if !$0 { model.closeBuild() } })) {
            BuildSheet(model: model)
                .interactiveDismissDisabled(model.build?.finished == false)
        }
        .confirmationDialog(buildQuestion, isPresented: $askingToBuild, titleVisibility: .visible) {
            Button("Построить") { model.startBuild() }
            Button("Отмена", role: .cancel) {}
        } message: {
            Text("Модель прочитает реплики, которых не покрывает ни одна карта, и потратит лимит подписки. Цену точно назовёт сам claude.")
        }
        .alert(model.notice?.title ?? "", isPresented: Binding(get: { model.notice != nil }, set: { if !$0 { model.notice = nil } })) {
            Button("Понятно") { model.notice = nil }
        } message: {
            Text(model.notice?.explanation ?? "")
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        if let task = model.confirmedTask {
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
            ToolbarItem {
                Button("Достроить карту", systemImage: "hammer") { askingToBuild = true }
                    .help("Разметить реплики, которых не покрывает ни одна карта")
                    .disabled(model.estimate?.unmappedReplies ?? 0 == 0 || model.build != nil)
            }
            ToolbarItem {
                Menu("Задача", systemImage: "ellipsis.circle") {
                    TaskActions(model: model, task: task)
                }
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

    private var buildQuestion: String {
        guard let estimate = model.estimate else { return "Достроить карту?" }
        let replies = Wording.count(estimate.unmappedReplies, "реплика", "реплики", "реплик")
        let usd = estimate.usd.formatted(.number.precision(.fractionLength(1)).locale(Wording.locale))
        return "Достроить карту: \(replies) в \(Wording.count(estimate.conversations, "беседе", "беседах", "беседах")), около \(usd) $?"
    }

    @ViewBuilder
    private var detail: some View {
        if case .failed(let error) = model.board {
            ServiceUnavailable(error: error, state: model.contextState)
        } else if case .loaded(let board) = model.board, model.choice == .board {
            TaskBoardView(model: model, board: board)
        } else if let candidate = model.candidate {
            CandidateView(model: model, candidate: candidate)
        } else if let task = model.confirmedTask {
            TaskMapScreen(model: model, task: task, build: { askingToBuild = true })
        } else if case .loaded = model.board {
            ContentUnavailableView("Выберите задачу", systemImage: "point.3.connected.trianglepath.dotted",
                                   description: Text("Слева — доска, задачи, которые вы подтвердили, и кандидаты сшивки."))
        } else {
            ProgressView("Спрашиваю службу…")
        }
    }

    @ViewBuilder
    private var inspector: some View {
        if case .loaded(let loaded) = model.map, let task = model.confirmedTask {
            NodeInspector(model: model, tree: loaded.tree, task: task)
        } else {
            ContentUnavailableView("Узел не выбран", systemImage: "cursorarrow.click",
                                   description: Text("Выберите развилку или риск на карте."))
        }
    }
}
