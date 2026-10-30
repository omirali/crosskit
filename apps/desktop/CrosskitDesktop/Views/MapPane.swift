import CrosskitKit
import SwiftUI

/// Карта решений того, что открыто: у задачи — дерево и карточка выбранного узла, у проекта —
/// его задачи с их линиями.
struct MapPane: View {

    @Bindable var model: DesktopModel
    /// Спросить, достраивать ли карту: цену называет окно, решение — человек.
    @State private var askingToBuild = false

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if let task = model.currentTask {
                taskMap(task)
            } else if let project = model.project {
                ProjectMap(model: model, project: project)
            } else {
                ContentUnavailableView("Выберите задачу или проект", systemImage: "sidebar.left")
            }
        }
        .background(.background.secondary)
        .confirmationDialog(buildQuestion, isPresented: $askingToBuild, titleVisibility: .visible) {
            Button("Дочитать") { model.startBuild() }
            Button("Отмена", role: .cancel) {}
        } message: {
            Text("Модель прочитает реплики, которых нет на карте, и потратит лимит подписки. Точную цену назовёт сам claude.")
        }
    }

    // MARK: Шапка

    private var header: some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.currentTask != nil ? "КАРТА РЕШЕНИЙ ЗАДАЧИ" : "КАРТА РЕШЕНИЙ ПРОЕКТА")
                    .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                Text(model.currentTask?.name ?? model.project?.name ?? "").font(.headline).lineLimit(1)
                status
            }
            Spacer()
            if let task = model.currentTask {
                if let estimate = model.estimate, estimate.unmappedReplies > 0, !model.notMapped(task.id) {
                    Button("Дочитать ≈ \(Wording.usd(estimate.usd))") { askingToBuild = true }
                        .help("Карта не знает \(Wording.count(estimate.unmappedReplies, "реплику", "реплики", "реплик")): модель их прочитает")
                        .disabled(model.build != nil)
                }
                Menu("Задача", systemImage: "ellipsis.circle") {
                    TaskActions(model: model, task: task)
                }
                .menuStyle(.button)
                .buttonStyle(.borderless)
                .labelStyle(.iconOnly)
                .fixedSize()
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(.background)
    }

    @ViewBuilder
    private var status: some View {
        if let task = model.currentTask {
            let line = model.statusLine(of: task)
            Text(line.text).font(.callout).foregroundStyle(line.color)
        } else if let project = model.project {
            Text(project.taskIDs.isEmpty
                 ? "задач пока нет — только кандидаты"
                 : "\(Wording.count(project.taskIDs.count, "задача", "задачи", "задач")) · нажмите задачу, чтобы открыть её карту")
                .font(.callout).foregroundStyle(.secondary)
        }
    }

    private var buildQuestion: String {
        guard let estimate = model.estimate else { return "Дочитать беседы?" }
        let replies = Wording.count(estimate.unmappedReplies, "реплика", "реплики", "реплик")
        return "Дочитать \(replies) в \(Wording.count(estimate.conversations, "беседе", "беседах", "беседах")), около \(Wording.usd(estimate.usd))?"
    }

    // MARK: Карта задачи

    @ViewBuilder
    private func taskMap(_ task: ConfirmedTask) -> some View {
        switch model.map(of: task.id) {
        case .idle, .loading:
            ProgressView("Читаю карту…").frame(maxHeight: .infinity)
        case .failed(let error):
            ServiceUnavailable(error: error)
        case .loaded(let loaded) where model.notMapped(task.id):
            NotMapped(task: task, answer: loaded.answer) { askingToBuild = true }
        case .loaded(let loaded):
            TreeArea(model: model, task: task, tree: loaded.tree)
            if let focus = model.focus {
                Divider()
                NodeCard(model: model, task: task, tree: loaded.tree, anchor: focus)
                    .frame(maxHeight: 330)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// Дерево и кнопка «к шагу, который ждёт решения» поверх него.
private struct TreeArea: View {
    let model: DesktopModel
    let task: ConfirmedTask
    let tree: DecisionTree
    @State private var position = ScrollPosition()

    var body: some View {
        let layout = TreeLayout(tree: tree, showsRejected: true)
        ZStack(alignment: .bottomTrailing) {
            ScrollView([.vertical, .horizontal]) {
                VStack(alignment: .leading, spacing: 12) {
                    DecisionTreeView(tree: tree, layout: layout, focus: model.focus) { anchor in
                        model.select(anchor, in: task.id)
                    }
                    Findings(model: model, tree: tree)
                }
                .padding(16)
                .frame(maxWidth: .infinity)
            }
            .scrollPosition($position)
            if let waiting = tree.waitingStep {
                Button {
                    show(waiting.id, in: layout, animated: true)
                    model.select(.step(waiting.id), in: task.id)
                } label: {
                    Label("К шагу \(waiting.id + 1): ждёт решения", systemImage: "arrow.down")
                        .font(.callout.weight(.semibold))
                }
                .buttonStyle(.borderedProminent)
                .buttonBorderShape(.capsule)
                .padding(14)
                .help("Прокрутить карту к шагу, который ждёт вашего решения, и открыть его реплику в чате")
            }
        }
        .task(id: task.id) {
            // Открытая задача показывает, где она сейчас: длинная карта начинается с выбранного
            // узла или с последнего шага. Кадр — чтобы дерево успело встать.
            try? await Task.sleep(for: .milliseconds(150))
            if let step = focusedStep ?? tree.head, tree.steps.count > 4 {
                show(step, in: layout, animated: false)
            }
        }
        .onChange(of: model.focus) {
            // Узел, выбранный в чате или в уведомлении, должен быть виден на карте.
            if let step = focusedStep { show(step, in: layout, animated: true) }
        }
    }

    /// Прокрутка по месту вопроса шага, которое знает `TreeLayout`: вопрос встаёт у верха
    /// видимой части, ниже — его ветки и риски.
    private func show(_ step: Int, in layout: TreeLayout, animated: Bool) {
        guard let question = layout.questions.first(where: { $0.step == step }) else { return }
        let point = CGPoint(x: 0, y: max(0, question.origin.y - 60))
        if animated {
            withAnimation { position.scrollTo(point: point) }
        } else {
            position.scrollTo(point: point)
        }
    }

    /// Шаг выбранного узла: у риска — шаг, после которого он появился.
    private var focusedStep: Int? {
        switch model.focus {
        case .step(let id): id
        case .risk(let id): tree.risks.first { $0.id == id && !$0.outside }?.step
        case nil: nil
        }
    }
}

/// Находки вне задачи: риски, найденные по дороге. На линии они не висят.
private struct Findings: View {
    let model: DesktopModel
    let tree: DecisionTree

    var body: some View {
        let outside = tree.risks.filter(\.outside)
        if !outside.isEmpty {
            DisclosureGroup("Находки вне задачи: \(outside.count)") {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(outside) { risk in
                        Button {
                            if let quote = risk.quotes.first { model.show(quote) }
                            model.focus = .risk(risk.id)
                        } label: {
                            HStack(spacing: 6) {
                                Circle().fill(Palette.color(risk.level)).frame(width: 6, height: 6)
                                Text(Wording.label(risk.level)).font(.caption.weight(.semibold))
                                Text(risk.title).font(.caption)
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.top, 4)
            }
            .font(.callout)
            .frame(maxWidth: 680, alignment: .leading)
        }
    }
}

/// Карта проекта: его задачи с линией каждой и где она стоит.
private struct ProjectMap: View {
    let model: DesktopModel
    let project: Project

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(project.taskIDs, id: \.self) { id in
                    if let task = model.task(id) {
                        Button {
                            model.open(.task(id))
                        } label: {
                            VStack(alignment: .leading, spacing: 8) {
                                HStack(alignment: .firstTextBaseline) {
                                    Text(task.name).fontWeight(.semibold)
                                    Spacer()
                                    Text(model.meta(of: task)).font(.caption).foregroundStyle(.secondary)
                                }
                                if let progress = task.progress, progress.mapped > 0 {
                                    StepDots(steps: progress.steps, head: progress.head, size: 16)
                                }
                                let line = model.statusLine(of: task)
                                Text(line.text).font(.callout).foregroundStyle(line.color)
                            }
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(.background, in: .rect(cornerRadius: 12))
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                    }
                }
                if !project.candidateSessions.isEmpty {
                    Text("Crosskit предлагает ещё \(Wording.count(project.candidateSessions.count, "задачу", "задачи", "задач")) — они на канбане.")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
            .padding(16)
        }
    }
}

/// Основная линия точками: шаг — точка, последняя — цветом того, где задача стоит.
struct StepDots: View {
    let steps: Int
    let head: BranchState?
    var size: CGFloat = 14
    var shown = 8

    var body: some View {
        HStack(spacing: 0) {
            Circle().fill(.primary).frame(width: size * 0.5, height: size * 0.5)
            if steps > shown {
                Text(" …\(steps - shown) ").font(.caption2).foregroundStyle(.secondary)
            }
            ForEach(0..<min(steps, shown), id: \.self) { index in
                Rectangle().fill(.primary).frame(width: size * 0.9, height: 1.5)
                let last = index == min(steps, shown) - 1
                let state = last ? (head ?? .chosen) : .chosen
                Circle()
                    .fill(state.leads ? AnyShapeStyle(Palette.color(state)) : AnyShapeStyle(.background))
                    .overlay { Circle().strokeBorder(state.leads ? .clear : Palette.color(state), lineWidth: 1.25) }
                    .overlay {
                        Text(Wording.mark(state)).font(.system(size: size * 0.5, weight: .bold))
                            .foregroundStyle(state.leads ? .white : Palette.color(state))
                    }
                    .frame(width: size, height: size)
            }
        }
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Путь: \(Wording.count(steps, "шаг", "шага", "шагов"))")
    }
}

extension DesktopModel {

    /// Задачу ещё не читали: ни одна её беседа не размечена.
    func notMapped(_ taskID: String) -> Bool {
        maps[taskID]?.value?.answer.conversations.allSatisfy { $0.mappedThrough == 0 } ?? false
    }

    /// Где задача стоит, одной строкой: что ждёт, что отстаёт.
    func statusLine(of task: ConfirmedTask) -> (text: String, color: Color) {
        if task.finishedAtUnixMs != nil {
            return ("отмечена готовой \(Wording.day(task.finishedAtUnixMs.map(Date.init(unixMilliseconds:))))", .secondary)
        }
        guard let progress = task.progress, progress.mapped > 0 else {
            return ("карта не построена", .secondary)
        }
        var parts: [String] = []
        if task.column == .waiting {
            parts.append(tree(of: task.id)?.waitingStep.map { "ждёт решения: «\($0.question)»" } ?? "ждёт вашего решения")
        } else {
            parts.append("решений не ждёт")
        }
        if progress.openCritical > 0 {
            parts.append(Wording.count(progress.openCritical, "критичный риск", "критичных риска", "критичных рисков"))
        }
        if progress.mapped < progress.conversations {
            parts.append("прочитано \(progress.mapped) из \(Wording.count(progress.conversations, "беседы", "бесед", "бесед"))")
        }
        if maps[task.id]?.value?.answer.joined?.stale == true {
            parts.append("карта отстаёт от бесед")
        }
        let color: Color = progress.openCritical > 0 ? .red : task.column == .waiting ? .accentColor : .green
        return (parts.joined(separator: " · "), color)
    }

    func meta(of task: ConfirmedTask) -> String {
        let risks = task.progress?.openRisks ?? 0
        return Wording.count(task.conversations.count, "беседа", "беседы", "бесед")
            + (risks > 0 ? " · " + Wording.count(risks, "риск", "риска", "рисков") : "")
    }
}
