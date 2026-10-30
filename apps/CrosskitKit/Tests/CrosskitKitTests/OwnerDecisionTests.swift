import Foundation
import Testing
@testable import CrosskitKit

@Suite("Решения владельца и доска")
struct OwnerDecisionTests {

    @Test("Решение становится аргументами команды tasks, как их понимает служба")
    func aDecisionIsTheCommandsArguments() {
        #expect(OwnerDecision.risk(task: "task:a", session: "aaaa1111", reply: 12, verdict: .accepted, reason: "есть снапшот").arguments
                == ["accept", "task:a", "aaaa1111", "12", "--reason", "есть снапшот", "--json"])
        #expect(OwnerDecision.risk(task: "t", session: "s", reply: 1, verdict: .resolved, reason: "MR !95").arguments.first == "resolve")
        #expect(OwnerDecision.confirm(conversation: "bbbb", name: "Скидки").arguments == ["confirm", "bbbb", "--name", "Скидки", "--json"])
        #expect(OwnerDecision.finish(task: "t").arguments == ["finish", "t", "--json"])
    }

    @Test("Отказ книги задач приходит данными")
    func aDeclinedDecisionIsData() throws {
        let line = #"{"schema":2,"declined":{"kind":"open-critical-risks","task":"Скидки","count":2}}"#
        #expect(throws: ServiceError.declined(Declined(kind: "open-critical-risks", task: "Скидки", name: nil,
                                                       session: nil, reference: nil, count: 2, reply: nil))) {
            _ = try ServiceAnswer.decode(TaskBoard.self, from: Data(line.utf8))
        }
    }

    @Test("Колонка доски: готово, не размечена, ждёт решения, в работе")
    func aTaskFallsIntoItsColumn() throws {
        func task(_ progress: String, finished: String = "null") throws -> ConfirmedTask {
            try ServiceAnswer.decode(ConfirmedTask.self, from: Data("""
                {"id":"task:a","name":"Скидки","conversations":[],"proposed":[],"progress":\(progress),
                 "finished_at_unix_ms":\(finished),"risk_decisions":[]}
                """.utf8))
        }
        let waiting = #"{"conversations":2,"mapped":2,"steps":4,"head":"open","open_risks":3,"open_critical":1,"decided_risks":0}"#
        let working = #"{"conversations":2,"mapped":1,"steps":4,"head":"chosen","open_risks":0,"open_critical":0,"decided_risks":1}"#
        let unmapped = #"{"conversations":2,"mapped":0,"steps":0,"head":null,"open_risks":0,"open_critical":0,"decided_risks":0}"#
        #expect(try task(waiting).column == .waiting)
        #expect(try task(working).column == .inWork)
        #expect(try task(unmapped).column == .unmapped)
        #expect(try task(working, finished: "1790000000000").column == .done)
        #expect(try task("null").column == .unmapped, "служба старее хода задачи")
    }

    @Test("Риск, принятый владельцем, не открыт и не тянет полосу")
    func aDecidedRiskIsNotOpen() throws {
        let line = MapFixture.line.replacingOccurrences(
            of: #""title":"Частичный возврат не пересчитан","state":"open","level":"critical","#,
            with: #""title":"Частичный возврат не пересчитан","state":"open","level":"critical","decided":{"verdict":"accepted","reason":"есть снапшот","at_unix_ms":1},"#)
        let tree = DecisionTree(map: try ServiceAnswer.decode(TaskMapAnswer.self, from: Data(line.utf8)))
        let refund = try #require(tree.risks.first { $0.title == "Частичный возврат не пересчитан" })
        #expect(refund.decided?.reason == "есть снапшот")
        #expect(refund.shownStatus == .accepted)
        #expect(!refund.isOpen && !refund.pullsDown)
        #expect(tree.openRisks.isEmpty)
    }
}

@Suite("Построение карты из окна", .serialized)
struct MapBuildTests {

    @Test("Оценка: реплики без карты и сводка")
    func theEstimateCountsWhatNoMapCovers() throws {
        let estimate = BuildEstimate(map: try MapFixture.answer())
        #expect(estimate.unmappedReplies == 6)
        #expect(estimate.conversations == 1)
        #expect(abs(estimate.usd - (6 * BuildEstimate.usdPerReply + BuildEstimate.usdPerJoin)) < 0.0001)
    }

    @Test("Аргументы идут через «$@» и не становятся командой")
    func argumentsStayArguments() {
        let arguments = MapBuild.shellArguments(program: URL(fileURLWithPath: "/data/bin/crosskit"),
                                                task: "Скидки; rm -rf ~", context: "work")
        #expect(arguments == ["-l", "-c", "exec \"$@\"", "crosskit", "/data/bin/crosskit",
                              "--context", "work", "tasks", "map", "Скидки; rm -rf ~", "--build"])
    }

    /// Поддельная программа: пишет ход в stderr и выходит с кодом `status`.
    private func program(status: Int) throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("ck-build-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let script = directory.appendingPathComponent("crosskit")
        try Data("#!/bin/sh\necho \"  replies 1–40…\" >&2\necho \"task: $4\" >&2\nexit \(status)\n".utf8).write(to: script)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        return script
    }

    @Test("Ход построения приходит строками, отказ — с последними словами команды")
    func progressComesAsLines() async throws {
        var lines: [String] = []
        for try await line in MapBuild.run(program: try program(status: 0), task: "Скидки", context: "work") {
            lines.append(line)
        }
        #expect(lines == ["  replies 1–40…", "task: map"])

        await #expect(throws: MapBuild.Failure(status: 3, said: ["  replies 1–40…", "task: map"])) {
            for try await _ in MapBuild.run(program: try program(status: 3), task: "Скидки", context: "work") {}
        }
    }
}
