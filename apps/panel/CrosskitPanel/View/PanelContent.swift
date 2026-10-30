import AppKit

/// Экран панели: собирает его из кусков и владеет состоянием.
///
/// Откуда берутся строки, здесь не знают: есть `ResultFeed`, у него спрашивают страницу и
/// ждут ответа не замирая. Всё, что рисуется, приходит оттуда — включая числа под списком.
final class PanelContent: NSVisualEffectView {

    /// Что панель умеет сделать по клавише.
    enum Command {
        case moveUp
        case moveDown
        case primary
        case secondary
        case graph
        /// ⌘M: карта знаний контекста — и с неё обратно к выдаче.
        case knowledge
        /// + − 0, ⌘= ⌘- ⌘0 и кнопки на холсте — графе или карте.
        case zoom(CanvasZoom)
        /// Стрелки на холсте: сдвиг видимой зоны долями её размера.
        case step(dx: Double, dy: Double)
        /// esc на экране графа или карты: назад к выдаче, с тем же выбором.
        case back
        case filter(FeedScope)
        case chooseContext
        case copyIdentifier
    }

    private enum Layout {
        static let sidePadding: CGFloat = 16
        static let headerHeight: CGFloat = 56
        static let listTopPadding: CGFloat = 4
        static let listBottomPadding: CGFloat = 8
        static let hairline: CGFloat = 1
        /// Список выдачи выше этого не растёт, а прокручивается — так в макете. Экраны,
        /// которые вместо выдачи объясняют (пустое окно, «ничего нет»), не ограничены:
        /// у них содержимое конечное и прокручивать нечего.
        static let listMaxHeight: CGFloat = 300
    }

    let queryField = NSTextField()
    var onHeightChange: ((CGFloat) -> Void)?
    /// Строка увела человека туда, куда он просил, — панель можно спрятать.
    var onLeft: (() -> Void)?
    /// Ответ на запрос с этим номером применён и экран перестроен. Нужно замеру.
    var onAnswerApplied: ((Int) -> Void)?
    /// То же для вида графа: номер — его собственный, у графа своя очередь ответов.
    var onGraphApplied: ((Int) -> Void)?
    /// То же для карты знаний.
    var onKnowledgeApplied: ((Int) -> Void)?

    private let feed: ResultFeed
    private let openAlwaysFails: Bool

    private var context: String
    private var scope: FeedScope
    private var sections: [FeedSection] = []
    private var counters: FeedCounters?
    private var indexState: IndexState = .fresh
    private var corpus: CorpusSummary?
    private var next: FeedCursor?
    private var failure: FeedFailure?
    private var openFailure: OpenFailure?
    /// Строка о том, что панель только что сделала. Живёт до следующего запроса.
    private var notice: String?
    private var steps: [GuidePieces.Step] = []
    private var selection = 0
    /// Экран графа поверх выдачи; выдача под ним не трогается и после возврата та же.
    private var graph: GraphScreen?
    /// Страница графа — `WKWebView` с Cytoscape. Создаётся с окном, а не по ⇥: первая загрузка
    /// WebKit и страницы — запуска, а не первого графа.
    let graphPage = GraphPage()
    /// Карта знаний поверх выдачи. Её страница — второй веб-вид, и создаётся при первом ⌘M, а не
    /// с окном: карту открывают реже, чем граф строки, и запуск панели за неё не платит.
    private var knowledge: KnowledgeScreen?
    private(set) lazy var knowledgePage = GraphPage(canvas: .knowledge)

    /// Ответы приходят не по порядку; ответ старее применённого выбрасывается.
    private var order = AnswerOrder()
    /// Запрос, чей ответ ещё ждут. Новая буква его отменяет: ответ на старую букву уже
    /// никому не нужен, и источник бросает работу — `CoreFeed` завершает свой процесс.
    private var inFlight: Task<Void, Never>?

    private var awaitingAnswer: Bool { order.awaiting }
    private var pageSize: Int { feed.pageSize }

    init(feed: ResultFeed, options: LaunchOptions) {
        self.feed = feed
        self.openAlwaysFails = options.openAlwaysFails
        self.context = options.context
        self.scope = options.scope
        super.init(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: 200))

        // Материал вместо «backdrop-filter: blur(40px) saturate(180%)»: размытие даёт система.
        material = .hudWindow
        blendingMode = .behindWindow
        // Без `.active` материал гаснет, пока приложение неактивно, — а оно активным не бывает.
        state = .active
        // Материал «из-за окна» рисует размытие по прямоугольнику окна, и скругление слоя его
        // не режет: в углах оставалась квадратная полоска. Режет его только своя маска.
        maskImage = Self.roundedMask(radius: Tokens.windowRadius)
        wantsLayer = true
        layer?.cornerRadius = Tokens.windowRadius
        // Круговая, а не `.continuous`: обводка обязана лечь ровно по краю маски.
        layer?.cornerCurve = .circular
        layer?.masksToBounds = true
        layer?.borderWidth = 0.5

        buildQueryField(text: options.query)
        refreshBorder()
        rebuild()
        request(resetting: true)
    }

    required init?(coder: NSCoder) { fatalError("панель собирается кодом, не из nib") }

    override var isFlipped: Bool { true }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        refreshBorder()
        rebuild()
    }

    /// Обводка окна `0 0 0 .5px` из раздела «Тени и материал»: CGColor темы не знает.
    private func refreshBorder() {
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.borderColor = Tokens.windowHairline.cgColor
        }
    }

    // MARK: Шов

    /// Номер последнего отправленного запроса.
    var lastGeneration: Int { order.issued }

    private func request(resetting: Bool, retryingBusy: Bool = false) {
        if !retryingBusy {
            busyRound = 0
            busyRetry?.cancel()
        }
        if resetting {
            next = nil
            selection = 0
            openFailure = nil
            notice = nil
            inFlight?.cancel()
        }
        let generation = order.issue()
        let request = FeedRequest(context: context, query: query, scope: scope,
                                  after: resetting ? nil : next, limit: pageSize,
                                  includeMissing: false, generation: generation)

        // Запрос уходит раньше, чем экран перерисуется с пометкой «обновляется», и не на
        // главном потоке. Задача, унаследовавшая главный поток, ждала бы конца его прохода —
        // перестройки и кадра, — и процесс ядра запускался бы на 30 мс позже (замер UI-3).
        let feed = feed
        let screen = WeakScreen(self)
        inFlight = Task.detached(priority: .userInitiated) {
            do {
                let page = try await feed.page(for: request)
                await MainActor.run { screen.content?.apply(page, appending: !resetting, generation: generation) }
            } catch is CancellationError {
                // Отменён, потому что пришла новая буква; её ответ и будет на экране.
            } catch {
                await MainActor.run { screen.content?.apply(failure: error, generation: generation) }
            }
        }
        rebuild()
    }

    private func apply(_ page: FeedPage, appending: Bool, generation: Int) {
        // Медленный ответ на прежнюю букву не должен затирать быстрый ответ на следующую.
        guard order.accept(generation) else { return }
        failure = nil
        sections = appending ? merged(sections, page.sections) : page.sections
        counters = page.counters
        indexState = page.index
        corpus = page.corpus
        next = page.next
        steps = nothingFound ? nextSteps() : []
        selection = min(selection, max(rowCount - 1, 0))
        rebuild()
        onAnswerApplied?(generation)
    }

    private func apply(failure: Error, generation: Int) {
        guard order.accept(generation) else { return }
        let failure = failure as? FeedFailure ?? .core(.failed)
        if failure == .core(.busy) {
            retryWhileBusy(generation)
            // Проход пишет граф: прежняя выдача остаётся на экране, в углу — «обновляется».
            // Отказ вместо неё прочитался бы как поломка, а это ожидание (ADR 0049).
            if !sections.isEmpty {
                indexState = .behind(text: "индекс обновляется")
                rebuild()
                onAnswerApplied?(generation)
                return
            }
        }
        sections = []
        steps = []
        counters = nil
        next = nil
        self.failure = failure
        rebuild()
        onAnswerApplied?(generation)
    }

    /// Сколько раз панель сама спрашивает снова, пока ядро отвечает `busy`: раз в 3 с, около
    /// полутора минут — дольше идёт только первый проход нового контекста.
    private static let busyRounds = 30
    private var busyRetry: Task<Void, Never>?
    private var busyRound = 0

    /// Тот же запрос через 3 с, если за это время не пришёл новый.
    private func retryWhileBusy(_ generation: Int) {
        busyRetry?.cancel()
        guard busyRound < Self.busyRounds else { return }
        busyRound += 1
        busyRetry = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            guard let self, !Task.isCancelled, self.order.issued == generation else { return }
            self.request(resetting: true, retryingBusy: true)
        }
    }

    private func merged(_ existing: [FeedSection], _ arriving: [FeedSection]) -> [FeedSection] {
        var merged = existing
        for section in arriving {
            if let at = merged.firstIndex(where: { $0.kind == section.kind }) {
                merged[at] = FeedSection(kind: section.kind,
                                         rows: merged[at].rows + section.rows,
                                         total: section.total)
            } else {
                merged.append(section)
            }
        }
        return merged
    }

    // MARK: Состояние экрана

    private var query: String { queryField.stringValue }
    private var rows: [FeedRow] { sections.flatMap(\.rows) }
    private var rowCount: Int { nothingFound ? steps.count : rows.count }
    private var showingRecent: Bool { query.isEmpty }
    private var nothingFound: Bool { !query.isEmpty && sections.isEmpty && failure == nil }

    private var selectedRow: FeedRow? {
        guard !nothingFound, selection >= 0, selection < rows.count else { return nil }
        return rows[selection]
    }

    // MARK: Клавиатура

    func queryChanged() {
        // На карте строка запроса ищет по карте: ответ уже в панели, процесс ядра не нужен.
        if let knowledge {
            knowledge.find(query)
            return
        }
        // Новый запрос — вопрос к выдаче, а не к графу: граф прошлой строки уходит.
        closeGraph()
        request(resetting: true)
    }

    // MARK: Карта знаний

    var showingKnowledge: Bool { knowledge != nil }

    /// Холст на экране — граф окрестности или карта знаний; клавиши и жесты окна идут ему.
    var openCanvas: GraphPage? {
        if knowledge != nil { return knowledgePage }
        return graph != nil ? graphPage : nil
    }

    /// На холсте открыт список найденного — ↑ ↓ выбирают в нём, а не двигают холст. Он есть
    /// только у карты знаний.
    var canvasListsFound: Bool { knowledge?.listsFound ?? false }

    /// Только для пробы жестов: ⏎ на холсте печатает, что открыл бы, и ничего не открывает и
    /// не кладёт в буфер обмена — проба идёт на машине человека.
    var rehearsesOpening = false

    /// Карта знаний этого контекста. С текстом из строки запроса: набранное в выдаче имя
    /// подсвечивается на карте сразу.
    func showKnowledge(picking pick: String? = nil) {
        guard let source = feed as? KnowledgeSource else {
            notice = "Карту знаний строит только ядро: запустите панель с --feed=core"
            rebuild()
            return
        }
        closeGraph()
        let screen = KnowledgeScreen(source: source, page: knowledgePage, context: context, query: query,
                                     pick: pick)
        screen.onChange = { [weak self] in self?.rebuild() }
        screen.onApplied = { [weak self] generation in self?.onKnowledgeApplied?(generation) }
        knowledge = screen
        notice = nil
        screen.request()
        rebuild()
    }

    private func closeKnowledge() {
        guard let knowledge else { return }
        knowledge.cancel()
        self.knowledge = nil
        notice = nil
        rebuild()
    }

    private func handleInKnowledge(_ command: Command, _ knowledge: KnowledgeScreen) {
        switch command {
        case .moveUp: knowledge.move(by: -1)
        case .moveDown: knowledge.move(by: 1)
        case .knowledge, .back, .graph: closeKnowledge()
        case .zoom, .step: break
        case .primary:
            guard let row = knowledge.selectedRow else { return knowledge.centerOnSelected() }
            openFromCanvas(row)
        case .secondary:
            guard case .file(let file)? = knowledge.selectedRow else { return }
            settle(Opening.reveal(file))
        case .copyIdentifier:
            guard case .conversation(let conversation)? = knowledge.selectedRow,
                  let session = conversation.sessionID else { return }
            report("скопировал бы идентификатор сессии", what: session)
        case .filter, .chooseContext:
            // Фильтр и контекст — свойства выдачи: сначала к ней, потом действие.
            closeKnowledge()
            handle(command)
        }
    }

    // MARK: Граф

    var showingGraph: Bool { graph != nil }

    /// Номер последнего запроса графа и сколько узлов в виде на экране. Нужно замеру.
    var lastGraphGeneration: Int { graph?.lastGeneration ?? 0 }
    var graphNodeCount: Int { graph?.view?.nodes.count ?? 0 }

    /// ⇥ на строке: граф с фокусом на ней.
    private func openGraph() {
        guard let row = selectedRow, let focus = GraphAnswer.focus(of: row) else {
            notice = "Граф строится от строки выдачи — выберите беседу или файл"
            rebuild()
            return
        }
        showGraph(focus: focus, depth: GraphScreen.defaultDepth)
    }

    /// Граф с этим фокусом. Им же пользуется замер, чтобы спросить и глубину 1.
    func showGraph(focus: String, depth: Int) {
        guard let source = feed as? GraphSource else {
            // Выдуманный источник касаний не знает, и граф из него был бы выдумкой.
            notice = "Граф строит только ядро: запустите панель с --feed=core"
            rebuild()
            return
        }
        graph?.cancel()
        let screen = GraphScreen(source: source, page: graphPage, context: context, focus: focus, depth: depth)
        screen.onChange = { [weak self] in self?.rebuild() }
        screen.onApplied = { [weak self] generation in self?.onGraphApplied?(generation) }
        graph = screen
        notice = nil
        screen.request()
        rebuild()
    }

    private func closeGraph() {
        guard let graph else { return }
        graph.cancel()
        self.graph = nil
        notice = nil
        rebuild()
    }

    private func handleInGraph(_ command: Command, _ graph: GraphScreen) {
        switch command {
        case .moveUp: graph.move(by: -1)
        case .moveDown: graph.move(by: 1)
        case .graph, .back: closeGraph()
        case .knowledge: showKnowledge()
        case .zoom, .step: break
        case .primary:
            guard let row = graph.selectedRow else { return }
            openFromCanvas(row)
        case .secondary:
            guard case .file(let file)? = graph.selectedRow else { return }
            settle(Opening.reveal(file))
        case .copyIdentifier:
            guard let session = graph.selectedNode?.conversation?.sessionId else { return }
            report("скопировал бы идентификатор сессии", what: session)
        case .filter, .chooseContext:
            // Фильтр и контекст — свойства выдачи: сначала к ней, потом действие.
            closeGraph()
            handle(command)
        }
    }

    /// Запрос целиком, как будто его вставили. Нужно замеру: печатать за человека панель
    /// без «Универсального доступа» не может, а строку запроса — может.
    func replaceQuery(_ text: String) {
        queryField.stringValue = text
        queryChanged()
    }

    func handle(_ command: Command) {
        // Камера одна на оба холста: экрану сдвиг и масштаб знать не нужно.
        switch command {
        case .zoom(let step): return openCanvas?.zoom(step) ?? ()
        case .step(let dx, let dy): return openCanvas?.step(dx: dx, dy: dy) ?? ()
        default: break
        }
        if let knowledge {
            handleInKnowledge(command, knowledge)
            return
        }
        if let graph {
            handleInGraph(command, graph)
            return
        }
        switch command {
        case .moveUp:
            selection = max(selection - 1, 0)
            rebuild()
        case .moveDown:
            // ↓ на последней строке догружает следующую страницу: отдельной клавиши под
            // «Показать ещё» макет не называет, а мёртвая кнопка хуже, чем её отсутствие.
            if selection >= rowCount - 1, next != nil, !awaitingAnswer {
                request(resetting: false)
            } else {
                selection = min(selection + 1, max(rowCount - 1, 0))
                rebuild()
            }
        case .primary:
            performPrimary()
        case .secondary:
            performSecondary()
        case .graph:
            openGraph()
        case .knowledge:
            showKnowledge()
        case .back, .zoom, .step:
            break
        case .filter(let wanted):
            scope = wanted
            print("фильтр: \(wanted.label)")
            fflush(stdout)
            request(resetting: true)
        case .chooseContext:
            switchContext()
        case .copyIdentifier:
            guard case .conversation(let conversation)? = selectedRow, let session = conversation.sessionID else { return }
            report("скопировал бы идентификатор сессии", what: session)
        }
    }

    /// ⏎ на холсте: источник связи карты или узел графа.
    private func openFromCanvas(_ row: FeedRow) {
        guard rehearsesOpening else { return settle(Opening.primary(row)) }
        switch row {
        case .file(let file): report("⏎ открыл бы файл", what: file.path)
        case .conversation(let conversation): report("⏎ скопировал бы", what: "claude --resume \(conversation.sessionID ?? "—")")
        }
    }

    private func performSecondary() {
        guard case .file(let file)? = selectedRow else {
            // У беседы вторичного действия этот этап не решает.
            report("показал бы здесь", what: describeSelection())
            return
        }
        settle(Opening.reveal(file))
    }

    /// Что стало с попыткой открыть, на экране.
    private func settle(_ outcome: Opening.Outcome) {
        switch outcome {
        case .left:
            openFailure = nil
            notice = nil
            rebuild()
            onLeft?()
        case .notice(let text):
            openFailure = nil
            notice = text
            rebuild()
        case .failed(let failure):
            openFailure = failure
            notice = nil
            rebuild()
        }
    }

    private func performPrimary() {
        if nothingFound {
            guard selection < steps.count else { return }
            performStep(steps[selection])
            return
        }
        guard let row = selectedRow else { return }
        if openAlwaysFails {
            openFailure = Self.failure(for: row)
            rebuild()
            return
        }
        settle(Opening.primary(row))
    }

    /// ⌘K переключает контекст на следующий и повторяет запрос в нём.
    ///
    /// В макете за ⌘K стоит меню выбора; меню этот этап не рисует. Переключение по кругу —
    /// то же действие без меню: контекст виден в чипе, а выдача меняется, потому что выдачи
    /// контекстов не смешиваются (ADR 0008).
    private func switchContext() {
        guard let next = nextContext else { return }
        context = next
        print("контекст: \(context)")
        fflush(stdout)
        request(resetting: true)
    }

    /// Куда уйдёт ⌘K. Шаг «повторить в контексте …» обязан называть тот же контекст,
    /// иначе он обещает одно, а клавиша делает другое.
    private var nextContext: String? {
        let contexts = feed.contexts
        guard contexts.count > 1, let at = contexts.firstIndex(of: context) else { return nil }
        return contexts[(at + 1) % contexts.count]
    }

    private func performStep(_ step: GuidePieces.Step) {
        switch step.key {
        case "⏎":
            scope = .everything
            request(resetting: true)
        case "⌥⏎":
            report("повторил бы запрос по любому из слов", what: query)
        default:
            switchContext()
        }
    }

    /// Панель открывает сама, поэтому и отказ знает она: источник выдачи об инструментах
    /// не знает ничего.
    private static func failure(for row: FeedRow) -> OpenFailure {
        switch row {
        case .conversation(let conversation):
            let session = conversation.sessionID ?? "идентификатор неизвестен"
            return OpenFailure(
                rowIdentity: conversation.id,
                title: "Не удалось открыть в \(conversation.tool.label): сессия не найдена",
                explanation: "\(conversation.tool.label) не знает сессию \(session). В индексе Crosskit "
                    + "беседа есть, последнее сообщение — \(conversation.when ?? "без даты"). Возможно, её удалили "
                    + "или перенесли в другое рабочее пространство.",
                actions: [("⌘⏎", "Показать беседу здесь"), ("⌘C", "Скопировать ID сессии"), ("⏎", "Повторить")])
        case .file(let file):
            return Opening.failure(opening: file)
        }
    }

    private func describeSelection() -> String {
        switch selectedRow {
        case .conversation(let conversation)?:
            return "беседу «\(conversation.title)» в \(conversation.tool.label)"
        case .file(let file)?:
            return "файл \(file.directory)\(file.name)"
        case nil:
            return "ничего: строка не выбрана"
        }
    }

    /// Действия, которые этот этап не решает — показать беседу здесь, развернуть граф, — только
    /// печатаются.
    private func report(_ action: String, what: String) {
        print("\(action): \(what)")
        fflush(stdout)
    }

    /// Три шага, которые предлагает пустая выдача. Считает их панель: каждый — свойство
    /// запроса, который она же и составила.
    private func nextSteps() -> [GuidePieces.Step] {
        var steps: [GuidePieces.Step] = []
        if scope != .everything {
            steps.append(GuidePieces.Step(glyph: "∗", title: "Искать во всём: чаты, файлы и темы",
                                          subtitle: "Снять фильтр «\(scope.label)»", key: "⏎"))
        }
        let words = query.split(separator: " ").map(String.init)
        if words.count > 1 {
            steps.append(GuidePieces.Step(glyph: "∨", title: "Любое из слов: " + words.joined(separator: " · "),
                                          subtitle: "Сейчас нужны все \(words.count) слова сразу", key: "⌥⏎"))
        }
        if let other = nextContext {
            steps.append(GuidePieces.Step(glyph: "⇄", title: "Повторить в контексте \(other)",
                                          subtitle: "Это отдельный поиск — выдачи контекстов не смешиваются",
                                          key: "⌘K"))
        }
        return steps
    }

    // MARK: Сборка экрана

    private func buildQueryField(text: String) {
        queryField.stringValue = text
        queryField.font = Tokens.text(22)
        queryField.textColor = Tokens.fg
        queryField.isBordered = false
        queryField.drawsBackground = false
        queryField.focusRingType = .none
        queryField.cell?.usesSingleLineMode = true
        // Без этого поле переносит слова: длинный запрос уходил на вторую строку, а первая
        // уезжала за верх поля высотой в одну строку. Теперь он прокручивается вбок.
        queryField.cell?.wraps = false
        queryField.cell?.isScrollable = true
        queryField.placeholderAttributedString = NSAttributedString(
            string: "Искать в чатах и файлах",
            attributes: [.font: Tokens.text(22), .foregroundColor: Tokens.fg3])
        addSubview(queryField)
    }

    /// Скруглённый прямоугольник, растягиваемый за середину: маска материала любой высоты.
    private static func roundedMask(radius: CGFloat) -> NSImage {
        let side = radius * 2 + 1
        let image = NSImage(size: NSSize(width: side, height: side), flipped: false) { rect in
            NSColor.black.setFill()
            NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius).fill()
            return true
        }
        image.capInsets = NSEdgeInsets(top: radius, left: radius, bottom: radius, right: radius)
        image.resizingMode = .stretch
        return image
    }

    private func rebuild() {
        for view in subviews where view !== queryField { view.removeFromSuperview() }

        var y = buildHeader()
        if let knowledge {
            addSubview(hairline(atTop: y))
            y = stack(knowledge.body(appearance: effectiveAppearance), at: y + Layout.hairline)
            if let notice {
                y = stack(GuidePieces.notice(notice), at: y, hairlineAbove: true)
            }
            y = stack(FooterPieces.keyBar(knowledge.keys(), escape: "К выдаче"), at: y, hairlineAbove: true)
            setFrameSize(NSSize(width: Tokens.windowWidth, height: y))
            onHeightChange?(y)
            return
        }
        if let graph {
            addSubview(hairline(atTop: y))
            y = stack(graph.body(appearance: effectiveAppearance), at: y + Layout.hairline)
            if let counts = graph.countsLine() {
                y = stack(counts, at: y, hairlineAbove: true)
            }
            if let notice {
                y = stack(GuidePieces.notice(notice), at: y, hairlineAbove: true)
            }
            y = stack(FooterPieces.keyBar(graph.keys(), escape: "К выдаче"), at: y, hairlineAbove: true)
            setFrameSize(NSSize(width: Tokens.windowWidth, height: y))
            onHeightChange?(y)
            return
        }
        y = buildBody(from: y)
        if let counters {
            y = stack(FooterPieces.counters(counters, hasMore: next != nil, pageSize: pageSize),
                      at: y, hairlineAbove: true)
        }
        if let notice {
            y = stack(GuidePieces.notice(notice), at: y, hairlineAbove: true)
        }
        y = stack(FooterPieces.keyBar(keyBar()), at: y, hairlineAbove: true)

        setFrameSize(NSSize(width: Tokens.windowWidth, height: y))
        onHeightChange?(y)
    }

    private func buildHeader() -> CGFloat {
        let top = (Layout.headerHeight - HeaderPieces.rowHeight) / 2 + 1

        let chip = HeaderPieces.contextChip(context)
        chip.setFrameOrigin(NSPoint(x: Layout.sidePadding, y: top))
        addSubview(chip)

        let badge = HeaderPieces.indexBadge(indexState)
        badge.setFrameOrigin(NSPoint(x: Tokens.windowWidth - Layout.sidePadding - badge.frame.width,
                                     y: top + (HeaderPieces.rowHeight - badge.frame.height) / 2))
        addSubview(badge)

        let left = chip.frame.maxX + 12
        queryField.frame = NSRect(x: left, y: top, width: badge.frame.minX - 12 - left,
                                  height: HeaderPieces.rowHeight)

        var y = Layout.headerHeight
        let filters = HeaderPieces.filterRow(scope: scope)
        filters.setFrameOrigin(NSPoint(x: 0, y: y))
        addSubview(filters)
        y += filters.frame.height
        return y
    }

    private func buildBody(from top: CGFloat) -> CGFloat {
        addSubview(hairline(atTop: top))

        let document = Widgets.Flipped(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: 0))
        var y = Layout.listTopPadding
        var selectedView: NSView?

        if let failure {
            let wording = failure.wording(context: context)
            y = place(GuidePieces.refusalHeader(title: wording.title, explanation: wording.explanation),
                      in: document, at: y, inset: 0)
        } else if nothingFound {
            y = place(GuidePieces.noResultsHeader(context: context, query: query, explanation: explanation()),
                      in: document, at: y, inset: 0)
            for (index, step) in steps.enumerated() {
                let view = GuidePieces.step(step, selected: index == selection)
                if index == selection { selectedView = view }
                y = place(view, in: document, at: y)
            }
        } else {
            var flat = 0
            for section in sections {
                y = place(RowPieces.sectionHeader(title: showingRecent ? "Недавние чаты" : section.title,
                                                  shown: section.rows.count,
                                                  total: showingRecent ? nil : section.total,
                                                  updating: awaitingAnswer),
                          in: document, at: y)
                for row in section.rows {
                    let selected = flat == selection
                    let view = RowPieces.row(row, selected: selected)
                    if selected { selectedView = view }
                    y = place(view, in: document, at: y)
                    if selected, let failure = openFailure, failure.rowIdentity == row.identity {
                        y = place(GuidePieces.openFailure(failure), in: document, at: y, inset: RowPieces.horizontalInset + 50)
                    }
                    flat += 1
                }
            }
            if showingRecent {
                y = place(GuidePieces.hints(feed.hints), in: document, at: y)
                y = place(GuidePieces.corpusLine(context: context, corpus: corpus), in: document, at: y)
            }
        }

        y += Layout.listBottomPadding
        document.setFrameSize(NSSize(width: Tokens.windowWidth, height: y))

        let visible = capsList ? min(y, Layout.listMaxHeight) : y
        let scroll = NSScrollView(frame: NSRect(x: 0, y: top + Layout.hairline,
                                                width: Tokens.windowWidth, height: visible))
        scroll.drawsBackground = false
        scroll.contentView.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        scroll.documentView = document
        addSubview(scroll)
        selectedView?.scrollToVisible(selectedView?.bounds ?? .zero)
        return top + Layout.hairline + visible
    }

    /// Выдача прокручивается; объясняющие экраны — нет.
    private var capsList: Bool { failure == nil && !nothingFound && !showingRecent }

    private func place(_ view: NSView, in document: NSView, at y: CGFloat,
                       inset: CGFloat = RowPieces.horizontalInset) -> CGFloat {
        view.setFrameOrigin(NSPoint(x: inset, y: y))
        document.addSubview(view)
        return y + view.frame.height
    }

    private func explanation() -> String {
        let where_ = scope == .everything ? "Искали во всём" : "Искали только в разделе «\(scope.label)»"
        let freshness: String
        switch indexState {
        case .behind(let text): freshness = " Индекс \(text) — самое свежее могло ещё не попасть."
        case .fresh: freshness = ""
        }
        let hidden = counters.map { ". Скрыто политикой приватности: \($0.hiddenByPolicy)." } ?? "."
        return where_ + hidden + freshness
    }

    private func keyBar() -> [(String, String)] {
        if case .scopeNotAvailable? = failure {
            return [("⌘1", "Всё"), ("⌘K", "Другой контекст")]
        }
        if failure != nil {
            return [("⌘K", "Другой контекст")]
        }
        if nothingFound {
            return [("↑↓", "Выбрать шаг"), ("⏎", "Выполнить"), ("⌘1", "Всё"), ("⌘K", "Контекст")]
        }
        if openFailure != nil {
            return [("⏎", "Повторить"), ("⌘⏎", "Показать здесь"), ("⌘C", "Скопировать ID"),
                    ("⇥", "Граф"), ("⌘K", "Контекст")]
        }
        if showingRecent, rows.isEmpty {
            return [("⌘1–4", "Фильтры"), ("⌘M", "Карта знаний"), ("⌘K", "Контекст")]
        }
        if showingRecent {
            return [("↓", "К недавним"), ("⏎", "Открыть выбранный"), ("⌘1–4", "Фильтры"),
                    ("⌘M", "Карта знаний"), ("⌘K", "Контекст")]
        }
        let open: String
        switch selectedRow {
        case .conversation?: open = "Скопировать claude --resume"
        case .file?: open = "Открыть"
        case nil: open = "Открыть"
        }
        if awaitingAnswer {
            return [("↑↓", "Выбрать"), ("⏎", open), ("⌘⏎", "Показать здесь"), ("⌘K", "Контекст")]
        }
        return [("↑↓", "Выбрать"), ("⏎", open), ("⌘⏎", "Показать здесь"), ("⇥", "Граф"), ("⌘K", "Контекст")]
    }

    // MARK: Укладка

    private func stack(_ view: NSView, at y: CGFloat, hairlineAbove: Bool = false) -> CGFloat {
        var y = y
        if hairlineAbove {
            addSubview(hairline(atTop: y))
            y += Layout.hairline
        }
        view.setFrameOrigin(NSPoint(x: 0, y: y))
        addSubview(view)
        return y + view.frame.height
    }

    private func hairline(atTop y: CGFloat) -> NSView {
        let line = Widgets.Fill(Tokens.separator)
        line.frame = NSRect(x: 0, y: y, width: Tokens.windowWidth, height: Layout.hairline)
        return line
    }
}

/// Слабая ссылка на экран для задачи, которая идёт не на главном потоке: ответ, пришедший
/// после закрытия окна, не держит его в памяти. Читается только на главном потоке.
private final class WeakScreen: @unchecked Sendable {
    weak var content: PanelContent?

    init(_ content: PanelContent) {
        self.content = content
    }
}
