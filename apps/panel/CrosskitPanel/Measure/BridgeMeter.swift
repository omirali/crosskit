import Foundation
import QuartzCore

/// Штампы одного запроса к ядру, по номеру запроса — для замера этапа UI-3.
///
/// Источник пишет, когда процесс запущен и вышел и сколько занял разбор ответа; замер
/// забирает это, когда страница уже на экране. Часы одни — `CACurrentMediaTime`, — поэтому
/// отрезки складываются.
final class BridgeMeter: @unchecked Sendable {

    struct CoreStages {
        let spawned: CFTimeInterval
        let exited: CFTimeInterval
        let decodeStarted: CFTimeInterval
        let decoded: CFTimeInterval
    }

    private let lock = NSLock()
    private var stages: [Int: CoreStages] = [:]

    func record(generation: Int, stages: CoreStages) {
        lock.withLock { self.stages[generation] = stages }
    }

    func take(generation: Int) -> CoreStages? {
        lock.withLock { stages.removeValue(forKey: generation) }
    }
}
