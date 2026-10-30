#if DEBUG
import AppKit
import SwiftUI

/// Снимки окна для проверки вида без доступа к записи экрана: `CROSSKIT_SNAPSHOT=<каталог>`
/// проходит по вкладкам и состояниям, сохраняет PNG окна в каждом и закрывает приложение.
/// Только в отладочной сборке; так же снимает себя панель (этап UI-4b).
enum Snapshots {

    static func runIfAsked(_ model: DesktopModel) {
        guard let folder = ProcessInfo.processInfo.environment["CROSSKIT_SNAPSHOT"] else { return }
        let directory = URL(fileURLWithPath: folder)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let steps: [(String, @MainActor () -> Void)] = [
            ("1-map", { model.pane = .map }),
            ("2-node", { if let id = model.taskID, let step = model.tree(of: id)?.waitingStep?.id ?? model.tree(of: id)?.head { model.select(.step(step), in: id) } }),
            ("3-chats", { model.showsChats = true }),
            ("4-board", { model.showsChats = false; model.pane = .board }),
            ("5-changes", { model.pane = .changes }),
            ("6-files", { model.pane = .files }),
            ("7-inbox", { model.pane = .map; model.inbox.isOpen = true }),
            ("8-project", { model.inbox.isOpen = false; if let project = model.project { model.open(.project(project.root)) } }),
        ]
        Task { @MainActor in
            if let context = ProcessInfo.processInfo.environment["CROSSKIT_SNAPSHOT_CONTEXT"] {
                try? await Task.sleep(for: .seconds(4))
                model.switchContext(context)
            }
            try? await Task.sleep(for: .seconds(12))
            for (name, apply) in steps {
                apply()
                try? await Task.sleep(for: .seconds(4))
                save(to: directory.appendingPathComponent(name + ".png"))
            }
            NSApp.terminate(nil)
        }
    }

    @MainActor
    private static func save(to file: URL) {
        guard let window = NSApp.windows.first(where: { $0.isVisible && $0.contentView != nil }),
              let view = window.contentView?.superview ?? window.contentView,
              let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return }
        view.cacheDisplay(in: view.bounds, to: bitmap)
        let rows = tables(in: view).map { "\($0.numberOfRows)" }.joined(separator: ",")
        try? "rows: \(rows)\n".write(to: file.appendingPathExtension("txt"), atomically: true, encoding: .utf8)
        try? bitmap.representation(using: .png, properties: [:])?.write(to: file)
    }

    @MainActor
    private static func tables(in view: NSView) -> [NSTableView] {
        (view as? NSTableView).map { [$0] } ?? view.subviews.flatMap(tables)
    }
}
#endif
