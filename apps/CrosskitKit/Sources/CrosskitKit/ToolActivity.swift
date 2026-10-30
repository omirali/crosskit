import Foundation

/// Вызов инструмента агента словами человека (макет `design/UXMain.dc.html`): «Прочитал
/// src/order.rs», «Изменил src/cart/view.rs +3 −2» и, для правки, сама разница.
public struct ToolActivity: Sendable, Equatable {
    public enum Kind: Sendable, Equatable { case read, run, edit, create, search, other }

    public let kind: Kind
    public let verb: String
    /// Файл, команда или шаблон поиска — главное одной строкой.
    public let subject: String
    /// Файл, если вызов о файле: для вкладок «Изменения» и «Файлы».
    public let path: String?
    public let added: Int
    public let removed: Int
    /// Разница правки или содержимое нового файла.
    public let lines: [DiffLine]

    /// `directory` — рабочий каталог беседы: пути внутри него показываются от него.
    public init(tool: String, input: String, directory: String? = nil) {
        let object = (try? JSONSerialization.jsonObject(with: Data(input.utf8)) as? [String: Any]) ?? [:]
        let string: (String) -> String? = { object[$0] as? String }
        let file = string("file_path") ?? string("notebook_path") ?? string("path")
        let shown = file.map { Self.relative($0, to: directory) }
        switch tool {
        case "Read":
            self.init(kind: .read, verb: "Прочитал", subject: shown ?? "", path: file)
        case "Bash":
            self.init(kind: .run, verb: "Запустил", subject: string("command") ?? "", path: nil)
        case "Edit":
            let lines = Self.replacement(string("old_string") ?? "", string("new_string") ?? "")
            self.init(kind: .edit, verb: "Изменил", subject: shown ?? "", path: file, lines: lines)
        case "MultiEdit":
            let edits = object["edits"] as? [[String: Any]] ?? []
            let lines = edits.flatMap { Self.replacement($0["old_string"] as? String ?? "", $0["new_string"] as? String ?? "") }
            self.init(kind: .edit, verb: "Изменил", subject: shown ?? "", path: file, lines: lines)
        case "Write":
            let lines = Self.split(string("content") ?? "").enumerated()
                .map { DiffLine(kind: .added, old: nil, new: $0.offset + 1, text: $0.element) }
            self.init(kind: .create, verb: "Записал", subject: shown ?? "", path: file, lines: lines)
        case "NotebookEdit":
            self.init(kind: .edit, verb: "Изменил", subject: shown ?? "", path: file)
        case "Grep", "Glob":
            self.init(kind: .search, verb: "Искал", subject: string("pattern") ?? "", path: nil)
        case "WebFetch":
            self.init(kind: .other, verb: "Открыл", subject: string("url") ?? "", path: nil)
        case "WebSearch":
            self.init(kind: .search, verb: "Искал в сети", subject: string("query") ?? "", path: nil)
        case "Task", "Agent":
            self.init(kind: .other, verb: "Поручил", subject: string("description") ?? "", path: nil)
        default:
            self.init(kind: .other, verb: tool, subject: shown ?? string("command") ?? string("description") ?? "", path: file)
        }
    }

    init(kind: Kind, verb: String, subject: String, path: String?, lines: [DiffLine] = []) {
        self.kind = kind
        self.verb = verb
        self.subject = subject
        self.path = path
        self.lines = lines
        added = lines.filter { $0.kind == .added }.count
        removed = lines.filter { $0.kind == .removed }.count
    }

    /// Правка «было → стало»: старые строки ушли, новые пришли. Номеров строк файла Edit не
    /// знает, поэтому номера — внутри правки.
    static func replacement(_ old: String, _ new: String) -> [DiffLine] {
        let before = split(old), after = split(new)
        let common = zip(before, after).prefix { $0 == $1 }.count
        let tail = zip(before.dropFirst(common).reversed(), after.dropFirst(common).reversed()).prefix { $0 == $1 }.count
        var lines: [DiffLine] = []
        for index in 0..<common {
            lines.append(DiffLine(kind: .context, old: index + 1, new: index + 1, text: before[index]))
        }
        for index in common..<(before.count - tail) {
            lines.append(DiffLine(kind: .removed, old: index + 1, new: nil, text: before[index]))
        }
        for index in common..<(after.count - tail) {
            lines.append(DiffLine(kind: .added, old: nil, new: index + 1, text: after[index]))
        }
        for offset in 0..<tail {
            lines.append(DiffLine(kind: .context, old: before.count - tail + offset + 1, new: after.count - tail + offset + 1,
                                  text: before[before.count - tail + offset]))
        }
        return lines
    }

    static func split(_ text: String) -> [String] {
        text.isEmpty ? [] : text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
    }

    static func relative(_ path: String, to directory: String?) -> String {
        guard let directory, path.hasPrefix(directory + "/") else {
            return path.replacingOccurrences(of: NSHomeDirectory(), with: "~")
        }
        return String(path.dropFirst(directory.count + 1))
    }
}
