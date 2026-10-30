import CrosskitKit
import SwiftUI

/// Беседы задачи: сколько каждой размечено и что в её карте.
struct ConversationsList: View {

    let answer: TaskMapAnswer
    let task: ConfirmedTask

    var body: some View {
        VStack(spacing: 8) {
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
                }
                .padding(12)
                .background(.background.secondary, in: .rect(cornerRadius: 12))
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
            group("Открытые", tree.risks.filter { !$0.outside && $0.status == .open })
            group("Принятые и закрытые", tree.risks.filter { !$0.outside && $0.status != .open })
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
                            if risk.raisedIn > 1 {
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
