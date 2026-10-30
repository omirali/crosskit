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
        // Беседа с агентом по задаче — своё окно рядом с картой (ADR 0062).
        WindowGroup("Беседа", id: "chat", for: ChatTarget.self) { $target in
            if let target {
                ChatWindow(target: target)
                    .environment(\.locale, Wording.locale)
            }
        }
        .defaultSize(width: 760, height: 860)
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
