import CrosskitKit
import SwiftUI

/// Карта одной задачи: шапка с итогами и один из трёх видов.
struct TaskMapScreen: View {

    @Bindable var model: DesktopModel
    let task: ConfirmedTask

    var body: some View {
        switch model.map {
        case .idle, .loading:
            ProgressView("Читаю карту…")
        case .failed(let error):
            ServiceUnavailable(error: error)
        case .loaded(let loaded) where loaded.answer.conversations.allSatisfy({ $0.mappedThrough == 0 }):
            NotMapped(task: task, answer: loaded.answer)
        case .loaded(let loaded):
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    MapHeader(task: task, loaded: loaded)
                    content(loaded)
                    UnmappedNote(task: task, answer: loaded.answer)
                }
                .padding(24)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .navigationTitle(task.name)
        }
    }

    @ViewBuilder
    private func content(_ loaded: LoadedMap) -> some View {
        switch model.mode {
        case .line:
            DecisionTreeView(tree: loaded.tree,
                             layout: TreeLayout(tree: loaded.tree, showsRejected: model.showsRejected),
                             conversations: Dictionary(task.conversations.map { ($0.session, $0) },
                                                       uniquingKeysWith: { first, _ in first }),
                             focus: $model.focus)
        case .conversations:
            ConversationsList(answer: loaded.answer, task: task)
        case .risks:
            RisksList(tree: loaded.tree, focus: $model.focus)
        }
    }
}

/// Итоги карты одной строкой: сколько шагов, открытых рисков и что доказано цитатой.
private struct MapHeader: View {

    let task: ConfirmedTask
    let loaded: LoadedMap

    var body: some View {
        let tree = loaded.tree
        let conversations = loaded.answer.conversations
        let kept = conversations.map(\.kept).reduce(0, +)
        let proved = kept + conversations.map(\.dropped).reduce(0, +)
        let critical = tree.openRisks.filter { $0.level == .critical }.count
        VStack(alignment: .leading, spacing: 10) {
            Text(subtitle).foregroundStyle(.secondary)
            GlassEffectContainer(spacing: 8) {
                HStack(spacing: 8) {
                    Chip(text: "Основная линия: \(Wording.count(tree.steps.count, "шаг", "шага", "шагов"))")
                    Chip(text: "Открытых рисков: \(tree.openRisks.count)" + (critical > 0 ? ", critical: \(critical)" : ""),
                         tint: critical > 0 ? .red : nil)
                    if proved > 0 {
                        Chip(text: "Доказано цитатой: \(kept * 100 / proved) %")
                    }
                    if loaded.answer.joined?.stale == true {
                        Chip(text: "Сводка устарела: карты бесед выросли", tint: .orange)
                    }
                    if !tree.joined {
                        Chip(text: "Сводки нет: шаги — развилки бесед по времени", tint: .orange)
                    }
                }
            }
        }
    }

    private var subtitle: String {
        let days = task.conversations.compactMap(\.firstAt)
        let tools = Set(task.conversations.map(\.platform)).map(Wording.tool).sorted().joined(separator: " и ")
        let span = days.min().map { first in
            let last = days.max() ?? first
            return first == last ? Wording.day(first) : "\(Wording.day(first)) – \(Wording.day(last))"
        } ?? "без дат"
        return "\(Wording.count(task.conversations.count, "беседа", "беседы", "бесед")) · \(tools) · \(span)"
    }
}

private struct Chip: View {
    let text: String
    var tint: Color?

    var body: some View {
        Text(text)
            .font(.callout)
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .foregroundStyle(tint ?? .primary)
            .glassEffect(.regular.tint(tint?.opacity(0.15)), in: .capsule)
    }
}

/// Что карта ещё не покрывает и как её достроить.
private struct UnmappedNote: View {

    let task: ConfirmedTask
    let answer: TaskMapAnswer

    var body: some View {
        let behind = answer.conversations.filter { $0.mappedThrough < $0.replies }
        if !behind.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text("Не размечено:").font(.headline)
                ForEach(behind) { conversation in
                    Text(conversation.mappedThrough == 0
                         ? "«\(conversation.title)» — вся беседа"
                         : "«\(conversation.title)» — \(Wording.count(conversation.replies - conversation.mappedThrough, "новая реплика", "новые реплики", "новых реплик"))")
                        .foregroundStyle(.secondary)
                }
                CommandBox(command: "crosskit --context \(answer.context) tasks map \"\(task.name)\" --build")
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.quaternary.opacity(0.4), in: .rect(cornerRadius: 12))
        }
    }
}
