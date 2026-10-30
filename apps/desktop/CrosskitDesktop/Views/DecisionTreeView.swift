import CrosskitKit
import SwiftUI

/// Дерево решений задачи (макет `design/UXMain.dc.html`). Линии и полосы рисков рисует
/// `Canvas`, узлы, вопросы и метки рисков — кнопки поверх него: клик выбирает узел, и чат
/// открывается на реплике, которой он доказан. Где что стоит, решает `TreeLayout`; здесь
/// только как оно выглядит.
struct DecisionTreeView: View {

    let tree: DecisionTree
    let layout: TreeLayout
    let focus: MapAnchor?
    let select: (MapAnchor) -> Void

    var body: some View {
        ZStack(alignment: .topLeading) {
            Canvas { context, _ in
                for rail in layout.rails {
                    context.stroke(polyline(rail.points), with: .color(Palette.color(rail.level).opacity(0.4)),
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
        }
        .frame(width: layout.size.width, height: layout.size.height, alignment: .topLeading)
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
        case .main: (.color(.primary), StrokeStyle(lineWidth: 2.25, lineCap: .round, lineJoin: .round))
        case .rejected: (.color(.secondary.opacity(0.6)), StrokeStyle(lineWidth: 1.5, lineCap: .round))
        case .loose: (.color(.secondary.opacity(0.6)), StrokeStyle(lineWidth: 1.5, lineCap: .round, dash: [4, 4]))
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
                .foregroundStyle(branch?.state.leads == true || branch?.state == .open ? .primary : .secondary)
            Text(branch.map { $0.state == .open ? "вариант на выбор" : Wording.label($0.state) } ?? "")
                .font(.caption).foregroundStyle(.tertiary)
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
        let waiting = tree.waitingStep?.id == step.id
        return Button {
            select(.step(step.id))
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text("\(step.id + 1)").font(.caption).foregroundStyle(.secondary)
                Text(step.question).font(.callout.weight(.bold)).lineLimit(1)
                if waiting {
                    Text("сейчас").font(.caption2.weight(.bold)).foregroundStyle(.white)
                        .padding(.horizontal, 6).padding(.vertical, 1)
                        .background(.tint, in: .rect(cornerRadius: 5))
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 2)
            .background(selected ? AnyShapeStyle(.tint.opacity(0.14)) : AnyShapeStyle(.background.secondary), in: .rect(cornerRadius: 8))
            .overlay {
                if selected { RoundedRectangle(cornerRadius: 8).strokeBorder(.tint) }
            }
        }
        .buttonStyle(.plain)
        .frame(maxWidth: layout.size.width - question.origin.x - 8, alignment: .leading)
        .fixedSize(horizontal: false, vertical: true)
        .offset(x: question.origin.x, y: question.origin.y)
        .help(step.title == step.question ? "Открыть реплику в чате" : step.title)
    }

    private func nodeButton(_ node: TreeLayout.Node) -> some View {
        let look = NodeLook(node: node, focus: focus)
        return Button {
            switch node.kind {
            case .start: if let first = tree.steps.first { select(.step(first.id)) }
            case .fork(let step), .branch(let step, _, _): select(.step(step))
            }
        } label: {
            Circle()
                .fill(look.fill)
                .overlay { Circle().strokeBorder(look.ring, lineWidth: look.ringWidth) }
                .overlay { Text(look.glyph).font(.system(size: node.diameter * 0.5, weight: .bold)).foregroundStyle(look.glyphColor) }
                .frame(width: node.diameter, height: node.diameter)
                .background {
                    if look.selected { Circle().fill(.tint.opacity(0.2)).padding(-5) }
                }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(look.accessibility)
        .offset(x: node.center.x - node.diameter / 2, y: node.center.y - node.diameter / 2)
    }

    private func pinButton(_ pin: TreeLayout.Pin) -> some View {
        let risk = tree.risks.first { $0.id == pin.risk }
        let selected = focus == .risk(pin.risk)
        let level = risk?.level ?? .info
        let open = risk?.isOpen == true
        return Button {
            select(.risk(pin.risk))
        } label: {
            HStack(spacing: 6) {
                Circle().fill(open ? Palette.color(level) : .secondary).frame(width: 6, height: 6)
                Text(Wording.label(level)).font(.caption.weight(.semibold))
                    .foregroundStyle(open ? Palette.color(level) : .secondary)
                Text(risk?.title ?? "").font(.caption).lineLimit(1)
                    .strikethrough(risk?.shownStatus == .resolved)
                if let risk, !risk.isOpen {
                    Text(risk.decided.map(Wording.decided) ?? Wording.label(risk.shownStatus))
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 2)
            .background(Palette.color(level).opacity(open ? 0.12 : 0.04), in: .rect(cornerRadius: 7))
            .overlay {
                if selected { RoundedRectangle(cornerRadius: 7).strokeBorder(.tint, lineWidth: 1.5) }
            }
        }
        .buttonStyle(.plain)
        .frame(maxWidth: 300, alignment: .leading)
        .offset(x: pin.origin.x, y: pin.origin.y)
        .help("Открыть реплику, где риск поднят")
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

    init(node: TreeLayout.Node, focus: MapAnchor?) {
        switch node.kind {
        case .start:
            (fill, ring, ringWidth, glyph, glyphColor) = (.primary, .clear, 0, "", .clear)
            selected = false
            accessibility = "Начало задачи"
        case .fork(let step):
            (fill, ring, ringWidth, glyph, glyphColor) = (.primary, .clear, 0, "", .clear)
            selected = focus == .step(step)
            accessibility = "Развилка \(step + 1) — открыть реплику в чате"
        case .branch(let step, _, let state):
            selected = focus == .step(step) && state.leads
            accessibility = Wording.label(state) + " — открыть реплику в чате"
            glyph = Wording.mark(state)
            switch state {
            case .chosen, .agentDefault:
                (fill, ring, ringWidth, glyphColor) = (Palette.color(state), .clear, 0, .white)
            case .open:
                (fill, ring, ringWidth, glyphColor) = (Color(nsColor: .windowBackgroundColor), .accentColor, 1.5, .accentColor)
            case .rejected:
                (fill, ring, ringWidth, glyphColor) = (.secondary.opacity(0.15), .secondary.opacity(0.5), 1, .secondary)
            case .deferred, .abandoned:
                (fill, ring, ringWidth, glyphColor) = (Color(nsColor: .windowBackgroundColor), .secondary.opacity(0.5), 1, .secondary)
            }
        }
    }
}
