import SwiftUI

/// Crosskit для macOS: задача как карта решений и беседа с агентом рядом с ней (ADR 0058, 0063).
@main
struct CrosskitApp: App {

    @State private var model = DesktopModel()

    var body: some Scene {
        WindowGroup("Crosskit") {
            RootView(model: model)
                .environment(\.locale, Wording.locale)
                #if DEBUG
                .onAppear { Snapshots.runIfAsked(model) }
                #endif
        }
        .defaultSize(width: 1500, height: 940)
        .commands {
            CommandGroup(after: .toolbar) {
                Button("Обновить") {
                    Task { await model.reload() }
                }
                .keyboardShortcut("r")
                Button("Уведомления") {
                    model.inbox.isOpen.toggle()
                }
                .keyboardShortcut("u", modifiers: [.command, .shift])
            }
        }
    }
}
