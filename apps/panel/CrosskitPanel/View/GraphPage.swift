import AppKit
import WebKit

/// Страница графа: Cytoscape.js в `WKWebView`, загруженная из бандла приложения (ADR 0046).
///
/// Страниц две, и обе отсюда: граф окрестности (`graph.html`) создаётся при запуске панели,
/// как само окно, — первая загрузка WebKit и страницы стоит сотен миллисекунд, и платить их
/// должен запуск, а не первый ⇥; карта знаний (`knowledge.html`, ADR 0048) — при первом ⌘M.
///
/// Граница веб-вида — четыре правила, и все они здесь:
///
/// * страница и её скрипты — только из каталога `GraphPage` в бандле; любая навигация за его
///   пределы отменяется делегатом, а CSP страницы запрещает внешние источники;
/// * Swift → страница — постоянное тело вызова и данные значением аргумента (`GraphPageCall`);
/// * страница → Swift — сообщения `tap`, `tapEdge` и `rendered` с идентификатором узла, связи
///   или номером вида; ничего другого не принимается;
/// * клавиатуру веб-вид не берёт никогда: строка запроса остаётся первым ответчиком, ⇥ ↑↓ ⏎ esc
///   разбирает панель и передаёт графу вызовом.
final class GraphPage: NSObject, WKNavigationDelegate, WKScriptMessageHandler {

    let webView: GraphWebView
    /// Какой это холст: от него — файл страницы и объект, которому идут вызовы.
    let canvas: GraphPageCall.Page
    private let directory: URL?
    private(set) var ready = false
    private var pending: GraphPageCall?

    /// Клик по узлу — идентификатор узла; по пустому месту — пустая строка.
    var onTap: ((String) -> Void)?
    /// Клик по связи — её идентификатор. Его шлёт только карта знаний.
    var onTapEdge: ((String) -> Void)?
    /// Сколько миллисекунд страница раскладывала последний вид, если она это сказала.
    var onLaidOut: ((Double) -> Void)?
    /// Вид с этим номером нарисован и стоит на экране.
    var onRendered: ((Int) -> Void)?
    /// Страница загрузилась и готова принимать виды. Нужно замеру памяти.
    var onReady: (() -> Void)?

    init(canvas: GraphPageCall.Page = .graph) {
        self.canvas = canvas
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let controller = WKUserContentController()
        configuration.userContentController = controller
        webView = GraphWebView(frame: NSRect(x: 0, y: 0, width: Tokens.windowWidth, height: 240),
                               configuration: configuration)
        directory = Bundle.main.url(forResource: "GraphPage", withExtension: nil)
        super.init()
        controller.add(WeakMessages(self), name: "graph")
        // Холст прозрачный: под ним материал окна, как у всего остального на панели. Фон «под
        // страницей» — тоже: WebKit показывает его, когда страница отскакивает при прокрутке, и
        // по умолчанию он светлый — полоса поперёк прозрачного холста.
        webView.setValue(false, forKey: "drawsBackground")
        webView.underPageBackgroundColor = .clear
        webView.navigationDelegate = self
        if let directory {
            let page = canvas == .graph ? "graph.html" : "knowledge.html"
            webView.loadFileURL(directory.appendingPathComponent(page), allowingReadAccessTo: directory)
        }
    }

    /// Выполнить выражение страницы и вернуть его значение — только для замера жестов
    /// (`--probe-gestures`). Тело — постоянная строка, данных в нём нет.
    func evaluate(_ body: String, done: @escaping (Any?) -> Void) {
        webView.callAsyncJavaScript(body, arguments: [:], in: nil, in: .page) { result in
            done(try? result.get())
        }
    }

    // MARK: Камера — одна на оба холста (`camera.js`)

    /// Кнопки − + ⤢, клавиши + − 0 и ⌘= ⌘- ⌘0.
    func zoom(_ step: CanvasZoom) {
        send(.zoom(step, on: canvas))
    }

    /// Стрелки: сдвиг видимой зоны долями её размера.
    func step(dx: Double, dy: Double) {
        send(.step(dx: dx, dy: dy, on: canvas))
    }

    /// Элемент — в середину видимой зоны.
    func center(on id: String) {
        send(.center(on: id, on: canvas))
    }

    /// Жест, пойманный окном: разобран, если он над холстом.
    func gesture(_ event: NSEvent) -> Bool {
        switch webView.gesture(of: event) {
        case .pan(let dx, let dy)?: send(.pan(dx: Double(dx), dy: Double(dy), on: canvas))
        case .zoom(let factor, let point)?: send(.zoomBy(Double(factor), at: point, on: canvas))
        case nil: return false
        }
        return true
    }

    /// Нарисовать вид. До загрузки страницы держится последний — более ранние не нужны.
    func send(_ call: GraphPageCall) {
        guard ready else {
            if call.function == .render || pending?.function != .render { pending = call }
            return
        }
        webView.callAsyncJavaScript(call.body, arguments: call.arguments, in: nil, in: .page) { _ in }
    }

    // MARK: WKNavigationDelegate

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        decisionHandler(Self.allows(navigationAction.request.url, inside: directory) ? .allow : .cancel)
    }

    /// Только файлы самой страницы. Ссылка, переход, форма — всё остальное отменяется.
    static func allows(_ url: URL?, inside directory: URL?) -> Bool {
        guard let url, let directory, url.isFileURL else { return false }
        return url.standardizedFileURL.path.hasPrefix(directory.standardizedFileURL.path + "/")
    }

    // MARK: WKScriptMessageHandler

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any], let type = body["type"] as? String else { return }
        switch type {
        case "ready":
            ready = true
            onReady?()
            if let pending {
                self.pending = nil
                send(pending)
            }
        case "tap":
            if let id = body["id"] as? String { onTap?(id) }
        case "tapEdge":
            if let id = body["id"] as? String { onTapEdge?(id) }
        case "rendered":
            if let layout = body["layoutMs"] as? Double { onLaidOut?(layout) }
            if let generation = body["generation"] as? Int { onRendered?(generation) }
        default:
            return
        }
    }
}

/// Веб-вид, который не берёт клавиатуру: клик рисуется страницей, а первым ответчиком остаётся
/// строка запроса панели — иначе после первого клика по узлу ⇥ и esc ушли бы в WebKit.
///
/// Жесты холста разбираются в AppKit, а не страницей: до страницы колесо не доходит, а щипок
/// Cytoscape не ловит. Ловит их окно (`PanelWindow.sendEvent`) и через `GraphPage.gesture`
/// спрашивает здесь, что это за жест, — до того, как macOS выберет вид под курсором.
/// Переопределить `magnify` и `scrollWheel` в этом подклассе было мало: настоящий щипок у
/// владельца до них не дошёл, а проба звала их напрямую и этого не видела. Распознаватель жеста
/// на контейнере не выбран: прокрутки он не ловит, и у колеса с щипком были бы два разных пути.
final class GraphWebView: WKWebView {
    override var acceptsFirstResponder: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    enum Gesture {
        /// Сдвиг холста в точках.
        case pan(CGFloat, CGFloat)
        /// Приближение в точке холста (сверху слева), множителем.
        case zoom(CGFloat, at: NSPoint)
    }

    /// Что это за жест, если он над этим видом, а вид на экране. Трекпад — сдвиг, двумя
    /// пальцами, как в картах; колесо мыши, а с ⌘ или ctrl — любая прокрутка, — приближение под
    /// курсором; щипок — приближение к точке под пальцами.
    func gesture(of event: NSEvent) -> Gesture? {
        guard let window, !isHiddenOrHasHiddenAncestor, bounds.contains(local(event, in: window)) else { return nil }
        switch event.type {
        case .magnify:
            return .zoom(1 + event.magnification, at: point(of: event))
        case .scrollWheel:
            let zooms = !event.hasPreciseScrollingDeltas
                || event.modifierFlags.contains(.command) || event.modifierFlags.contains(.control)
            guard zooms else { return .pan(event.scrollingDeltaX, event.scrollingDeltaY) }
            let step = event.hasPreciseScrollingDeltas ? event.scrollingDeltaY * 0.01 : event.scrollingDeltaY * 0.1
            return .zoom(exp(step), at: point(of: event))
        default:
            return nil
        }
    }

    private func point(of event: NSEvent) -> NSPoint {
        guard let window else { return .zero }
        let local = local(event, in: window)
        return NSPoint(x: local.x, y: bounds.height - local.y)
    }

    /// Точка события в координатах вида. У события без окна — построенного пробой — AppKit
    /// кладёт в `locationInWindow` экранные координаты.
    private func local(_ event: NSEvent, in window: NSWindow) -> NSPoint {
        let inWindow = event.window == nil ? window.convertPoint(fromScreen: event.locationInWindow) : event.locationInWindow
        return convert(inWindow, from: nil)
    }
}

/// Контроллер содержимого держит обработчик сильно; слабая ссылка не даёт им держать друг друга.
private final class WeakMessages: NSObject, WKScriptMessageHandler {
    weak var page: GraphPage?

    init(_ page: GraphPage) {
        self.page = page
    }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        page?.userContentController(controller, didReceive: message)
    }
}
