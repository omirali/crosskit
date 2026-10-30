import AppKit
import Foundation
import Observation
import WebKit

/// Встроенный браузер (макет `design/UXMain.dc.html`): страница, которую делает агент, рядом с
/// его беседой. Ошибки консоли, снимок и указанный элемент уходят в строку ввода чата —
/// отправляет их человек.
@Observable
final class BrowserModel: NSObject {

    enum Device: String, CaseIterable, Identifiable {
        case desktop = "Компьютер", tablet = "Планшет", phone = "Телефон"
        var id: Self { self }
        var width: CGFloat? {
            switch self {
            case .desktop: nil
            case .tablet: 768
            case .phone: 390
            }
        }
    }

    struct ConsoleLine: Identifiable {
        enum Level: String { case log, info, warn, error }
        let id = UUID()
        let level: Level
        let text: String
    }

    var address = "http://localhost:3000"
    var device: Device = .desktop
    var consoleOpen = false
    private(set) var console: [ConsoleLine] = []
    private(set) var title = ""
    private(set) var status = "Страница не открыта"
    private(set) var canGoBack = false
    private(set) var canGoForward = false
    private(set) var isPicking = false
    /// Что передать агенту: окно кладёт это в строку ввода.
    var handOff: ((String) -> Void)?

    @ObservationIgnored private(set) lazy var view: WKWebView = makeView()

    var errorCount: Int { console.filter { $0.level == .error }.count }

    func go() {
        var text = address.trimmingCharacters(in: .whitespaces)
        if !text.contains("://") { text = (text.hasPrefix("localhost") || text.hasPrefix("127.") ? "http://" : "https://") + text }
        guard let url = URL(string: text) else {
            status = "Не адрес: \(text)"
            return
        }
        address = text
        console = []
        view.load(URLRequest(url: url))
    }

    func back() { view.goBack() }
    func forward() { view.goForward() }
    func reload() { view.reload() }
    func openInSafari() { view.url.map { _ = NSWorkspace.shared.open($0) } }

    func togglePicking() {
        isPicking.toggle()
        view.evaluateJavaScript("window.__crosskitPick && window.__crosskitPick(\(isPicking))")
    }

    /// Снимок страницы — во временный файл; путь уходит агенту, и он прочитает картинку.
    func snapshot() {
        view.takeSnapshot(with: nil) { [weak self] image, _ in
            guard let self, let image, let tiff = image.tiffRepresentation,
                  let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) else { return }
            let file = FileManager.default.temporaryDirectory.appendingPathComponent("crosskit-snapshot-\(Int(Date().timeIntervalSince1970)).png")
            do {
                try png.write(to: file)
                handOff?("[снимок страницы \(view.url?.absoluteString ?? ""): \(file.path)]")
                status = "Снимок добавлен в сообщение агенту"
            } catch {
                status = "Снимок не сохранён: \(error.localizedDescription)"
            }
        }
    }

    func send(_ line: ConsoleLine) {
        handOff?("[ошибка консоли \(view.url?.absoluteString ?? ""): \(line.text)]")
        status = "Ошибка консоли добавлена в сообщение агенту"
    }

    private func makeView() -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.userContentController.addUserScript(
            WKUserScript(source: Self.script, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        configuration.userContentController.add(MessageRelay(self), name: "crosskit")
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = self
        return view
    }

    fileprivate func receive(_ body: Any) {
        guard let message = body as? [String: Any], let kind = message["kind"] as? String else { return }
        let text = message["text"] as? String ?? ""
        switch kind {
        case "console":
            let level = ConsoleLine.Level(rawValue: message["level"] as? String ?? "log") ?? .log
            console.append(ConsoleLine(level: level, text: text))
            if console.count > 500 { console.removeFirst(console.count - 500) }
            if level == .error { consoleOpen = true }
        case "pick":
            isPicking = false
            handOff?("[элемент \(view.url?.path() ?? ""): \(text)]")
            status = "Элемент \(text) добавлен в сообщение агенту"
        default:
            break
        }
    }

    /// Консоль и ошибки страницы — окну; в режиме выбора клик по элементу отдаёт его селектор.
    private static let script = """
    (() => {
      const post = (m) => window.webkit.messageHandlers.crosskit.postMessage(m);
      const text = (args) => Array.from(args).map(a => { try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (e) { return String(a); } }).join(' ');
      for (const level of ['log', 'info', 'warn', 'error']) {
        const original = console[level];
        console[level] = function () { post({ kind: 'console', level, text: text(arguments) }); original.apply(console, arguments); };
      }
      window.addEventListener('error', e => post({ kind: 'console', level: 'error', text: e.message + ' (' + e.filename + ':' + e.lineno + ')' }));
      window.addEventListener('unhandledrejection', e => post({ kind: 'console', level: 'error', text: 'Promise: ' + String(e.reason) }));
      const selector = (el) => {
        if (el.id) return '#' + el.id;
        const parts = [];
        while (el && el.nodeType === 1 && parts.length < 4) {
          let part = el.tagName.toLowerCase();
          if (el.classList.length) part += '.' + Array.from(el.classList).slice(0, 2).join('.');
          const parent = el.parentElement;
          if (parent) { const same = Array.from(parent.children).filter(c => c.tagName === el.tagName); if (same.length > 1) part += ':nth-of-type(' + (same.indexOf(el) + 1) + ')'; }
          parts.unshift(part);
          el = parent;
        }
        return parts.join(' > ');
      };
      let picking = false;
      const pick = (e) => { if (!picking) return; e.preventDefault(); e.stopPropagation(); picking = false; document.documentElement.style.cursor = ''; post({ kind: 'pick', text: selector(e.target) }); };
      document.addEventListener('click', pick, true);
      window.__crosskitPick = (on) => { picking = on; document.documentElement.style.cursor = on ? 'crosshair' : ''; };
    })();
    """
}

extension BrowserModel: WKNavigationDelegate {

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        settle(webView, status: "Загружено")
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        settle(webView, status: "Ошибка: \(error.localizedDescription)")
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        settle(webView, status: "Не открылось: \(error.localizedDescription)")
    }

    private func settle(_ webView: WKWebView, status: String) {
        title = webView.title ?? ""
        address = webView.url?.absoluteString ?? address
        canGoBack = webView.canGoBack
        canGoForward = webView.canGoForward
        let errors = errorCount
        self.status = status + (errors > 0 ? " · ошибок в консоли: \(errors)" : "")
    }
}

/// Посредник: контроллер скриптов держит обработчик сильно, а браузер держит контроллер.
private final class MessageRelay: NSObject, WKScriptMessageHandler {
    weak var browser: BrowserModel?

    init(_ browser: BrowserModel) {
        self.browser = browser
    }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        browser?.receive(message.body)
    }
}
