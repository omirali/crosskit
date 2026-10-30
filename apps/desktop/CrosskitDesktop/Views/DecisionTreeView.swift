import CrosskitKit
import SwiftUI

/// Дерево решений задачи (макет `design/CCDesktop.dc.html`). Линии и полосы рисков рисует
/// `Canvas`, узлы, вопросы и метки рисков — кнопки поверх него: клик выбирает, что показать
/// в инспекторе, наведение показывает, из какой беседы узел. Где что стоит, решает
/// `TreeLayout`; здесь только как оно выглядит.
struct DecisionTreeView: View {

    let tree: DecisionTree
    let layout: TreeLayout
    /// Беседы задачи по сессии: инструмент и дата для карточки при наведении.
    let conversations: [String: TaskConversation]
    @Binding var focus: NodeFocus?

    /// Узел под указателем и точка, у которой показать карточку.
    @State private var hovered: Hovered?

    private enum Hovered: Equatable {
        case step(Int, at: CGPoint)
        case branch(step: Int, branch: Int, at: CGPoint)
        case risk(String, at: CGPoint)
    }

    var body: some View {
        ScrollView(.horizontal) {
            ZStack(alignment: .topLeading) {
                Canvas { context, _ in
                    for rail in layout.rails {
                        context.stroke(polyline(rail.points), with: .color(Palette.color(rail.level).opacity(0.55)),
                                       style: StrokeStyle(lineWidth: 3, lineCap: .round, lineJoin: .round))
                    }
                    for edge in layout.edges {
                        let (shading, style) = look(of: edge.style)
                        context.stroke(path(of: edge), with: shading, style: style)
                    }
                }
                startLabel
                ForEach(Array(layout.labels.enumerated()), id: \.offset) { _, label in
                    branchLabel(label)
                }
                ForEach(layout.questions, id: \.step) { question in
                    questionButton(question)
                }
                ForEach(Array(layout.nodes.enumerated()), id: \.offset) { _, node in
                    nodeButton(node)
                }
                ForEach(layout.pins, id: \.risk) { pin in
                    pinButton(pin)
                }
                if let hovered {
                    sourceCard(for: hovered)
                }
            }
            .frame(width: layout.size.width, height: layout.size.height, alignment: .topLeading)
            .frame(maxWidth: .infinity)
        }
        .background(.background.secondary, in: .rect(cornerRadius: 16))
    }

    // MARK: Линии

    private func path(of edge: TreeLayout.Edge) -> Path {
        Path { path in
            path.move(to: edge.from)
            if edge.curved {
                path.addQuadCurve(to: edge.to, control: CGPoint(x: edge.to.x, y: edge.from.y))
            } else {
                path.addLine(to: edge.to)
            }
        }
    }

    private func polyline(_ points: [CGPoint]) -> Path {
        Path { path in path.addLines(points) }
    }

    private func look(of style: TreeLayout.EdgeStyle) -> (GraphicsContext.Shading, StrokeStyle) {
        switch style {
        case .main: (.color(.primary), StrokeStyle(lineWidth: 2.5, lineCap: .round, lineJoin: .round))
        case .rejected: (.color(.secondary), StrokeStyle(lineWidth: 1.5, lineCap: .round))
        case .loose: (.color(.secondary), StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [4, 4]))
        case .open: (.color(.accentColor), StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [4, 4]))
        }
    }

    // MARK: Узлы и подписи

    private var startLabel: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("НАЧАЛО · \(Wording.day(tree.start?.firstAt))").font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            Text("«\(tree.start?.title ?? tree.task)»").font(.callout.weight(.semibold)).lineLimit(1)
        }
        .frame(width: 320, alignment: .leading)
        .offset(x: layout.startLabel.x, y: layout.startLabel.y)
    }

    private func branchLabel(_ label: TreeLayout.Label) -> some View {
        let branch = tree.steps[label.step].branches.first { $0.id == label.branch }
        let alignment: HorizontalAlignment = label.towardsRight ? .leading : .trailing
        return VStack(alignment: alignment, spacing: 1) {
            Text(branch?.option ?? "")
                .font(.caption.weight(.semibold))
                .foregroundStyle(branch?.state.leads == true ? .primary : .secondary)
            if let reason = branch?.reason, !reason.isEmpty {
                Text(reason).font(.caption).foregroundStyle(.tertiary)
            }
        }
        .lineLimit(2)
        .multilineTextAlignment(label.towardsRight ? .leading : .trailing)
        .frame(width: label.frame.width, alignment: label.towardsRight ? .leading : .trailing)
        .offset(x: label.frame.minX, y: label.frame.minY)
        .allowsHitTesting(false)
    }

    private func questionButton(_ question: TreeLayout.Question) -> some View {
        let step = tree.steps[question.step]
        let selected = focus == .step(step.id)
        return Button {
            focus = .step(step.id)
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(step.question).font(.headline).lineLimit(1)
                Text(Wording.label(step.state)).font(.caption.weight(.semibold)).foregroundStyle(Palette.color(step.state))
                if step.id == tree.head {
                    Text("HEAD").font(.caption2.weight(.bold)).padding(.horizontal, 6).padding(.vertical, 1)
                        .foregroundStyle(.tint).background(.tint.opacity(0.15), in: .capsule)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(selected ? AnyShapeStyle(.tint.opacity(0.15)) : AnyShapeStyle(.background.secondary), in: .rect(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .frame(maxWidth: layout.size.width - question.origin.x - 8, alignment: .leading)
        .fixedSize(horizontal: false, vertical: true)
        .offset(x: question.origin.x, y: question.origin.y)
        .onHover { hover($0, .step(step.id, at: question.origin)) }
    }

    private func nodeButton(_ node: TreeLayout.Node) -> some View {
        let look = NodeLook(node: node, focus: focus)
        return Button {
            switch node.kind {
            case .start: focus = tree.steps.first.map { .step($0.id) }
            case .fork(let step), .branch(let step, _, _): focus = .step(step)
            }
        } label: {
            Circle()
                .fill(look.fill)
                .overlay { Circle().strokeBorder(look.ring, lineWidth: look.ringWidth) }
                .overlay { Text(look.glyph).font(.system(size: node.diameter * 0.55, weight: .bold)).foregroundStyle(look.glyphColor) }
                .frame(width: node.diameter, height: node.diameter)
                .shadow(color: look.selected ? .accentColor.opacity(0.6) : .clear, radius: 4)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(look.accessibility)
        .offset(x: node.center.x - node.diameter / 2, y: node.center.y - node.diameter / 2)
        .onHover { inside in
            switch node.kind {
            case .start: break
            case .fork(let step): hover(inside, .step(step, at: node.center))
            case .branch(let step, let branch, _): hover(inside, .branch(step: step, branch: branch, at: node.center))
            }
        }
    }

    private func pinButton(_ pin: TreeLayout.Pin) -> some View {
        let risk = tree.risks.first { $0.id == pin.risk }
        let selected = focus == .risk(pin.risk)
        let level = risk?.level ?? .info
        return Button {
            focus = .risk(pin.risk)
        } label: {
            HStack(spacing: 6) {
                Circle().fill(Palette.color(level)).frame(width: 7, height: 7)
                Text(level.rawValue).font(.caption.weight(.semibold)).foregroundStyle(Palette.color(level))
                Text(risk?.title ?? "").font(.caption).lineLimit(1)
                if let risk, !risk.isOpen {
                    Text(risk.decided.map(Wording.decided) ?? Wording.label(risk.shownStatus))
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 9)
            .padding(.vertical, 3)
            .background(selected ? AnyShapeStyle(.tint.opacity(0.18)) : AnyShapeStyle(Palette.color(level).opacity(risk?.isOpen == true ? 0.12 : 0.05)),
                        in: .rect(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .frame(maxWidth: 300, alignment: .leading)
        .offset(x: pin.origin.x, y: pin.origin.y)
        .onHover { hover($0, .risk(pin.risk, at: pin.origin)) }
    }

    // MARK: Наведение

    private func hover(_ inside: Bool, _ target: Hovered) {
        if inside {
            hovered = target
        } else if hovered == target {
            hovered = nil
        }
    }

    /// Карточка справа от узла, а у правого края холста — слева от него. Указатель она не
    /// перехватывает: иначе карточка, вставшая под ним, гасила бы саму себя.
    private func sourceCard(for target: Hovered) -> some View {
        let (title, quotes, anchor): (String, [DecisionTree.Quote], CGPoint) = switch target {
        case .step(let id, let at):
            (tree.steps[id].question, tree.steps[id].quotes, at)
        case .branch(let step, let branch, let at):
            tree.steps[step].branches.first { $0.id == branch }
                .map { ("\(Wording.mark($0.state)) \($0.option) — \(Wording.label($0.state))", [$0.quote], at) }
                ?? ("", [], at)
        case .risk(let id, let at):
            tree.risks.first { $0.id == id }.map { ("\($0.level.rawValue): \($0.title)", $0.quotes, at) } ?? ("", [], at)
        }
        let cardWidth: CGFloat = 300
        let x = anchor.x + 18 + cardWidth <= layout.size.width ? anchor.x + 18 : max(0, anchor.x - 18 - cardWidth)
        let y = min(anchor.y + 12, max(0, layout.size.height - 220))
        return SourceCard(title: title, quotes: quotes, conversations: conversations)
            .offset(x: x, y: y)
            .allowsHitTesting(false)
            .zIndex(1)
            .transition(.opacity)
    }
}

/// Как выглядит узел: развилка — точка, ведущая ветка — крупный круг со знаком, остальные —
/// маленькие, ждущая решения — пустой круг с «?».
private struct NodeLook {
    let fill: Color
    let ring: Color
    let ringWidth: CGFloat
    let glyph: String
    let glyphColor: Color
    let selected: Bool
    let accessibility: String

    init(node: TreeLayout.Node, focus: NodeFocus?) {
        switch node.kind {
        case .start:
            (fill, ring, ringWidth, glyph, glyphColor) = (.primary, .clear, 0, "", .clear)
            selected = false
            accessibility = "Начало задачи"
        case .fork(let step):
            (fill, ring, ringWidth, glyph, glyphColor) = (.primary, .clear, 0, "", .clear)
            selected = focus == .step(step)
            accessibility = "Развилка \(step + 1)"
        case .branch(let step, _, let state):
            selected = focus == .step(step) && state.leads
            accessibility = Wording.label(state)
            glyph = Wording.mark(state)
            switch state {
            case .chosen, .agentDefault:
                (fill, ring, ringWidth, glyphColor) = (Palette.color(state), .clear, 0, .white)
            case .open:
                (fill, ring, ringWidth, glyphColor) = (Color(nsColor: .windowBackgroundColor), .accentColor, 1.5, .accentColor)
            case .rejected:
                (fill, ring, ringWidth, glyphColor) = (.secondary.opacity(0.2), .secondary, 1, .secondary)
            case .deferred, .abandoned:
                (fill, ring, ringWidth, glyphColor) = (Color(nsColor: .windowBackgroundColor), .secondary, 1, .secondary)
            }
        }
    }
}
