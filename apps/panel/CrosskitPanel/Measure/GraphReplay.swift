import AppKit
import QuartzCore

/// Замер этапа UI-4: от ⇥ до кадра с графом, по отрезкам.
///
/// Фокусы берутся из файла (`--graph-replay=<файл>`), по одному на строку: глубина, пробел,
/// фокус — `2 conversation:<сессия>` или `1 path:<путь>`. Строка выдачи даёт фокус ровно так
/// (`GraphAnswer.focus(of:)`), и ⇥ вызывает тот же `showGraph`, что и здесь: нажатия клавиши
/// нет по той же причине, что в замере моста, — подделать его без «Универсального доступа»
/// нельзя, а доставка нажатия до обработчика — доли миллисекунды (замер UI-1).
///
/// Отрезки — те же, что у `BridgeReplay`, на часах `CACurrentMediaTime`: запуск процесса,
/// процесс `crosskit graph --json`, передача, разбор, перестройка экрана (с раскладкой) и
/// ближайший кадр.
final class GraphReplay: NSObject {

    private struct Sample {
        let total: Double
        let stages: [Double]
        let nodes: Int
    }

    private static let stageNames = ["запуск", "процесс", "передача", "разбор", "отрисовка", "кадр"]

    private let focuses: [(depth: Int, focus: String)]
    private let content: PanelContent
    private let meter: BridgeMeter

    private var index = -1
    private var startedAt: CFTimeInterval = 0
    private var appliedAt: CFTimeInterval = 0
    private var waitingFor = 0
    private var samples: [String: [Sample]] = [:]
    private var refused = 0
    private var displayLink: CADisplayLink?

    init(focuses: [(depth: Int, focus: String)], content: PanelContent, meter: BridgeMeter) {
        // Первый фокус повторён впереди: это первый ⇥ после запуска, и он считается отдельно —
        // в нём первый вид страницы и первая раскладка, а не только процесс ядра.
        self.focuses = focuses.first.map { [$0] + focuses } ?? []
        self.content = content
        self.meter = meter
    }

    static func focuses(in file: String) -> [(depth: Int, focus: String)]? {
        guard let text = try? String(contentsOfFile: (file as NSString).expandingTildeInPath, encoding: .utf8) else {
            return nil
        }
        return text.split(whereSeparator: \.isNewline).compactMap { line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty, !trimmed.hasPrefix("#"),
                  let space = trimmed.firstIndex(of: " "), let depth = Int(trimmed[..<space]) else { return nil }
            return (depth, String(trimmed[trimmed.index(after: space)...]))
        }
    }

    func start() {
        content.onGraphApplied = { [weak self] generation in self?.applied(generation) }
        print("замер графа: \(focuses.count - 1) фокусов и первый ⇥ после запуска перед ними")
        fflush(stdout)
        waitForPage()
    }

    /// Страница создаётся при запуске; первый ⇥ человек нажимает, когда она уже загружена.
    private func waitForPage() {
        guard content.graphPage.ready else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { self.waitForPage() }
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { self.next() }
    }

    private func next() {
        index += 1
        guard index < focuses.count else { return finish() }
        startedAt = CACurrentMediaTime()
        content.showGraph(focus: focuses[index].focus, depth: focuses[index].depth)
        waitingFor = content.lastGraphGeneration
    }

    private func applied(_ generation: Int) {
        guard generation == waitingFor else { return }
        appliedAt = CACurrentMediaTime()
        let link = content.displayLink(target: self, selector: #selector(frame))
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    @objc private func frame() {
        displayLink?.invalidate()
        displayLink = nil
        let shown = CACurrentMediaTime()
        record(shown: shown)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { self.next() }
    }

    private func record(shown: CFTimeInterval) {
        let (depth, focus) = focuses[index]
        let total = (shown - startedAt) * 1000
        guard let core = meter.take(generation: waitingFor) else {
            refused += 1
            print(String(format: "  %d %@: отказ, %.1f мс", depth, focus, total))
            return
        }
        let stages = [core.spawned - startedAt, core.exited - core.spawned, core.decodeStarted - core.exited,
                      core.decoded - core.decodeStarted, appliedAt - core.decoded, shown - appliedAt]
            .map { $0 * 1000 }
        let nodes = content.graphNodeCount
        let series = index == 0 ? "первый ⇥ после запуска: \(depth) \(focus)" : "\(depth) \(focus)"
        samples[series, default: []].append(Sample(total: total, stages: stages, nodes: nodes))
        print(String(format: "  %d %@: %.1f мс, узлов %d", depth, focus, total, nodes))
        fflush(stdout)
    }

    private func finish() {
        for (name, series) in samples.sorted(by: { $0.key < $1.key }) {
            print(summary("\(name): от ⇥ до кадра, узлов \(series.last?.nodes ?? 0)", series.map(\.total)))
            for (position, stage) in Self.stageNames.enumerated() {
                print(summary("  " + stage, series.map { $0.stages[position] }))
            }
        }
        if refused > 0 { print("отказов: \(refused) — в числа не вошли") }
        fflush(stdout)
        NSApp.terminate(nil)
    }

    private func summary(_ name: String, _ values: [Double]) -> String {
        guard !values.isEmpty else { return "\(name): нет данных" }
        let sorted = values.sorted()
        return String(format: "%@: p50 %.1f мс · p95 %.1f мс · n %d", name,
                      BridgeReplay.percentile(sorted, 0.50), BridgeReplay.percentile(sorted, 0.95), sorted.count)
    }
}
