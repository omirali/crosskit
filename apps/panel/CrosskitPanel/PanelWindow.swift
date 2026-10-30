import AppKit

/// Окно панели: `NSPanel` со стилем `.nonactivatingPanel`.
///
/// Весь смысл спайка сидит в двух местах этого файла:
/// `.nonactivatingPanel` в стиле — окно получает клавиатуру, не активируя своё приложение;
/// `canBecomeKey` — без него безрамочное окно ключевым не становится и печатать в него нельзя.
final class PanelWindow: NSPanel {

    /// Безрамочная панель по умолчанию ключевой не становится, и строка запроса не получила бы ввод.
    override var canBecomeKey: Bool { true }

    /// Главным не становится никогда: главное окно — то, в котором человек работает.
    override var canBecomeMain: Bool { false }

    /// esc, пришедший мимо строки запроса, тоже должен прятать панель.
    var onCancel: (() -> Void)?

    /// ⌘-сочетания панели. У accessory-приложения нет строки меню, поэтому клавиши
    /// с ⌘ до строки запроса не доходят и разбираются здесь.
    var onCommandKey: ((NSEvent) -> Bool)?

    /// Клавиши и жесты, которые разбирает экран раньше вида под курсором и строки запроса:
    /// карта знаний. `true` — событие разобрано и дальше не идёт.
    var onEarlyEvent: ((NSEvent) -> Bool)?

    init(contentRect: NSRect) {
        super.init(contentRect: contentRect,
                   styleMask: [.nonactivatingPanel, .borderless],
                   backing: .buffered,
                   defer: false)

        isFloatingPanel = true
        level = .floating
        // Панель показывается на том Space, где человек работает сейчас, и не утаскивает
        // на свой; `.fullScreenAuxiliary` — право лечь поверх чужого полноэкранного окна.
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
        // Панели по умолчанию прячутся, когда их приложение перестаёт быть активным.
        // Это приложение активным не бывает вовсе, так что правило надо снять.
        hidesOnDeactivate = false

        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        isMovableByWindowBackground = false
        animationBehavior = .none
        // Панель не участвует в Exposé и не попадает в снимки окон приложения.
        isExcludedFromWindowsMenu = true
    }

    override func sendEvent(_ event: NSEvent) {
        switch event.type {
        case .keyDown, .scrollWheel, .magnify:
            if onEarlyEvent?(event) == true { return }
        default:
            break
        }
        super.sendEvent(event)
    }

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }

    /// Безрамочное окно не отдаёт ⌘-сочетания меню, которого у accessory-приложения нет.
    /// ⌘Q и ⌘W должны работать хотя бы для того, чтобы спайк можно было закрыть.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard event.modifierFlags.contains(.command) else { return super.performKeyEquivalent(with: event) }
        if onCommandKey?(event) == true { return true }
        if event.isCommand("q") {
            NSApp.terminate(nil)
            return true
        }
        if event.isCommand("w") {
            orderOut(nil)
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
}
