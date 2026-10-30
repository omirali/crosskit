import AppKit
import CrosskitKit
import SwiftUI

/// Выбранный узел: из чего выбирали и почему, дословная цитата, кто её сказал, в какой беседе.
struct NodeInspector: View {

    let tree: DecisionTree
    let task: ConfirmedTask
    let focus: NodeFocus?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                switch focus {
                case .step(let id) where tree.steps.indices.contains(id):
                    StepDetail(step: tree.steps[id], total: tree.steps.count, task: task)
                case .risk(let id):
                    if let risk = tree.risks.first(where: { $0.id == id }) {
                        RiskDetail(risk: risk, task: task)
                    }
                default:
                    ContentUnavailableView("Узел не выбран", systemImage: "cursorarrow.click")
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

private struct StepDetail: View {
    let step: DecisionTree.Step
    let total: Int
    let task: ConfirmedTask

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("РАЗВИЛКА · ШАГ \(step.id + 1) ИЗ \(total)").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            Text(step.title).font(.title3.weight(.bold))
            if step.question != step.title {
                Text(step.question).foregroundStyle(.secondary)
            }
        }
        ForEach(step.branches) { branch in
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline) {
                    Text(Wording.mark(branch.state)).fontWeight(.bold).foregroundStyle(Palette.color(branch.state))
                    Text(branch.option).fontWeight(.semibold)
                    Spacer(minLength: 8)
                    Text(Wording.label(branch.state)).font(.caption.weight(.semibold)).foregroundStyle(Palette.color(branch.state))
                }
                if !branch.reason.isEmpty {
                    Text(branch.reason).foregroundStyle(.secondary)
                }
                QuoteBox(quote: branch.quote, task: task)
            }
            Divider()
        }
        if step.quotes.count > 1 {
            Text("Шаг сведён из \(Wording.count(step.quotes.count, "развилки", "развилок", "развилок")) в разных беседах:")
                .font(.caption).foregroundStyle(.secondary)
            ForEach(step.quotes, id: \.self) { quote in
                QuoteBox(quote: quote, task: task)
            }
        }
    }
}

private struct RiskDetail: View {
    let risk: DecisionTree.Risk
    let task: ConfirmedTask

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(risk.level.rawValue).font(.caption.weight(.bold)).padding(.horizontal, 8).padding(.vertical, 2)
                    .foregroundStyle(Palette.color(risk.level))
                    .background(Palette.color(risk.level).opacity(0.15), in: .capsule)
                Text(Wording.label(risk.status)).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                if risk.outside {
                    Text("вне задачи").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                }
            }
            Text(risk.title).font(.title3.weight(.bold))
            if risk.raisedIn > 1 {
                Text("Поднимался в \(Wording.count(risk.raisedIn, "беседе", "беседах", "беседах")).")
                    .foregroundStyle(.secondary)
            }
        }
        ForEach(risk.quotes, id: \.self) { quote in
            QuoteBox(quote: quote, task: task)
        }
    }
}

/// Цитата, которой доказан узел, и откуда она; из неё же — команда продолжить беседу.
private struct QuoteBox: View {
    let quote: DecisionTree.Quote
    let task: ConfirmedTask

    var body: some View {
        let platform = task.conversations.first { $0.session == quote.session }?.platform
        VStack(alignment: .leading, spacing: 6) {
            Text("«\(quote.text)»").textSelection(.enabled)
            Text("\(Wording.speaker(owner: quote.owner)) · реплика \(quote.reply) · «\(quote.conversation)» · \(Wording.tool(platform))")
                .font(.caption).foregroundStyle(.secondary)
            Button("Скопировать команду продолжения", systemImage: "doc.on.doc") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(Wording.resume(session: quote.session, platform: platform), forType: .string)
            }
            .buttonStyle(.link)
            .font(.caption)
            .help(Wording.resume(session: quote.session, platform: platform))
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: 10))
    }
}
