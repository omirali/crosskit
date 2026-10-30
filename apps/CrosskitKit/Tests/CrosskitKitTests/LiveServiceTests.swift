import Foundation
import Testing
@testable import CrosskitKit

/// Карта настоящей задачи у работающей службы. Задачу и контекст называют переменные окружения,
/// иначе проверка не запускается: в исходниках нет ни чужих задач, ни чужих путей.
///
///     CROSSKIT_LIVE_TASK="<задача>" CROSSKIT_LIVE_CONTEXT=work swift test --filter LiveService
@Suite("Живая служба", .enabled(if: ProcessInfo.processInfo.environment["CROSSKIT_LIVE_TASK"] != nil))
struct LiveServiceTests {

    @Test("Карта задачи приходит, становится деревом, и дерево помещается на холст")
    func aRealMapBecomesATree() async throws {
        let environment = ProcessInfo.processInfo.environment
        let task = try #require(environment["CROSSKIT_LIVE_TASK"])
        let client = ServiceClient.located()
        let board = try await client.taskBoard(context: environment["CROSSKIT_LIVE_CONTEXT"])
        let id = try #require(board.confirmed.first { $0.name == task }?.id, "в книге задач нет «\(task)»")
        let map = try await client.taskMap(id, context: board.context)
        let tree = DecisionTree(map: map)
        let layout = TreeLayout(tree: tree, showsRejected: true)

        #expect(!tree.steps.isEmpty)
        #expect(tree.steps.allSatisfy { !$0.branches.isEmpty || !tree.joined }, "у каждого шага сводки нашлась развилка")
        for label in layout.labels {
            #expect(label.frame.minX >= 0 && label.frame.maxX <= TreeLayout.width)
        }
        print("""
            шагов \(tree.steps.count), без развилки \(tree.steps.filter(\.branches.isEmpty).count), \
            с ведущей веткой \(tree.steps.filter { $0.lead != nil }.count); \
            рисков \(tree.risks.count), открытых \(tree.openRisks.count), полос \(layout.rails.count); \
            холст \(Int(layout.size.width))×\(Int(layout.size.height))
            """)
    }
}

/// Одна настоящая беседа с установленным `claude` через login-оболочку: поток, текст кусками,
/// конец хода. Тратит лимит подписки, поэтому только по переменной окружения.
///
///     CROSSKIT_LIVE_CHAT=1 swift test --filter LiveChat
@Suite("Живой claude", .enabled(if: ProcessInfo.processInfo.environment["CROSSKIT_LIVE_CHAT"] != nil))
struct LiveChatTests {

    @Test("Ход настоящего claude приходит потоком и кончается итогом")
    func aRealTurnStreams() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("ck-live-chat")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let session = ChatSession(start: .init(directory: directory, model: "haiku"))
        var pieces = 0
        var finished: TurnResult?
        // Первым пишет человек: в режиме потокового ввода claude молчит до первого слова.
        let events = await session.events()
        await session.send("Ответь одним словом: готово.")
        for try await event in events {
            switch event {
            case .text: pieces += 1
            case .finished(let result):
                finished = result
                await session.close()
            default: break
            }
        }
        #expect(pieces > 0, "текст пришёл кусками")
        #expect(finished?.isError == false)
        print("кусков текста \(pieces), стоимость \(finished?.costUSD ?? 0) $")
    }
}
