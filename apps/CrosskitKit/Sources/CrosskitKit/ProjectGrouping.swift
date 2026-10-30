import Foundation

/// Проект — папка или репозиторий, где шли беседы задач (макет `design/UXMain.dc.html`).
///
/// Ядро проектов не знает: у задачи есть беседы, у беседы — каталог, где она началась. Проект
/// окна — корень этого каталога: репозиторий, если каталог в нём, иначе сам каталог. Задача
/// попадает в проект, где шло больше всего её бесед; задача без единого каталога — в «Без папки».
public struct Project: Sendable, Equatable, Identifiable {
    /// Корень проекта; пустой у «Без папки».
    public let root: String
    public let name: String
    public let taskIDs: [String]
    /// Кандидаты сшивки этого проекта: по первой беседе каждого.
    public let candidateSessions: [String]

    public var id: String { root }

    public static let unplaced = "Без папки"
}

public enum ProjectGrouping {

    /// Проекты доски в порядке её задач. `root` находит корень каталога; по умолчанию — ближайший
    /// предок с `.git`.
    public static func projects(of board: TaskBoard, root: (String) -> String = gitRoot) -> [Project] {
        var order: [String] = []
        var tasks: [String: [String]] = [:]
        var candidates: [String: [String]] = [:]
        func place(_ key: String) {
            if tasks[key] == nil, candidates[key] == nil { order.append(key) }
        }
        for task in board.confirmed {
            let key = home(of: task.conversations, root: root)
            place(key)
            tasks[key, default: []].append(task.id)
        }
        for candidate in board.candidates {
            guard let first = candidate.conversations.first else { continue }
            let key = home(of: candidate.conversations, root: root)
            place(key)
            candidates[key, default: []].append(first.session)
        }
        // «Без папки» — последней: у неё нет места на диске, и она не проект.
        order.sort { ($0.isEmpty ? 1 : 0) < ($1.isEmpty ? 1 : 0) }
        return order.map { key in
            Project(root: key, name: key.isEmpty ? Project.unplaced : URL(fileURLWithPath: key).lastPathComponent,
                    taskIDs: tasks[key] ?? [], candidateSessions: candidates[key] ?? [])
        }
    }

    /// Корень, где шло больше всего бесед; при равенстве — где шла последняя.
    static func home(of conversations: [TaskConversation], root: (String) -> String) -> String {
        var count: [String: Int] = [:]
        var latest: [String: Int64] = [:]
        for conversation in conversations {
            guard let directory = conversation.workingDirectory else { continue }
            let key = root(directory)
            count[key, default: 0] += 1
            latest[key] = max(latest[key] ?? 0, conversation.lastAtUnixMs ?? 0)
        }
        return count.keys.max { (count[$0]!, latest[$0] ?? 0) < (count[$1]!, latest[$1] ?? 0) } ?? ""
    }

    /// Ближайший предок каталога, в котором есть `.git`; без него — сам каталог.
    public static func gitRoot(_ directory: String) -> String {
        var url = URL(fileURLWithPath: directory).standardizedFileURL
        while url.path != "/" {
            if FileManager.default.fileExists(atPath: url.appendingPathComponent(".git").path) { return url.path }
            url.deleteLastPathComponent()
        }
        return URL(fileURLWithPath: directory).standardizedFileURL.path
    }
}
