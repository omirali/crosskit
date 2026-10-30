import AppKit
import CrosskitKit
import SwiftUI

/// Выбранный узел под картой: из чего выбирали, чьими словами, и что с ним можно сделать.
/// Цитата — кнопка: она открывает свою реплику в чате справа.
struct NodeCard: View {

    let model: DesktopModel
    let task: ConfirmedTask
    let tree: DecisionTree
    let anchor: MapAnchor

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                switch anchor {
                case .step(let id) where tree.steps.indices.contains(id):
                    step(tree.steps[id])
                case .risk(let id):
                    if let risk = tree.risks.first(where: { $0.id == id }) {
                        RiskCard(model: model, task: task, risk: risk)
                    }
                default:
                    EmptyView()
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(.background)
        .overlay(alignment: .topTrailing) {
            Button("Закрыть", systemImage: "xmark") { model.focus = nil }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .padding(10)
        }
    }

    @ViewBuilder
    private func step(_ step: DecisionTree.Step) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text("\(step.id + 1). \(step.question)").font(.headline)
            Spacer()
            Text(Wording.label(step.state)).font(.callout.weight(.semibold)).foregroundStyle(Palette.color(step.state))
                .padding(.trailing, 24)
        }
        if step.title != step.question {
            Text(step.title).foregroundStyle(.secondary)
        }
        ForEach(step.branches) { branch in
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(Wording.mark(branch.state)).fontWeight(.bold).foregroundStyle(Palette.color(branch.state))
                        .frame(width: 14)
                    Text(branch.option).fontWeight(.semibold)
                    if !branch.reason.isEmpty, branch.state != .open {
                        Text("— \(branch.reason)").foregroundStyle(.secondary).lineLimit(2)
                    }
                    Spacer(minLength: 8)
                    if branch.state == .open {
                        Button("Выбрать") { model.choose(branch, in: task.id) }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.small)
                            .help("Открыть беседу, где задан вопрос, и сказать агенту, что вы выбрали")
                    }
                }
                QuoteButton(model: model, task: task, quote: branch.quote)
                    .padding(.leading, 22)
            }
        }
        if step.quotes.count > 1 {
            Text("Шаг сведён из \(Wording.count(step.quotes.count, "развилки", "развилок", "развилок")) в разных беседах:")
                .font(.caption).foregroundStyle(.secondary)
            ForEach(step.quotes, id: \.self) { quote in
                QuoteButton(model: model, task: task, quote: quote)
            }
        }
    }
}

/// Риск: уровень, кто решил и как, действия и цитаты из каждой беседы, где он поднимался.
private struct RiskCard: View {
    let model: DesktopModel
    let task: ConfirmedTask
    let risk: DecisionTree.Risk

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(risk.title).font(.headline)
            Spacer()
            Text("\(Wording.label(risk.level)) · \(Wording.label(risk.shownStatus))" + (risk.outside ? " · вне задачи" : ""))
                .font(.callout.weight(.semibold))
                .foregroundStyle(risk.isOpen ? Palette.color(risk.level) : .secondary)
                .padding(.trailing, 24)
        }
        if risk.raisedIn > 1 {
            Text("Поднимался в \(Wording.count(risk.raisedIn, "беседе", "беседах", "беседах")).").foregroundStyle(.secondary)
        }
        decision
        ForEach(risk.quotes, id: \.self) { quote in
            QuoteButton(model: model, task: task, quote: quote)
        }
    }

    /// Слово владельца о риске: каким оно уже есть или каким его сказать. Решение хранится по
    /// месту первой цитаты риска; у риска из нескольких бесед годится любое (ADR 0061).
    @ViewBuilder
    private var decision: some View {
        if let anchor = risk.quotes.first {
            if let decided = risk.decided {
                VStack(alignment: .leading, spacing: 4) {
                    Text(Wording.decided(decided).capitalizedFirst).font(.callout.weight(.semibold))
                    Text(decided.reason).foregroundStyle(.secondary).textSelection(.enabled)
                    Button("Вернуть риск на карту") {
                        Task { await model.decide(.reopenRisk(task: task.id, session: anchor.session, reply: anchor.reply)) }
                    }
                    .buttonStyle(.link)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.green.opacity(0.08), in: .rect(cornerRadius: 10))
            } else if risk.status == .open {
                HStack {
                    ForEach([RiskStatus.accepted, .resolved], id: \.self) { verdict in
                        Button(verdict == .accepted ? "Принять с причиной…" : "Закрыть с причиной…") {
                            model.reasonRequest = ReasonRequest(risk: risk.title, verdict: verdict) {
                                .risk(task: task.id, session: anchor.session, reply: anchor.reply, verdict: verdict, reason: $0)
                            }
                        }
                    }
                    if risk.level == .critical {
                        Text("Пока он открыт, задачу нельзя отметить готовой.").font(.caption).foregroundStyle(.secondary)
                    }
                }
                .controlSize(.small)
            }
        }
    }
}

/// Цитата узла и откуда она; нажатие открывает её реплику в чате.
struct QuoteButton: View {
    let model: DesktopModel
    let task: ConfirmedTask
    let quote: DecisionTree.Quote

    var body: some View {
        let here = model.shownChatKey == quote.session && model.target == quote.reply
        let platform = task.conversations.first { $0.session == quote.session }?.platform
        Button {
            model.show(quote)
        } label: {
            Text("«\(quote.text)» \(Text("— \(Wording.speaker(owner: quote.owner)), «\(quote.conversation)», реплика \(quote.reply)").foregroundStyle(.secondary))")
                .font(.callout)
                .multilineTextAlignment(.leading)
                .padding(.horizontal, 9)
                .padding(.vertical, 6)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(here ? AnyShapeStyle(.tint.opacity(0.12)) : AnyShapeStyle(.quaternary.opacity(0.5)), in: .rect(cornerRadius: 8))
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .help("Открыть реплику \(quote.reply) в чате")
        .contextMenu {
            Button("Скопировать команду продолжения") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(Wording.resume(session: quote.session, platform: platform), forType: .string)
            }
        }
    }
}

extension String {
    var capitalizedFirst: String { prefix(1).uppercased() + dropFirst() }
}
