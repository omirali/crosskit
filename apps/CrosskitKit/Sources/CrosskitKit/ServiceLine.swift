import Darwin
import Foundation

/// Одна строка JSON туда и одна обратно по сокету службы (протокол 1, ADR 0050).
///
/// Вызов блокирующий: каждая сторона ждёт не дольше `timeout`. Ждать его в общем пуле задач
/// нельзя, поэтому `ServiceClient` уводит его на свою очередь, а панель — на свою.
public enum ServiceLine {

    public static func exchange(_ request: Data, socketPath: String, timeout: Duration) throws(ServiceError) -> Data {
        var address = sockaddr_un()
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        // Адрес сокета вмещает 104 байта. Каталог данных глубже — не случай для этого окна.
        guard socketPath.utf8.count < capacity else { throw .notRunning }
        address.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &address.sun_path) { bytes in
            bytes.copyBytes(from: Array(socketPath.utf8) + [0])
        }
        let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else { throw .notRunning }
        defer { close(descriptor) }
        limit(descriptor, to: timeout)
        let connected = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else { throw .notRunning }
        // Сокет, поднятый другим пользователем, вопроса не получает.
        guard servedBySameUser(descriptor) else { throw .notRunning }
        guard writeAll(descriptor, request + Data([0x0A])) else { throw .unreadable }
        guard let answer = readLine(descriptor) else { throw .unreadable }
        return answer
    }

    private static func limit(_ descriptor: Int32, to timeout: Duration) {
        let (seconds, attoseconds) = timeout.components
        var wait = timeval(tv_sec: Int(seconds), tv_usec: Int32(attoseconds / 1_000_000_000_000))
        setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, &wait, socklen_t(MemoryLayout<timeval>.size))
        setsockopt(descriptor, SOL_SOCKET, SO_SNDTIMEO, &wait, socklen_t(MemoryLayout<timeval>.size))
        var noSignal: Int32 = 1
        setsockopt(descriptor, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size))
    }

    private static func servedBySameUser(_ descriptor: Int32) -> Bool {
        var uid: uid_t = 0
        var gid: gid_t = 0
        return getpeereid(descriptor, &uid, &gid) == 0 && uid == geteuid()
    }

    private static func writeAll(_ descriptor: Int32, _ data: Data) -> Bool {
        data.withUnsafeBytes { buffer in
            guard let base = buffer.baseAddress else { return false }
            var sent = 0
            while sent < buffer.count {
                let written = write(descriptor, base.advanced(by: sent), buffer.count - sent)
                if written <= 0 { return false }
                sent += written
            }
            return true
        }
    }

    /// Строка до перевода строки; конец соединения раньше неё — не ответ.
    private static func readLine(_ descriptor: Int32) -> Data? {
        var answer = Data()
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let count = read(descriptor, &chunk, chunk.count)
            if count <= 0 { return nil }
            if let end = chunk[..<count].firstIndex(of: 0x0A) {
                answer.append(contentsOf: chunk[..<end])
                return answer
            }
            answer.append(contentsOf: chunk[..<count])
        }
    }
}
