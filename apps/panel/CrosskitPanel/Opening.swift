import AppKit

/// Что делают ⏎ и ⌘⏎ со строкой.
///
/// Этап UI-3 решает «открыть» ровно на три случая и не больше: файл открывается приложением
/// по умолчанию, ⌘⏎ показывает его в Finder, а беседа даёт команду, которой её продолжают, —
/// `claude --resume <сессия>` в буфер обмена. Открыть беседу в инструменте напрямую панель
/// не берётся: это управление чужим приложением, и это отдельное решение (README, «Разрешения»).
enum Opening {

    enum Outcome {
        /// Сделано, и панель можно спрятать: человек ушёл туда, куда просил.
        case left
        /// Сделано или отказано на месте; панель остаётся и говорит строкой, что случилось.
        case notice(String)
        /// Открыть не удалось — плашка под строкой (решение 16 дизайнера).
        case failed(OpenFailure)
    }

    /// ⏎.
    static func primary(_ row: FeedRow) -> Outcome {
        switch row {
        case .conversation(let conversation):
            guard let session = conversation.sessionID else {
                return .notice("У этой беседы нет идентификатора сессии — продолжить её нечем")
            }
            let command = "claude --resume \(session)"
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(command, forType: .string)
            return .notice("Скопировано: \(command) — запустите в каталоге, где шла беседа")
        case .file(let file):
            guard file.presence == .present else {
                return .notice("Файла нет на диске — открыть нечего")
            }
            return NSWorkspace.shared.open(URL(fileURLWithPath: file.path))
                ? .left
                : .failed(failure(opening: file))
        }
    }

    /// ⌘⏎ на файле. У беседы вторичного действия этот этап не решает.
    static func reveal(_ file: FileRow) -> Outcome {
        guard file.presence == .present, FileManager.default.fileExists(atPath: file.path) else {
            return .failed(failure(opening: file))
        }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: file.path)])
        return .left
    }

    static func failure(opening file: FileRow) -> OpenFailure {
        let seen = file.when.map { ", последний раз его видели \($0.text)" } ?? ""
        return OpenFailure(
            rowIdentity: file.id,
            title: "Не удалось открыть \(file.name)",
            explanation: "Файла нет по пути \(file.directory)\(file.name). В индексе Crosskit его "
                + "содержимое есть\(seen).",
            actions: [("⌘⏎", "Показать в Finder"), ("⏎", "Повторить")])
    }
}

/// Строка выдачи для узла экрана, у которого есть только путь или сессия: то, что умеют открыть
/// ⏎ и ⌘⏎. Нужна графу окрестности и карте знаний — один способ на оба экрана.
extension FeedRow {

    static func openable(path: String, id: String, present: Bool) -> FeedRow {
        let (directory, name) = CoreAnswer.split(path, home: NSHomeDirectory())
        return .file(FileRow(
            id: id, cursor: FeedCursor(opaque: id), path: path, directory: directory, name: name,
            fragment: Fragment(text: "", matches: []), matched: [], presence: present ? .present : .missing,
            project: nil, when: nil, topic: nil))
    }

    static func resumable(session: String, id: String, title: String, messageCount: Int?) -> FeedRow {
        .conversation(ConversationRow(
            id: id, cursor: FeedCursor(opaque: id), tool: .claudeCode, title: title,
            fragment: Fragment(text: "", matches: []), project: nil, when: nil,
            messageCount: messageCount, topic: nil, sessionID: session))
    }
}
