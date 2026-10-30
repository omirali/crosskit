import AppKit
import Carbon.HIToolbox

/// Приложение целиком: одна панель, одна глобальная клавиша, один замер. Выдача приходит
/// из-за шва `ResultFeed`: по умолчанию из ядра через процесс `crosskit`, ключом `--state`
/// — из выдуманного источника.
final class AppDelegate: NSObject, NSApplicationDelegate {

    private let probe = FirstFrameProbe()
    private var controller: PanelController?
    private var hotkey: GlobalHotkey?
    private var manualTrigger: DispatchSourceSignal?
    private var replay: BridgeReplay?
    private var graphReplay: GraphReplay?
    private var gestureProbe: GestureProbe?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Accessory: приложение без иконки в Dock и без строки меню. То же говорит
        // `LSUIElement` в Info.plist; политика здесь — чтобы это было видно в коде.
        NSApp.setActivationPolicy(.accessory)

        print("расхождение часов событий и CoreAnimation: "
              + String(format: "%.3f мс", FirstFrameProbe.clockOffsetMilliseconds()))

        // Окно строится и прогревается при запуске, а не по первому нажатию. Это не
        // настройка: первый вызов платит за первую отрисовку, и платить за неё должен
        // запуск, которого никто не ждёт.
        let options = LaunchOptions.fromCommandLine()
        switch options.appearance {
        case "dark": NSApp.appearance = NSAppearance(named: .darkAqua)
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        default: break
        }
        let before = ProcessMemory.residentMegabytes()
        let controller = PanelController(probe: probe, options: options)
        // Страница графа создаётся вместе с окном; сколько она стоит памяти этого процесса,
        // видно по двум числам. Процессы WebKit считаются снаружи (ADR 0046).
        controller.content.graphPage.onReady = {
            print(String(format: "память панели: до окна и веб-вида %.1f МБ, со страницей графа %.1f МБ",
                         before, ProcessMemory.residentMegabytes()))
            fflush(stdout)
        }
        controller.prewarm()
        self.controller = controller

        do {
            hotkey = try GlobalHotkey(keyCode: UInt32(kVK_Space),
                                      modifiers: UInt32(optionKey),
                                      signature: 0x4343_5031 /* CCP1 */) { [weak self] eventTime in
                self?.hotkeyPressed(at: eventTime)
            }
            print("⌥Space зарегистрирована. Разрешений система не спрашивала.")
        } catch {
            print("⌥Space не зарегистрирована: \(error)")
        }
        installManualTrigger()
        fflush(stdout)
        startReplay(options, controller: controller)
        startGraphReplay(options, controller: controller)
        if let focus = options.graphFocus {
            controller.show(takingKeyboard: options.probeGestures == nil)
            if let probe = options.probeGestures {
                let content = controller.content
                content.onGraphApplied = { [weak self] _ in
                    guard self?.gestureProbe == nil else { return }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                        let gestures = GestureProbe(content: content, output: probe)
                        self?.gestureProbe = gestures
                        gestures.start()
                    }
                }
            } else if let path = options.snapshot {
                let content = controller.content
                content.onGraphApplied = { _ in
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                        WindowSnapshot.write(content, webView: content.graphPage.webView, to: path) { written in
                            print(written ? "снимок: \(path)" : "снимок не записан: \(path)")
                            fflush(stdout)
                            NSApp.terminate(nil)
                        }
                    }
                }
            }
            controller.content.showGraph(focus: focus, depth: GraphScreen.defaultDepth)
        }
        if options.knowledge {
            showKnowledge(options, controller: controller)
        }
    }

    /// Карта знаний сразу после показа — для снимка (`--snapshot`) или чтобы посмотреть.
    private func showKnowledge(_ options: LaunchOptions, controller: PanelController) {
        controller.show(takingKeyboard: options.probeGestures == nil)
        let content = controller.content
        if let probe = options.probeGestures {
            content.onKnowledgeApplied = { [weak self] _ in
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                    let gestures = GestureProbe(content: content, output: probe)
                    self?.gestureProbe = gestures
                    gestures.start()
                }
            }
        } else if let path = options.snapshot {
            content.onKnowledgeApplied = { _ in
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
                    WindowSnapshot.write(content, webView: content.knowledgePage.webView, to: path) { written in
                        print(written ? "снимок: \(path)" : "снимок не записан: \(path)")
                        fflush(stdout)
                        NSApp.terminate(nil)
                    }
                }
            }
        }
        content.showKnowledge(picking: options.pick)
    }

    /// Замер графа: панель на экране, фокусы из файла по одному, числа в stdout, выход.
    private func startGraphReplay(_ options: LaunchOptions, controller: PanelController) {
        guard let file = options.graphReplay, let meter = options.meter else { return }
        guard let focuses = GraphReplay.focuses(in: file), !focuses.isEmpty else {
            print("замер графа: в \(file) нет фокусов — нужен файл, «<глубина> <фокус>» на строку")
            NSApp.terminate(nil)
            return
        }
        controller.show()
        let replay = GraphReplay(focuses: focuses, content: controller.content, meter: meter)
        graphReplay = replay
        replay.start()
    }

    /// Замер моста: панель на экране, запросы из файла по одному, числа в stdout, выход.
    private func startReplay(_ options: LaunchOptions, controller: PanelController) {
        guard let file = options.replay, let meter = options.meter else { return }
        guard let queries = BridgeReplay.queries(in: file), !queries.isEmpty else {
            print("замер моста: в \(file) нет запросов — нужен файл, по запросу на строку")
            NSApp.terminate(nil)
            return
        }
        controller.show()
        let replay = BridgeReplay(queries: queries, content: controller.content, meter: meter,
                                  core: (options.feed as? CoreFeed)?.core)
        self.replay = replay
        replay.start()
    }

    /// Показ по `SIGUSR1` — только для проверки спайка, и вот почему он есть: подделать
    /// нажатие ⌥Space из скрипта нельзя без разрешения «Универсальный доступ», а без показа
    /// панели не посмотреть ни на фокус соседнего окна, ни на материал. Сигнал идёт мимо
    /// оконного сервера и ничьей активации не трогает. Горячую клавишу он НЕ проверяет.
    private func installManualTrigger() {
        signal(SIGUSR1, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
        source.setEventHandler { [weak self] in self?.toggle(eventTime: nil) }
        source.resume()
        manualTrigger = source
        print("служебный показ: kill -USR1 \(ProcessInfo.processInfo.processIdentifier)")
    }

    private func hotkeyPressed(at eventTime: EventTime) {
        toggle(eventTime: eventTime)
    }

    private func toggle(eventTime: EventTime?) {
        guard let controller else { return }
        if controller.isVisible {
            controller.hide()
            return
        }
        // Замер начинается до показа; у сигнала штампа нажатия нет, и выдавать его
        // за замер нельзя.
        if let eventTime { probe.begin(eventTime: eventTime) }
        controller.show()
    }
}
