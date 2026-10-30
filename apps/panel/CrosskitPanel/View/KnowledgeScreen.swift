import AppKit
import QuartzCore

/// Экран «Карта знаний»: весь граф знаний контекста, поиск по нему и источник каждой связи
/// (ADR 0048).
///
/// Живёт поверх выдачи, как граф окрестности: ⌘M открывает карту, esc или ⌘M возвращают к
/// выдаче с тем же запросом. Строка запроса остаётся первым ответчиком и на карте ищет: каждая
/// буква подсвечивает найденные сущности и их соседей, не спрашивая ядро, — карта уже в панели.
/// Клик по сущности — карточка с описанием и написаниями; клик по связи — карточка с цитатой и
/// источником; ⏎ на связи открывает файл или копирует `claude --resume` беседы.
///
/// Рисует страница (`knowledge.html`, раскладка `cose`); здесь — нативное вокруг неё: шапка,
/// строка-объяснение, легенда типов и карточка.
final class KnowledgeScreen {

    let context: String
    private(set) var map: KnowledgeAnswer.Map?
    private(set) var failure: FeedFailure?
    private var query: String
    private var found = KnowledgePicture.Found.nothing
    /// Выбранная сущность или связь — её идентификатор.
    private(set) var selected: String?
    private let source: KnowledgeSource
    private let page: GraphPage
    private var order = AnswerOrder()
    private var inFlight: Task<Void, Never>?
    private var drawn: (generation: Int, dark: Bool, filter: KnowledgePicture.Filter)?
    /// Что скрыто: типы, выключенные в легенде, и папка источников.
    private(set) var filter = KnowledgePicture.Filter.everything
    /// Цель выбора папки; держится экраном, пока жив пункт меню.
    private var folderChooser: Widgets.Chooser?
    /// Что выбрать, когда карта придёт; только для снимка (`--pick`).
    private let pickOnArrival: String?
    private var requestedAt: CFTimeInterval = 0
    private var answeredAt: CFTimeInterval = 0

    var onChange: (() -> Void)?
    /// Карта с этим номером разложена и стоит на экране. Нужно снимку и замеру.
    var onApplied: ((Int) -> Void)?

    init(source: KnowledgeSource, page: GraphPage, context: String, query: String, pick: String? = nil) {
        self.source = source
        self.pickOnArrival = pick
        self.page = page
        self.context = context
        self.query = query
        page.onTap = { [weak self] id in self?.pick(id.isEmpty ? nil : id) }
        page.onTapEdge = { [weak self] id in self?.pick(id) }
        page.onLaidOut = { [weak self] layout in self?.report(layoutMilliseconds: layout) }
        page.onRendered = { [weak self] generation in self?.onApplied?(generation) }
    }

    deinit { inFlight?.cancel() }

    // MARK: Запрос

    func request() {
        inFlight?.cancel()
        let generation = order.issue()
        requestedAt = CACurrentMediaTime()
        let (source, context) = (source, context)
        let screen = WeakKnowledge(self)
        inFlight = Task.detached(priority: .userInitiated) {
            do {
                let map = try await source.knowledge(context: context)
                await MainActor.run { screen.knowledge?.apply(map, generation: generation) }
            } catch is CancellationError {
                // Закрыта раньше, чем ядро ответило.
            } catch {
                await MainActor.run { screen.knowledge?.apply(failure: error, generation: generation) }
            }
        }
    }

    func cancel() {
        inFlight?.cancel()
    }

    private func apply(_ map: KnowledgeAnswer.Map, generation: Int) {
        guard order.accept(generation) else { return }
        answeredAt = CACurrentMediaTime()
        self.map = map
        failure = nil
        found = KnowledgePicture.found(query, in: shown(map))
        selected = pickOnArrival ?? found.hits.first
        drawn = nil
        onChange?()
        if map.entities.isEmpty { onApplied?(generation) }
    }

    private func apply(failure: Error, generation: Int) {
        guard order.accept(generation) else { return }
        map = nil
        self.failure = failure as? FeedFailure ?? .core(.failed)
        onChange?()
        onApplied?(generation)
    }

    /// Строка замера: сколько ответ ядра и сколько раскладка — числа, по которым назван предел.
    private func report(layoutMilliseconds: Double) {
        guard let map else { return }
        print(String(format: "карта знаний: %d сущностей, %d связей — ядро %.0f мс, раскладка %.0f мс",
                     map.entities.count, map.facts.count, (answeredAt - requestedAt) * 1000, layoutMilliseconds))
        fflush(stdout)
    }

    // MARK: Действия

    /// Набранный текст — поиск по карте, без процесса ядра.
    func find(_ text: String) {
        query = text
        guard let map else { return }
        found = KnowledgePicture.found(text, in: shown(map))
        selected = found.hits.first
        page.send(.highlight(found))
        page.send(.pick(selected))
        onChange?()
    }

    /// Карта после фильтра: то, что на холсте, и то, по чему ищет строка запроса.
    private func shown(_ map: KnowledgeAnswer.Map) -> KnowledgeAnswer.Map {
        KnowledgePicture.filtered(map, by: filter)
    }

    /// Клик по типу в легенде: скрыть его или показать снова.
    func toggle(type: String) {
        if filter.hiddenTypes.contains(type) {
            filter.hiddenTypes.remove(type)
        } else {
            filter.hiddenTypes.insert(type)
        }
        refilter()
    }

    /// Выбор папки источников; `nil` — все.
    func choose(folder: String?) {
        filter.folder = folder
        refilter()
    }

    private func refilter() {
        guard let map else { return }
        found = KnowledgePicture.found(query, in: shown(map))
        if let id = selected, shown(map).entity(id) == nil, shown(map).fact(id) == nil { selected = nil }
        onChange?()
    }

    /// Найденное есть — ↑ ↓ ходят по нему.
    var listsFound: Bool { map != nil && !found.hits.isEmpty }

    /// ⏎ на выбранной сущности: она — в середину карты.
    func centerOnSelected() {
        guard let id = selected, map?.entity(id) != nil else { return }
        page.center(on: id)
    }

    private func pick(_ id: String?) {
        selected = id
        page.send(.pick(id))
        onChange?()
    }

    /// ↑↓ — по найденным сущностям, по кругу.
    func move(by delta: Int) {
        guard !found.hits.isEmpty else { return }
        let at = found.hits.firstIndex { $0 == selected } ?? -delta
        let next = (at + delta + found.hits.count) % found.hits.count
        pick(found.hits[next])
    }

    /// Что открывает ⏎: источник выбранной связи.
    var selectedRow: FeedRow? {
        guard let id = selected, let fact = map?.fact(id) else { return nil }
        switch fact.source.kind {
        case .file:
            return .openable(path: fact.source.path, id: fact.id, present: true)
        case .conversation:
            guard let session = fact.source.sessionId else { return nil }
            return .resumable(session: session, id: fact.id, title: fact.source.conversation ?? "",
                              messageCount: nil)
        }
    }

    func keys() -> [(String, String)] {
        var keys = CanvasKey.hints
        if listsFound { keys[0] = ("← →", "Сдвиг"); keys.insert(("↑↓", "По найденным"), at: 1) }
        if let id = selected, let fact = map?.fact(id) {
            keys.append(("⏎", fact.source.kind == .file ? "Открыть файл" : "Скопировать claude --resume"))
        } else if let id = selected, map?.entity(id) != nil {
            keys.append(("⏎", "В середину"))
        }
        return keys
    }

    // MARK: Экран

    static let headerHeight: CGFloat = 34
    static let lineHeight: CGFloat = 22
    static let canvasHeight: CGFloat = 440
    static let legendHeight: CGFloat = 24
    static let cardHeight: CGFloat = 70

    func body(appearance: NSAppearance) -> NSView {
        let width = Tokens.windowWidth
        let body = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: width, height: 0))
        var y = place(header(), in: body, at: 0)

        if let failure {
            let wording = failure.wording(context: context)
            y = place(GuidePieces.refusalHeader(title: wording.title, explanation: wording.explanation),
                      in: body, at: y)
        } else if let map, map.entities.isEmpty {
            y = place(wrapped(KnowledgeWording.empty), in: body, at: y)
        } else if let whole = map {
            let map = shown(whole)
            y = place(line(KnowledgeWording.explanation(map, query: query, found: found)), in: body, at: y)
            page.webView.frame = NSRect(x: 0, y: y, width: width, height: Self.canvasHeight)
            body.addSubview(page.webView)
            y += Self.canvasHeight
            draw(map, appearance: appearance)
            y = place(legend(whole), in: body, at: y)
            y = place(sources(whole), in: body, at: y)
            if let card = selectedCard(map) {
                y = place(cardView(card), in: body, at: y)
            }
        } else {
            let waiting = Widgets.label("Собираем карту…", font: Tokens.text(12), color: Tokens.fg3)
            waiting.setFrameOrigin(NSPoint(x: 18, y: y + 20))
            body.addSubview(waiting)
            y += 60
        }
        body.setFrameSize(NSSize(width: width, height: y + 6))
        return body
    }

    private func selectedCard(_ map: KnowledgeAnswer.Map) -> KnowledgeWording.Card? {
        guard let id = selected else { return nil }
        if let fact = map.fact(id) { return KnowledgeWording.card(of: fact, in: map) }
        return map.entity(id).map(KnowledgeWording.card(of:))
    }

    /// Карта уходит странице один раз на ответ и ещё раз при смене оформления; поиск и выбор —
    /// своими вызовами, без новой раскладки.
    private func draw(_ map: KnowledgeAnswer.Map, appearance: NSAppearance) {
        let dark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let generation = order.accepted
        let unchanged = drawn.map { $0.generation == generation && $0.dark == dark && $0.filter == filter }
        guard unchanged != true else { return }
        drawn = (generation, dark, filter)
        let theme = KnowledgeTheme.current(appearance)
        page.send(.renderKnowledge(KnowledgePicture.of(map, palette: theme.palette), theme: theme,
                                   found: found, generation: generation))
        page.send(.pick(selected))
    }

    private func header() -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: Self.headerHeight))
        let title = Widgets.label("Карта знаний", font: Tokens.text(12, .semibold), color: Tokens.fg)
        title.setFrameOrigin(NSPoint(x: 16, y: (Self.headerHeight - title.frame.height) / 2))
        row.addSubview(title)

        let hint = Widgets.label("к выдаче", font: Tokens.text(11), color: Tokens.fg3)
        hint.setFrameOrigin(NSPoint(x: Tokens.windowWidth - 16 - hint.frame.width,
                                    y: (Self.headerHeight - hint.frame.height) / 2))
        row.addSubview(hint)
        let cap = Widgets.keyCap("⌘M")
        cap.setFrameOrigin(NSPoint(x: hint.frame.minX - 5 - cap.frame.width, y: (Self.headerHeight - 18) / 2))
        row.addSubview(cap)

        var right = cap.frame.minX - 14
        if map != nil {
            right = HeaderPieces.zoomButtons(in: row, endingAt: right, height: Self.headerHeight) { [page] step in
                page.zoom(step)
            }
        }

        let counts = map.map(KnowledgeWording.counts) ?? (order.awaiting ? "обновляется" : "")
        let label = Widgets.label(counts, font: Tokens.text(11), color: Tokens.fg3)
        label.lineBreakMode = .byTruncatingTail
        label.frame = NSRect(x: title.frame.maxX + 10, y: (Self.headerHeight - label.frame.height) / 2,
                             width: max(0, right - 8 - title.frame.maxX - 10), height: label.frame.height)
        row.addSubview(label)
        return row
    }

    private func line(_ text: String) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: Self.lineHeight))
        let label = Widgets.label(text, font: Tokens.text(11.5), color: Tokens.fg2)
        label.lineBreakMode = .byTruncatingTail
        label.frame = NSRect(x: 16, y: (Self.lineHeight - label.frame.height) / 2,
                             width: Tokens.windowWidth - 32, height: label.frame.height)
        row.addSubview(label)
        return row
    }

    private func wrapped(_ text: String) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: 58))
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = Tokens.text(12)
        label.textColor = Tokens.fg2
        label.frame = NSRect(x: 16, y: 12, width: Tokens.windowWidth - 32, height: 36)
        row.addSubview(label)
        return row
    }

    /// Типы, которые есть на этой карте, в порядке словаря — их цвет и имя. Клик скрывает тип
    /// или показывает снова; скрытый — бледный и зачёркнутый.
    private func legend(_ map: KnowledgeAnswer.Map) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: Self.legendHeight))
        let present = Set(map.entities.map(\.type))
        var x: CGFloat = 16
        for (index, type) in map.entityTypes.enumerated() where present.contains(type) {
            let hidden = filter.hiddenTypes.contains(type)
            let text = NSAttributedString(string: type, attributes: [
                .font: Tokens.text(10.5), .foregroundColor: hidden ? Tokens.fg3.withAlphaComponent(0.5) : Tokens.fg2,
                .strikethroughStyle: hidden ? NSUnderlineStyle.single.rawValue : 0])
            let label = Widgets.label(text)
            let width = 13 + label.frame.width
            guard x + width < Tokens.windowWidth - 16 else { break }
            let item = Widgets.Clickable(frame: NSRect(x: x, y: 0, width: width, height: Self.legendHeight)) { [weak self] in
                self?.toggle(type: type)
            }
            let color = Tokens.knowledgePalette[index % Tokens.knowledgePalette.count]
            let dot = Widgets.Fill(hidden ? color.withAlphaComponent(0.25) : color, radius: 4)
            dot.frame = NSRect(x: 0, y: (Self.legendHeight - 8) / 2, width: 8, height: 8)
            item.addSubview(dot)
            label.setFrameOrigin(NSPoint(x: 13, y: (Self.legendHeight - label.frame.height) / 2))
            item.addSubview(label)
            row.addSubview(item)
            x += width + 12
        }
        return row
    }

    /// Выбор папки источников: все, корень контекста или папка первого уровня под ним.
    private func sources(_ map: KnowledgeAnswer.Map) -> NSView {
        let row = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: Self.legendHeight + 4))
        let caption = Widgets.label("Источники:", font: Tokens.text(10.5), color: Tokens.fg3)
        caption.setFrameOrigin(NSPoint(x: 16, y: (row.frame.height - caption.frame.height) / 2))
        row.addSubview(caption)
        let folders = KnowledgePicture.folders(of: map)
        let menu = NSPopUpButton(frame: NSRect(x: caption.frame.maxX + 6, y: 1, width: 420, height: 22), pullsDown: false)
        menu.controlSize = .small
        menu.font = Tokens.text(11)
        menu.addItem(withTitle: "все — \(map.facts.count) \(KnowledgeWording.factsWord(map.facts.count))")
        for (folder, facts) in folders {
            menu.addItem(withTitle: "\(GraphWording.shortened(folder)) — \(facts) \(KnowledgeWording.factsWord(facts))")
        }
        if let chosen = filter.folder, let at = folders.firstIndex(where: { $0.folder == chosen }) {
            menu.selectItem(at: at + 1)
        }
        let chooser = Widgets.Chooser { [weak self] index in
            self?.choose(folder: index == 0 ? nil : folders[index - 1].folder)
        }
        folderChooser = chooser
        menu.target = chooser
        menu.action = #selector(Widgets.Chooser.chosen(_:))
        row.addSubview(menu)
        return row
    }

    private func cardView(_ card: KnowledgeWording.Card) -> NSView {
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
        title.lineBreakMode = .byTruncatingTail
        title.frame = NSRect(x: left, y: 8, width: right - left, height: title.frame.height)
        box.addSubview(title)
        let detail = NSTextField(wrappingLabelWithString: card.lines.joined(separator: "\n"))
        detail.font = Tokens.text(11)
        detail.textColor = Tokens.fg3
        detail.maximumNumberOfLines = 3
        detail.lineBreakMode = .byTruncatingTail
        detail.frame = NSRect(x: left, y: 25, width: box.frame.width - left - 10, height: 42)
        box.addSubview(detail)
        return outer
    }

    private func place(_ view: NSView, in parent: NSView, at y: CGFloat) -> CGFloat {
        view.setFrameOrigin(NSPoint(x: 0, y: y))
        parent.addSubview(view)
        return y + view.frame.height
    }
}

extension KnowledgeTheme {

    /// Токены `Tokens.swift` и палитра типов в этом оформлении — строками CSS для страницы.
    static func current(_ appearance: NSAppearance) -> KnowledgeTheme {
        var theme: KnowledgeTheme!
        appearance.performAsCurrentDrawingAppearance {
            theme = KnowledgeTheme(
                fg: GraphTheme.css(Tokens.fg), fg2: GraphTheme.css(Tokens.fg2), edge: GraphTheme.css(Tokens.edge),
                accent: GraphTheme.css(Tokens.accent), solid: GraphTheme.css(Tokens.solidBackground),
                palette: Tokens.knowledgePalette.map(GraphTheme.css))
        }
        return theme
    }
}

/// Слабая ссылка на экран карты для задачи не на главном потоке.
private final class WeakKnowledge: @unchecked Sendable {
    weak var knowledge: KnowledgeScreen?

    init(_ knowledge: KnowledgeScreen) {
        self.knowledge = knowledge
    }
}
