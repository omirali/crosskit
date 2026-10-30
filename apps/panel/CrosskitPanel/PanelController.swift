import AppKit

/// Показ и скрытие панели, и разбор клавиш до того, как они дойдут до экрана.
///
/// Поведение окна доказано этапом UI-1 и здесь не меняется: `.nonactivatingPanel`,
/// `canBecomeKey`, `collectionBehavior`, материал. Новое здесь одно — окно меняет высоту
/// вместе с содержимым, и делает это, не двигая верхний край: панель, прыгающая по экрану
/// при наборе, читается как сбой.
final class PanelController: NSObject, NSTextFieldDelegate {

    private let window: PanelWindow
    let content: PanelContent
    private let probe: FirstFrameProbe

    init(probe: FirstFrameProbe, options: LaunchOptions) {
        self.probe = probe
        content = PanelContent(feed: options.feed, options: options)
        window = PanelWindow(contentRect: NSRect(x: 0, y: 0, width: Tokens.windowWidth,
                                                 height: content.frame.height))
        super.init()

        window.contentView = content
        window.onCancel = { [weak self] in self?.cancel() }
        window.onCommandKey = { [weak self] event in self?.commandKey(event) ?? false }
        window.onEarlyEvent = { [weak self] event in self?.canvasEvent(event) ?? false }
        content.queryField.delegate = self
        content.onHeightChange = { [weak self] height in self?.resize(to: height) }
        content.onLeft = { [weak self] in self?.hide() }
    }

    var isVisible: Bool { window.isVisible }

    /// Прогрев: заставить окно один раз отрисоваться, пока его никто не ждёт.
    /// Первый показ дороже остальных не постройкой окна, а первой отрисовкой — слоями,
    /// растеризацией шрифта, первым обращением к материалу. Прозрачное окно система
    /// композитит так же, как видимое, поэтому эта цена платится здесь, а не при вызове.
    func prewarm() {
        window.alphaValue = 0
        window.orderFrontRegardless()
        content.displayIfNeeded()
        window.orderOut(nil)
        window.alphaValue = 1
    }

    /// `takingKeyboard: false` — только для пробы жестов: окно на экране, но клавиатура
    /// остаётся у того, кто печатает, — иначе его буквы попадут в строку запроса и в замер.
    func show(takingKeyboard: Bool = true) {
        let before = NSWorkspace.shared.frontmostApplication?.localizedName ?? "—"
        moveToActiveScreen()

        probe.trackingShow(in: content) {
            // `orderFrontRegardless` показывает окно, не активируя приложение;
            // `makeKey` отдельно делает панель ключевой — это и есть право на клавиатуру.
            window.orderFrontRegardless()
            if takingKeyboard { window.makeKey() }
            window.makeFirstResponder(content.queryField)
            if let editor = window.fieldEditor(false, for: content.queryField) as? NSTextView {
                editor.insertionPointColor = Tokens.accent
                // Курсор в конец, а не выделение всего: панель открывается на продолжение
                // запроса, а не на его замену.
                editor.selectedRange = NSRange(location: editor.string.count, length: 0)
            }
        }

        let after = NSWorkspace.shared.frontmostApplication?.localizedName ?? "—"
        report("показана", frontBefore: before, frontAfter: after)
    }

    func hide() {
        let before = NSWorkspace.shared.frontmostApplication?.localizedName ?? "—"
        window.orderOut(nil)
        let after = NSWorkspace.shared.frontmostApplication?.localizedName ?? "—"
        report("скрыта", frontBefore: before, frontAfter: after)
    }

    /// esc: с графа или карты — назад к выдаче, с выдачи — спрятать панель.
    private func cancel() {
        if content.showingGraph || content.showingKnowledge {
            content.handle(.back)
        } else {
            hide()
        }
    }

    /// Высоту меняет содержимое; верхний край окна при этом стоит на месте.
    private func resize(to height: CGFloat) {
        let frame = window.frame
        guard abs(frame.height - height) > 0.5 else { return }
        window.setFrame(NSRect(x: frame.minX, y: frame.maxY - height,
                               width: Tokens.windowWidth, height: height),
                        display: true)
        // Тень безрамочного окна снята с прежней формы; после смены высоты её снимают заново.
        window.invalidateShadow()
    }

    /// Панель встаёт на экране, где сейчас курсор, — как Spotlight. Вертикаль: верхняя треть.
    private func moveToActiveScreen() {
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main
        guard let visible = screen?.visibleFrame else { return }
        let size = window.frame.size
        let origin = NSPoint(x: visible.midX - size.width / 2,
                             y: visible.maxY - visible.height * 0.22 - size.height)
        window.setFrameOrigin(origin)
    }

    /// Строки, по которым видно поведение окна. Признаков активации три, и они не синонимы:
    ///
    /// * `NSWorkspace.frontmostApplication` — кого система считает фронтом;
    /// * `menuBarOwningApplication` — чьё меню в строке меню. Это и есть «приложение активно»
    ///   в том смысле, в каком активность видит человек: меню и заголовок окна;
    /// * `NSApp.isActive` — что о себе думает само приложение панели.
    ///
    /// Первые два отвечают на вопрос (б) этапа UI-1. Третий печатается рядом именно потому,
    /// что может с ними разойтись, и тогда полагаться надо не на него.
    private func report(_ action: String, frontBefore: String, frontAfter: String) {
        let screen = window.screen.map { "\(Int($0.frame.width))×\(Int($0.frame.height)) @\(Int($0.frame.minX))" } ?? "—"
        let menuOwner = NSWorkspace.shared.menuBarOwningApplication?.localizedName ?? "—"
        let editing = window.fieldEditor(false, for: content.queryField) === window.firstResponder
        print("панель \(action): фронт до «\(frontBefore)» → после «\(frontAfter)» · "
              + "меню у «\(menuOwner)» · NSApp.isActive=\(NSApp.isActive) · "
              + "key=\(window.isKeyWindow) · строка запроса готова: \(editing) · "
              + "onActiveSpace=\(window.isOnActiveSpace) · экран \(screen) · "
              + "окно \(Int(window.frame.width))×\(Int(window.frame.height)) pt")
        fflush(stdout)
    }

    // MARK: Клавиатура

    /// ⌘-сочетания. Возвращает `true`, если событие разобрано.
    private func commandKey(_ event: NSEvent) -> Bool {
        if event.isCommand("\r") {
            content.handle(.secondary)
        } else if let digit = event.commandDigit(), digit <= FeedScope.allCases.count {
            content.handle(.filter(FeedScope.allCases[digit - 1]))
        } else if event.isCommand("k") {
            content.handle(.chooseContext)
        } else if event.isCommand("c") {
            content.handle(.copyIdentifier)
        } else if event.isCommand("m") {
            content.handle(.knowledge)
        } else if content.openCanvas != nil, event.isCommand("=") || event.isCommand("+") {
            content.handle(.zoom(.zoomIn))
        } else if content.openCanvas != nil, event.isCommand("-") {
            content.handle(.zoom(.zoomOut))
        } else if content.openCanvas != nil, event.isCommand("0") {
            content.handle(.zoom(.fit))
        } else {
            return false
        }
        return true
    }

    /// Клавиши и жесты холста — графа окрестности или карты знаний — раньше строки запроса и
    /// вида под курсором (`CanvasKey`, `GraphPage.gesture`). Без холста — ничего: выдача как была.
    private func canvasEvent(_ event: NSEvent) -> Bool {
        guard let canvas = content.openCanvas else { return false }
        guard event.type == .keyDown else { return canvas.gesture(event) }
        guard let key = CanvasKey.of(keyCode: event.keyCode, characters: event.characters,
                                     modifiers: event.modifierFlags, foundListed: content.canvasListsFound) else {
            return false
        }
        switch key {
        case .pan(let dx, let dy): content.handle(.step(dx: dx, dy: dy))
        case .zoom(let step): content.handle(.zoom(step))
        case .choose(let delta): content.handle(delta < 0 ? .moveUp : .moveDown)
        }
        return true
    }

    // MARK: NSTextFieldDelegate

    func controlTextDidChange(_ notification: Notification) {
        content.queryChanged()
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        switch commandSelector {
        case #selector(NSResponder.cancelOperation(_:)):
            cancel()
        case #selector(NSResponder.moveUp(_:)):
            content.handle(.moveUp)
        case #selector(NSResponder.moveDown(_:)):
            content.handle(.moveDown)
        case #selector(NSResponder.insertNewline(_:)):
            content.handle(.primary)
        case #selector(NSResponder.insertTab(_:)):
            content.handle(.graph)
        default:
            return false
        }
        return true
    }
}
