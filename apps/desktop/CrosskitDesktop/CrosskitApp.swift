import SwiftUI

/// Crosskit для macOS: задача как карта решений (ADR 0058, этап D-1).
@main
struct CrosskitApp: App {

    @State private var model = DesktopModel()

    var body: some Scene {
        WindowGroup("Crosskit") {
            RootView(model: model)
                .environment(\.locale, Wording.locale)
        }
        .defaultSize(width: 1320, height: 880)
        .commands {
            CommandGroup(after: .toolbar) {
                Button("Обновить") {
                    Task { await model.reload() }
                }
                .keyboardShortcut("r")
            }
        }
    }
}
