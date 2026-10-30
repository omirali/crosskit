import AppKit

/// Экран графа: окрестность выбранной строки, её состояние и её куски на экране.
///
/// Живёт поверх выдачи, а не вместо неё: ⇥ на строке открывает граф с фокусом на ней, esc или
/// ⇥ возвращают к выдаче с тем же выбором. Клик по беседе или файлу — новый фокус и новый
/// запрос к ядру; по остальному — только карточка: «клик — фокус, не открытие». Открывает ⏎.
///
/// Рисует страница (`GraphPage`, Cytoscape + dagre); здесь — нативное вокруг неё: строка-
/// объяснение сверху, легенда и карточка снизу. Они стоят стопкой и ничего не перекрывают.
///
/// Ответы приходят не по порядку: быстрый клик по двум узлам не должен оставить на экране вид
/// первого — `AnswerOrder`, и отмена завершает процесс старого.
final class GraphScreen {

    /// Глубина, которую панель спрашивает: два шага, как в макете. Замер спрашивает и одну.
    static let defaultDepth = 2

    let context: String
    private(set) var focus: String
    private(set) var view: GraphAnswer.View?
    private(set) var failure: FeedFailure?
    private(set) var selected: String?
    private let depth: Int
    private let source: GraphSource
    private let page: GraphPage
    private var order = AnswerOrder()
    private var inFlight: Task<Void, Never>?
    /// Номер вида, отправленного странице, и оформление, в котором он отправлен.
    private var drawn: (generation: Int, dark: Bool)?

    /// Состояние сменилось — экран надо перестроить.
    var onChange: (() -> Void)?
    /// Вид по запросу с этим номером нарисован страницей и стоит на экране. Нужно замеру.
    var onApplied: ((Int) -> Void)?

    var lastGeneration: Int { order.issued }
    var awaiting: Bool { order.awaiting }

    init(source: GraphSource, page: GraphPage, context: String, focus: String,
         depth: Int = GraphScreen.defaultDepth) {
        self.source = source
        self.page = page
        self.context = context
        self.focus = focus
        self.depth = depth
        page.onTap = { [weak self] id in self?.click(id) }
        page.onRendered = { [weak self] generation in self?.onApplied?(generation) }
    }

    deinit { inFlight?.cancel() }

    // MARK: Запрос

    /// Спросить ядро о текущем фокусе. Запрос уходит отвязанной задачей, не дожидаясь
    /// перерисовки главного потока — урок замера UI-3.
    func request() {
        inFlight?.cancel()
        let generation = order.issue()
        let (source, context, focus, depth) = (source, context, focus, depth)
        let screen = WeakGraph(self)
        inFlight = Task.detached(priority: .userInitiated) {
            do {
                let view = try await source.graph(context: context, focus: focus, depth: depth,
                                                  generation: generation)
                await MainActor.run { screen.graph?.apply(view, generation: generation) }
            } catch is CancellationError {
                // Отменён новым фокусом или возвратом к выдаче.
            } catch {
                await MainActor.run { screen.graph?.apply(failure: error, generation: generation) }
            }
        }
    }

    func cancel() {
        inFlight?.cancel()
    }

    private func apply(_ view: GraphAnswer.View, generation: Int) {
        guard order.accept(generation) else { return }
        self.view = view
        failure = nil
        selected = view.focus
        drawn = nil
        onChange?()
    }

    private func apply(failure: Error, generation: Int) {
        guard order.accept(generation) else { return }
        view = nil
        self.failure = failure as? FeedFailure ?? .core(.failed)
        onChange?()
        onApplied?(generation)
    }

    // MARK: Действия

    /// Клик: беседа или файл — новый фокус, остальное — только выбор.
    func click(_ id: String) {
        guard let node = view?.node(id) else { return }
        selected = id
        if node.focusable, id != view?.focus {
            focus = id
            request()
        } else {
            page.send(.select(id))
        }
        onChange?()
    }

    /// ⌥↑ ⌥↓ — по узлам холста в порядке ядра: от фокуса к дальним. Реплик на холсте нет.
    /// Простые стрелки двигают холст (`CanvasKey`).
    func move(by delta: Int) {
        guard let nodes = view?.nodes.filter({ $0.kind != .reply }), !nodes.isEmpty else { return }
        let at = nodes.firstIndex { $0.id == selected } ?? 0
        selected = nodes[min(max(at + delta, 0), nodes.count - 1)].id
        page.send(.select(selected))
        onChange?()
    }

    var selectedNode: GraphAnswer.Node? {
        selected.flatMap { view?.node($0) }
    }

    /// Строка выдачи для выбранного узла — то, что умеют открыть ⏎ и ⌘⏎ (`Opening`).
    var selectedRow: FeedRow? {
        guard let node = selectedNode else { return nil }
        switch node.kind {
        case .conversation:
            guard let facts = node.conversation else { return nil }
            return .resumable(session: facts.sessionId, id: node.id, title: GraphWording.title(of: node),
                              messageCount: facts.messageCount)
        case .file:
            guard let file = node.file, file.state != .outside else { return nil }
            return .openable(path: file.path, id: node.id, present: file.state == .present)
        default:
            return nil
        }
    }

    // MARK: Экран

    static let headerHeight: CGFloat = 34
    static let explanationHeight: CGFloat = 22
    static let legendHeight: CGFloat = 24
    static let cardHeight: CGFloat = 52

    /// Высота холста: вид из нескольких узлов не растягивается на полэкрана, а вид из ста —
    /// листается внутри страницы.
    private static func canvasHeight(for view: GraphAnswer.View) -> CGFloat {
        let drawn = view.nodes.filter { $0.kind != .reply }.count
        return min(320, max(150, CGFloat(drawn) * 24))
    }

    /// Тело экрана: шапка, объяснение, холст, легенда, карточка выбранного.
    func body(appearance: NSAppearance) -> NSView {
        let width = Tokens.windowWidth
        let body = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: width, height: 0))
        var y: CGFloat = 0
        y = place(header(), in: body, at: y)

        if let failure {
            let wording = failure.wording(context: context)
            y = place(GuidePieces.refusalHeader(title: wording.title, explanation: wording.explanation),
                      in: body, at: y)
        } else if let view {
            y = place(line(GraphWording.explanation(view), color: Tokens.fg2), in: body, at: y)
            if let empty = GraphWording.emptiness(view) {
                y = place(line(empty, color: Tokens.fg3), in: body, at: y)
            }
            let height = Self.canvasHeight(for: view)
            page.webView.frame = NSRect(x: 0, y: y, width: width, height: height)
            body.addSubview(page.webView)
            y += height
            draw(view, appearance: appearance)
            y = place(legend(), in: body, at: y)
            if let node = selectedNode {
                y = place(card(GraphWording.card(of: node, in: view, now: Date())), in: body, at: y)
            }
        } else {
            let waiting = Widgets.label("Строим окрестность…", font: Tokens.text(12), color: Tokens.fg3)
            waiting.setFrameOrigin(NSPoint(x: 18, y: y + 20))
            body.addSubview(waiting)
            y += 60
        }
        body.setFrameSize(NSSize(width: width, height: y + 6))
        return body
    }

    /// Вид уходит странице один раз на ответ и ещё раз при смене оформления; выбор узла — своим
    /// вызовом, без перерисовки.
    private func draw(_ view: GraphAnswer.View, appearance: NSAppearance) {
        let dark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let generation = order.accepted
        guard drawn.map({ $0.generation != generation || $0.dark != dark }) ?? true else { return }
        drawn = (generation, dark)
        page.send(.render(GraphPicture.of(view), theme: GraphTheme.current(appearance),
                          generation: generation, selected: selected))
    }

    /// Строка под экраном: сколько каждого вида найдено и показано, и что свёрнуто правилами.
    func countsLine() -> NSView? {
        guard let view else { return nil }
        var parts = [GraphWording.counts(view)]
        if view.notShown.inPassing > 0 { parts.append("мимоходом, свёрнуто: \(view.notShown.inPassing)") }
        if view.notShown.byPolicy > 0 { parts.append("скрыто политикой: \(view.notShown.byPolicy)") }
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth,
                                                height: FooterPieces.countersHeight))
        let label = Widgets.label(parts.joined(separator: " · "), font: Tokens.text(11.5), color: Tokens.fg3)
        label.frame = NSRect(x: 18, y: (FooterPieces.countersHeight - label.frame.height) / 2,
                             width: Tokens.windowWidth - 36, height: label.frame.height)
        row.addSubview(label)
        return row
    }

    /// Клавиши графа: что можно сделать с выбранным узлом сейчас.
    func keys() -> [(String, String)] {
        var keys = CanvasKey.hints + [("⌥↑↓", "Узел")]
        switch selectedNode?.kind {
        case .conversation?: keys.append(("⏎", "Скопировать claude --resume"))
        case .file? where selectedNode?.file?.state == .present:
            keys.append(("⏎", "Открыть"))
            keys.append(("⌘⏎", "Finder"))
        default: break
        }
        return keys
    }

    private func header() -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: Self.headerHeight))
        let title = Widgets.label("Граф", font: Tokens.text(12, .semibold), color: Tokens.fg)
        title.setFrameOrigin(NSPoint(x: 16, y: (Self.headerHeight - title.frame.height) / 2))
        row.addSubview(title)

        var right = Tokens.windowWidth - 16
        let hint = Widgets.label("к выдаче", font: Tokens.text(11), color: Tokens.fg3)
        hint.setFrameOrigin(NSPoint(x: right - hint.frame.width, y: (Self.headerHeight - hint.frame.height) / 2))
        row.addSubview(hint)
        let cap = Widgets.keyCap("⇥")
        cap.setFrameOrigin(NSPoint(x: hint.frame.minX - 5 - cap.frame.width, y: (Self.headerHeight - 18) / 2))
        row.addSubview(cap)
        right = cap.frame.minX - 12
        if view != nil {
            right = HeaderPieces.zoomButtons(in: row, endingAt: right - 2, height: Self.headerHeight) { [page] step in
                page.zoom(step)
            } - 8
        }

        let state = awaiting ? "обновляется" : view.flatMap(GraphWording.truncation)
        if let state {
            let text = Widgets.label(state, font: Tokens.text(11), color: Tokens.fg2)
            let pill = Widgets.Fill(.clear, radius: 10, border: Tokens.fg3)
            pill.frame = NSRect(x: right - text.frame.width - 16, y: (Self.headerHeight - 20) / 2,
                                width: text.frame.width + 16, height: 20)
            text.setFrameOrigin(NSPoint(x: 8, y: (20 - text.frame.height) / 2))
            pill.addSubview(text)
            row.addSubview(pill)
            right = pill.frame.minX - 12
        }

        let limits = view.map { "глубина \($0.depth) · до \($0.limit) узлов" } ?? "глубина \(depth)"
        let depthLabel = Widgets.label(limits, font: Tokens.text(11), color: Tokens.fg3)
        depthLabel.setFrameOrigin(NSPoint(x: title.frame.maxX + 10, y: (Self.headerHeight - depthLabel.frame.height) / 2))
        row.addSubview(depthLabel)
        return row
    }

    private func line(_ text: String, color: NSColor) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: Self.explanationHeight))
        let label = Widgets.label(text, font: Tokens.text(11.5), color: color)
        label.frame = NSRect(x: 16, y: (Self.explanationHeight - label.frame.height) / 2,
                             width: Tokens.windowWidth - 32, height: label.frame.height)
        row.addSubview(label)
        return row
    }

    private func legend() -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: Self.legendHeight))
        let items: [(LegendGlyph, String)] = [
            (.plate(Tokens.nodeConversation), "Беседа"), (.square(Tokens.nodeFile), "Файл"),
            (.dashed(Tokens.warn), "нет на диске"), (.dashed(Tokens.fg3), "вне контекста"),
            (.diamond(Tokens.nodeProject), "Проект"), (.capsule(Tokens.topicMark), "Тема"),
            (.line(Tokens.accent), "вы назвали"), (.dotted(Tokens.fg3), "только читала")]
        var x: CGFloat = 16
        for (glyph, title) in items {
            let mark = LegendMark(glyph)
            mark.frame = NSRect(x: x, y: (Self.legendHeight - 10) / 2, width: glyph.width, height: 10)
            row.addSubview(mark)
            let label = Widgets.label(title, font: Tokens.text(10.5), color: Tokens.fg3)
            label.setFrameOrigin(NSPoint(x: mark.frame.maxX + 5, y: (Self.legendHeight - label.frame.height) / 2))
            row.addSubview(label)
            x = label.frame.maxX + 12
        }
        let click = Widgets.label("клик — фокус", font: Tokens.text(10.5), color: Tokens.fg3)
        click.setFrameOrigin(NSPoint(x: Tokens.windowWidth - 16 - click.frame.width,
                                     y: (Self.legendHeight - click.frame.height) / 2))
        if click.frame.minX > x { row.addSubview(click) }
        return row
    }

    private func card(_ card: GraphWording.Card) -> NSView {
        let outer = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: Self.cardHeight + 6))
        let box = Widgets.Fill(Tokens.fill, radius: 9)
        box.frame = NSRect(x: 8, y: 2, width: Tokens.windowWidth - 16, height: Self.cardHeight)
        outer.addSubview(box)

        let type = Widgets.label(card.type, font: Tokens.text(10.5, .semibold), color: Tokens.fg2)
        let chip = Widgets.Fill(Tokens.fill2, radius: 5)
        chip.frame = NSRect(x: 10, y: 8, width: type.frame.width + 12, height: 18)
        type.setFrameOrigin(NSPoint(x: 6, y: (18 - type.frame.height) / 2))
        chip.addSubview(type)
        box.addSubview(chip)

        var right = box.frame.width - 10
        if let key = card.key, let action = card.action {
            let label = Widgets.label(action, font: Tokens.text(11), color: Tokens.fg2)
            label.setFrameOrigin(NSPoint(x: right - label.frame.width, y: 9))
            box.addSubview(label)
            let cap = Widgets.keyCap(key)
            cap.setFrameOrigin(NSPoint(x: label.frame.minX - 6 - cap.frame.width, y: 8))
            box.addSubview(cap)
            right = cap.frame.minX - 10
        }
        let left = chip.frame.maxX + 10
        let title = Widgets.label(card.title, font: Tokens.text(12, .semibold), color: Tokens.fg)
        title.frame = NSRect(x: left, y: 8, width: right - left, height: title.frame.height)
        box.addSubview(title)
        let detail = NSTextField(wrappingLabelWithString: card.detail)
        detail.font = Tokens.text(11)
        detail.textColor = Tokens.fg3
        detail.maximumNumberOfLines = 2
        detail.lineBreakMode = .byTruncatingTail
        detail.frame = NSRect(x: left, y: 25, width: box.frame.width - left - 10, height: 26)
        box.addSubview(detail)
        return outer
    }

    private func place(_ view: NSView, in parent: NSView, at y: CGFloat) -> CGFloat {
        view.setFrameOrigin(NSPoint(x: 0, y: y))
        parent.addSubview(view)
        return y + view.frame.height
    }
}

/// Значки легенды — те же формы, что рисует страница, нарисованные AppKit.
private enum LegendGlyph {
    case plate(NSColor), square(NSColor), dashed(NSColor), diamond(NSColor), capsule(NSColor), line(NSColor)
    case dotted(NSColor)

    var width: CGFloat {
        switch self {
        case .plate, .line, .dotted: return 16
        default: return 10
        }
    }
}

private final class LegendMark: NSView {
    private let glyph: LegendGlyph

    init(_ glyph: LegendGlyph) {
        self.glyph = glyph
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("панель собирается кодом, не из nib") }

    override func draw(_ dirtyRect: NSRect) {
        let rect = bounds.insetBy(dx: 1, dy: 1)
        switch glyph {
        case .plate(let color):
            color.setFill()
            NSBezierPath(roundedRect: rect, xRadius: 3, yRadius: 3).fill()
        case .square(let color):
            color.setFill()
            NSBezierPath(roundedRect: rect, xRadius: 2, yRadius: 2).fill()
        case .dashed(let color):
            let path = NSBezierPath(roundedRect: rect, xRadius: 2, yRadius: 2)
            path.lineWidth = 1.5
            path.setLineDash([2.5, 2], count: 2, phase: 0)
            color.setStroke()
            path.stroke()
        case .diamond(let color):
            let path = NSBezierPath()
            path.move(to: NSPoint(x: rect.midX, y: rect.minY))
            path.line(to: NSPoint(x: rect.maxX, y: rect.midY))
            path.line(to: NSPoint(x: rect.midX, y: rect.maxY))
            path.line(to: NSPoint(x: rect.minX, y: rect.midY))
            path.close()
            color.setFill()
            path.fill()
        case .capsule(let color):
            let path = NSBezierPath(roundedRect: rect.insetBy(dx: 0, dy: 1.5), xRadius: 3.5, yRadius: 3.5)
            path.lineWidth = 1.5
            color.setStroke()
            path.stroke()
        case .dotted(let color):
            let path = NSBezierPath()
            path.move(to: NSPoint(x: rect.minX, y: rect.midY))
            path.line(to: NSPoint(x: rect.maxX, y: rect.midY))
            path.lineWidth = 1.4
            path.setLineDash([1.5, 2], count: 2, phase: 0)
            color.setStroke()
            path.stroke()
        case .line(let color):
            let path = NSBezierPath()
            path.move(to: NSPoint(x: rect.minX, y: rect.midY))
            path.line(to: NSPoint(x: rect.maxX, y: rect.midY))
            path.lineWidth = 1.8
            color.setStroke()
            path.stroke()
        }
    }
}

extension GraphTheme {

    /// Токены `Tokens.swift` в этом оформлении — строками CSS для страницы.
    static func current(_ appearance: NSAppearance) -> GraphTheme {
        var theme: GraphTheme!
        appearance.performAsCurrentDrawingAppearance {
            theme = GraphTheme(
                fg: css(Tokens.fg), fg2: css(Tokens.fg2), fg3: css(Tokens.fg3),
                conversation: css(Tokens.nodeConversation), file: css(Tokens.nodeFile),
                project: css(Tokens.nodeProject), topic: css(Tokens.topicMark), warn: css(Tokens.warn),
                edge: css(Tokens.edge), accent: css(Tokens.accent), solid: css(Tokens.solidBackground))
        }
        return theme
    }

    static func css(_ color: NSColor) -> String {
        guard let rgb = color.usingColorSpace(.sRGB) else { return "rgba(128,128,128,1)" }
        return String(format: "rgba(%d,%d,%d,%.3f)", Int(rgb.redComponent * 255), Int(rgb.greenComponent * 255),
                      Int(rgb.blueComponent * 255), rgb.alphaComponent)
    }
}

/// Слабая ссылка на экран графа для задачи не на главном потоке.
private final class WeakGraph: @unchecked Sendable {
    weak var graph: GraphScreen?

    init(_ graph: GraphScreen) {
        self.graph = graph
    }
}
