import CrosskitKit
import SwiftUI

/// Откуда узел дерева: беседа с её инструментом и датой, реплика, кто её сказал, и цитата.
/// Карточка появляется при наведении; выбирать узел для инспектора по-прежнему кликом.
struct SourceCard: View {

    let title: String
    let quotes: [DecisionTree.Quote]
    /// Беседы задачи по сессии: инструмент и дата.
    let conversations: [String: TaskConversation]

    /// Больше строк карточка не показывает; остальное — в инспекторе.
    private static let shown = 3

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title).font(.callout.weight(.semibold)).lineLimit(2)
            ForEach(quotes.prefix(Self.shown), id: \.self) { quote in
                let conversation = conversations[quote.session]
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 8) {
                        ToolBadge(platform: conversation?.platform, size: 20)
                        VStack(alignment: .leading, spacing: 0) {
                            Text(quote.conversation).font(.caption.weight(.semibold)).lineLimit(1)
                            Text("\(Wording.tool(conversation?.platform)) · \(Wording.day(conversation?.firstAt)) · реплика \(quote.reply) · \(Wording.speaker(owner: quote.owner))")
                                .font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                    Text("«\(quote.text)»").font(.caption).foregroundStyle(.secondary).lineLimit(3)
                }
            }
            if quotes.count > Self.shown {
                Text("Ещё \(Wording.count(quotes.count - Self.shown, "беседа", "беседы", "бесед")) — в инспекторе")
                    .font(.caption2).foregroundStyle(.tertiary)
            }
        }
        .padding(12)
        .frame(width: 300, alignment: .leading)
        .glassEffect(.regular, in: .rect(cornerRadius: 14))
        .shadow(color: .black.opacity(0.12), radius: 10, y: 4)
    }
}
