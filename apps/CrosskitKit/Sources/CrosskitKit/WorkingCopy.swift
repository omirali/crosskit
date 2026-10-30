import Foundation

/// Что изменилось в рабочем каталоге задачи, по словам `git` (вкладка «Изменения» и «Файлы»).
///
/// Ядро правок агентов не хранит, а git знает их точно: всё, что агенты и человек поменяли в
/// репозитории задачи и ещё не влили в основную ветку. Окно только читает: ни коммита, ни PR.
public struct WorkingCopy: Sendable, Equatable {

    public struct File: Sendable, Equatable, Identifiable {
        public let path: String
        public let added: Int
        public let removed: Int
        /// `M` — изменён, `A` — добавлен, `D` — удалён, `?` — новый, git его ещё не знает.
        public let mark: Character

        public var id: String { path }
    }

    public let root: String
    public let branch: String
    /// С чем сравнивается: основная ветка, от которой ушла текущая, или последний коммит.
    public let base: String
    public let files: [File]

    public var added: Int { files.map(\.added).reduce(0, +) }
    public var removed: Int { files.map(\.removed).reduce(0, +) }
}

/// Строка разницы файла: что было, что стало и номера строк по обе стороны.
public struct DiffLine: Sendable, Equatable {
    public enum Kind: Sendable, Equatable { case context, added, removed, hunk }
    public let kind: Kind
    public let old: Int?
    public let new: Int?
    public let text: String
}

public enum Git {

    /// Рабочая копия каталога: ветка, база и файлы с числом строк. Ничего — если это не
    /// репозиторий или git не ответил.
    public static func workingCopy(at directory: String) async -> WorkingCopy? {
        guard let root = await run(["rev-parse", "--show-toplevel"], in: directory)?.trimmed, !root.isEmpty,
              let branch = await run(["rev-parse", "--abbrev-ref", "HEAD"], in: root)?.trimmed
        else { return nil }
        let base = await base(of: branch, in: root)
        let numstat = await run(["diff", "--numstat", base.commit], in: root) ?? ""
        let status = await run(["status", "--porcelain", "--untracked-files=all"], in: root) ?? ""
        return WorkingCopy(root: root, branch: branch, base: base.name,
                           files: files(numstat: numstat, status: status))
    }

    /// Разница одного файла с базой; новый файл — целиком как добавленный.
    public static func diff(of file: WorkingCopy.File, in copy: WorkingCopy) async -> [DiffLine] {
        if file.mark == "?" {
            let url = URL(fileURLWithPath: copy.root).appendingPathComponent(file.path)
            let text = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
            return text.split(separator: "\n", omittingEmptySubsequences: false).prefix(Self.shownLines)
                .enumerated().map { DiffLine(kind: .added, old: nil, new: $0.offset + 1, text: String($0.element)) }
        }
        let base = await base(of: copy.branch, in: copy.root)
        let unified = await run(["diff", base.commit, "--", file.path], in: copy.root) ?? ""
        return Array(lines(ofUnified: unified).prefix(Self.shownLines))
    }

    /// Больше строк разницы окно не рисует: дальше — терминал.
    static let shownLines = 400

    /// Основная ветка, если текущая от неё ушла: тогда разница — всё, что сделано в ветке.
    /// Иначе — последний коммит, и разница — то, что ещё не закоммичено.
    static func base(of branch: String, in root: String) async -> (name: String, commit: String) {
        for main in ["main", "master"] where main != branch {
            if let commit = await run(["merge-base", main, "HEAD"], in: root)?.trimmed, !commit.isEmpty {
                return (main, commit)
            }
        }
        return ("последний коммит", "HEAD")
    }

    /// Файлы по `git diff --numstat` и `git status --porcelain`: числа строк от первого, новые
    /// файлы и метки — от второго.
    static func files(numstat: String, status: String) -> [WorkingCopy.File] {
        var marks: [String: Character] = [:]
        for line in status.split(separator: "\n") where line.count > 3 {
            let code = line.prefix(2)
            let path = String(line.dropFirst(3)).components(separatedBy: " -> ").last ?? ""
            marks[path] = code == "??" ? "?" : code.contains("A") ? "A" : code.contains("D") ? "D" : "M"
        }
        var files: [WorkingCopy.File] = []
        var seen: Set<String> = []
        for line in numstat.split(separator: "\n") {
            let parts = line.split(separator: "\t", maxSplits: 2).map(String.init)
            guard parts.count == 3 else { continue }
            let path = parts[2].components(separatedBy: " => ").last ?? parts[2]
            seen.insert(path)
            files.append(.init(path: path, added: Int(parts[0]) ?? 0, removed: Int(parts[1]) ?? 0, mark: marks[path] ?? "M"))
        }
        for (path, mark) in marks.sorted(by: { $0.key < $1.key }) where mark == "?" && !seen.contains(path) {
            files.append(.init(path: path, added: 0, removed: 0, mark: "?"))
        }
        return files
    }

    /// Строки unified diff с номерами по обе стороны; заголовки файла пропускаются.
    static func lines(ofUnified text: String) -> [DiffLine] {
        var result: [DiffLine] = []
        var old = 0, new = 0
        var inHunk = false
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            if line.hasPrefix("@@") {
                inHunk = true
                let numbers = line.split(separator: " ")
                old = Int(numbers.dropFirst().first?.dropFirst().split(separator: ",").first ?? "") ?? 0
                new = Int(numbers.dropFirst(2).first?.dropFirst().split(separator: ",").first ?? "") ?? 0
                result.append(DiffLine(kind: .hunk, old: nil, new: nil, text: line))
                continue
            }
            guard inHunk, let sign = line.first else { continue }
            switch sign {
            case "+":
                result.append(DiffLine(kind: .added, old: nil, new: new, text: String(line.dropFirst())))
                new += 1
            case "-":
                result.append(DiffLine(kind: .removed, old: old, new: nil, text: String(line.dropFirst())))
                old += 1
            case " ":
                result.append(DiffLine(kind: .context, old: old, new: new, text: String(line.dropFirst())))
                old += 1
                new += 1
            default:
                continue
            }
        }
        return result
    }

    /// `git` с аргументами в каталоге; вывод — если git закончил без ошибки. Вывод читается,
    /// пока git пишет: большой diff иначе заполнил бы канал, и git ждал бы вечно.
    static func run(_ arguments: [String], in directory: String) async -> String? {
        await Task.detached {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/git")
            // Имена файлов — как есть: иначе git прячет кириллицу за восьмеричными кодами.
            process.arguments = ["-c", "core.quotePath=false", "-C", directory] + arguments
            let output = Pipe()
            process.standardOutput = output
            process.standardError = FileHandle.nullDevice
            do {
                try process.run()
            } catch {
                return nil
            }
            let data = output.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return process.terminationStatus == 0 ? String(decoding: data, as: UTF8.self) : nil
        }.value
    }
}

private extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
