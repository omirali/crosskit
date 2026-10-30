import CrosskitKit
import Foundation
import Observation

/// Терминал снизу (макет `design/UXMain.dc.html`): команды человека в каталоге задачи и, рядом,
/// команды, которые запускал агент открытой беседы.
@Observable
final class TerminalModel {

    enum Tab { case mine, agent }

    struct Line: Identifiable {
        enum Kind { case command, output, error, note }
        let id = UUID()
        let text: String
        let kind: Kind
    }

    var isOpen = false
    var isTall = false
    var tab: Tab = .mine
    var input = ""
    /// Каталог, где идут команды; ничего — каталог задачи.
    var directory: String?
    private(set) var lines: [Line] = []
    private(set) var running: Task<Void, Never>?

    func run(in fallback: String) {
        let command = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !command.isEmpty, running == nil else { return }
        input = ""
        if command == "clear" {
            lines = []
            return
        }
        let directory = directory ?? fallback
        lines.append(Line(text: "\(URL(fileURLWithPath: directory).lastPathComponent) ❯ \(command)", kind: .command))
        running = Task {
            for await line in ShellCommand.run(command, in: URL(fileURLWithPath: directory)) {
                lines.append(Line(text: line.text, kind: line.isError ? .error : .output))
                if lines.count > Self.kept { lines.removeFirst(lines.count - Self.kept) }
            }
            running = nil
        }
    }

    /// Больше строк терминал не держит: длинный вывод — в настоящий терминал.
    static let kept = 2_000

    func interrupt() {
        running?.cancel()
        running = nil
        lines.append(Line(text: "остановлено", kind: .note))
    }

    func clear() { lines = [] }

    func open(at directory: String) {
        self.directory = directory
        isOpen = true
        tab = .mine
    }
}
