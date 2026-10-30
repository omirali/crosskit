import AppKit

/// Проба клавиш и жестов холста — карты знаний (`--knowledge --probe-gestures`) или графа
/// окрестности (`--graph-focus=<фокус> --probe-gestures`) — без рук и без «Универсального
/// доступа». Механизм у холстов один, проба — тоже.
///
/// События строятся в процессе и отдаются **окну** (`sendEvent`) — тем же путём, каким
/// `NSApplication` отдаёт настоящие: так проверяется, что окно ловит их раньше вида под курсором и
/// строки запроса. Прокрутка — `CGEvent` над серединой холста; клавиши — `NSEvent.keyEvent`.
/// После каждого шага страница называет зум и сдвиг, а в конце проба говорит, не попал ли
/// символ в строку запроса. ⏎ ничего не открывает и не трогает буфер обмена
/// (`rehearsesOpening`): печатает, что сделал бы. Щипок так не построить: у `NSEvent` нет публичного конструктора
/// жеста увеличения; он идёт тем же путём окна и тем же `GraphPage.gesture`, что колесо с ctrl, и
/// проверяется рукой.
///
/// В конце — снимок окна тем же способом, что `--snapshot`: настоящих пикселей окна у процесса
/// нет — `CGWindowListCreateImage` закрыт, а ScreenCaptureKit требует «Записи экрана».
final class GestureProbe {

    private let content: PanelContent
    private let output: String?

    init(content: PanelContent, output: String?) {
        self.content = content
        self.output = output
    }

    func start() {
        guard let page = content.openCanvas else { return capture(page: nil) { NSApp.terminate(nil) } }
        content.rehearsesOpening = true
        let typed = content.queryField.stringValue
        let steps: [(String, () -> Void)] = [
            ("две пальца вниз, через окно", { self.scroll(dx: 0, dy: -80, control: false) }),
            ("две пальца вправо, через окно", { self.scroll(dx: -60, dy: 0, control: false) }),
            ("колесо с ctrl, через окно — масштаб под курсором", { self.scroll(dx: 0, dy: -40, control: true) }),
            ("→", { self.key(124, "\u{F703}") }),
            ("←", { self.key(123, "\u{F702}") }),
            ("→ удержание: два повтора подряд, быстрее анимации", {
                self.key(124, "\u{F703}", repeated: true)
                self.key(124, "\u{F703}", repeated: true)
            }),
            ("↓", { self.key(125, "\u{F701}") }),
            ("⇧↑", { self.key(126, "\u{F700}", shift: true) }),
            ("+", { self.key(24, "+", shift: true) }),
            ("=", { self.key(24, "=") }),
            ("−", { self.key(27, "-") }),
            ("0 — вписать", { self.key(29, "0") }),
            ("кнопка «+»", { self.content.handle(.zoom(.zoomIn)) }),
            ("⏎ на выбранном", { self.key(36, "\r") }),
            ("⌥↓ — следующий элемент", { self.key(125, "\u{F701}", option: true) }),
            ("⌥↓ — ещё один", { self.key(125, "\u{F701}", option: true) }),
            ("⏎ на выбранном теперь", { self.key(36, "\r") }),
        ]
        report("до жестов", page: page) {
            self.run(steps, page: page) {
                let untouched = self.content.queryField.stringValue
                // Снимок — до буквы: на графе набранное закрывает граф и возвращает к выдаче.
                self.capture(page: page) {
                    // Проверка самой пробы: буква обязана дойти до строки, иначе «не изменилась»
                    // ничего бы не доказывала.
                    self.key(40, "к")
                    let now = self.content.queryField.stringValue
                    print("строка запроса: «\(typed)» → после шагов «\(untouched)» → после буквы «к» «\(now)»")
                    fflush(stdout)
                    NSApp.terminate(nil)
                }
            }
        }
    }

    private func run(_ steps: [(String, () -> Void)], page: GraphPage, then: @escaping () -> Void) {
        guard let (name, step) = steps.first else { return then() }
        step()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
            self.report(name, page: page) { self.run(Array(steps.dropFirst()), page: page, then: then) }
        }
    }

    private func report(_ name: String, page: GraphPage, then: @escaping () -> Void) {
        page.evaluate("return window.\(page.canvas.rawValue).state()") { value in
            let state = value as? [String: Any]
            let zoom = (state?["zoom"] as? Double).map { String(format: "%.3f", $0) } ?? "—"
            let x = (state?["x"] as? Double).map { String(format: "%.0f", $0) } ?? "—"
            let y = (state?["y"] as? Double).map { String(format: "%.0f", $0) } ?? "—"
            print("шаг: \(name): зум \(zoom), сдвиг \(x), \(y)")
            fflush(stdout)
            then()
        }
    }

    /// Событие прокрутки трекпада в пикселях над серединой холста — окну.
    private func scroll(dx: Int32, dy: Int32, control: Bool) {
        guard let view = content.openCanvas?.webView, let window = view.window,
              let event = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2,
                                  wheel1: dy, wheel2: dx, wheel3: 0) else { return }
        let inWindow = view.convert(NSPoint(x: view.bounds.midX, y: view.bounds.midY), to: nil)
        let onScreen = window.convertPoint(toScreen: inWindow)
        let height = NSScreen.screens.first?.frame.height ?? 0
        event.location = CGPoint(x: onScreen.x, y: height - onScreen.y)
        if control { event.flags = .maskControl }
        guard let scroll = NSEvent(cgEvent: event) else { return }
        window.sendEvent(scroll)
    }

    /// Нажатие клавиши — окну, как его отдаёт приложение.
    private func key(_ code: UInt16, _ characters: String, shift: Bool = false, option: Bool = false,
                     repeated: Bool = false) {
        var flags: NSEvent.ModifierFlags = []
        if shift { flags.insert(.shift) }
        if option { flags.insert(.option) }
        guard let window = content.window,
              let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags,
                                           timestamp: ProcessInfo.processInfo.systemUptime,
                                           windowNumber: window.windowNumber, context: nil,
                                           characters: characters, charactersIgnoringModifiers: characters,
                                           isARepeat: repeated, keyCode: code) else { return }
        window.sendEvent(event)
    }

    private func capture(page: GraphPage?, then: @escaping () -> Void) {
        guard let output, let page else { return then() }
        WindowSnapshot.write(content, webView: page.webView, to: output) { written in
            print(written ? "снимок: \(output)" : "снимок не записан: \(output)")
            fflush(stdout)
            then()
        }
    }
}
