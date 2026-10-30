import Foundation
import QuartzCore

/// Откуда экран графа берёт вид. Отдельно от `ResultFeed`: выдача и граф — два разных вопроса
/// к ядру, и источник, который умеет первый, не обязан уметь второй. Выдуманный источник графа
/// не рисует — у него нет ни касаний, ни реплик, и придумывать их значило бы рисовать то,
/// чего не бывает.
protocol GraphSource: AnyObject {
    /// Окрестность узла `focus` в контексте `context`: глубина 1 или 2, не больше 100 узлов.
    func graph(context: String, focus: String, depth: Int, generation: Int) async throws -> GraphAnswer.View
}

extension CoreFeed: GraphSource {

    /// Процесс `crosskit --context <имя> graph --focus <id> --depth <N> --json` на каждый вид —
    /// тот же мост, что у выдачи (ADR 0044): с той же отменой и тем же таймаутом.
    func graph(context: String, focus: String, depth: Int, generation: Int) async throws -> GraphAnswer.View {
        guard let core else { throw FeedFailure.coreNotFound }
        let outcome = try await core.run(["--context", context, "graph", "--focus", focus,
                                          "--depth", String(depth), "--json"])
        let decodeStarted = CACurrentMediaTime()
        let view = try GraphAnswer.view(from: outcome.stdout)
        meter?.record(generation: generation,
                      stages: BridgeMeter.CoreStages(spawned: outcome.spawnedAt, exited: outcome.exitedAt,
                                                     decodeStarted: decodeStarted, decoded: CACurrentMediaTime()))
        return view
    }
}
