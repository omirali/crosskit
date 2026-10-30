import Darwin
import XCTest

/// Панель спрашивает службу раньше процесса, и только о том, что служба отвечает.
final class ServiceSocketTests: XCTestCase {

    func testAMachineQuestionBecomesARequestAndAnythingElseStaysAProcess() throws {
        let search = try XCTUnwrap(ServiceSocket.request(for: ["--context", "work", "search", "retry", "--json", "--limit", "8"]))
        XCTAssertEqual(search["op"] as? String, "search")
        XCTAssertEqual(search["context"] as? String, "work")
        XCTAssertEqual(search["arguments"] as? [String], ["retry", "--json", "--limit", "8"])

        let map = try XCTUnwrap(ServiceSocket.request(for: ["--context", "work", "knowledge", "map", "--json"]))
        XCTAssertEqual(map["op"] as? String, "knowledge-map")
        XCTAssertEqual(map["arguments"] as? [String], ["map", "--json"])

        XCTAssertNil(ServiceSocket.request(for: ["--version"]))
        XCTAssertNil(ServiceSocket.request(for: ["search", "retry"]), "без --json это не машинный ответ")
        XCTAssertNil(ServiceSocket.request(for: ["--context", "work", "ask", "что", "--json"]), "ask служба не отвечает")
    }

    /// Поддельная служба: принимает одно соединение, читает строку и отвечает `answer`.
    private func serving(_ answer: String) throws -> (ServiceSocket, Thread) {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("ck-\(UUID().uuidString.prefix(8))").appendingPathComponent("service")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent("crosskit.sock").path
        let listener = socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: Array(path.utf8) + [0]) }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(listener, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        XCTAssertEqual(bound, 0)
        XCTAssertEqual(Darwin.listen(listener, 1), 0)
        let thread = Thread {
            let client = accept(listener, nil, nil)
            var buffer = [UInt8](repeating: 0, count: 4096)
            _ = read(client, &buffer, buffer.count)
            let line = Array((answer + "\n").utf8)
            _ = write(client, line, line.count)
            close(client)
            close(listener)
        }
        thread.start()
        return (ServiceSocket(directory: directory.path), thread)
    }

    func testTheServicesLineIsTheAnswer() throws {
        let (service, _) = try serving(#"{"schema":2,"contexts":["work"]}"#)
        let answer = service.ask(["protocol": 1, "op": "contexts", "arguments": ["--json"]], timeout: 5)
        XCTAssertEqual(answer.map { String(decoding: $0, as: UTF8.self) }, #"{"schema":2,"contexts":["work"]}"#)
    }

    func testARefusalOfTheProtocolSendsTheQuestionToAProcess() throws {
        let (service, _) = try serving(#"{"protocol":1,"refused":"not-served"}"#)
        XCTAssertNil(service.ask(["protocol": 1, "op": "search", "arguments": ["x", "--json"]], timeout: 5))
    }

    func testNoServiceIsNoAnswerAtOnce() {
        let started = Date()
        let service = ServiceSocket(directory: "/nonexistent/service")
        XCTAssertNil(service.ask(["protocol": 1, "op": "hello"], timeout: 5))
        XCTAssertLessThan(Date().timeIntervalSince(started), 1)
    }
}
