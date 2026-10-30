import AppKit
import QuartzCore

/// Замер моста этапа UI-3: от запроса в строке до строк на экране, по отрезкам.
///
/// Запросы берутся из файла (`--replay=<файл>`, по одному на строку) и подставляются в
/// строку запроса целиком — так, как будто их вставили. Нажатия клавиш здесь нет: подделать
/// его без «Универсального доступа» нельзя (README, «Разрешения»). Доставка нажатия до
/// обработчика — доли миллисекунды (замер UI-1), и её в числе нет; это сказано в отчёте.
///
/// Отрезки, все на часах `CACurrentMediaTime`:
///
/// * `запуск` — от смены строки до запущенного процесса;
/// * `процесс` — от запуска до выхода `crosskit`: старт программы, открытие графа и индекса,
///   запрос, печать ответа. Сколько из этого — старт программы, говорит отдельная серия
///   `crosskit --version`, которая не открывает ничего;
/// * `передача` — от выхода процесса до начала разбора;
/// * `разбор` — JSON в страницу панели;
/// * `отрисовка` — от разобранной страницы до перестроенного экрана;
/// * `кадр` — до ближайшего обновления дисплея после этого.
final class BridgeReplay: NSObject {

    private struct Sample {
        let query: String
        let total: Double
        let stages: [Double]
    }

    private static let stageNames = ["запуск", "процесс", "передача", "разбор", "отрисовка", "кадр"]

    private let queries: [String]
    private let content: PanelContent
    private let meter: BridgeMeter
    private let core: CoreProcess?

    private var index = -1
    private var startedAt: CFTimeInterval = 0
    private var appliedAt: CFTimeInterval = 0
    private var waitingFor = 0
    private var samples: [Sample] = []
    private var refused = 0
    private var displayLink: CADisplayLink?

    init(queries: [String], content: PanelContent, meter: BridgeMeter, core: CoreProcess?) {
        // Первый запрос повторён впереди как прогрев: первый процесс после простоя платит за
        // холодный кеш файлов, а в работе панели такой запрос — исключение.
        self.queries = queries.first.map { [$0] + queries } ?? []
        self.content = content
        self.meter = meter
        self.core = core
    }

    static func queries(in file: String) -> [String]? {
        guard let text = try? String(contentsOfFile: (file as NSString).expandingTildeInPath, encoding: .utf8) else {
            return nil
        }
        return text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty && !$0.hasPrefix("#") }
    }

    func start() {
        content.onAnswerApplied = { [weak self] generation in self?.applied(generation) }
        Task { @MainActor in
            await self.measureProgramStart()
            print("замер моста: \(self.queries.count - 1) запросов и прогрев перед ними")
            fflush(stdout)
            self.nextQuery()
        }
    }

    /// Старт программы без единого открытия: столько стоит процесс сам по себе.
    private func measureProgramStart() async {
        guard let core else { return }
        var runs: [Double] = []
        for _ in 1..<max(queries.count, 2) {
            guard let outcome = try? await core.run(["--version"]) else { continue }
            runs.append((outcome.exitedAt - outcome.spawnedAt) * 1000)
        }
        print(Self.summary("crosskit --version", runs))
    }

    private func nextQuery() {
        index += 1
        guard index < queries.count else { return finish() }
        startedAt = CACurrentMediaTime()
        content.replaceQuery(queries[index])
        waitingFor = content.lastGeneration
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
        if index > 0 {
            record(shown: shown)
        }
        // Пауза между запросами, чтобы следующий не встал в очередь за хвостом предыдущего.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { self.nextQuery() }
    }

    private func record(shown: CFTimeInterval) {
        let total = (shown - startedAt) * 1000
        guard let core = meter.take(generation: waitingFor) else {
            refused += 1
            print(String(format: "  «%@»: отказ, %.1f мс", queries[index], total))
            return
        }
        let stages = [core.spawned - startedAt, core.exited - core.spawned, core.decodeStarted - core.exited,
                      core.decoded - core.decodeStarted, appliedAt - core.decoded, shown - appliedAt]
            .map { $0 * 1000 }
        samples.append(Sample(query: queries[index], total: total, stages: stages))
        print(String(format: "  «%@»: %.1f мс", queries[index], total))
        fflush(stdout)
    }

    private func finish() {
        print(Self.summary("от строки до кадра", samples.map(\.total)))
        for (position, name) in Self.stageNames.enumerated() {
            print(Self.summary("  " + name, samples.map { $0.stages[position] }))
        }
        if refused > 0 { print("отказов: \(refused) — в числа не вошли") }
        fflush(stdout)
        NSApp.terminate(nil)
    }

    private static func summary(_ name: String, _ values: [Double]) -> String {
        guard !values.isEmpty else { return "\(name): нет данных" }
        let sorted = values.sorted()
        return String(format: "%@: p50 %.1f мс · p95 %.1f мс · n %d", name,
                      percentile(sorted, 0.50), percentile(sorted, 0.95), sorted.count)
    }

    /// Ближайший ранг: значение, не меньше которого `share` выборки.
    static func percentile(_ sorted: [Double], _ share: Double) -> Double {
        let rank = Int((share * Double(sorted.count)).rounded(.up))
        return sorted[min(max(rank, 1), sorted.count) - 1]
    }
}
