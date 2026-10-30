import CrosskitKit
import SwiftUI

/// Беседы задачи: сколько каждой размечено и что в её карте.
struct ConversationsList: View {

    let model: DesktopModel
    let answer: TaskMapAnswer
    let task: ConfirmedTask
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !task.proposed.isEmpty {
                Text("ПРЕДЛАГАЕТ СШИВКА").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                ForEach(task.proposed) { conversation in
                    HStack(spacing: 10) {
                        ToolBadge(platform: conversation.platform)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(conversation.title).lineLimit(1)
                            Text(conversation.signals.joined(separator: " · ")).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Добавить") { Task { await model.decide(.add(task: task.id, conversation: conversation.session)) } }
                        Button("Не из этой задачи") { Task { await model.decide(.drop(task: task.id, conversation: conversation.session)) } }
                    }
                    .controlSize(.small)
                    .padding(10)
                    .background(.tint.opacity(0.06), in: .rect(cornerRadius: 10))
                }
                Text("БЕСЕДЫ ЗАДАЧИ").font(.caption.weight(.semibold)).foregroundStyle(.secondary).padding(.top, 8)
            }
            ForEach(answer.conversations) { conversation in
                let platform = task.conversations.first { $0.session == conversation.session }?.platform
                HStack(alignment: .top, spacing: 14) {
                    ToolBadge(platform: platform, size: 30)
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(alignment: .firstTextBaseline) {
                            Text(conversation.title).fontWeight(.semibold)
                            Text("\(Wording.tool(platform)) · \(Wording.day(conversation.firstAt)) · \(Wording.count(conversation.replies, "реплика", "реплики", "реплик"))")
                                .foregroundStyle(.secondary)
                        }
                        ProgressView(value: Double(conversation.mappedThrough), total: Double(max(conversation.replies, 1)))
                        Text(status(of: conversation))
                            .font(.callout)
                            .foregroundStyle(conversation.mappedThrough < conversation.replies ? .orange : .secondary)
                    }
                    if let directory = task.conversations.first(where: { $0.session == conversation.session })?.workingDirectory,
                       platform != .codex {
                        Button("Продолжить", systemImage: "arrowshape.turn.up.right") {
                            openWindow(value: ChatTarget(taskID: task.id, taskName: task.name, context: model.context,
                                                         directory: directory, resume: conversation.session, brief: nil))
                        }
                        .help("Продолжить эту беседу в окне")
                    }
                }
                .padding(12)
                .background(.background.secondary, in: .rect(cornerRadius: 12))
                .contextMenu {
                    Button("Убрать из задачи") {
                        Task { await model.decide(.drop(task: task.id, conversation: conversation.session)) }
                    }
                }
            }
        }
    }

    private func status(of conversation: MapConversation) -> String {
        let nodes = "\(Wording.count(conversation.forks.count, "развилка", "развилки", "развилок")) · \(Wording.count(conversation.risks.count, "риск", "риска", "рисков"))"
        let total = conversation.kept + conversation.dropped
        let proved = total > 0 ? " · доказано \(conversation.kept * 100 / total) %" : ""
        if conversation.mappedThrough == 0 { return "Не размечена" }
        if conversation.mappedThrough >= conversation.replies { return "Размечена вся · \(nodes)\(proved)" }
        return "Размечено \(conversation.mappedThrough) из \(conversation.replies) · \(nodes)\(proved)"
    }
}

/// Риски задачи по группам: открытые, принятые и закрытые, находки вне задачи.
struct RisksList: View {

    let tree: DecisionTree
    @Binding var focus: NodeFocus?

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            group("Открытые", tree.risks.filter { !$0.outside && $0.isOpen })
            group("Принятые и закрытые", tree.risks.filter { !$0.outside && !$0.isOpen })
            group("Находки вне задачи", tree.risks.filter(\.outside))
        }
    }

    @ViewBuilder
    private func group(_ title: String, _ risks: [DecisionTree.Risk]) -> some View {
        if !risks.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                Text(title.uppercased()).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                ForEach(risks.sorted { $0.level < $1.level }) { risk in
                    Button {
                        focus = .risk(risk.id)
                    } label: {
                        HStack(spacing: 10) {
                            Circle().fill(Palette.color(risk.level)).frame(width: 8, height: 8)
                            Text(risk.level.rawValue).fontWeight(.semibold).foregroundStyle(Palette.color(risk.level))
                                .frame(width: 64, alignment: .leading)
                            Text(risk.title).frame(maxWidth: .infinity, alignment: .leading)
                            if let decided = risk.decided {
                                Text(Wording.decided(decided)).foregroundStyle(.secondary)
                            } else if risk.raisedIn > 1 {
                                Text("в \(Wording.count(risk.raisedIn, "беседе", "беседах", "беседах"))").foregroundStyle(.secondary)
                            }
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .contentShape(.rect)
                        .background(focus == .risk(risk.id) ? AnyShapeStyle(.tint.opacity(0.15)) : AnyShapeStyle(.background.secondary),
                                    in: .rect(cornerRadius: 10))
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}
