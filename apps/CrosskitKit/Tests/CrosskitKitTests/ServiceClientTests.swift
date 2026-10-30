import Darwin
import Foundation
import Testing
@testable import CrosskitKit

@Suite("Клиент службы", .serialized)
struct ServiceClientTests {

    /// Поддельная служба: одно соединение, одна строка вопроса, ответ `answer`. Возвращает
    /// каталог `service/` и то, что спросили.
    private func serving(_ answer: String, after delay: Duration = .zero) throws -> (URL, Task<String, Never>) {
        // Короткий путь: адрес сокета вмещает 104 байта.
        let directory = URL(fileURLWithPath: "/tmp/ck-\(UUID().uuidString.prefix(8))/service")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent("crosskit.sock").path
        let listener = socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: Array(path.utf8) + [0]) }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(listener, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        try #require(bound == 0 && listen(listener, 1) == 0)
        let asked = Task.detached { () async -> String in
            let peer = accept(listener, nil, nil)
            defer { close(peer); close(listener) }
            // Клиент, у которого вышел срок, уже ушёл: запись ему не должна ронять тесты.
            var noSignal: Int32 = 1
            setsockopt(peer, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size))
            var question = Data()
            var byte: UInt8 = 0
            while read(peer, &byte, 1) == 1, byte != 0x0A { question.append(byte) }
            try? await Task.sleep(for: delay)
            let reply = Array((answer + "\n").utf8)
            _ = reply.withUnsafeBytes { write(peer, $0.baseAddress, reply.count) }
            return String(decoding: question, as: UTF8.self)
        }
        return (directory, asked)
    }

    @Test("Карта задачи спрашивается операцией task-map и читается")
    func aTaskMapIsAskedAndRead() async throws {
        let (directory, asked) = try serving(MapFixture.line.replacingOccurrences(of: "\n", with: ""))
        let map = try await ServiceClient(serviceDirectory: directory).taskMap("Двойная скидка", context: "work")

        #expect(map.conversations.count == 2)
        #expect(map.joined?.mainLine.count == 2)
        let request = try JSONSerialization.jsonObject(with: Data(await asked.value.utf8)) as? [String: Any]
        #expect(request?["op"] as? String == "task-map")
        #expect(request?["context"] as? String == "work")
        #expect(request?["arguments"] as? [String] == ["Двойная скидка", "--json"])
    }

    @Test("Отказ протокола, отказ команды и «нет такой задачи» — разные ошибки")
    func refusalsAreTold() async throws {
        let cases: [(String, ServiceError)] = [
            (#"{"protocol":1,"refused":"not-served"}"#, .refused("not-served")),
            (#"{"schema":2,"error":{"category":"usage","context":null}}"#, .commandRefused(category: "usage")),
            (#"{"schema":2,"missing_task":"Нет"}"#, .noSuchTask("Нет")),
            ("не json", .unreadable),
        ]
        for (line, expected) in cases {
            let (directory, _) = try serving(line)
            await #expect(throws: expected) {
                _ = try await ServiceClient(serviceDirectory: directory).taskMap("Нет", context: nil)
            }
        }
    }

    @Test("Без сокета служба не запущена")
    func noSocketIsNotRunning() async {
        let nowhere = URL(fileURLWithPath: "/tmp/ck-none-\(UUID().uuidString.prefix(8))")
        await #expect(throws: ServiceError.notRunning) {
            _ = try await ServiceClient(serviceDirectory: nowhere).contexts()
        }
    }

    @Test("Каталог службы — из CROSSKIT_DATA_DIR, иначе из Application Support")
    func theServiceIsLocated() async {
        let named = ServiceClient.located(environment: ["CROSSKIT_DATA_DIR": "/tmp/data"])
        #expect(await named.socketPath == "/tmp/data/service/crosskit.sock")
    }

    @Test("Состояние контекста читается из статуса службы: фаза и причина ожидания")
    func aContextStateIsReadFromTheStatus() async throws {
        let directory = URL(fileURLWithPath: "/tmp/ck-\(UUID().uuidString.prefix(8))/service")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let status = #"{"pid":1,"contexts":[{"name":"work","phase":"waiting","since_ms":0,"waiting":"context work did not open","last_pass":null},{"name":"home","phase":"starting","since_ms":0,"waiting":null,"last_pass":{"at_ms":5,"text":"Pass 1"}}]}"#
        try Data(status.utf8).write(to: directory.appendingPathComponent("status.json"))
        let client = ServiceClient(serviceDirectory: directory)

        #expect(await client.state(of: "work")?.waiting == "context work did not open")
        #expect(await client.state(of: "home")?.lastPass?.text == "Pass 1")
        #expect(await client.state(of: "nowhere") == nil)
    }

    @Test("Служба, которая молчит дольше срока, — это срок, а не нечитаемый ответ")
    func silenceIsATimeout() async throws {
        let (directory, _) = try serving(#"{"schema":2,"contexts":[]}"#, after: .seconds(1))
        await #expect(throws: ServiceError.timedOut) {
            _ = try await ServiceClient(serviceDirectory: directory, timeout: .milliseconds(200)).contexts()
        }
    }
}
